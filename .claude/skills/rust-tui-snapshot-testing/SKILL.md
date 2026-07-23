---
name: rust-tui-snapshot-testing
description: Use when writing or testing ratatui rendering code — drawing widgets, laying out a frame, or anything that produces terminal output. Gives this repo's insta + TestBackend snapshot pattern, deterministic rendering, and terminal-restore discipline.
---

# TUI snapshot testing (herdr-pets conventions)

## When to use

Any change to how the pane draws: new widgets, layout, per-status glyphs,
sprites. Snapshot the rendered cells so a visual regression shows up as a diff.
See `src/render.rs`.

## The pattern

**1. Separate the pure draw from the live loop.** `draw(frame, agents)` takes
state and renders; `run`/`run_loop` handle terminal setup and events. Only the
pure `draw` is snapshot-tested:

```rust
pub fn draw(frame: &mut Frame, agents: &[Agent]) {
    let block = Block::default().title("herdr-pets").borders(Borders::ALL);
    // ... one Line per agent ...
    frame.render_widget(Paragraph::new(lines).block(block), frame.area());
}
```

**2. Snapshot with `insta` + ratatui's `TestBackend`** at a fixed size:

```rust
#[test]
fn renders_agent_lines() {
    let agents = parse_agent_list(FIXTURE).unwrap();
    let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
    terminal.draw(|f| draw(f, &agents)).unwrap();
    insta::assert_snapshot!(terminal.backend());
}
```

**3. Cover the empty/edge frame too** (`renders_empty_herd`) — the "no agents"
path is a real state the pane must render cleanly.

**4. Keep rendering deterministic.** Anything that varies (a status → symbol
mapping) goes through a pure, testable function so snapshots are stable and you
can assert properties directly:

```rust
pub fn status_glyph(status: AgentStatus) -> char { /* Idle => 'z', ... */ }

#[test]
fn glyphs_are_distinct_per_status() { /* dedup the mapped set, assert len */ }
```

## Rules

- **Pure `draw`, impure `run`.** Never put `enable_raw_mode`, event polling, or
  `stdout` inside the function you snapshot.
- **Fixed `TestBackend` dimensions** per test so snapshots are reproducible.
- **Restore the terminal on every exit path** in the live loop —
  `disable_raw_mode`, `LeaveAlternateScreen`, `show_cursor` — even when the inner
  loop returned an error (`src/render.rs:55`).
- **Review snapshots deliberately.** Run `cargo insta review`; a changed `.snap`
  is a visual diff to approve, not noise to `--accept` blindly.
- **No wall-clock or randomness in rendered content** — it would make snapshots
  flap.

## Anti-patterns

- Testing rendering by scraping `stdout` from the real backend instead of
  `TestBackend`.
- Baking `SystemTime::now()` or a random seed into a drawn line.
- One giant snapshot of the whole app loop instead of small `draw`-level
  snapshots you can actually read in a diff.
