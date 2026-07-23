//! Half-block renderer: blit the roaming herd into a pixel buffer, emit it as
//! `▀` cells (fg = top pixel, bg = bottom pixel), then overlay state bubbles/
//! badges and a `+N` counter.

use std::io;
use std::time::Duration;

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

use crate::agent::parse_agent_list;
use crate::anim::{Overlay, OverlayColor, Rgb, motion_offset};
use crate::herd::{Herd, visible_and_hidden};
use crate::herdr::HerdrCli;
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

/// Run the render loop: fetch agents, draw, poll for input, repeat until `q`
/// or Ctrl-C. Restores the terminal on exit.
pub fn run(herdr: &dyn HerdrCli) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let result = run_loop(&mut terminal, herdr);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    herdr: &dyn HerdrCli,
) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    // Minimal adaptation so the Phase 0 loop shell compiles against the new
    // renderer; Task 11 rewrites this into the real roam/animation loop.
    let species = crate::sprite::load_species();
    let mut herd = Herd::new();
    let mut rng = crate::herd::Lcg::new(1);

    loop {
        let agents = herdr
            .run_json(&["agent", "list"])
            .ok()
            .and_then(|s| parse_agent_list(&s).ok())
            .unwrap_or_default();

        let strip_w = terminal.size().map(|s| s.width as f32).unwrap_or(120.0);
        herd.reconcile(&agents, species.len().max(1), strip_w, &mut rng);

        terminal.draw(|f| draw_herd(f, &herd, &species, Theme::Dark))?;

        // ~1.5s refresh cadence; wake early on a keypress.
        if event::poll(Duration::from_millis(1500))? {
            if let Event::Key(key) = event::read()? {
                let quit = key.code == KeyCode::Char('q')
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL));
                if quit {
                    return Ok(());
                }
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
}
