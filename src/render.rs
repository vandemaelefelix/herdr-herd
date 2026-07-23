//! Half-block renderer: blit the roaming herd into a pixel buffer, emit it as
//! `▀` cells (fg = top pixel, bg = bottom pixel), then overlay state bubbles/
//! badges and a `+N` counter.

use std::io;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
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
use crate::palette::{StateStyle, Theme, role_color};
use crate::pet::priority;
use crate::sprite::Species;

/// Height of the sprite draw strip in pixels (6 half-block rows).
pub const PET_PX_H: usize = 12;

/// A pixel canvas: `w * h` optional colors, row-major. `None` = transparent.
pub struct PixelBuf {
    pub w: usize,
    pub h: usize,
    pub px: Vec<Option<Rgb>>,
}

impl PixelBuf {
    /// A fully-transparent buffer of `w` by `h` pixels.
    pub fn new(w: usize, h: usize) -> Self {
        Self { w, h, px: vec![None; w * h] }
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
            let bot = if ry * 2 + 1 < buf.h { buf.px[(ry * 2 + 1) * buf.w + x] } else { None };
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
/// any pets the strip has no room for.
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
        let Some(sp) = species.get(pet.identity.species_index).or_else(|| species.first()) else {
            continue;
        };
        let Some(state) = sp.states.get(&pet.status) else { continue };
        let fi = pet.frame_index(state.frames.len());
        let fr = &state.frames[fi];
        let style = StateStyle { dim: state.dim, ghost: state.ghost };
        let off = motion_offset(&state.motion, pet.phase);
        let ox = (pet.x + off.dx).round() as i32;
        let oy = (off.dy).round() as i32; // ground-aligned; dy<=0 lifts
        for y in 0..fr.h {
            for x in 0..fr.w {
                if let Some(c) = role_color(fr.cells[y * fr.w + x], pet.identity.hue, theme, style)
                {
                    buf.set(ox + x as i32, oy + y as i32, c);
                }
            }
        }
    }
    draw_pixels(frame, area, &buf);

    // Overlays (bubbles/badges) as text cells above each visible pet.
    for &i in &order {
        let pet = &herd.pets[i];
        let Some(sp) = species.get(pet.identity.species_index).or_else(|| species.first()) else {
            continue;
        };
        let Some(state) = sp.states.get(&pet.status) else { continue };
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
        frame.buffer_mut().set_span(cx, area.y, &Span::styled(glyph, Style::default().fg(color)), area.width);
    }

    if hidden > 0 {
        let label = format!("+{hidden}");
        let label_w = label.len() as u16;
        let x = area.right().saturating_sub(label_w + 1);
        frame.buffer_mut().set_span(
            x,
            area.y + area.height / 2,
            &Span::styled(label, Style::default().fg(Color::DarkGray)),
            label_w,
        );
    }
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
                Some(b) => priority(pet.status) > priority(herd.pets[b].status),
            };
            if take {
                best = Some(i);
            }
        }
    }
    best
}

/// Render thread: ~12 fps tick. Drains snapshots, reconciles, steps the herd,
/// draws, and quits on `q`/Ctrl-C. Restores the terminal on exit.
pub fn run(rx: Receiver<Vec<Agent>>, species: Vec<Species>, theme: Theme) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let result = run_loop(&mut terminal, rx, &species, theme);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    rx: Receiver<Vec<Agent>>,
    species: &[Species],
    theme: Theme,
) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    let tick = Duration::from_millis(83); // ~12 fps
    let species_count = species.len().max(1);
    let mut herd = Herd::new();
    let mut rng = Lcg::new(0xC0FFEE);
    let mut last = Instant::now();
    loop {
        while let Ok(agents) = rx.try_recv() {
            let w = terminal.size()?.width as f32;
            herd.reconcile(&agents, species_count, w, &mut rng);
        }
        let now = Instant::now();
        let dt_ms = (now - last).as_millis() as f32;
        last = now;
        let w = terminal.size()?.width as f32;
        let pet_w = species.first().map(|s| s.size().0).unwrap_or(12) as f32;
        herd.step(dt_ms, w, pet_w, &mut rng);
        for p in herd.pets.iter_mut() {
            let fm = species
                .get(p.identity.species_index)
                .or_else(|| species.first())
                .and_then(|s| s.states.get(&p.status))
                .map(|st| st.frame_ms)
                .unwrap_or(0);
            p.advance(dt_ms, fm);
        }
        terminal.draw(|f| draw_herd(f, &herd, species, theme))?;

        if event::poll(tick)?
            && let Event::Key(k) = event::read()? {
                let quit = k.code == KeyCode::Char('q')
                    || (k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL));
                if quit {
                    return Ok(());
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

    const BLOB: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/sprites/test-blob.sprite"));

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
        }
    }

    fn fixed_herd(states: &[AgentStatus]) -> Herd {
        let mut h = Herd::new();
        let mut rng = Lcg::new(1);
        let agents: Vec<_> =
            states.iter().enumerate().map(|(i, s)| agent(&format!("t{i}"), *s)).collect();
        h.reconcile(&agents, 1, 120.0, &mut rng);
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
        let mut terminal = Terminal::new(TestBackend::new(90, 6)).unwrap();
        terminal.draw(|f| draw_herd(f, &herd, &species, Theme::Dark)).unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn renders_overflow_counter() {
        use AgentStatus::*;
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[Idle; 20]);
        let mut terminal = Terminal::new(TestBackend::new(40, 6)).unwrap();
        terminal.draw(|f| draw_herd(f, &herd, &species, Theme::Dark)).unwrap();
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
        herd.reconcile(&[agent("a", Working), agent("b", Blocked)], 1, 60.0, &mut rng);
        for (i, p) in herd.pets.iter_mut().enumerate() { p.x = 4.0 + i as f32 * 18.0; p.target_x = p.x; }
        let mut terminal = Terminal::new(TestBackend::new(60, 6)).unwrap();
        terminal.draw(|f| draw_herd(f, &herd, &species, Theme::Dark)).unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    use crate::pet::Pet;
    use crate::identity::identity_for;

    #[test]
    fn pet_at_column_returns_the_topmost_pet_when_they_overlap() {
        let species = vec![parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        // Two overlapping pets near x=10: idle (low priority) and blocked (high).
        herd.pets.push(Pet::new("idle".into(), identity_for("idle", 1), AgentStatus::Idle, 10.0));
        herd.pets.push(Pet::new("blk".into(), identity_for("blk", 1), AgentStatus::Blocked, 12.0));
        let hit = pet_at_column(&herd, &species, 200, 13).expect("a pet under column 13");
        assert_eq!(herd.pets[hit].terminal_id, "blk", "blocked draws on top, so it wins the hit");
    }

    #[test]
    fn pet_at_column_is_none_over_a_gap() {
        let species = vec![parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        herd.pets.push(Pet::new("a".into(), identity_for("a", 1), AgentStatus::Idle, 4.0));
        assert!(pet_at_column(&herd, &species, 200, 150).is_none(), "column far past the pet is empty");
    }
}
