//! Half-block renderer: blit the roaming herd into a pixel buffer, emit it as
//! `▀` cells (fg = top pixel, bg = bottom pixel), then overlay state bubbles/
//! badges and a `+N` counter.

use std::io;
use std::sync::mpsc::Receiver;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;

use crate::agent::Agent;
use crate::anim::{Overlay, OverlayColor, Rgb};
use crate::herd::{Herd, visible_and_hidden};
use crate::herdr::HerdrCli;
use crate::motion::animate;
use crate::palette::{StateStyle, Theme, role_color};
use crate::pet::priority;
use crate::sprite::{Frame as SpriteFrame, Role, Species};

/// Rows the focus hat occupies above a pet's head, plus the 1px hop/bounce
/// headroom sprites already reserve (see `sprites/*.sprite`, `<= 14` px).
pub(crate) const HAT_H: usize = 3;
/// Columns the focus hat occupies, centered over the head anchor. Wider than
/// the hat's own crown to leave room for the brim.
pub(crate) const HAT_W: usize = 7;
/// The hat's pixel grid, top row first: `.` transparent, `#` outline, `r` red
/// fill. A simple fedora — a rounded crown over a flat, full-width brim.
/// Symmetric, so facing flip never needs to mirror it.
const HAT_ROWS: [&str; HAT_H] = ["..#r#..", ".#rrr#.", "#rrrrr#"];
const HAT_OUTLINE: Rgb = Rgb(0x20, 0x18, 0x18);
const HAT_FILL: Rgb = Rgb(0xd6, 0x2b, 0x2b);

/// Milliseconds since the Unix epoch — the same absolute reference on every
/// process on this machine (all `herdr-pets render` panes run server-side, so
/// there's no cross-machine clock-skew concern even under `herdr --remote`).
/// This is what makes `motion::animate` agree across every independent pane.
fn wall_clock_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Height of the pet band in pixels. Sprites are 16x14 (see sprites/*.sprite);
/// the band is the sprite height, plus 1px of headroom for the hop/bounce
/// lift, plus [`HAT_H`] rows so the focus hat has room above even the tallest
/// (standing) pose without clipping.
pub const PET_PX_H: usize = 15 + HAT_H;

/// A pixel canvas: `w * h` optional colors, row-major. `None` = transparent.
pub struct PixelBuf {
    pub w: usize,
    pub h: usize,
    pub px: Vec<Option<Rgb>>,
}

impl PixelBuf {
    /// A fully-transparent buffer of `w` by `h` pixels.
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            px: vec![None; w * h],
        }
    }

    /// Set the pixel at `(x, y)`, silently ignoring out-of-bounds writes.
    pub fn set(&mut self, x: i32, y: i32, c: Rgb) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            self.px[y as usize * self.w + x as usize] = Some(c);
        }
    }
}

fn to_color(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

/// The (row, drawn-column) of `fr`'s topmost non-transparent pixel, scanned in
/// already-flipped drawn-space (`flip` mirrors which source column each drawn
/// column samples, matching the body blit below). This anchors the focus hat
/// above the head — or, for the idle "dozing" pose with no distinct head,
/// above the topmost point of the lying-down lump — without any per-species
/// or per-state table: whichever pixel is highest just is the head. Falls
/// back to top-center for a fully transparent frame (never hit by real
/// sprites, all of which paint something).
pub(crate) fn head_anchor(fr: &SpriteFrame, flip: bool) -> (usize, usize) {
    for y in 0..fr.h {
        let mut lo = None;
        let mut hi = None;
        for x in 0..fr.w {
            let sx = if flip { fr.w - 1 - x } else { x };
            if fr.cells[y * fr.w + sx] != Role::Transparent {
                lo.get_or_insert(x);
                hi = Some(x);
            }
        }
        if let (Some(lo), Some(hi)) = (lo, hi) {
            return (y, (lo + hi) / 2);
        }
    }
    (0, fr.w / 2)
}

/// Blit the focus hat into `buf` above the head anchor `(head_row, head_col)`,
/// at body-blit offset `(ox, oy)` — the same offset the sprite body was drawn
/// at, so the hat inherits the identical motion offset, bottom-anchor, and
/// facing-flip transform and never detaches from the head. `head_row` is in
/// the sprite's own local coordinates (pre-offset); `head_col` is already in
/// drawn (post-flip) space, so the hat is not flipped again.
fn draw_hat(buf: &mut PixelBuf, ox: i32, oy: i32, head_row: usize, head_col: usize) {
    let top = oy + head_row as i32 - HAT_H as i32;
    let left = ox + head_col as i32 - (HAT_W / 2) as i32;
    for (y, row) in HAT_ROWS.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            let color = match ch {
                '#' => Some(HAT_OUTLINE),
                'r' => Some(HAT_FILL),
                _ => None,
            };
            if let Some(c) = color {
                buf.set(left + x as i32, top + y as i32, c);
            }
        }
    }
}

/// Rasterize the focus hat to RGBA at `scale` px per hat-pixel, padded by
/// `pad` hat-pixels of transparent margin on every side — mirrors
/// `raster::pad_frame` + `raster::rasterize`, but for the fixed [`HAT_W`]x
/// [`HAT_H`] hat bitmap rather than a per-species sprite `Frame`. Used by the kitty
/// backend so the hat can be transmitted once and panned by motion offset
/// like the body (`kitty_render::crop_rect`), the same trick `icon::
/// rasterize_icon` uses for overlay icons.
pub(crate) fn rasterize_hat(scale: usize, pad: usize) -> crate::raster::Rgba {
    let scale = scale.max(1);
    let (w, h) = ((HAT_W + pad * 2) * scale, (HAT_H + pad * 2) * scale);
    let mut px = vec![0u8; w * h * 4];
    for (y, row) in HAT_ROWS.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            let color = match ch {
                '#' => Some(HAT_OUTLINE),
                'r' => Some(HAT_FILL),
                _ => None,
            };
            let Some(c) = color else { continue };
            let (px_x, px_y) = ((x + pad) * scale, (y + pad) * scale);
            for dy in 0..scale {
                for dx in 0..scale {
                    let i = ((px_y + dy) * w + (px_x + dx)) * 4;
                    px[i] = c.0;
                    px[i + 1] = c.1;
                    px[i + 2] = c.2;
                    px[i + 3] = 255;
                }
            }
        }
    }
    crate::raster::Rgba { w, h, px }
}

/// Emit the pixel buffer as half-block cells into `area` (top-left aligned):
/// each cell packs two pixel rows into one terminal row via `▀` (fg = top
/// pixel, bg = bottom pixel) or `▄` when only the bottom pixel is set.
pub fn draw_pixels(frame: &mut Frame, area: Rect, buf: &PixelBuf) {
    let rows = buf.h.div_ceil(2);
    for ry in 0..rows {
        for x in 0..buf.w {
            let top = buf.px[(ry * 2) * buf.w + x];
            let bot = if ry * 2 + 1 < buf.h {
                buf.px[(ry * 2 + 1) * buf.w + x]
            } else {
                None
            };
            let cx = area.x + x as u16;
            let cy = area.y + ry as u16;
            if cx >= area.right() || cy >= area.bottom() {
                continue;
            }
            let (ch, style) = match (top, bot) {
                (None, None) => continue,
                (Some(t), Some(b)) => ('▀', Style::default().fg(to_color(t)).bg(to_color(b))),
                (Some(t), None) => ('▀', Style::default().fg(to_color(t))),
                (None, Some(b)) => ('▄', Style::default().fg(to_color(b))),
            };
            frame.buffer_mut().set_string(cx, cy, ch.to_string(), style);
        }
    }
}

/// Blit every visible pet's body — and, for the focused pet, its focus hat —
/// into a fresh pixel buffer, in priority z-order (blocked draws last, i.e. on
/// top). The hat is composited into the same buffer right after its pet's
/// body, at the same offset, so it shares the body's full transform (motion
/// offset, bottom-anchor, facing flip) and never detaches during motion.
/// `now_ms` (milliseconds since the Unix epoch, or a frozen value under
/// reduced motion) drives every pet's position/pose via `motion::animate` — a
/// pure function, so this is fully deterministic given the same inputs.
/// Returns the buffer plus the visible set's z-order (draw order, lowest
/// priority first) so the caller can reuse it for overlays without
/// recomputing the visible/capacity selection.
fn build_band(
    herd: &Herd,
    species: &[Species],
    strip_w: usize,
    theme: Theme,
    now_ms: u64,
) -> (PixelBuf, Vec<usize>) {
    let mut buf = PixelBuf::new(strip_w, PET_PX_H);

    let pet_w = species.first().map(|s| s.size().0).unwrap_or(12);
    let max_x = (strip_w as f32 - pet_w as f32).max(0.0);
    let capacity = (strip_w / (pet_w * 3 / 4).max(1)).max(1);
    let (visible, _hidden) = visible_and_hidden(&herd.pets, capacity);

    // z-order: lowest priority first so blocked draws last (on top).
    let mut order = visible;
    order.sort_by_key(|&i| priority(herd.pets[i].status));

    for &i in &order {
        let pet = &herd.pets[i];
        let Some(sp) = species
            .get(pet.identity.species_index)
            .or_else(|| species.first())
        else {
            continue;
        };
        let Some(state) = sp.states.get(&pet.status) else {
            continue;
        };
        let animated = animate(&pet.terminal_id, pet.status, state, now_ms);
        let fr = &state.frames[animated.frame_index];
        let style = StateStyle {
            dim: state.dim,
            ghost: state.ghost,
        };
        let ox = (animated.x_fraction * max_x + animated.offset.dx).round() as i32;
        // Bottom-anchor: feet rest on the band floor; motion (dy<=0) lifts the
        // pet up into the headroom above it, so a hop/bounce never clips.
        let floor = PET_PX_H as i32 - fr.h as i32;
        let oy = floor + animated.offset.dy.round() as i32;
        for y in 0..fr.h {
            for x in 0..fr.w {
                let sx = if animated.facing_left {
                    fr.w - 1 - x
                } else {
                    x
                };
                if let Some(c) = role_color(fr.cells[y * fr.w + sx], pet.identity.hue, theme, style)
                {
                    buf.set(ox + x as i32, oy + y as i32, c);
                }
            }
        }
        if pet.focused {
            let (head_row, head_col) = head_anchor(fr, animated.facing_left);
            draw_hat(&mut buf, ox, oy, head_row, head_col);
        }
    }
    (buf, order)
}

/// Draw the whole strip: visible pets in priority z-order (blocked draws
/// last, i.e. on top), their overlays (bubbles/badges), the hovered pet's
/// caption, and a `+N` marker for any pets the strip has no room for. All of
/// these live in a reserved top lane (`lane_y`); the pet band is drawn below
/// it, so nothing in the lane ever covers a pet. `now_ms` (milliseconds since
/// the Unix epoch, or a frozen value under reduced motion) drives every pet's
/// position/pose via `motion::animate` — a pure function, so this is fully
/// deterministic given the same inputs.
pub fn draw_herd(
    frame: &mut Frame,
    herd: &Herd,
    species: &[Species],
    theme: Theme,
    now_ms: u64,
    hover_label: Option<&str>,
) {
    let area = frame.area();
    let strip_w = area.width as usize;
    let (buf, order) = build_band(herd, species, strip_w, theme, now_ms);

    let pet_w = species.first().map(|s| s.size().0).unwrap_or(12);
    let max_x = (strip_w as f32 - pet_w as f32).max(0.0);
    let capacity = (strip_w / (pet_w * 3 / 4).max(1)).max(1);
    let (_visible, hidden) = visible_and_hidden(&herd.pets, capacity);

    // Bottom-align the whole strip so it reads as a slim status line whatever
    // the pane's height (herdr enforces a minimum pane height, so the pane can be
    // taller than the content needs): the pet band bottom-aligns to the pane
    // floor, and the icon lane sits just above it. Any extra rows fall at the
    // top, blending with the pane above. The icon lane keeps overlays/`+N`/the
    // caption off the pet.
    let band_rows = PET_PX_H.div_ceil(2) as u16;
    let band_top = area.bottom().saturating_sub(band_rows);
    let lane_y = band_top.saturating_sub(1);
    let pet_area = Rect {
        x: area.x,
        y: band_top,
        width: area.width,
        // Clamped so a pane shorter than the band (below herdr's enforced
        // minimum) crops the top of the pets instead of handing
        // `draw_pixels` a Rect that overruns the real frame buffer.
        height: band_rows.min(area.height.saturating_sub(band_top)),
    };
    draw_pixels(frame, pet_area, &buf);

    // Overlays (bubbles/badges) as text cells above each visible pet.
    for &i in &order {
        let pet = &herd.pets[i];
        let Some(sp) = species
            .get(pet.identity.species_index)
            .or_else(|| species.first())
        else {
            continue;
        };
        let Some(state) = sp.states.get(&pet.status) else {
            continue;
        };
        let (glyph, _kind) = match &state.overlay.kind {
            Overlay::Bubble(g) => (g.clone(), 'b'),
            Overlay::Badge(g) => (g.clone(), 'a'),
            Overlay::None => continue,
        };
        let color = match state.overlay.color {
            OverlayColor::Literal(c) => to_color(c),
            OverlayColor::Accent => Color::Rgb(0xe6, 0xc8, 0x77),
            OverlayColor::Default => Color::Gray,
        };
        let animated = animate(&pet.terminal_id, pet.status, state, now_ms);
        let cx = area.x
            + ((animated.x_fraction * max_x).round() as u16)
                .saturating_add(3)
                .min(area.width.saturating_sub(glyph.chars().count() as u16));
        frame.buffer_mut().set_span(
            cx,
            lane_y,
            &Span::styled(glyph, Style::default().fg(color)),
            area.width,
        );
    }

    if hidden > 0 {
        let label = format!("+{hidden}");
        let label_w = label.len() as u16;
        let x = area.right().saturating_sub(label_w + 1);
        // In the reserved icon lane (just above the band), right-aligned.
        frame.buffer_mut().set_span(
            x,
            lane_y,
            &Span::styled(label, Style::default().fg(Color::DarkGray)),
            label_w,
        );
    }

    draw_caption(frame, area, lane_y, hover_label, hidden);
}

/// Ochre — the hovered pet's caption color, distinct from the `+N` marker's
/// neutral dark gray.
const CAPTION_OCHRE: Color = Color::Rgb(0xd9, 0xa4, 0x41);

/// Draw the hover caption top-right in the reserved top lane (row `y`),
/// right-aligned and ochre, or nothing when `label` is `None`. When `hidden`
/// pets overflow (the `+N` marker also lives in this lane, further right) the
/// caption stops one column short of it; either way it's truncated so it
/// never overruns the strip width.
pub fn draw_caption(frame: &mut Frame, area: Rect, y: u16, label: Option<&str>, hidden: usize) {
    let Some(label) = label else { return };
    if area.height == 0 || area.width == 0 {
        return;
    }
    // 1-col margin from the strip edge (matching `+N`'s own margin), plus,
    // when `+N` is also shown, its width and a further 1-col gap before it.
    let hidden_w = if hidden > 0 {
        format!("+{hidden}").chars().count() as u16 + 1
    } else {
        0
    };
    let right = area.right().saturating_sub(1 + hidden_w);
    let max_chars = right.saturating_sub(area.x) as usize;
    if max_chars == 0 {
        return;
    }
    let text: String = label.chars().take(max_chars).collect();
    let w = text.chars().count() as u16;
    let x = right.saturating_sub(w);
    frame.buffer_mut().set_span(
        x,
        y,
        &Span::styled(text, Style::default().fg(CAPTION_OCHRE)),
        w,
    );
}

/// The index of the visible pet drawn under terminal column `col`, if any.
/// A mouse column maps 1:1 to a pixel x (half-block cells are one pixel wide).
/// Only pets that are actually drawn (the visible set on overflow) are
/// hit-testable; when pets overlap, the topmost — highest `priority`, matching
/// the draw z-order — wins. Returns `None` over a gap or out of range. `now_ms`
/// must match whatever was passed to `draw_herd` this frame, so hit-testing
/// agrees with what's actually on screen.
pub fn pet_at_column(
    herd: &Herd,
    species: &[Species],
    strip_w: usize,
    col: u16,
    now_ms: u64,
) -> Option<usize> {
    let base_w = species.first().map(|s| s.size().0).unwrap_or(12);
    let max_x = (strip_w as f32 - base_w as f32).max(0.0);
    let capacity = (strip_w / (base_w * 3 / 4).max(1)).max(1);
    let (visible, _hidden) = visible_and_hidden(&herd.pets, capacity);

    let x = col as i32;
    let mut best: Option<usize> = None;
    for &i in &visible {
        let pet = &herd.pets[i];
        let Some(sp) = species
            .get(pet.identity.species_index)
            .or_else(|| species.first())
        else {
            continue;
        };
        let Some(state) = sp.states.get(&pet.status) else {
            continue;
        };
        let w = sp.size().0 as i32;
        let animated = animate(&pet.terminal_id, pet.status, state, now_ms);
        let left = (animated.x_fraction * max_x).round() as i32;
        if x >= left && x < left + w {
            let take = match best {
                None => true,
                Some(b) => priority(pet.status) >= priority(herd.pets[b].status),
            };
            if take {
                best = Some(i);
            }
        }
    }
    best
}

/// A pluggable pet-strip renderer. The simulation is shared; only drawing and
/// hit-testing differ between backends (half-block vs kitty graphics).
pub trait PetRenderer {
    /// Draw the whole strip for this frame: the pet band, and (where the
    /// backend supports it) overlays/`+N`/the hover caption. `now_ms` drives
    /// every pet's position/pose (see `motion::animate`). `hover_label` is the
    /// hovered pet's name, if any, drawn top-right in the reserved lane.
    fn draw(
        &mut self,
        frame: &mut Frame,
        herd: &Herd,
        species: &[Species],
        theme: Theme,
        now_ms: u64,
        hover_label: Option<&str>,
    );
    /// The visible pet under terminal column `col`, if any (for hover/click).
    /// `now_ms` must match the value passed to `draw` this frame.
    fn pet_at_column(
        &self,
        herd: &Herd,
        species: &[Species],
        strip_w: usize,
        col: u16,
        now_ms: u64,
    ) -> Option<usize>;
    /// Release backend resources (kitty: delete transmitted images). Default no-op.
    fn teardown(&mut self) -> io::Result<()> {
        Ok(())
    }
    /// Short backend id (`"half-block"` / `"kitty"`), so selection is
    /// assertable in tests without downcasting.
    fn backend_name(&self) -> &'static str;
}

/// The universal half-block renderer (ratatui `▀▄` cells).
pub struct HalfBlockRenderer;

impl PetRenderer for HalfBlockRenderer {
    fn draw(
        &mut self,
        frame: &mut Frame,
        herd: &Herd,
        species: &[Species],
        theme: Theme,
        now_ms: u64,
        hover_label: Option<&str>,
    ) {
        draw_herd(frame, herd, species, theme, now_ms, hover_label);
    }
    fn pet_at_column(
        &self,
        herd: &Herd,
        species: &[Species],
        strip_w: usize,
        col: u16,
        now_ms: u64,
    ) -> Option<usize> {
        pet_at_column(herd, species, strip_w, col, now_ms)
    }
    fn backend_name(&self) -> &'static str {
        "half-block"
    }
}

/// Choose the backend: forced kinds win; `Auto` probes and falls back to
/// half-blocks when kitty graphics are unavailable (herdr flag off, non-kitty
/// terminal, etc.).
pub fn select_renderer(
    kind: crate::config::RendererKind,
    caps: &mut dyn crate::caps::TerminalCaps,
    scale: usize,
) -> Box<dyn PetRenderer> {
    use crate::config::RendererKind::*;
    let use_kitty = match kind {
        HalfBlock => false,
        Kitty => true,
        Auto => caps.supports_kitty_graphics(),
    };
    if use_kitty {
        Box::new(crate::kitty_render::KittyRenderer::new(
            scale,
            Box::new(io::stdout()),
        ))
    } else {
        Box::new(HalfBlockRenderer)
    }
}

/// Focus the agent identified by `terminal_id` via `herdr agent focus`.
/// The caller swallows the error — a failed focus must never crash the strip.
pub fn focus_agent(cli: &dyn HerdrCli, terminal_id: &str) -> io::Result<()> {
    cli.run_json(&["agent", "focus", terminal_id]).map(|_| ())
}

/// Render thread: ~12 fps tick. Drains snapshots, reconciles, steps the herd,
/// draws, handles mouse hover/click, and quits on `q`/Ctrl-C. Restores the
/// terminal (raw mode, alternate screen, mouse capture) on exit.
#[allow(clippy::too_many_arguments)]
pub fn run(
    rx: Receiver<Vec<Agent>>,
    species: Vec<Species>,
    theme: Theme,
    focus: Box<dyn HerdrCli>,
    reduced_motion: bool,
    renderer_kind: crate::config::RendererKind,
    pet_scale: usize,
    sound_cfg: crate::config::SoundConfig,
    sound_player: Box<dyn crate::sound::SoundPlayer>,
) -> io::Result<()> {
    enable_raw_mode()?;

    // Probe for kitty support BEFORE entering the alternate screen and enabling
    // mouse capture: the query/DA round-trip then happens on the main screen
    // with no mouse-event bytes to wade through, and any reply is fully consumed
    // before rendering starts. Raw mode (enabled above) is all the probe needs.
    let mut caps = crate::caps::RealCaps::new();
    let mut renderer = select_renderer(renderer_kind, &mut caps, pet_scale);

    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    let result = run_loop(
        &mut terminal,
        rx,
        &species,
        theme,
        focus.as_ref(),
        reduced_motion,
        renderer.as_mut(),
        &sound_cfg,
        sound_player.as_ref(),
    );

    let _ = renderer.teardown(); // best-effort: deletes any transmitted kitty images
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    result
}

#[allow(clippy::too_many_arguments)]
fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    rx: Receiver<Vec<Agent>>,
    species: &[Species],
    theme: Theme,
    focus: &dyn HerdrCli,
    reduced_motion: bool,
    renderer: &mut dyn PetRenderer,
    sound_cfg: &crate::config::SoundConfig,
    sound_player: &dyn crate::sound::SoundPlayer,
) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    let tick = Duration::from_millis(83); // ~12 fps
    let species_count = species.len().max(1);
    let mut herd = Herd::new();
    let mut hovered: Option<String> = None;
    loop {
        let mut transitions = Vec::new();
        while let Ok(agents) = rx.try_recv() {
            transitions.extend(herd.reconcile(&agents, species_count));
        }
        if !transitions.is_empty() {
            let sounds = crate::sound::sounds_to_play(&transitions, sound_cfg);
            crate::sound::play_all(sound_player, &sounds);
        }
        // Reduced motion freezes every pet at one fixed instant (0) instead of
        // the live clock — `motion::animate` is a pure function of this value,
        // so "frozen" falls out for free with no separate code path.
        let now_ms = if reduced_motion {
            0
        } else {
            wall_clock_now_ms()
        };
        let strip_w = terminal.size()?.width as usize;
        let caption = hovered.clone();
        terminal.draw(|f| {
            renderer.draw(f, &herd, species, theme, now_ms, caption.as_deref());
        })?;

        if event::poll(tick)? {
            match event::read()? {
                Event::Key(k) => {
                    let quit = k.code == KeyCode::Char('q')
                        || (k.code == KeyCode::Char('c')
                            && k.modifiers.contains(KeyModifiers::CONTROL));
                    if quit {
                        return Ok(());
                    }
                }
                Event::Mouse(MouseEvent { kind, column, .. }) => match kind {
                    MouseEventKind::Moved => {
                        hovered = renderer
                            .pet_at_column(&herd, species, strip_w, column, now_ms)
                            .map(|i| herd.pets[i].label.clone());
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        if let Some(i) =
                            renderer.pet_at_column(&herd, species, strip_w, column, now_ms)
                        {
                            let tid = herd.pets[i].terminal_id.clone();
                            // Swallow focus errors: the strip must keep running.
                            let _ = focus_agent(focus, &tid);
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, AgentStatus};
    use crate::herd::Herd;
    use crate::identity::identity_for;
    use crate::palette::Theme;
    use crate::pet::Pet;
    use crate::sprite::parse_species;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    const BLOB: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/sprites/test-blob.sprite"
    ));

    /// An arbitrary fixed instant: snapshot tests just need *some* stable
    /// value, since `motion::animate` is pure (same inputs, same pixels).
    const NOW_MS: u64 = 1_700_000_000_000;

    fn agent(tid: &str, s: AgentStatus) -> Agent {
        Agent {
            agent: None,
            agent_status: s,
            name: None,
            cwd: "/".into(),
            foreground_cwd: "/".into(),
            workspace_id: "w".into(),
            tab_id: "t".into(),
            pane_id: "p".into(),
            terminal_id: tid.into(),
            revision: 0,
            focused: false,
            hover_label: None,
        }
    }

    fn fixed_herd(states: &[AgentStatus]) -> Herd {
        let mut h = Herd::new();
        let agents: Vec<_> = states
            .iter()
            .enumerate()
            .map(|(i, s)| agent(&format!("t{i}"), *s))
            .collect();
        h.reconcile(&agents, 1);
        h
    }

    #[test]
    fn renders_each_state_in_the_strip() {
        use AgentStatus::*;
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[Idle, Working, Done, Blocked, Unknown]);
        let mut terminal = Terminal::new(TestBackend::new(90, 11)).unwrap();
        terminal
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark, NOW_MS, None))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn renders_a_hat_above_the_focused_pet_and_nothing_above_the_rest() {
        use AgentStatus::*;
        let species = vec![parse_species(BLOB).unwrap()];
        let mut h = Herd::new();
        let agents: Vec<_> = [Working, Idle]
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let mut a = agent(&format!("t{i}"), *s);
                a.focused = i == 0; // only the first pet is focused
                a
            })
            .collect();
        h.reconcile(&agents, 1);
        let mut terminal = Terminal::new(TestBackend::new(40, 11)).unwrap();
        terminal
            .draw(|f| draw_herd(f, &h, &species, Theme::Dark, NOW_MS, None))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn renders_overflow_counter() {
        use AgentStatus::*;
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[Idle; 20]);
        let mut terminal = Terminal::new(TestBackend::new(40, 11)).unwrap();
        terminal
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark, NOW_MS, None))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn reconcile_then_draw_shows_the_incoming_herd() {
        // A focused integration check: feed one snapshot, reconcile, draw, snapshot.
        use crate::agent::AgentStatus::*;
        let species = vec![crate::sprite::parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        herd.reconcile(&[agent("a", Working), agent("b", Blocked)], 1);
        let mut terminal = Terminal::new(TestBackend::new(60, 11)).unwrap();
        terminal
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark, NOW_MS, None))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn pet_at_column_returns_the_topmost_pet_when_they_overlap() {
        let species = vec![parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        herd.pets.push(Pet::new(
            "idle".into(),
            identity_for("idle", 1),
            AgentStatus::Idle,
        ));
        herd.pets.push(Pet::new(
            "blk".into(),
            identity_for("blk", 1),
            AgentStatus::Blocked,
        ));
        // A strip narrower than 2x the pet width (test-blob is 4px wide)
        // guarantees any two pets' hit ranges intersect, however their
        // identity-derived rest positions land — no need to know the exact
        // hash values.
        let strip_w = 7usize;
        let max_x = (strip_w as f32 - 4.0).max(0.0);
        let sp = &species[0];
        let idle_left = (animate(
            "idle",
            AgentStatus::Idle,
            &sp.states[&AgentStatus::Idle],
            NOW_MS,
        )
        .x_fraction
            * max_x)
            .round() as i32;
        let blk_left = (animate(
            "blk",
            AgentStatus::Blocked,
            &sp.states[&AgentStatus::Blocked],
            NOW_MS,
        )
        .x_fraction
            * max_x)
            .round() as i32;
        let overlap_col = idle_left.max(blk_left) as u16;

        let hit = pet_at_column(&herd, &species, strip_w, overlap_col, NOW_MS)
            .expect("a pet under the overlap column");
        assert_eq!(
            herd.pets[hit].terminal_id, "blk",
            "blocked draws on top, so it wins the hit"
        );
    }

    #[test]
    fn pet_at_column_breaks_ties_by_draw_order_topmost_wins() {
        let species = vec![parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        // Same-priority overlap: "b" is pushed later, so the stable sort in
        // draw_herd keeps it later in z-order and it draws on top. Same
        // narrow-strip trick as above forces the overlap.
        herd.pets.push(Pet::new(
            "a".into(),
            identity_for("a", 1),
            AgentStatus::Idle,
        ));
        herd.pets.push(Pet::new(
            "b".into(),
            identity_for("b", 1),
            AgentStatus::Idle,
        ));
        let strip_w = 7usize;
        let max_x = (strip_w as f32 - 4.0).max(0.0);
        let sp = &species[0];
        let idle_state = &sp.states[&AgentStatus::Idle];
        let a_left =
            (animate("a", AgentStatus::Idle, idle_state, NOW_MS).x_fraction * max_x).round() as i32;
        let b_left =
            (animate("b", AgentStatus::Idle, idle_state, NOW_MS).x_fraction * max_x).round() as i32;
        let overlap_col = a_left.max(b_left) as u16;

        let hit = pet_at_column(&herd, &species, strip_w, overlap_col, NOW_MS)
            .expect("a pet under the overlap column");
        assert_eq!(
            herd.pets[hit].terminal_id, "b",
            "later-pushed same-priority pet draws on top, so it wins the hit"
        );
    }

    /// Row `y` as one `String` of cell symbols, plus each cell's foreground —
    /// lets a test assert on text and color together from a `Buffer` directly
    /// (`TestBackend`'s `Display` only exposes symbols).
    fn row_text_and_fg(buffer: &ratatui::buffer::Buffer, y: u16) -> (String, Vec<Color>) {
        let text = (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol().chars().next().unwrap_or(' '))
            .collect();
        let fg = (0..buffer.area.width).map(|x| buffer[(x, y)].fg).collect();
        (text, fg)
    }

    #[test]
    fn caption_renders_top_right_in_ochre() {
        // A pane exactly the band's height + 1 lane row, so the lane sits at
        // row 0 (matching `overlays_never_occlude_the_pet`'s convention).
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Working, AgentStatus::Idle]);
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let completed = terminal
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark, NOW_MS, Some("backend-api")))
            .unwrap();
        let (row0, fg) = row_text_and_fg(completed.buffer, 0);
        assert!(
            row0.trim_end().ends_with("backend-api"),
            "caption is right-aligned in the top lane (row 0): {row0:?}"
        );
        let label_start = row0.find("backend-api").unwrap();
        assert_eq!(
            fg[label_start],
            Color::Rgb(0xd9, 0xa4, 0x41),
            "caption is ochre"
        );
    }

    #[test]
    fn caption_no_longer_reserves_the_bottom_row() {
        // Freeing the bottom row means the pet band now bottom-aligns exactly
        // to the pane floor: the last row is pet pixels, not blank/caption.
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Working, AgentStatus::Idle]);
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        terminal
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark, NOW_MS, Some("backend-api")))
            .unwrap();
        let rows = rows_of(terminal.backend());
        let last = rows.last().unwrap();
        assert!(
            !last.contains("backend-api"),
            "caption no longer occupies the bottom row: {last:?}"
        );
        assert!(
            last.contains('▀') || last.contains('▄'),
            "the band now bottom-aligns to the pane floor: {last:?}"
        );
    }

    #[test]
    fn caption_truncates_and_stays_left_of_the_overflow_marker() {
        // `Working` carries no overlay glyph (unlike `Idle`'s "Zz" bubble),
        // so the top lane holds only the caption and the `+N` marker.
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Working; 30]);
        let mut terminal = Terminal::new(TestBackend::new(24, 10)).unwrap();
        terminal
            .draw(|f| {
                draw_herd(
                    f,
                    &herd,
                    &species,
                    Theme::Dark,
                    NOW_MS,
                    Some("a-very-long-agent-name-that-does-not-fit"),
                )
            })
            .unwrap();
        let rows = rows_of(terminal.backend());
        let row0 = &rows[0];
        assert!(row0.contains('+'), "the +N marker is still shown");
        let plus_at = row0.find('+').unwrap();
        let last_caption_char = row0[..plus_at].trim_end().len();
        assert!(
            last_caption_char < plus_at,
            "caption stays left of the +N marker with a gap: {row0:?}"
        );
        assert!(row0.chars().count() <= 24, "never overruns the strip width");
    }

    /// Dump a TestBackend as one `String` per terminal row (symbols only).
    fn rows_of<B: std::fmt::Display>(backend: &B) -> Vec<String> {
        format!("{backend}")
            .lines()
            .map(|l| l.trim().trim_matches('"').to_string())
            .collect()
    }

    #[test]
    fn overlays_never_occlude_the_pet() {
        // A done pet shows a '!' badge. The badge must live in the top lane
        // (row 0) with NO pet pixels, and the pet must be drawn in the band
        // below (row 1+), so the icon can never cover the animal.
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Done]);
        let mut terminal = Terminal::new(TestBackend::new(30, 10)).unwrap();
        terminal
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark, NOW_MS, None))
            .unwrap();
        let rows = rows_of(terminal.backend());
        assert!(rows[0].contains('!'), "badge sits in the top lane (row 0)");
        assert!(
            !rows[0].contains('▀') && !rows[0].contains('▄'),
            "the top lane holds no pet pixels — nothing to occlude"
        );
        let band_has_pet = rows[1..].iter().any(|r| r.contains('▀') || r.contains('▄'));
        assert!(band_has_pet, "the pet is drawn in the band below the lane");
    }

    #[test]
    fn overflow_counter_lives_in_the_top_lane() {
        // With many pets, +N must render in the reserved top lane (row 0),
        // never mid-band on top of a pet.
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Idle; 30]);
        let mut terminal = Terminal::new(TestBackend::new(24, 10)).unwrap();
        terminal
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark, NOW_MS, None))
            .unwrap();
        let rows = rows_of(terminal.backend());
        assert!(rows[0].contains('+'), "the +N marker is in the top lane");
    }

    #[test]
    fn pet_at_column_is_none_over_a_gap() {
        let species = vec![parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        herd.pets.push(Pet::new(
            "a".into(),
            identity_for("a", 1),
            AgentStatus::Idle,
        ));
        // A pet's occupied range never reaches past `strip_w` itself (its
        // rest fraction is in [0,1] of the walkable width), so the column
        // right at the strip's edge is a gap regardless of the identity hash.
        assert!(
            pet_at_column(&herd, &species, 200, 200, NOW_MS).is_none(),
            "column past the strip's edge is empty"
        );
    }

    use crate::herdr::{CommandRunner, LiveHerdr};
    use std::cell::RefCell;
    use std::ffi::OsStr;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{ExitStatus, Output};
    use std::rc::Rc;

    struct Recorder {
        args: Rc<RefCell<Vec<String>>>,
    }
    impl CommandRunner for Recorder {
        fn run(&self, _program: &OsStr, args: &[&str]) -> std::io::Result<Output> {
            *self.args.borrow_mut() = args.iter().map(|s| s.to_string()).collect();
            Ok(Output {
                status: ExitStatus::from_raw(0),
                stdout: b"{\"result\":{}}".to_vec(),
                stderr: Vec::new(),
            })
        }
    }

    #[test]
    fn left_facing_pet_is_mirrored() {
        // A working pet's facing flips as it wanders back and forth. Find two
        // instants where the same agent faces opposite ways (motion::animate
        // as an oracle, rather than forcing the field directly — it's derived
        // now, not stored), and assert the rendered band differs between them.
        use crate::agent::AgentStatus::Working;
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[Working]);
        let state = &species[0].states[&Working];
        let right_ms = (0..80_000u64)
            .step_by(97)
            .find(|&ms| !animate("t0", Working, state, ms).facing_left)
            .expect("some instant facing right");
        let left_ms = (0..80_000u64)
            .step_by(97)
            .find(|&ms| animate("t0", Working, state, ms).facing_left)
            .expect("some instant facing left");
        let render = |ms: u64| {
            let mut t = Terminal::new(TestBackend::new(40, 10)).unwrap();
            t.draw(|f| draw_herd(f, &herd, &species, Theme::Dark, ms, None))
                .unwrap();
            format!("{}", t.backend())
        };
        assert_ne!(
            render(right_ms),
            render(left_ms),
            "mirroring must change the pixels"
        );
    }

    #[test]
    fn focus_agent_shells_agent_focus_with_the_terminal_id() {
        let args = Rc::new(RefCell::new(Vec::new()));
        let cli = LiveHerdr::with_runner(
            "herdr",
            Recorder {
                args: Rc::clone(&args),
            },
        );
        focus_agent(&cli, "term_abc").unwrap();
        assert_eq!(*args.borrow(), vec!["agent", "focus", "term_abc"]);
    }

    #[test]
    fn auto_picks_kitty_when_supported_else_half_block() {
        use crate::caps::FakeCaps;
        use crate::config::RendererKind;
        let is_kitty = |r: &dyn PetRenderer| r.backend_name() == "kitty";
        let mut yes = FakeCaps { supported: true };
        assert!(is_kitty(
            select_renderer(RendererKind::Auto, &mut yes, 7).as_ref()
        ));
        let mut no = FakeCaps { supported: false };
        assert!(!is_kitty(
            select_renderer(RendererKind::Auto, &mut no, 7).as_ref()
        ));
        // Forced modes ignore the probe:
        let mut yes2 = FakeCaps { supported: true };
        assert!(!is_kitty(
            select_renderer(RendererKind::HalfBlock, &mut yes2, 7).as_ref()
        ));
    }

    #[test]
    fn half_block_renderer_matches_the_free_function() {
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Working, AgentStatus::Blocked]);
        let mut via_trait = Terminal::new(TestBackend::new(60, 11)).unwrap();
        via_trait
            .draw(|f| HalfBlockRenderer.draw(f, &herd, &species, Theme::Dark, NOW_MS, None))
            .unwrap();
        let mut via_fn = Terminal::new(TestBackend::new(60, 11)).unwrap();
        via_fn
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark, NOW_MS, None))
            .unwrap();
        assert_eq!(
            format!("{}", via_trait.backend()),
            format!("{}", via_fn.backend())
        );
    }

    #[test]
    fn draw_herd_does_not_panic_in_a_pane_shorter_than_the_band() {
        // Growing PET_PX_H for the focus hat also grew the band's minimum
        // height; a pane too short to fit it must crop gracefully instead of
        // handing draw_pixels a Rect that overruns the real frame buffer.
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Working, AgentStatus::Blocked]);
        let mut terminal = Terminal::new(TestBackend::new(60, 8)).unwrap();
        terminal
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark, NOW_MS, None))
            .unwrap();
    }

    /// Total non-transparent pixels in `HAT_ROWS` — how many hat pixels should
    /// land in the buffer when the hat is drawn with no clipping at all.
    const HAT_PIXEL_COUNT: usize = 15;

    fn count_hat_pixels(buf: &PixelBuf) -> usize {
        buf.px
            .iter()
            .filter(|p| matches!(p, Some(c) if *c == HAT_OUTLINE || *c == HAT_FILL))
            .count()
    }

    /// Scan `now_ms` in `[0, period_ms)` for the instant where `state`'s
    /// motion lifts `terminal_id` highest (most negative `offset.dy`) — the
    /// tightest case for top-clipping. Uses `motion::animate` as an oracle
    /// (same trick as `left_facing_pet_is_mirrored` below) rather than
    /// reverse-engineering `identity::unit_hash`.
    fn peak_lift_ms(
        terminal_id: &str,
        status: AgentStatus,
        state: &crate::sprite::StateSpec,
        period_ms: u64,
    ) -> u64 {
        (0..period_ms)
            .step_by(37)
            .min_by(|&a, &b| {
                let dy_a = animate(terminal_id, status, state, a).offset.dy;
                let dy_b = animate(terminal_id, status, state, b).offset.dy;
                dy_a.partial_cmp(&dy_b).unwrap()
            })
            .expect("a non-empty scan range")
    }

    #[test]
    fn build_band_draws_a_hat_only_for_the_focused_pet() {
        let species = vec![parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        // "wearer"'s hash-derived rest position sits safely away from either
        // edge (unlike e.g. "f", which happens to rest flush against the
        // right edge and would clip the hat there) — this test is about
        // focused-vs-unfocused, not edge clipping (see the dedicated
        // clipping tests below).
        let mut focused = Pet::new(
            "wearer".into(),
            identity_for("wearer", 1),
            AgentStatus::Idle,
        );
        focused.focused = true;
        herd.pets.push(focused);
        herd.pets.push(Pet::new(
            "unfocused".into(),
            identity_for("unfocused", 1),
            AgentStatus::Idle,
        ));
        let (buf, _order) = build_band(&herd, &species, 50, Theme::Dark, NOW_MS);
        assert_eq!(
            count_hat_pixels(&buf),
            HAT_PIXEL_COUNT,
            "exactly one hat is drawn, for the focused pet"
        );
    }

    #[test]
    fn hat_is_never_clipped_at_the_top_even_when_the_sprite_has_no_headroom_row() {
        // TestBlob's working frame paints all the way up to row 0 (`MM..`),
        // the worst case for top clipping: no spare row inside the frame
        // itself. Scan for the instant of peak hop lift, which pushes the
        // sprite as high as it ever goes.
        let species = vec![parse_species(BLOB).unwrap()];
        let state = &species[0].states[&AgentStatus::Working];
        let peak_ms = peak_lift_ms("f", AgentStatus::Working, state, 20_000);
        let mut herd = Herd::new();
        let mut pet = Pet::new("f".into(), identity_for("f", 1), AgentStatus::Working);
        pet.focused = true;
        herd.pets.push(pet);
        let (buf, _order) = build_band(&herd, &species, 40, Theme::Dark, peak_ms);
        assert_eq!(
            count_hat_pixels(&buf),
            HAT_PIXEL_COUNT,
            "every hat pixel survives — none fell off the top of the band"
        );
    }

    #[test]
    fn hat_is_never_clipped_on_the_real_sheep_standing_pose_mid_bounce() {
        // The acceptance criterion's tight case: the real (16x14) standing
        // pose, which has only ~1 empty row above the head, at peak bounce lift.
        let species = crate::sprite::embedded_species();
        let sheep_index = species
            .iter()
            .position(|s| s.name == "Sheep")
            .expect("Sheep is embedded");
        let state = &species[sheep_index].states[&AgentStatus::Blocked];
        let peak_ms = peak_lift_ms("f", AgentStatus::Blocked, state, 20_000);
        let mut herd = Herd::new();
        let mut pet = Pet::new(
            "f".into(),
            crate::identity::Identity {
                species_index: sheep_index,
                hue: 0,
            },
            AgentStatus::Blocked,
        );
        pet.focused = true;
        herd.pets.push(pet);
        let (buf, _order) = build_band(&herd, &species, 40, Theme::Dark, peak_ms);
        assert_eq!(
            count_hat_pixels(&buf),
            HAT_PIXEL_COUNT,
            "the hat fits above the standing pose's head even mid-bounce"
        );
    }

    #[test]
    fn hat_renders_on_top_of_the_idle_dozing_lump() {
        let species = crate::sprite::embedded_species();
        let sheep_index = species.iter().position(|s| s.name == "Sheep").unwrap();
        let mut herd = Herd::new();
        let mut pet = Pet::new(
            "f".into(),
            crate::identity::Identity {
                species_index: sheep_index,
                hue: 0,
            },
            AgentStatus::Idle,
        );
        pet.focused = true;
        herd.pets.push(pet);
        let (buf, _order) = build_band(&herd, &species, 40, Theme::Dark, NOW_MS);
        assert_eq!(
            count_hat_pixels(&buf),
            HAT_PIXEL_COUNT,
            "the hat renders above the dozing lump, not clipped or hidden"
        );
    }

    #[test]
    fn hat_never_clips_on_the_goat_despite_its_taller_horned_silhouette() {
        // The goat's horns sit a row above the sheep's head, so its head
        // anchor is one row higher — the generic topmost-opaque-pixel scan
        // must pick that up on its own, with no per-species table, and the
        // reserved headroom must still cover it, even at peak bounce lift.
        let species = crate::sprite::embedded_species();
        let goat_index = species
            .iter()
            .position(|s| s.name == "Goat")
            .expect("Goat is embedded");
        let state = &species[goat_index].states[&AgentStatus::Blocked];
        let peak_ms = peak_lift_ms("f", AgentStatus::Blocked, state, 20_000);
        let mut herd = Herd::new();
        let mut pet = Pet::new(
            "f".into(),
            crate::identity::Identity {
                species_index: goat_index,
                hue: 0,
            },
            AgentStatus::Blocked,
        );
        pet.focused = true;
        herd.pets.push(pet);
        let (buf, _order) = build_band(&herd, &species, 40, Theme::Dark, peak_ms);
        assert_eq!(
            count_hat_pixels(&buf),
            HAT_PIXEL_COUNT,
            "the hat fits above the goat's horns even mid-bounce"
        );
    }

    #[test]
    fn head_anchor_column_flips_with_facing() {
        // TestBlob's working frame is asymmetric (`MM../MMM./M##./.MM.`), so a
        // facing flip must move the head anchor's column — otherwise the hat
        // would stay put while the body mirrors underneath it.
        let species = parse_species(BLOB).unwrap();
        let fr = &species.states[&AgentStatus::Working].frames[0];
        let (_row_right, col_right) = head_anchor(fr, false);
        let (_row_left, col_left) = head_anchor(fr, true);
        assert_ne!(col_right, col_left, "facing flip must move the head anchor");
    }
}
