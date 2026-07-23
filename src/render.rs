//! Phase 0 render: draw a placeholder header + one line per agent. No sprites,
//! no animation — that is Phase 1.

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
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::agent::{Agent, AgentStatus, parse_agent_list};
use crate::herdr::HerdrCli;

/// Placeholder ASCII glyph per status (deterministic for snapshots; sprites are Phase 1).
pub fn status_glyph(status: AgentStatus) -> char {
    match status {
        AgentStatus::Idle => 'z',
        AgentStatus::Working => '*',
        AgentStatus::Blocked => '!',
        AgentStatus::Done => '^',
        AgentStatus::Unknown => '?',
    }
}

/// Draw the pets strip placeholder: a bordered block titled "herdr-pets" with
/// one `<glyph>  <label>` line per agent.
pub fn draw(frame: &mut Frame, agents: &[Agent]) {
    let block = Block::default().title("herdr-pets").borders(Borders::ALL);
    let lines: Vec<Line> = if agents.is_empty() {
        vec![Line::from("no agents")]
    } else {
        agents
            .iter()
            .map(|a| Line::from(format!("{}  {}", status_glyph(a.agent_status), a.label())))
            .collect()
    };
    frame.render_widget(Paragraph::new(lines).block(block), frame.area());
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
    loop {
        let agents = herdr
            .run_json(&["agent", "list"])
            .ok()
            .and_then(|s| parse_agent_list(&s).ok())
            .unwrap_or_default();

        terminal.draw(|f| draw(f, &agents))?;

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
    use crate::agent::parse_agent_list;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    const FIXTURE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/agent-list.json"));

    #[test]
    fn glyphs_are_distinct_per_status() {
        use crate::agent::AgentStatus::*;
        let g = [Idle, Working, Blocked, Done, Unknown].map(status_glyph);
        let mut seen = g.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 5, "each status needs a distinct glyph");
    }

    #[test]
    fn renders_agent_lines() {
        let agents = parse_agent_list(FIXTURE).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        terminal.draw(|f| draw(f, &agents)).unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn renders_empty_herd() {
        let mut terminal = Terminal::new(TestBackend::new(40, 4)).unwrap();
        terminal.draw(|f| draw(f, &[])).unwrap();
        insta::assert_snapshot!(terminal.backend());
    }
}
