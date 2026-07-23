# Phase 0 — Foundations & spikes: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A herdr plugin that links + runs, opens a pane rendering the live agent list, with both de-risking spikes (full-width injection; new-tab trigger) answered and documented.

**Architecture:** One Rust binary (`herdr-pets`) with a `render` subcommand. All logic lives in a library (`src/lib.rs`) behind small modules; the binary is a thin dispatcher. State is read by shelling out to the `herdr` CLI behind a `HerdrCli` trait (a `CommandRunner` seam keeps tests hermetic). A minimal raw-socket module exists only to support Spike A's `layout_apply` experiment.

**Tech Stack:** Rust (edition 2024, rust-version 1.96), `ratatui` 0.30, `crossterm` 0.29, `serde`/`serde_json`, `toml` (parse), dev-dep `insta`.

## Global Constraints

- Rust **edition = "2024"**, **rust-version = "1.96"**.
- License: **MIT**. `Cargo.toml` `license = "MIT"`; a `LICENSE` file at repo root.
- Manifest required fields (verbatim values): `id = "herdr-pets"`, `name = "herdr-pets"`, `version = "0.1.0"`, `min_herdr_version = "0.7.0"`, `platforms = ["linux", "macos"]`.
- Pane command in the manifest MUST be `["./target/release/herdr-pets", "render"]`.
- **Verified CLI (herdr 0.7.0):** `herdr agent list` takes **no flag** and prints `{"id":...,"result":{"agents":[…],"type":"agent_list"}}`. Read `.result.agents`. Per-agent `agent` and `name` are optional; there is a `revision` int.
- **Git:** work on branch `feat/phase-0-foundations` (already created). Conventional Commits (`type(scope): desc`). **Do not push.** Commit locally at each task's final step.
- **Scope discipline:** no sprites, animation, identity hashing, mouse, auto-injection, `control` mode, or config. Placeholder glyphs only.
- Unix-only code is acceptable (`platforms` excludes Windows); no Windows `.exe` seam.

---

### Task 1: Project scaffold + manifest

**Files:**
- Create: `Cargo.toml`
- Create: `src/lib.rs`
- Create: `src/main.rs`
- Create: `herdr-plugin.toml`
- Create: `scripts/build.sh`
- Create: `LICENSE`
- Create: `.gitignore`
- Test: `tests/manifest.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: a buildable crate `herdr_pets` (lib) + `herdr-pets` (bin); a valid `herdr-plugin.toml`. Later tasks add modules to `src/lib.rs`.

- [ ] **Step 1: Write the failing test** — `tests/manifest.rs`

```rust
//! Validates herdr-plugin.toml has the fields herdr requires (herdr 0.7.0).

use toml::Value;

fn manifest() -> Value {
    let raw = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/herdr-plugin.toml"))
        .expect("herdr-plugin.toml must exist at repo root");
    raw.parse::<Value>().expect("herdr-plugin.toml must be valid TOML")
}

#[test]
fn manifest_has_required_top_level_fields() {
    let m = manifest();
    assert_eq!(m.get("id").and_then(Value::as_str), Some("herdr-pets"));
    assert_eq!(m.get("name").and_then(Value::as_str), Some("herdr-pets"));
    assert_eq!(m.get("version").and_then(Value::as_str), Some("0.1.0"));
    assert_eq!(m.get("min_herdr_version").and_then(Value::as_str), Some("0.7.0"));
    let platforms: Vec<&str> = m.get("platforms").and_then(Value::as_array).unwrap()
        .iter().filter_map(Value::as_str).collect();
    assert_eq!(platforms, vec!["linux", "macos"]);
}

#[test]
fn manifest_pane_runs_the_release_binary_in_render_mode() {
    let m = manifest();
    let panes = m.get("panes").and_then(Value::as_array).expect("[[panes]] present");
    let pane = &panes[0];
    assert_eq!(pane.get("id").and_then(Value::as_str), Some("pets"));
    assert_eq!(pane.get("placement").and_then(Value::as_str), Some("split"));
    let cmd: Vec<&str> = pane.get("command").and_then(Value::as_array).unwrap()
        .iter().filter_map(Value::as_str).collect();
    assert_eq!(cmd, vec!["./target/release/herdr-pets", "render"]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test manifest`
Expected: FAIL — compile error / `herdr-plugin.toml must exist` (no manifest yet, no Cargo.toml).

- [ ] **Step 3: Write the scaffold**

`Cargo.toml`:
```toml
[package]
name = "herdr-pets"
version = "0.1.0"
edition = "2024"
rust-version = "1.96"
description = "A herd of pixel-art pets for your herdr agents."
license = "MIT"
repository = "https://github.com/vandemaelefelix/herdr-pets"
keywords = ["herdr", "tui", "ratatui", "agents"]
categories = ["command-line-utilities"]

[lib]
name = "herdr_pets"
path = "src/lib.rs"

[[bin]]
name = "herdr-pets"
path = "src/main.rs"

[dependencies]
ratatui = "0.30"
crossterm = "0.29"
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[dev-dependencies]
insta = "1"
toml = { version = "0.8", default-features = false, features = ["parse"] }
```

`src/lib.rs`:
```rust
//! herdr-pets — a herd of pixel-art pets for your herdr agents.
//!
//! Phase 0: foundations. Modules are added task-by-task.
```

`src/main.rs`:
```rust
fn main() {
    eprintln!("herdr-pets: no subcommand yet");
}
```

`herdr-plugin.toml`:
```toml
# herdr-plugin.toml — manifest for herdr-pets (Phase 0).
id = "herdr-pets"
name = "herdr-pets"
version = "0.1.0"
description = "A herd of pixel-art pets for your herdr agents."
min_herdr_version = "0.7.0"
platforms = ["linux", "macos"]

[[build]]
platforms = ["linux", "macos"]
command = ["/bin/sh", "scripts/build.sh"]

# Phase 0: the pane is opened manually via `herdr plugin pane open`.
# Full-width bottom placement + auto-injection are Phases 2-3.
[[panes]]
id = "pets"
title = "Pets"
placement = "split"
command = ["./target/release/herdr-pets", "render"]
```

`scripts/build.sh`:
```sh
#!/bin/sh
# herdr [[build]] step: build the release binary.
# Source ~/.cargo/env so cargo is found even when herdr launches without
# ~/.cargo/bin on PATH (GUI / login-less launch). The [ -f ] guard means a
# missing env file can't abort the build.
set -e
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
exec cargo build --release
```

`.gitignore`:
```
/target
```

`LICENSE`: standard MIT text, copyright line `Copyright (c) 2026 vandemaelefelix`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `chmod +x scripts/build.sh && cargo test --test manifest`
Expected: PASS (2 tests). Also run `cargo build` — expected: succeeds.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/lib.rs src/main.rs herdr-plugin.toml scripts/build.sh LICENSE .gitignore tests/manifest.rs
git commit -m "chore: scaffold cargo project and herdr plugin manifest"
```

---

### Task 2: Agent model & JSON parsing

**Files:**
- Create: `src/agent.rs`
- Modify: `src/lib.rs` (add `pub mod agent;`)
- Create: `tests/fixtures/agent-list.json`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `herdr_pets::agent::AgentStatus` — enum `{ Idle, Working, Blocked, Done, Unknown }` (`Copy`).
  - `herdr_pets::agent::Agent` — struct with `agent: Option<String>`, `agent_status: AgentStatus`, `name: Option<String>`, `cwd: String`, `foreground_cwd: String`, `workspace_id: String`, `tab_id: String`, `pane_id: String`, `terminal_id: String`, `revision: i64`, `focused: bool`; method `fn label(&self) -> String`.
  - `herdr_pets::agent::parse_agent_list(json: &str) -> Result<Vec<Agent>, serde_json::Error>`.

- [ ] **Step 1: Write the fixture** — `tests/fixtures/agent-list.json`

```json
{"id":"cli:agent:list","result":{"agents":[
  {"agent":"claude","agent_status":"working","cwd":"/Users/felix/projects/herdr-pets","focused":true,"foreground_cwd":"/Users/felix/projects/herdr-pets","name":"pets-dev","pane_id":"w1T:p1","revision":0,"tab_id":"w1T:t1","terminal_id":"term_aaa","workspace_id":"w1T"},
  {"agent":"claude","agent_status":"idle","cwd":"/Users/felix","focused":false,"foreground_cwd":"/Users/felix","pane_id":"w7:p8","revision":2,"tab_id":"w7:t6","terminal_id":"term_bbb","workspace_id":"w7"},
  {"agent_status":"unknown","cwd":"/Users/felix/x","focused":false,"foreground_cwd":"/Users/felix/x","pane_id":"w1F:p3","revision":0,"tab_id":"w1F:t1","terminal_id":"term_ccc","workspace_id":"w1F"},
  {"agent":"claude","agent_status":"blocked","cwd":"/tmp","focused":false,"foreground_cwd":"/tmp","name":"watcher","pane_id":"w4:p5","revision":1,"tab_id":"w4:t3","terminal_id":"term_ddd","workspace_id":"w4"}
],"type":"agent_list"}}
```

- [ ] **Step 2: Write the failing test** — append to `src/agent.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/agent-list.json"));

    #[test]
    fn parses_all_agents_from_the_envelope() {
        let agents = parse_agent_list(FIXTURE).expect("valid fixture");
        assert_eq!(agents.len(), 4);
    }

    #[test]
    fn parses_statuses_including_unknown_and_blocked() {
        let a = parse_agent_list(FIXTURE).unwrap();
        assert_eq!(a[0].agent_status, AgentStatus::Working);
        assert_eq!(a[1].agent_status, AgentStatus::Idle);
        assert_eq!(a[2].agent_status, AgentStatus::Unknown);
        assert_eq!(a[3].agent_status, AgentStatus::Blocked);
    }

    #[test]
    fn optional_agent_and_name_are_none_when_absent() {
        let a = parse_agent_list(FIXTURE).unwrap();
        assert_eq!(a[2].agent, None);
        assert_eq!(a[2].name, None);
        assert_eq!(a[1].name, None);
    }

    #[test]
    fn unrecognised_status_falls_back_to_unknown() {
        let json = r#"{"result":{"agents":[{"agent_status":"wat","cwd":"/","focused":false,"foreground_cwd":"/","pane_id":"p","revision":0,"tab_id":"t","terminal_id":"x","workspace_id":"w"}]}}"#;
        let a = parse_agent_list(json).unwrap();
        assert_eq!(a[0].agent_status, AgentStatus::Unknown);
    }

    #[test]
    fn label_prefers_name_then_agent_then_pane_id() {
        let a = parse_agent_list(FIXTURE).unwrap();
        assert_eq!(a[0].label(), "pets-dev");   // has name
        assert_eq!(a[1].label(), "claude");     // no name, has agent
        assert_eq!(a[2].label(), "w1F:p3");     // neither -> pane_id
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test --lib agent`
Expected: FAIL — `parse_agent_list`/`Agent`/`AgentStatus` not found.

- [ ] **Step 4: Write minimal implementation** — top of `src/agent.rs`

```rust
//! Agent model: deserialize `herdr agent list` output.

use serde::Deserialize;

/// An agent's live status. `Unknown` is the fallback for panes with no detected
/// agent and for any status string herdr adds later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    #[serde(other)]
    Unknown,
}

/// One agent, as reported in `result.agents[]`. `agent` and `name` are absent
/// for `unknown`-status panes, so both are optional.
#[derive(Debug, Clone, Deserialize)]
pub struct Agent {
    #[serde(default)]
    pub agent: Option<String>,
    pub agent_status: AgentStatus,
    #[serde(default)]
    pub name: Option<String>,
    pub cwd: String,
    pub foreground_cwd: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub pane_id: String,
    pub terminal_id: String,
    #[serde(default)]
    pub revision: i64,
    pub focused: bool,
}

impl Agent {
    /// Human label: prefer the user-set `name`, else the detected `agent`
    /// kind, else the stable `pane_id`.
    pub fn label(&self) -> String {
        self.name
            .clone()
            .or_else(|| self.agent.clone())
            .unwrap_or_else(|| self.pane_id.clone())
    }
}

#[derive(Debug, Deserialize)]
struct Envelope {
    result: EnvelopeResult,
}

#[derive(Debug, Deserialize)]
struct EnvelopeResult {
    agents: Vec<Agent>,
}

/// Parse the `herdr agent list` envelope into the agent vector.
pub fn parse_agent_list(json: &str) -> Result<Vec<Agent>, serde_json::Error> {
    let env: Envelope = serde_json::from_str(json)?;
    Ok(env.result.agents)
}
```

Add to `src/lib.rs`: `pub mod agent;`

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib agent`
Expected: PASS (5 tests).

- [ ] **Step 6: Commit**

```bash
git add src/agent.rs src/lib.rs tests/fixtures/agent-list.json
git commit -m "feat(agent): parse herdr agent list into an Agent model"
```

---

### Task 3: herdr CLI seam

**Files:**
- Create: `src/herdr.rs`
- Modify: `src/lib.rs` (add `pub mod herdr;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `herdr_pets::herdr::HerdrCli` — trait with `fn run_json(&self, args: &[&str]) -> std::io::Result<String>`.
  - `herdr_pets::herdr::CommandRunner` — trait with `fn run(&self, program: &OsStr, args: &[&str]) -> std::io::Result<Output>`.
  - `herdr_pets::herdr::RealRunner` — unit struct implementing `CommandRunner` via `std::process::Command`.
  - `herdr_pets::herdr::LiveHerdr<R>` — implements `HerdrCli`; `LiveHerdr::from_env() -> LiveHerdr<RealRunner>`, `LiveHerdr::with_runner(program, runner) -> LiveHerdr<R>`.
  - `herdr_pets::herdr::resolve_program(var: Option<String>) -> OsString`.

- [ ] **Step 1: Write the failing test** — append to `src/herdr.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    struct Fake {
        stdout: String,
        raw_status: i32,
    }
    impl CommandRunner for Fake {
        fn run(&self, _program: &OsStr, _args: &[&str]) -> io::Result<Output> {
            Ok(Output {
                status: ExitStatus::from_raw(self.raw_status),
                stdout: self.stdout.clone().into_bytes(),
                stderr: Vec::new(),
            })
        }
    }

    #[test]
    fn run_json_returns_stdout_on_success() {
        let h = LiveHerdr::with_runner("herdr", Fake { stdout: r#"{"ok":true}"#.into(), raw_status: 0 });
        assert_eq!(h.run_json(&["agent", "list"]).unwrap(), r#"{"ok":true}"#);
    }

    #[test]
    fn run_json_errors_on_nonzero_exit() {
        // from_raw(256) => exit code 1 on unix.
        let h = LiveHerdr::with_runner("herdr", Fake { stdout: String::new(), raw_status: 256 });
        assert!(h.run_json(&["agent", "list"]).is_err());
    }

    #[test]
    fn resolve_program_falls_back_to_herdr() {
        assert_eq!(resolve_program(None), OsString::from("herdr"));
        assert_eq!(resolve_program(Some(String::new())), OsString::from("herdr"));
        assert_eq!(resolve_program(Some("/custom/herdr".into())), OsString::from("/custom/herdr"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib herdr`
Expected: FAIL — items not found.

- [ ] **Step 3: Write minimal implementation** — top of `src/herdr.rs`

```rust
//! herdr query seam: shell out to the `herdr` CLI, behind traits so tests never
//! spawn a real process. Ported from the herdr-file-viewer plugin's pattern
//! (unix-only here; platforms exclude Windows).

use std::ffi::{OsStr, OsString};
use std::io;
use std::process::{Command, Output};

/// The substitution point the app depends on: run a herdr subcommand expected
/// to emit JSON on stdout.
pub trait HerdrCli {
    fn run_json(&self, args: &[&str]) -> io::Result<String>;
}

/// Inner seam: lets tests assert argv without real spawning.
pub trait CommandRunner {
    fn run(&self, program: &OsStr, args: &[&str]) -> io::Result<Output>;
}

/// Real command execution via `std::process::Command`.
pub struct RealRunner;
impl CommandRunner for RealRunner {
    fn run(&self, program: &OsStr, args: &[&str]) -> io::Result<Output> {
        Command::new(program).args(args).output()
    }
}

/// The live herdr adapter.
pub struct LiveHerdr<R: CommandRunner = RealRunner> {
    program: OsString,
    runner: R,
}

impl LiveHerdr<RealRunner> {
    /// Resolve `herdr` from `$HERDR_BIN_PATH` (or `"herdr"` on PATH).
    pub fn from_env() -> Self {
        Self {
            program: resolve_program(std::env::var("HERDR_BIN_PATH").ok()),
            runner: RealRunner,
        }
    }
}

impl<R: CommandRunner> LiveHerdr<R> {
    pub fn with_runner(program: impl Into<OsString>, runner: R) -> Self {
        Self { program: program.into(), runner }
    }
}

impl<R: CommandRunner> HerdrCli for LiveHerdr<R> {
    fn run_json(&self, args: &[&str]) -> io::Result<String> {
        let out = self.runner.run(&self.program, args)?;
        if !out.status.success() {
            return Err(io::Error::other("herdr exited non-zero"));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// `Some(non-empty)` → that path; `None`/empty → `"herdr"`.
pub fn resolve_program(var: Option<String>) -> OsString {
    match var {
        Some(v) if !v.is_empty() => OsString::from(v),
        _ => OsString::from("herdr"),
    }
}
```

Add to `src/lib.rs`: `pub mod herdr;`

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib herdr`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/herdr.rs src/lib.rs
git commit -m "feat(herdr): add HerdrCli seam shelling out to the herdr CLI"
```

---

### Task 4: Render the pane

**Files:**
- Create: `src/render.rs`
- Modify: `src/lib.rs` (add `pub mod render;`)

**Interfaces:**
- Consumes: `herdr_pets::agent::{Agent, AgentStatus, parse_agent_list}`, `herdr_pets::herdr::HerdrCli`.
- Produces:
  - `herdr_pets::render::status_glyph(AgentStatus) -> char`.
  - `herdr_pets::render::draw(frame: &mut ratatui::Frame, agents: &[Agent])`.
  - `herdr_pets::render::run(herdr: &dyn HerdrCli) -> std::io::Result<()>` — the terminal loop.

- [ ] **Step 1: Write the failing snapshot test** — append to `src/render.rs`

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib render`
Expected: FAIL — `status_glyph`/`draw` not found.

- [ ] **Step 3: Write minimal implementation** — top of `src/render.rs`

```rust
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
) -> io::Result<()> {
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
```

Add to `src/lib.rs`: `pub mod render;`

- [ ] **Step 4: Run tests; accept snapshots**

Run: `cargo test --lib render` → first run reports new snapshots pending.
Run: `cargo insta accept` (or review with `cargo insta review`).
Run: `cargo test --lib render`
Expected: PASS (3 tests). Snapshot files land in `src/snapshots/`.

- [ ] **Step 5: Commit**

```bash
git add src/render.rs src/lib.rs src/snapshots/
git commit -m "feat(render): draw the agent list placeholder in the pets pane"
```

---

### Task 5: Wire the binary

**Files:**
- Modify: `src/main.rs`
- Test: `tests/cli.rs` (create)

**Interfaces:**
- Consumes: `herdr_pets::herdr::LiveHerdr`, `herdr_pets::render`.
- Produces: a `herdr-pets` binary that dispatches `render`, `--version`/`-V`, and a usage error otherwise.

- [ ] **Step 1: Write the failing test** — `tests/cli.rs`

```rust
//! CLI smoke: --version prints the crate version; unknown args fail with usage.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_herdr-pets"))
}

#[test]
fn version_flag_prints_version_and_succeeds() {
    let out = bin().arg("--version").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")), "got: {stdout}");
}

#[test]
fn no_subcommand_fails_with_usage() {
    let out = bin().output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("usage"), "got: {stderr}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test cli`
Expected: FAIL — no `--version` handling; `main` prints the stub.

- [ ] **Step 3: Write implementation** — `src/main.rs`

```rust
use std::process::ExitCode;

use herdr_pets::herdr::LiveHerdr;
use herdr_pets::render;

fn main() -> ExitCode {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("render") => {
            let herdr = LiveHerdr::from_env();
            match render::run(&herdr) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("herdr-pets: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("--version") | Some("-V") => {
            println!("herdr-pets {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: herdr-pets render");
            ExitCode::FAILURE
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test cli`
Expected: PASS (2 tests). Then `cargo test` — expected: whole suite green.

- [ ] **Step 5: Manual dev-loop check (real herdr)**

```bash
cargo build --release
herdr plugin link .
herdr plugin list --json    # confirm herdr-pets appears
herdr plugin pane open --plugin herdr-pets --entrypoint pets
```
Expected: a split pane opens showing the "herdr-pets" box with your live agents; `q` closes the loop. Note anything surprising for the spike work.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs tests/cli.rs
git commit -m "feat: dispatch render subcommand and --version from the binary"
```

---

### Task 6: Raw-socket scaffolding for Spike A

**Files:**
- Create: `src/socket.rs`
- Modify: `src/lib.rs` (add `pub mod socket;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `herdr_pets::socket::socket_path() -> Option<std::path::PathBuf>` — reads `$HERDR_SOCKET_PATH`.
  - `herdr_pets::socket::request(path: &std::path::Path, payload: &str) -> std::io::Result<String>` — connect, write `payload` + `\n`, read the reply to EOF.

This module is deliberately minimal — it exists to let Spike A probe `layout_export`/`layout_apply`. The exact request schema is unknown by design; Spike A discovers it.

- [ ] **Step 1: Write the failing test** — append to `src/socket.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;

    #[test]
    fn request_writes_payload_and_reads_reply() {
        let dir = std::env::temp_dir().join(format!("herdr-pets-sock-{}", std::process::id()));
        let _ = std::fs::remove_file(&dir);
        let listener = UnixListener::bind(&dir).unwrap();

        let server = std::thread::spawn({
            let dir = dir.clone();
            move || {
                let (mut conn, _) = listener.accept().unwrap();
                let mut got = String::new();
                let mut buf = [0u8; 64];
                // read the one line the client sends
                loop {
                    let n = conn.read(&mut buf).unwrap();
                    got.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if got.contains('\n') { break; }
                }
                conn.write_all(b"PONG").unwrap();
                drop(conn); // EOF for the client
                let _ = std::fs::remove_file(&dir);
                got
            }
        });

        let reply = request(&dir, "PING").unwrap();
        assert_eq!(reply, "PONG");
        let got = server.join().unwrap();
        assert_eq!(got, "PING\n");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib socket`
Expected: FAIL — `request` not found.

- [ ] **Step 3: Write minimal implementation** — top of `src/socket.rs`

```rust
//! Minimal raw unix-socket helper — Spike A scaffolding only.
//!
//! Phase 0 does NOT ship a full socket client; that is Phase 1 (event
//! subscription). This exists so Spike A can send a `layout_export` /
//! `layout_apply` request to `$HERDR_SOCKET_PATH` and read the reply.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

/// The herdr control socket, from `$HERDR_SOCKET_PATH`.
pub fn socket_path() -> Option<PathBuf> {
    std::env::var_os("HERDR_SOCKET_PATH").map(PathBuf::from)
}

/// Connect, send `payload` followed by a newline, and read the reply to EOF.
pub fn request(path: &Path, payload: &str) -> std::io::Result<String> {
    let mut stream = UnixStream::connect(path)?;
    stream.write_all(payload.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    Ok(reply)
}
```

Add to `src/lib.rs`: `pub mod socket;`

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib socket`
Expected: PASS (1 test).

- [ ] **Step 5: Commit**

```bash
git add src/socket.rs src/lib.rs
git commit -m "feat(socket): add minimal unix-socket helper for Spike A"
```

---

### Task 7: Spike A — full-width injection (experiment + findings)

**Files:**
- Modify: `docs/superpowers/specs/2026-07-23-phase-0-foundations-design.md` (fill §5 Spike A "Finding").

**This is an experiment, not TDD.** Run it in a **throwaway scratch tab** so the live session is never at risk. Record what actually happens.

- [ ] **Step 1: Create an isolated scratch tab with multiple panes**

```bash
herdr tab create
herdr tab list          # note the new tab id
# In the new tab, create a second pane so the tab is already split:
herdr pane split --direction right
herdr pane layout --current    # capture the starting layout tree (JSON)
```

- [ ] **Step 2: Test the CLI fallback first (lowest risk)**

```bash
herdr pane split --direction down
herdr pane layout --current    # is the new pane full-width across the bottom, or only under one column?
# If not full-width, try repositioning:
herdr pane move --help
herdr pane move <new-pane-id> ...   # attempt to make it the full-width bottom child
```
Record: does `pane split --direction down` on a multi-pane tab yield a full-width bottom pane, or only split one column? Does `pane move` fix it?

- [ ] **Step 3: Probe the socket `layout_export` schema**

```bash
echo $HERDR_SOCKET_PATH
# Discover the exact request framing by sending a layout_export-style request.
# Start from the CLI's own shape (herdr pane layout --current is the read side);
# send the corresponding socket method and inspect the reply structure.
printf '%s\n' '{"id":"spike","method":"layout_export","params":{}}' | nc -U "$HERDR_SOCKET_PATH"
```
If the framing differs (length-prefixed, different envelope, method name), iterate until a reply is returned. Capture the exact working request + the reply's `LayoutNode`/`LayoutPane` shape.

- [ ] **Step 4: Attempt `layout_apply` with a new command pane as the full-width bottom child**

Using the schema learned in Step 3, wrap the exported tree in a root vertical split whose bottom child is a new `LayoutPane { command: ["<abs path>/target/release/herdr-pets", "render"], env: {} }`, and send `layout_apply`. Observe whether herdr **spawns the new command pane** full-width, rearranges only, or rejects it.

- [ ] **Step 5: Tear down the scratch tab**

```bash
herdr tab list
herdr tab close <scratch-tab-id>
```

- [ ] **Step 6: Write the finding**

In the design doc §5 Spike A, replace `_(to be filled in during implementation)_` with: which approach works (socket `layout_apply` vs. `pane split`+`pane move`), the exact working request/command, and the recommendation for Phase 2. If the answer contradicts the strip-per-tab design, update `GOAL.md` + `docs/PLAN.md` and flag it to the user (per guardrails).

- [ ] **Step 7: Commit**

```bash
git add docs/superpowers/specs/2026-07-23-phase-0-foundations-design.md GOAL.md docs/PLAN.md
git commit -m "docs(phase-0): record Spike A (full-width injection) findings"
```

---

### Task 8: Spike B — new-tab / bootstrap trigger (experiment + findings)

**Files:**
- Modify: `herdr-plugin.toml` (temporary experimental `[[events]]` hook; may be reverted after).
- Modify: `docs/superpowers/specs/2026-07-23-phase-0-foundations-design.md` (fill §5 Spike B "Finding").

**Experiment, not TDD.**

- [ ] **Step 1: Add an experimental event hook + logging action to the manifest**

```toml
# --- Spike B experiment (temporary) ---
[[actions]]
id = "pets-spike-log"
platforms = ["linux", "macos"]
title = "pets spike log"
description = "Spike B: prove whether event hooks fire."
command = ["/bin/sh", "-c", "date >> /tmp/herdr-pets-spikeb.log"]

[[events]]
on = "TabCreated"
action = "pets-spike-log"
```
(If manifest parsing rejects the `on` value, note the accepted event names from the error and retry — that itself is a finding.)

- [ ] **Step 2: Re-link and trigger**

```bash
rm -f /tmp/herdr-pets-spikeb.log
herdr plugin unlink herdr-pets 2>/dev/null; herdr plugin link .
herdr tab create           # should fire TabCreated if hooks work
sleep 2
cat /tmp/herdr-pets-spikeb.log 2>&1     # did it fire?
herdr plugin log list --plugin herdr-pets --limit 20
```
Also probe plugin-enable / session-start: `herdr plugin disable herdr-pets && herdr plugin enable herdr-pets`, then re-check the log.

- [ ] **Step 3: If nothing fires, confirm the polling fallback**

```bash
herdr tab list             # snapshot 1 (note tab ids)
herdr tab create
herdr tab list             # snapshot 2 — diff detects the new tab within ~1-2s
```
Confirm polling `herdr tab list` reliably detects new tabs (the Phase 3 fallback).

- [ ] **Step 4: Revert the experimental manifest changes**

```bash
git checkout herdr-plugin.toml    # drop the temporary [[events]]/[[actions]]
herdr plugin unlink herdr-pets; herdr plugin link .
```

- [ ] **Step 5: Write the finding**

In the design doc §5 Spike B, replace `_(to be filled in during implementation)_` with: whether `[[events]]` hooks fire (and on which event names), whether a session-start/enable trigger exists, and the Phase 3 recommendation (event-driven vs. polling). Update `GOAL.md`/`docs/PLAN.md` if it changes the design; flag to the user.

- [ ] **Step 6: Commit**

```bash
git add docs/superpowers/specs/2026-07-23-phase-0-foundations-design.md GOAL.md docs/PLAN.md
git commit -m "docs(phase-0): record Spike B (new-tab trigger) findings"
```

---

### Task 9: Close out Phase 0

**Files:**
- Modify: `docs/PLAN.md` (Phase tracker row for Phase 0).

**Interfaces:** none.

- [ ] **Step 1: Update the Phase tracker**

In `docs/PLAN.md`, set the Phase 0 row `Status` to `Done`, and link the design + plan docs:

```markdown
| 0 | Foundations & spikes | Done | [design](superpowers/specs/2026-07-23-phase-0-foundations-design.md) | [plan](superpowers/plans/2026-07-23-phase-0-foundations.md) |
```

- [ ] **Step 2: Verify the whole suite + a clean build**

Run: `cargo test && cargo build --release`
Expected: all tests pass; release binary builds.

- [ ] **Step 3: Commit**

```bash
git add docs/PLAN.md
git commit -m "docs(phase-0): mark Phase 0 done and link design + plan"
```

- [ ] **Step 4: Report to the user**

Summarize: what landed, both spike findings, any GOAL/PLAN changes, and confirm nothing was pushed. Ask whether to push `feat/phase-0-foundations` and/or open a PR.

---

## Self-Review

**Spec coverage:**
- Crate/repo shape (design §2) → Task 1. ✅
- Modules `herdr`/`agent`/`render`/`socket` (design §3) → Tasks 3/2/4/6. ✅
- Manifest (design §4) → Task 1 (+ temporary edit in Task 8). ✅
- Build script (design §4) → Task 1. ✅
- Spike A (design §5) → Task 7. ✅
- Spike B (design §5) → Task 8. ✅
- Testing: manifest, agent, herdr, render snapshot (design §6) → Tasks 1/2/3/4; CLI smoke added in Task 5. ✅
- Exit criteria (design §1): links + runs → Task 5 Step 5; pane shows live herd → Task 4/5; spikes documented → Tasks 7/8; tracker updated → Task 9. ✅

**Placeholder scan:** No "TBD/TODO" in shippable code steps. The only intentional fill-ins are the two spike *findings*, which are the deliverables of Tasks 7/8 (experiments). All code steps contain complete code.

**Type consistency:** `HerdrCli::run_json`, `parse_agent_list`, `Agent`/`AgentStatus`, `Agent::label`, `status_glyph`, `draw`, `render::run`, `LiveHerdr::{from_env,with_runner}`, `resolve_program`, `socket::{socket_path,request}` — names/signatures match across Producer/Consumer blocks and their usages in Tasks 4 and 5.
