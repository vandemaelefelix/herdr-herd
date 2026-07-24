//! Half-block renderer: blit the roaming herd into a pixel buffer, emit it as
//! `▀` cells (fg = top pixel, bg = bottom pixel), then overlay state bubbles/
//! badges and a `+N` counter.

use std::io;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

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
use crate::anim::{Overlay, OverlayColor, Rgb, motion_offset};
use crate::herd::{Herd, Lcg, visible_and_hidden};
use crate::herdr::HerdrCli;
use crate::palette::{StateStyle, Theme, role_color};
use crate::pet::priority;
use crate::sprite::Species;

/// Height of the pet band in pixels. Sprites are 16x14 (see sprites/*.sprite);
/// the band is the sprite height plus 1px of headroom for the hop/shake lift.
pub const PET_PX_H: usize = 15;

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

/// Draw the whole strip: visible pets in priority z-order (blocked draws
/// last, i.e. on top), their overlays (bubbles/badges), and a `+N` marker for
/// any pets the strip has no room for. Overlays and `+N` live in a reserved top
/// lane (row 0); the pet band is drawn below it, so an icon never covers a pet.
pub fn draw_herd(frame: &mut Frame, herd: &Herd, species: &[Species], theme: Theme) {
    let area = frame.area();
    let strip_w = area.width as usize;
    let mut buf = PixelBuf::new(strip_w, PET_PX_H);

    let pet_w = species.first().map(|s| s.size().0).unwrap_or(12);
    let capacity = (strip_w / (pet_w * 3 / 4).max(1)).max(1);
    let (visible, hidden) = visible_and_hidden(&herd.pets, capacity);

    // z-order: lowest priority first so blocked draws last (on top).
    let mut order = visible.clone();
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
        let fi = pet.frame_index(state.frames.len());
        let fr = &state.frames[fi];
        let style = StateStyle {
            dim: state.dim,
            ghost: state.ghost,
        };
        let off = motion_offset(&state.motion, pet.phase);
        let ox = (pet.x + off.dx).round() as i32;
        // Bottom-anchor: feet rest on the band floor; motion (dy<=0) lifts the
        // pet up into the headroom above it, so a hop/shake never clips.
        let floor = PET_PX_H as i32 - fr.h as i32;
        let oy = floor + off.dy.round() as i32;
        for y in 0..fr.h {
            for x in 0..fr.w {
                let sx = if pet.facing_left { fr.w - 1 - x } else { x };
                if let Some(c) = role_color(fr.cells[y * fr.w + sx], pet.identity.hue, theme, style)
                {
                    buf.set(ox + x as i32, oy + y as i32, c);
                }
            }
        }
    }
    // Bottom-align the whole strip so it reads as a slim status line whatever
    // the pane's height (herdr enforces a minimum pane height, so the pane can be
    // taller than the content needs): the caption is the bottom row, the pet band
    // sits just above it, and the icon lane sits just above the band. Any extra
    // rows fall at the top, blending with the pane above. The icon lane keeps
    // overlays/`+N` off the pet.
    let band_rows = PET_PX_H.div_ceil(2) as u16;
    let caption_row = area.bottom().saturating_sub(1);
    let band_top = caption_row.saturating_sub(band_rows);
    let lane_y = band_top.saturating_sub(1);
    let pet_area = Rect {
        x: area.x,
        y: band_top,
        width: area.width,
        height: band_rows,
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
        let cx = area.x
            + (pet.x.round() as u16)
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
}

/// Draw the hover caption on the strip's bottom row: the hovered pet's label,
/// or nothing when `label` is `None`. It has its own row so hovering never
/// shifts the herd; the label is truncated to the strip width.
pub fn draw_caption(frame: &mut Frame, area: Rect, label: Option<&str>) {
    let Some(label) = label else { return };
    if area.height == 0 || area.width == 0 {
        return;
    }
    let y = area.bottom() - 1;
    let text: String = label.chars().take(area.width as usize).collect();
    let w = text.chars().count() as u16;
    frame.buffer_mut().set_span(
        area.x,
        y,
        &Span::styled(text, Style::default().fg(Color::Gray)),
        w,
    );
}

/// The index of the visible pet drawn under terminal column `col`, if any.
/// A mouse column maps 1:1 to a pixel x (half-block cells are one pixel wide).
/// Only pets that are actually drawn (the visible set on overflow) are
/// hit-testable; when pets overlap, the topmost — highest `priority`, matching
/// the draw z-order — wins. Returns `None` over a gap or out of range.
pub fn pet_at_column(herd: &Herd, species: &[Species], strip_w: usize, col: u16) -> Option<usize> {
    let base_w = species.first().map(|s| s.size().0).unwrap_or(12);
    let capacity = (strip_w / (base_w * 3 / 4).max(1)).max(1);
    let (visible, _hidden) = visible_and_hidden(&herd.pets, capacity);

    let x = col as i32;
    let mut best: Option<usize> = None;
    for &i in &visible {
        let pet = &herd.pets[i];
        let w = species
            .get(pet.identity.species_index)
            .or_else(|| species.first())
            .map(|s| s.size().0)
            .unwrap_or(base_w) as i32;
        let left = pet.x.round() as i32;
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
    /// Draw the whole strip for this frame (pet band + overlays + `+N`).
    fn draw(&mut self, frame: &mut Frame, herd: &Herd, species: &[Species], theme: Theme);
    /// The visible pet under terminal column `col`, if any (for hover/click).
    fn pet_at_column(
        &self,
        herd: &Herd,
        species: &[Species],
        strip_w: usize,
        col: u16,
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
    fn draw(&mut self, frame: &mut Frame, herd: &Herd, species: &[Species], theme: Theme) {
        draw_herd(frame, herd, species, theme);
    }
    fn pet_at_column(
        &self,
        herd: &Herd,
        species: &[Species],
        strip_w: usize,
        col: u16,
    ) -> Option<usize> {
        pet_at_column(herd, species, strip_w, col)
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
        Box::new(crate::kitty_render::KittyRenderer::new_stdout(scale))
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
pub fn run(
    rx: Receiver<Vec<Agent>>,
    species: Vec<Species>,
    theme: Theme,
    focus: Box<dyn HerdrCli>,
    reduced_motion: bool,
    renderer_kind: crate::config::RendererKind,
    pet_scale: usize,
) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    // The kitty-graphics probe reads/writes the tty once, here, before the
    // event loop starts (raw mode must already be enabled for it to work).
    let mut caps = crate::caps::RealCaps::new();
    let mut renderer = select_renderer(renderer_kind, &mut caps, pet_scale);
    let result = run_loop(
        &mut terminal,
        rx,
        &species,
        theme,
        focus.as_ref(),
        reduced_motion,
        renderer.as_mut(),
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

/// Advance the herd one tick: roam + animation phase. A no-op under
/// `reduced_motion`, which freezes both horizontal wander and the per-frame
/// bounce/shake/breathe (phase stays 0, so `motion_offset` stays zero).
fn simulate_tick(
    herd: &mut Herd,
    species: &[Species],
    dt_ms: f32,
    w: f32,
    pet_w: f32,
    rng: &mut dyn crate::herd::Rng,
    reduced_motion: bool,
) {
    if reduced_motion {
        return;
    }
    herd.step(dt_ms, w, pet_w, rng);
    for p in herd.pets.iter_mut() {
        let fm = species
            .get(p.identity.species_index)
            .or_else(|| species.first())
            .and_then(|s| s.states.get(&p.status))
            .map(|st| st.frame_ms)
            .unwrap_or(0);
        p.advance(dt_ms, fm);
    }
}

fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    rx: Receiver<Vec<Agent>>,
    species: &[Species],
    theme: Theme,
    focus: &dyn HerdrCli,
    reduced_motion: bool,
    renderer: &mut dyn PetRenderer,
) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    let tick = Duration::from_millis(83); // ~12 fps
    let species_count = species.len().max(1);
    // Same pet_w feeds both reconcile's spawn bound and step's clamp bound,
    // so a freshly-spawned pet never lands outside the walkable strip.
    let pet_w = species.first().map(|s| s.size().0).unwrap_or(12) as f32;
    let mut herd = Herd::new();
    let mut rng = Lcg::new(0xC0FFEE);
    let mut last = Instant::now();
    let mut hovered: Option<String> = None;
    loop {
        while let Ok(agents) = rx.try_recv() {
            let w = terminal.size()?.width as f32;
            herd.reconcile(&agents, species_count, w, pet_w, &mut rng);
        }
        let now = Instant::now();
        let dt_ms = (now - last).as_millis() as f32;
        last = now;
        let w = terminal.size()?.width as f32;
        simulate_tick(
            &mut herd,
            species,
            dt_ms,
            w,
            pet_w,
            &mut rng,
            reduced_motion,
        );
        let strip_w = terminal.size()?.width as usize;
        let caption = hovered.clone();
        terminal.draw(|f| {
            renderer.draw(f, &herd, species, theme);
            draw_caption(f, f.area(), caption.as_deref());
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
                            .pet_at_column(&herd, species, strip_w, column)
                            .map(|i| herd.pets[i].label.clone());
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        if let Some(i) = renderer.pet_at_column(&herd, species, strip_w, column) {
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
    use crate::herd::{Herd, Lcg};
    use crate::palette::Theme;
    use crate::sprite::parse_species;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    const BLOB: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/sprites/test-blob.sprite"
    ));

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
        let mut rng = Lcg::new(1);
        let agents: Vec<_> = states
            .iter()
            .enumerate()
            .map(|(i, s)| agent(&format!("t{i}"), *s))
            .collect();
        h.reconcile(&agents, 1, 120.0, 16.0, &mut rng);
        // Freeze positions + phase for a deterministic snapshot.
        for (i, p) in h.pets.iter_mut().enumerate() {
            p.x = 4.0 + i as f32 * 16.0;
            p.target_x = p.x;
            p.phase = 0.0;
        }
        h
    }

    #[test]
    fn renders_each_state_in_the_strip() {
        use AgentStatus::*;
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[Idle, Working, Done, Blocked, Unknown]);
        let mut terminal = Terminal::new(TestBackend::new(90, 11)).unwrap();
        terminal
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark))
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
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn reconcile_then_draw_shows_the_incoming_herd() {
        // A focused integration check: feed one snapshot, reconcile, draw, snapshot.
        use crate::agent::AgentStatus::*;
        use crate::herd::{Herd, Lcg};
        let species = vec![crate::sprite::parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        let mut rng = Lcg::new(3);
        herd.reconcile(
            &[agent("a", Working), agent("b", Blocked)],
            1,
            60.0,
            16.0,
            &mut rng,
        );
        for (i, p) in herd.pets.iter_mut().enumerate() {
            p.x = 4.0 + i as f32 * 18.0;
            p.target_x = p.x;
        }
        let mut terminal = Terminal::new(TestBackend::new(60, 11)).unwrap();
        terminal
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    use crate::identity::identity_for;
    use crate::pet::Pet;

    #[test]
    fn pet_at_column_returns_the_topmost_pet_when_they_overlap() {
        let species = vec![parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        // Two overlapping pets near x=10: idle (low priority) and blocked (high).
        herd.pets.push(Pet::new(
            "idle".into(),
            identity_for("idle", 1),
            AgentStatus::Idle,
            10.0,
        ));
        herd.pets.push(Pet::new(
            "blk".into(),
            identity_for("blk", 1),
            AgentStatus::Blocked,
            12.0,
        ));
        let hit = pet_at_column(&herd, &species, 200, 13).expect("a pet under column 13");
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
        // draw_herd keeps it later in z-order and it draws on top.
        herd.pets.push(Pet::new(
            "a".into(),
            identity_for("a", 1),
            AgentStatus::Idle,
            10.0,
        ));
        herd.pets.push(Pet::new(
            "b".into(),
            identity_for("b", 1),
            AgentStatus::Idle,
            12.0,
        ));
        let hit = pet_at_column(&herd, &species, 200, 13).expect("a pet under column 13");
        assert_eq!(
            herd.pets[hit].terminal_id, "b",
            "later-pushed same-priority pet draws on top, so it wins the hit"
        );
    }

    #[test]
    fn caption_shows_the_hovered_name_on_the_bottom_row() {
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Working, AgentStatus::Idle]);
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        terminal
            .draw(|f| {
                draw_herd(f, &herd, &species, Theme::Dark);
                draw_caption(f, f.area(), Some("backend-api"));
            })
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
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
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark))
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
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark))
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
            4.0,
        ));
        assert!(
            pet_at_column(&herd, &species, 200, 150).is_none(),
            "column far past the pet is empty"
        );
    }

    #[test]
    fn simulate_tick_freezes_position_and_phase_under_reduced_motion() {
        let species = vec![parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        herd.pets.push(Pet::new(
            "a".into(),
            identity_for("a", 1),
            AgentStatus::Working,
            10.0,
        ));
        let mut rng = Lcg::new(1);
        let (x0, ph0) = (herd.pets[0].x, herd.pets[0].phase);
        simulate_tick(&mut herd, &species, 500.0, 200.0, 12.0, &mut rng, true);
        assert_eq!(herd.pets[0].x, x0, "reduced motion freezes x");
        assert_eq!(herd.pets[0].phase, ph0, "reduced motion freezes phase");
        // Motion on: phase advances (Working has frame_ms > 0).
        simulate_tick(&mut herd, &species, 500.0, 200.0, 12.0, &mut rng, false);
        assert!(
            herd.pets[0].phase != ph0,
            "phase advances when motion is on"
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
        // Build a herd with one working pet, force facing_left, freeze it, and
        // assert the rendered band differs from the same pet facing right.
        use crate::agent::AgentStatus::*;
        let species = vec![parse_species(BLOB).unwrap()];
        let mut right = fixed_herd(&[Working]);
        right.pets[0].facing_left = false;
        let mut left = fixed_herd(&[Working]);
        left.pets[0].facing_left = true;
        let render = |h: &Herd| {
            let mut t = Terminal::new(TestBackend::new(40, 10)).unwrap();
            t.draw(|f| draw_herd(f, h, &species, Theme::Dark)).unwrap();
            format!("{}", t.backend())
        };
        assert_ne!(
            render(&right),
            render(&left),
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
        let mut via_trait = Terminal::new(TestBackend::new(60, 8)).unwrap();
        via_trait
            .draw(|f| HalfBlockRenderer.draw(f, &herd, &species, Theme::Dark))
            .unwrap();
        let mut via_fn = Terminal::new(TestBackend::new(60, 8)).unwrap();
        via_fn
            .draw(|f| draw_herd(f, &herd, &species, Theme::Dark))
            .unwrap();
        assert_eq!(
            format!("{}", via_trait.backend()),
            format!("{}", via_fn.backend())
        );
    }
}
