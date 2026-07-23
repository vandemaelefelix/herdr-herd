# Phase 2 — Interactivity & Placement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Place a full-width pet strip beneath a multi-pane tab on demand, and make pets interactive — hover shows the agent's name, click focuses that agent.

**Architecture:** A new `place.rs` injector reads `$HERDR_TAB_ID`, measures the tab via the `herdr` CLI, exports its layout tree over the control socket, wraps it in a root vertical split with a fresh pets pane as the full-width bottom child, and applies it (Spike A protocol). Mouse hover/click are grafted onto the existing `render.rs` loop, with a stable bottom caption row for hover names and clicks routed to `herdr agent focus` through the existing CLI seam.

**Tech Stack:** Rust (edition 2024), ratatui + crossterm (mouse capture), serde_json (layout trees), insta + `TestBackend` (snapshots). No new dependencies.

## Global Constraints

- Rust **edition 2024**, `rust-version = 1.96`.
- **No `unwrap`/`expect` outside `#[cfg(test)]`.** Fallible code returns `io::Result` and uses `?`; construct ad-hoc errors with `io::Error::other(...)` (per `rust-error-handling`).
- **No new crate dependencies.** `serde_json`, `crossterm`, `ratatui`, `insta` are already in `Cargo.toml`.
- Work on branch **`feature/phase-2`** (already checked out); never commit to `main`.
- **TDD:** write the failing test first, watch it fail, implement the minimum, watch it pass, commit.
- Doc comments: `//!` module headers, `///` on public items; **sentence-style test names** (`fn does_the_thing_when_condition()`) per `rust-project-conventions`.
- New `insta` snapshots must be **reviewed and accepted** (`cargo insta accept`) before the test passes.
- Scope: Phase 2 only. **No** auto-injection, new-tab hooks, respawn, strip de-dup (Phase 3); **no** config knobs (Phase 4).

---

## File Structure

- `src/pet.rs` (modify) — `Pet` gains a `label: String`.
- `src/herd.rs` (modify) — `reconcile` populates/refreshes each pet's `label`.
- `src/render.rs` (modify) — `pet_at_column` hit-testing, `draw_caption`, `focus_agent`, and mouse wiring in the run loop.
- `src/socket.rs` (modify) — `request_line`: one-shot request that reads a single reply line (persistent socket).
- `src/place.rs` (create) — the strip injector: pure ratio/tree helpers + the `place` orchestration.
- `src/lib.rs` (modify) — register `pub mod place;`.
- `src/main.rs` (modify) — dispatch the `place` subcommand; pass a focus CLI into `render::run`.
- `herdr-plugin.toml` (modify) — add the `[[actions]]` entry.
- `tests/manifest.rs` (modify) — assert the new action.

---

## Task 1: `Pet` carries an agent label

**Files:**
- Modify: `src/pet.rs` (struct + `Pet::new`)
- Modify: `src/herd.rs` (`reconcile`, + test)

**Interfaces:**
- Produces: `Pet.label: String` (public field); `Herd::reconcile` sets it from `Agent::label()` for new and surviving pets.

- [ ] **Step 1: Write the failing test** — add to the `tests` module in `src/herd.rs`:

```rust
    #[test]
    fn reconcile_sets_and_updates_the_pet_label() {
        let mut h = Herd::new();
        let mut rng = Lcg::new(1);
        let mut a = agent("a", AgentStatus::Idle);
        a.name = Some("backend".into());
        h.reconcile(&[a], 1, 100.0, &mut rng);
        assert_eq!(h.pets[0].label, "backend");

        // A survivor renamed mid-session picks up the new label.
        let mut a2 = agent("a", AgentStatus::Idle);
        a2.name = Some("frontend".into());
        h.reconcile(&[a2], 1, 100.0, &mut rng);
        assert_eq!(h.pets[0].label, "frontend");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p herdr-pets --lib herd::tests::reconcile_sets_and_updates_the_pet_label`
Expected: FAIL — `no field 'label' on type '&Pet'`.

- [ ] **Step 3: Add the field to `Pet`** — in `src/pet.rs`, add `label` to the struct and initialise it in `new`:

```rust
#[derive(Debug, Clone)]
pub struct Pet {
    pub terminal_id: String,
    pub identity: Identity,
    pub status: AgentStatus,
    pub label: String,
    pub x: f32,
    pub target_x: f32,
    pub phase: f32,
}
```

In `Pet::new`, add `label: String::new(),` to the returned struct literal (leave the rest unchanged):

```rust
    pub fn new(terminal_id: String, identity: Identity, status: AgentStatus, x: f32) -> Self {
        Self {
            terminal_id,
            identity,
            status,
            label: String::new(),
            x,
            target_x: x,
            phase: 0.0,
        }
    }
```

- [ ] **Step 4: Populate `label` in `reconcile`** — in `src/herd.rs`, update both branches of the survivor/new loop:

```rust
        for a in agents {
            if let Some(p) = self.pets.iter_mut().find(|p| p.terminal_id == a.terminal_id) {
                p.status = a.agent_status;
                p.label = a.label();
            } else {
                let x = rng.next_unit() * strip_w.max(1.0);
                let mut pet = Pet::new(
                    a.terminal_id.clone(),
                    identity_for(&a.terminal_id, species_count),
                    a.agent_status,
                    x,
                );
                pet.label = a.label();
                self.pets.push(pet);
            }
        }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p herdr-pets --lib herd::`
Expected: PASS (including the existing `reconcile_*` tests).

- [ ] **Step 6: Commit**

```bash
git add src/pet.rs src/herd.rs
git commit -m "feat(pet): carry the agent label on each pet for hover"
```

---

## Task 2: Hit-test the pet under a terminal column

**Files:**
- Modify: `src/render.rs` (new public fn + tests)

**Interfaces:**
- Consumes: `Pet.label` is not needed here; uses `Herd`, `Species`, `visible_and_hidden`, `priority` (already imported in `render.rs`).
- Produces: `pub fn pet_at_column(herd: &Herd, species: &[Species], strip_w: usize, col: u16) -> Option<usize>` — index into `herd.pets`.

- [ ] **Step 1: Write the failing tests** — add to the `tests` module in `src/render.rs` (the module already imports `Herd`, `Lcg`, `AgentStatus`, `parse_species`, `Theme`, `Terminal`, `TestBackend`; add the two `use` lines shown):

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p herdr-pets --lib render::tests::pet_at_column`
Expected: FAIL — `cannot find function 'pet_at_column'`.

- [ ] **Step 3: Implement `pet_at_column`** — add near `draw_herd` in `src/render.rs`:

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p herdr-pets --lib render::tests::pet_at_column`
Expected: PASS (both).

- [ ] **Step 5: Commit**

```bash
git add src/render.rs
git commit -m "feat(render): hit-test the pet under a terminal column"
```

---

## Task 3: Draw the hover caption on the strip's bottom row

**Files:**
- Modify: `src/render.rs` (new public fn + snapshot test)
- Create (generated): `src/snapshots/herdr_pets__render__tests__caption_*.snap`

**Interfaces:**
- Produces: `pub fn draw_caption(frame: &mut Frame, area: Rect, label: Option<&str>)` — writes the label on `area`'s bottom row; a no-op when `label` is `None`.

- [ ] **Step 1: Write the failing snapshot test** — add to the `tests` module in `src/render.rs`:

```rust
    #[test]
    fn caption_shows_the_hovered_name_on_the_bottom_row() {
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[AgentStatus::Working, AgentStatus::Idle]);
        let mut terminal = Terminal::new(TestBackend::new(40, 7)).unwrap();
        terminal
            .draw(|f| {
                draw_herd(f, &herd, &species, Theme::Dark);
                draw_caption(f, f.area(), Some("backend-api"));
            })
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p herdr-pets --lib render::tests::caption_shows_the_hovered_name_on_the_bottom_row`
Expected: FAIL — `cannot find function 'draw_caption'`.

- [ ] **Step 3: Implement `draw_caption`** — add after `draw_herd` in `src/render.rs`:

```rust
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
    frame
        .buffer_mut()
        .set_span(area.x, y, &Span::styled(text, Style::default().fg(Color::Gray)), w);
}
```

- [ ] **Step 4: Run the test, then accept the snapshot**

Run: `cargo test -p herdr-pets --lib render::tests::caption_shows_the_hovered_name_on_the_bottom_row`
Expected: FAIL — insta reports a new, unreviewed snapshot.

Review the pending snapshot (confirm the bottom row reads `backend-api` and the herd is unshifted), then accept:

Run: `cargo insta accept`
Then re-run the test:
Run: `cargo test -p herdr-pets --lib render::tests::caption_shows_the_hovered_name_on_the_bottom_row`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/render.rs src/snapshots/
git commit -m "feat(render): draw the hover caption on the strip's bottom row"
```

---

## Task 4: Mouse hover + click-to-focus in the run loop

**Files:**
- Modify: `src/render.rs` (`focus_agent`, imports, `run`, `run_loop`, + test)
- Modify: `src/main.rs` (pass a focus CLI into `render::run`)

**Interfaces:**
- Consumes: `pet_at_column` (Task 2), `draw_caption` (Task 3), `HerdrCli` from `crate::herdr`.
- Produces: `pub fn focus_agent(cli: &dyn HerdrCli, terminal_id: &str) -> io::Result<()>`; `render::run` now takes a fourth argument `focus: Box<dyn HerdrCli>`.

- [ ] **Step 1: Write the failing test** — add to the `tests` module in `src/render.rs`:

```rust
    use std::cell::RefCell;
    use std::ffi::OsStr;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{ExitStatus, Output};
    use std::rc::Rc;
    use crate::herdr::{CommandRunner, LiveHerdr};

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
    fn focus_agent_shells_agent_focus_with_the_terminal_id() {
        let args = Rc::new(RefCell::new(Vec::new()));
        let cli = LiveHerdr::with_runner("herdr", Recorder { args: Rc::clone(&args) });
        focus_agent(&cli, "term_abc").unwrap();
        assert_eq!(*args.borrow(), vec!["agent", "focus", "term_abc"]);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p herdr-pets --lib render::tests::focus_agent_shells_agent_focus_with_the_terminal_id`
Expected: FAIL — `cannot find function 'focus_agent'`.

- [ ] **Step 3: Implement `focus_agent` and extend the imports** — in `src/render.rs`, extend the crossterm event import and add the `HerdrCli` import at the top:

```rust
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
```

```rust
use crate::herdr::HerdrCli;
```

Add the function (near `pet_at_column`):

```rust
/// Focus the agent identified by `terminal_id` via `herdr agent focus`.
/// The caller swallows the error — a failed focus must never crash the strip.
pub fn focus_agent(cli: &dyn HerdrCli, terminal_id: &str) -> io::Result<()> {
    cli.run_json(&["agent", "focus", terminal_id]).map(|_| ())
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p herdr-pets --lib render::tests::focus_agent_shells_agent_focus_with_the_terminal_id`
Expected: PASS.

- [ ] **Step 5: Enable mouse capture in `run` and thread the focus CLI** — replace `run` in `src/render.rs`:

```rust
/// Render thread: ~12 fps tick. Drains snapshots, reconciles, steps the herd,
/// draws, handles mouse hover/click, and quits on `q`/Ctrl-C. Restores the
/// terminal (raw mode, alternate screen, mouse capture) on exit.
pub fn run(
    rx: Receiver<Vec<Agent>>,
    species: Vec<Species>,
    theme: Theme,
    focus: Box<dyn HerdrCli>,
) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let result = run_loop(&mut terminal, rx, &species, theme, focus.as_ref());

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;
    result
}
```

- [ ] **Step 6: Handle hover + click in `run_loop`** — in `src/render.rs`, change the `run_loop` signature to accept `focus`, track `hovered`, draw the caption, and match mouse events. Replace the signature line and the draw/event portion:

Signature (add the `focus` parameter):

```rust
fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    rx: Receiver<Vec<Agent>>,
    species: &[Species],
    theme: Theme,
    focus: &dyn HerdrCli,
) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
```

Add a `hovered` accumulator alongside the other `let mut` bindings before the loop:

```rust
    let mut hovered: Option<String> = None;
```

Replace the `terminal.draw(...)` call and the `if event::poll(tick)? { ... }` block with:

```rust
        let strip_w = terminal.size()?.width as usize;
        let caption = hovered.clone();
        terminal.draw(|f| {
            draw_herd(f, &herd, species, theme);
            draw_caption(f, f.area(), caption.as_deref());
        })?;

        if event::poll(tick)? {
            match event::read()? {
                Event::Key(k) => {
                    let quit = k.code == KeyCode::Char('q')
                        || (k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL));
                    if quit {
                        return Ok(());
                    }
                }
                Event::Mouse(MouseEvent { kind, column, .. }) => match kind {
                    MouseEventKind::Moved => {
                        hovered = pet_at_column(&herd, species, strip_w, column)
                            .map(|i| herd.pets[i].label.clone());
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        if let Some(i) = pet_at_column(&herd, species, strip_w, column) {
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
```

(Remove the now-duplicated `let w = terminal.size()?.width as f32;` only if it becomes unused; the `herd.step` call above still needs its own `w` — leave that one intact.)

- [ ] **Step 7: Pass a focus CLI from `main.rs`** — in `src/main.rs`, in the `Some("render")` arm, build a second `LiveHerdr` and pass it in:

```rust
        Some("render") => {
            let species = load_species();
            let (tx, rx) = mpsc::channel();
            let cli = Box::new(LiveHerdr::from_env());
            let focus = Box::new(LiveHerdr::from_env());
            let socket: Option<Box<dyn SocketClient + Send>> = socket_path()
                .and_then(|p| RealSocket::connect(&p).ok())
                .map(|s| Box::new(s) as Box<dyn SocketClient + Send>);
            let _watcher = watch(cli, socket, Box::new(RealClock::new()), tx, 2500, 250);
            match render::run(rx, species, Theme::Dark, focus) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("herdr-pets: {e}");
                    ExitCode::FAILURE
                }
            }
        }
```

- [ ] **Step 8: Build and run the full test suite**

Run: `cargo build -p herdr-pets && cargo test -p herdr-pets`
Expected: PASS — everything compiles and all tests pass. (The loop's mouse integration is exercised by the manual verification in Task 8.)

- [ ] **Step 9: Commit**

```bash
git add src/render.rs src/main.rs
git commit -m "feat(render): mouse hover caption and click-to-focus"
```

---

## Task 5: One-line socket request/reply helper

**Files:**
- Modify: `src/socket.rs` (new public fn + test)

**Interfaces:**
- Produces: `pub fn request_line(path: &Path, payload: &str) -> std::io::Result<String>` — connects, sends `payload` + `\n`, returns exactly one reply line (newline stripped), without waiting for the server to close.

- [ ] **Step 1: Write the failing test** — add to the `tests` module in `src/socket.rs`:

```rust
    #[test]
    fn request_line_reads_one_reply_line_without_needing_eof() {
        let path = std::env::temp_dir().join(format!("herdr-pets-rl-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn({
            let path = path.clone();
            move || {
                let (conn, _) = listener.accept().unwrap();
                let mut r = BufReader::new(conn.try_clone().unwrap());
                let mut w = conn;
                let mut got = String::new();
                r.read_line(&mut got).unwrap();
                w.write_all(b"{\"reply\":1}\n").unwrap();
                w.flush().unwrap();
                // Hold the connection open (a persistent socket does not EOF).
                std::thread::sleep(std::time::Duration::from_millis(50));
                let _ = std::fs::remove_file(&path);
                got
            }
        });
        let reply = request_line(&path, "{\"ping\":1}").unwrap();
        assert_eq!(reply, "{\"reply\":1}");
        let got = server.join().unwrap();
        assert_eq!(got, "{\"ping\":1}\n");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p herdr-pets --lib socket::tests::request_line_reads_one_reply_line_without_needing_eof`
Expected: FAIL — `cannot find function 'request_line'`.

- [ ] **Step 3: Implement `request_line`** — add after `request` in `src/socket.rs`:

```rust
/// Connect, send `payload` + a newline, and read exactly one reply line (with
/// the trailing newline stripped). Unlike [`request`], this does **not** wait
/// for the server to close: the herdr control socket is persistent, so a
/// request/reply is framed by the newline, not by EOF.
pub fn request_line(path: &Path, payload: &str) -> std::io::Result<String> {
    let stream = UnixStream::connect(path)?;
    let mut writer = stream.try_clone()?;
    writer.write_all(payload.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let n = reader.read_line(&mut line)?;
    if n == 0 {
        return Err(std::io::Error::other("socket closed before reply"));
    }
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p herdr-pets --lib socket::tests::request_line_reads_one_reply_line_without_needing_eof`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/socket.rs
git commit -m "feat(socket): add one-line request/reply for the persistent socket"
```

---

## Task 6: Placement helpers — ratio, tree wrapping, and parsing

**Files:**
- Create: `src/place.rs`
- Modify: `src/lib.rs` (register the module)

**Interfaces:**
- Produces:
  - `pub const TARGET_ROWS: u16` — the strip's target height in rows.
  - `pub fn slim_ratio(tab_rows: u16, target_rows: u16) -> f32`
  - `pub fn wrap_root(tree: Value, ratio: f32, cmd: &[String], cwd: &str) -> Value`
  - `pub fn parse_tab_rows(cli_json: &str) -> io::Result<u16>`
  - `pub fn extract_export_root(reply: &str) -> io::Result<Value>`
  - `pub fn export_request(tab_id: &str) -> String`
  - `pub fn apply_request(tab_id: &str, root: &Value) -> String`
  - `pub fn check_reply(reply: &str) -> io::Result<()>`
  - (`Value` is `serde_json::Value`.)

- [ ] **Step 1: Register the module** — add to `src/lib.rs` between `pet` and `render`:

```rust
pub mod place;
```

- [ ] **Step 2: Create `src/place.rs` with the failing tests** — write the module header, the `use`s, and the test module first (the functions come next and will not yet exist, so this fails to compile):

```rust
//! The full-width strip injector: measure the tab, export its layout tree,
//! wrap it in a root vertical split with a new pets pane as the bottom child,
//! and apply. Pure ratio/tree helpers live here; socket + env orchestration is
//! in [`place`]. See Phase 0 Spike A (design §5) for the verified wire protocol
//! (newline-delimited JSON-RPC, dotted methods, a command-leaf with no
//! `pane_id` spawns a fresh pane).

use std::io;

use serde_json::{Value, json};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slim_ratio_leaves_the_target_rows_for_the_strip() {
        // 64-row tab, 7-row strip => 1 - 7/64.
        let r = slim_ratio(64, 7);
        assert!((r - (1.0 - 7.0 / 64.0)).abs() < 1e-6, "got {r}");
    }

    #[test]
    fn slim_ratio_clamps_up_on_a_tiny_tab() {
        // 8-row tab, 7-row strip => 1 - 7/8 = 0.125, clamped up to the 0.3 floor.
        assert_eq!(slim_ratio(8, 7), 0.3);
    }

    #[test]
    fn wrap_root_puts_a_command_leaf_with_no_pane_id_at_the_bottom() {
        let tree = json!({"type": "pane", "pane_id": "w1:p1", "cwd": "/x"});
        let cmd = vec!["/abs/herdr-pets".to_string(), "render".to_string()];
        let root = wrap_root(tree.clone(), 0.89, &cmd, "/work");
        assert_eq!(root["type"], "split");
        assert_eq!(root["direction"], "down");
        assert_eq!(root["first"], tree, "existing tree preserved verbatim on top");
        let bottom = &root["second"];
        assert_eq!(bottom["type"], "pane");
        assert_eq!(bottom["command"], json!(["/abs/herdr-pets", "render"]));
        assert_eq!(bottom["cwd"], "/work");
        assert!(bottom.get("pane_id").is_none(), "a fresh pane must carry no pane_id");
    }

    #[test]
    fn parse_tab_rows_reads_the_area_height() {
        let j = r#"{"result":{"layout":{"area":{"height":64,"width":214,"x":40,"y":1}}}}"#;
        assert_eq!(parse_tab_rows(j).unwrap(), 64);
    }

    #[test]
    fn parse_tab_rows_errors_when_height_is_absent() {
        assert!(parse_tab_rows(r#"{"result":{"layout":{}}}"#).is_err());
    }

    #[test]
    fn extract_export_root_returns_the_recursive_tree() {
        let reply = r#"{"result":{"type":"layout_export","layout":{"tab_id":"w1:t1","root":{"type":"pane","pane_id":"w1:p1","cwd":"/x"}}}}"#;
        let root = extract_export_root(reply).unwrap();
        assert_eq!(root["type"], "pane");
        assert_eq!(root["pane_id"], "w1:p1");
    }

    #[test]
    fn check_reply_errors_on_an_error_envelope_and_passes_a_result() {
        assert!(check_reply(r#"{"error":{"code":"invalid_target"}}"#).is_err());
        assert!(check_reply(r#"{"result":{"ok":true}}"#).is_ok());
    }
}
```

- [ ] **Step 3: Run tests to verify they fail (do not compile)**

Run: `cargo test -p herdr-pets --lib place::`
Expected: FAIL — `cannot find function 'slim_ratio'` (and the other helpers).

- [ ] **Step 4: Implement the helpers** — insert above the `#[cfg(test)]` module in `src/place.rs`:

```rust
/// Rows the strip should occupy: pets take 6 half-block rows, plus 1 caption.
pub const TARGET_ROWS: u16 = 7;

/// The split ratio that leaves the bottom `target_rows` for the strip on a tab
/// `tab_rows` tall: `1 - target/tab`. Clamped to `[0.3, 0.95]` so a tiny tab
/// still keeps a usable top region and a huge tab still yields a real strip.
pub fn slim_ratio(tab_rows: u16, target_rows: u16) -> f32 {
    if tab_rows == 0 {
        return 0.85;
    }
    let r = 1.0 - (target_rows as f32 / tab_rows as f32);
    r.clamp(0.3, 0.95)
}

/// Wrap `tree` (a `layout.export` root) in a root `down` split whose bottom
/// child is a new command pane running `cmd` in `cwd`. The bottom leaf carries
/// a `command` and **no `pane_id`** — that is how herdr spawns a fresh pane
/// (Spike A). The existing tree is preserved verbatim as the top child.
pub fn wrap_root(tree: Value, ratio: f32, cmd: &[String], cwd: &str) -> Value {
    json!({
        "type": "split",
        "direction": "down",
        "ratio": ratio,
        "first": tree,
        "second": {
            "type": "pane",
            "command": cmd,
            "cwd": cwd,
        }
    })
}

/// Extract `result.layout.area.height` (the tab's total row count) from a
/// `herdr pane layout --current` CLI JSON envelope.
pub fn parse_tab_rows(cli_json: &str) -> io::Result<u16> {
    let v: Value = serde_json::from_str(cli_json).map_err(io::Error::other)?;
    v.get("result")
        .and_then(|r| r.get("layout"))
        .and_then(|l| l.get("area"))
        .and_then(|a| a.get("height"))
        .and_then(Value::as_u64)
        .and_then(|h| u16::try_from(h).ok())
        .ok_or_else(|| io::Error::other("no result.layout.area.height in pane layout output"))
}

/// Extract the recursive `result.layout.root` tree from a socket
/// `layout.export` reply, ready to feed to [`wrap_root`].
pub fn extract_export_root(reply: &str) -> io::Result<Value> {
    let v: Value = serde_json::from_str(reply).map_err(io::Error::other)?;
    v.get("result")
        .and_then(|r| r.get("layout"))
        .and_then(|l| l.get("root"))
        .cloned()
        .ok_or_else(|| io::Error::other("no result.layout.root in layout.export reply"))
}

/// Build the `layout.export` request line for `tab_id`.
pub fn export_request(tab_id: &str) -> String {
    json!({"id": "pets-place", "method": "layout.export", "params": {"tab_id": tab_id}})
        .to_string()
}

/// Build the `layout.apply` request line placing `root` on `tab_id`.
pub fn apply_request(tab_id: &str, root: &Value) -> String {
    json!({"id": "pets-place", "method": "layout.apply", "params": {"tab_id": tab_id, "root": root}})
        .to_string()
}

/// Error if a JSON-RPC reply carries an `error` object; otherwise `Ok`.
pub fn check_reply(reply: &str) -> io::Result<()> {
    let v: Value = serde_json::from_str(reply).map_err(io::Error::other)?;
    if let Some(err) = v.get("error") {
        return Err(io::Error::other(format!("herdr rejected the request: {err}")));
    }
    Ok(())
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p herdr-pets --lib place::`
Expected: PASS (all six).

- [ ] **Step 6: Commit**

```bash
git add src/lib.rs src/place.rs
git commit -m "feat(place): pure layout-tree and ratio helpers for strip injection"
```

---

## Task 7: The `place` subcommand and herdr action

**Files:**
- Modify: `src/place.rs` (add the `place` orchestration fn)
- Modify: `src/main.rs` (dispatch `place`; update usage)
- Modify: `herdr-plugin.toml` (add `[[actions]]`)
- Modify: `tests/manifest.rs` (assert the action)

**Interfaces:**
- Consumes: `HerdrCli` (`crate::herdr`), `socket::{socket_path, request_line}`, and all Task 6 helpers.
- Produces: `pub fn place(cli: &dyn HerdrCli, self_exe: &str, cwd: &str) -> io::Result<()>`.

- [ ] **Step 1: Write the failing manifest test** — add to `tests/manifest.rs`:

```rust
#[test]
fn manifest_action_places_the_strip_via_the_release_binary() {
    let m = manifest();
    let actions = m.get("actions").and_then(Value::as_array).expect("[[actions]] present");
    let a = &actions[0];
    assert_eq!(a.get("id").and_then(Value::as_str), Some("place-pets"));
    let cmd: Vec<&str> = a
        .get("command")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(cmd, vec!["./target/release/herdr-pets", "place"]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p herdr-pets --test manifest manifest_action_places_the_strip_via_the_release_binary`
Expected: FAIL — panic `[[actions]] present` (no `actions` array yet).

- [ ] **Step 3: Add the `[[actions]]` entry** — append to `herdr-plugin.toml`:

```toml

[[actions]]
id = "place-pets"
title = "Place pets strip"
command = ["./target/release/herdr-pets", "place"]
```

- [ ] **Step 4: Run the manifest test to verify it passes**

Run: `cargo test -p herdr-pets --test manifest`
Expected: PASS (existing manifest tests plus the new one).

- [ ] **Step 5: Implement the `place` orchestration** — add to `src/place.rs`, above the `#[cfg(test)]` module. Add the two `use` lines to the top of the file (below the existing `use`s):

```rust
use crate::herdr::HerdrCli;
use crate::socket;
```

```rust
/// Inject a full-width pets strip into the current tab. Reads `$HERDR_TAB_ID`
/// (the target) and `$HERDR_SOCKET_PATH` (the control socket); measures the tab
/// with `herdr pane layout --current` via `cli`, then `layout.export` +
/// `layout.apply` over the socket to place the strip. `self_exe` is the
/// absolute path to this binary; the bottom pane runs `<self_exe> render` in
/// `cwd`.
///
/// De-duplication (avoiding a second strip if one already exists) is a Phase 3
/// concern; this one-shot wraps whatever tree it exports.
pub fn place(cli: &dyn HerdrCli, self_exe: &str, cwd: &str) -> io::Result<()> {
    let tab_id = std::env::var("HERDR_TAB_ID")
        .map_err(|_| io::Error::other("HERDR_TAB_ID is not set — run `place` inside a herdr session"))?;
    let sock = socket::socket_path()
        .ok_or_else(|| io::Error::other("HERDR_SOCKET_PATH is not set"))?;

    let layout_json = cli.run_json(&["pane", "layout", "--current"])?;
    let tab_rows = parse_tab_rows(&layout_json)?;
    let ratio = slim_ratio(tab_rows, TARGET_ROWS);

    let export_reply = socket::request_line(&sock, &export_request(&tab_id))?;
    check_reply(&export_reply)?;
    let tree = extract_export_root(&export_reply)?;

    let cmd = vec![self_exe.to_string(), "render".to_string()];
    let root = wrap_root(tree, ratio, &cmd, cwd);

    let apply_reply = socket::request_line(&sock, &apply_request(&tab_id, &root))?;
    check_reply(&apply_reply)?;
    Ok(())
}
```

- [ ] **Step 6: Dispatch `place` from `main.rs`** — add the arm and update the usage line in `src/main.rs`:

```rust
        Some("place") => {
            let cli = LiveHerdr::from_env();
            let self_exe = std::env::current_exe()
                .ok()
                .and_then(|p| p.to_str().map(String::from))
                .unwrap_or_else(|| "herdr-pets".to_string());
            let cwd = std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(String::from))
                .unwrap_or_else(|| ".".to_string());
            match herdr_pets::place::place(&cli, &self_exe, &cwd) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("herdr-pets: {e}");
                    ExitCode::FAILURE
                }
            }
        }
```

Update the fallthrough usage message:

```rust
        _ => {
            eprintln!("usage: herdr-pets render|place");
            ExitCode::FAILURE
        }
```

- [ ] **Step 7: Build and run the full suite**

Run: `cargo build -p herdr-pets && cargo test -p herdr-pets`
Expected: PASS — compiles and all tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/place.rs src/main.rs herdr-plugin.toml tests/manifest.rs
git commit -m "feat(place): wire the place subcommand and herdr action"
```

---

## Task 8: Live verification & wrap-up

This task has no new unit tests — it verifies the two things unit tests cannot (mouse forwarding into a pane, and a real `layout.apply`), then updates the phase tracker. **Run the layout steps in an isolated scratch tab** (a fresh tab with 2 dummy panes) so any mishap stays away from the live session — per the Phase 0 spike discipline.

- [ ] **Step 1: Build the release binary**

Run: `cargo build --release -p herdr-pets`
Expected: builds `./target/release/herdr-pets`.

- [ ] **Step 2: Re-link the plugin so herdr sees the new action**

Run: `herdr plugin link .` (from the repo root)
Then: `herdr plugin list` — expect `herdr-pets` listed with no errors, and `herdr plugin action list --plugin herdr-pets` shows `place-pets`.

- [ ] **Step 3: Verify mouse forwarding (gates hover/click)**

In a scratch tab, open the pane manually and interact:
Run: `herdr plugin pane open --plugin herdr-pets --entrypoint pets`
- Move the mouse over a pet → the bottom caption row shows that agent's name; move off → it clears.
- Left-click a pet → herdr focuses that agent's pane.

Expected: hover names and click-focus both work. **If mouse events do not reach the pane** (no hover reaction), stop and flag it: update `GOAL.md` + `docs/PLAN.md` per the handoff guardrails before continuing, since it invalidates §4.2 of the spec.

- [ ] **Step 4: Verify `place` in a scratch tab with 2+ panes**

In a scratch tab split into 2+ panes, from any pane run:
Run: `./target/release/herdr-pets place`
Expected: a full-width strip appears across the bottom at ~7 rows, running the renderer with the live herd; the existing panes remain on top.

- [ ] **Step 5: Verify the herdr action path**

Invoke the `place-pets` action from herdr's UI (or `herdr plugin action invoke --plugin herdr-pets --action place-pets`) from a focused multi-pane tab.
Expected: identical result — the strip lands full-width at the bottom, confirming the action inherits the focused tab's `$HERDR_TAB_ID`.

- [ ] **Step 6: Run the full gate**

Run: `cargo test -p herdr-pets && cargo clippy -p herdr-pets --all-targets -- -D warnings && cargo fmt --check`
Expected: all pass, no warnings.

- [ ] **Step 7: Update the phase tracker** — in `docs/PLAN.md`, set the Phase 2 row to:

```markdown
| 2 | Interactivity & placement | Done | [design](superpowers/specs/2026-07-23-phase-2-interactivity-placement-design.md) | [plan](superpowers/plans/2026-07-23-phase-2-interactivity-placement.md) |
```

- [ ] **Step 8: Commit**

```bash
git add docs/PLAN.md
git commit -m "docs(phase-2): mark Phase 2 done and link the plan"
```

---

## Self-Review

**Spec coverage:**
- §2 `place.rs` injector (`$HERDR_TAB_ID`, `pane.edges`/`pane layout` height, export, wrap, apply) → Tasks 5, 6, 7. *(Row count comes from `herdr pane layout --current` — the flattened CLI view that carries `area.height` — rather than `pane edges`; both expose the same number and `pane layout` has the shallower path.)*
- §2 `slim_ratio` / `wrap_root` pure fns → Task 6.
- §2 de-dup deferred to Phase 3 → documented in `place`'s doc comment (Task 7).
- §3 mouse capture, `pet_at_column`, caption line, click→`agent focus` → Tasks 2, 3, 4.
- §4 `Pet` gains `label`, set in `reconcile` → Task 1.
- §5 manifest `[[actions]]` → Task 7.
- §6 testing (place helpers, `pet_at_column`, caption snapshot, reconcile label, click→focus argv) → Tasks 1–7.
- §7 verification (mouse forwarding first, `place` in a scratch tab, action path) → Task 8.
- §8 out-of-scope items → excluded; no tasks touch auto-injection/config.

**Placeholder scan:** No TBD/TODO/"handle edge cases"; every code step shows complete code and every command shows expected output.

**Type consistency:** `pet_at_column(&Herd, &[Species], usize, u16) -> Option<usize>` used identically in Task 2 and Task 4. `focus_agent(&dyn HerdrCli, &str) -> io::Result<()>` consistent across Task 4. `place`/`wrap_root`/`slim_ratio`/`parse_tab_rows`/`extract_export_root`/`check_reply`/`export_request`/`apply_request` signatures match between Tasks 6 and 7. `render::run` gains exactly one param (`focus: Box<dyn HerdrCli>`), threaded from `main.rs` (Task 4 Step 7) and into `run_loop` (Task 4 Step 6).
