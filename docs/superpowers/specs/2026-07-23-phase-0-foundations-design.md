# Phase 0 — Foundations & spikes (design)

**Date:** 2026-07-23
**Phase:** 0 of 5 (see [`docs/PLAN.md`](../../PLAN.md))
**Status:** approved, pre-implementation
**Resolves against:** [`GOAL.md`](../../../GOAL.md)

## 1. Goal & exit criteria

A herdr plugin that **installs and runs** via `herdr plugin link`, opens a pane
showing the **live agent list**, with both de-risking spikes **answered and
written down**.

**Exit criteria:**
- Plugin links cleanly: `herdr plugin link .` succeeds and `herdr plugin list`
  shows it.
- `herdr plugin pane open --plugin herdr-pets --entrypoint pets` opens a pane
  that renders a placeholder plus one line per live agent, updating as the herd
  changes.
- Spike A and Spike B are answered, with findings written into §5 of this doc.
- If a spike contradicts the design, `GOAL.md` and `docs/PLAN.md` are updated and
  the contradiction is flagged to the user (per the handoff guardrails).

**Explicitly out of scope** (these are Phases 1–4): real sprites, animation,
deterministic identity hashing, mouse (hover/click), full-width auto-injection,
the `control` watchdog mode, and any config surface.

## 2. Crate & repository shape

- Rust **edition 2024**, `rust-version = 1.96`.
- License: **MIT** (matches the file-viewer plugin).
- Dependencies (kept minimal for Phase 0):
  - `ratatui = "0.30"` — TUI rendering.
  - `crossterm = "0.29"` — terminal backend.
  - `serde = { version = "1", features = ["derive"] }`, `serde_json = "1"` —
    parse `herdr … --json` output and build the one Spike-A socket request.
  - `toml` (parse-only) — only if `tests/manifest.rs` parses the manifest
    (expected yes).
  - dev: `insta` (snapshot tests).
- `src/lib.rs` + `src/main.rs` split so all logic lives in the library and is
  unit/snapshot-testable; the binary is a thin entry point. Mirrors the
  canonical `herdr-file-viewer` plugin.

## 3. Modules

| Module | Responsibility | Depends on |
|---|---|---|
| `main.rs` | Bin entry. Parse the subcommand (`render`; plus `--version`). Hand-rolled arg parsing over `std::env::args` — no `clap` for one subcommand. Dispatches into the library. | lib |
| `herdr.rs` | The herdr query seam. `HerdrCli` trait (`run_json`), `LiveHerdr` implementation that shells out to the `herdr` CLI via `std::process::Command`, and an inner `CommandRunner` seam so tests substitute a recorder/fake and never spawn a real process. Resolves the `herdr` program from `$HERDR_BIN_PATH` or falls back to `"herdr"` on `PATH`. Direct port of file-viewer's pattern. | — |
| `agent.rs` | `Agent` struct (serde `Deserialize`) for one entry of `herdr agent list --json`: `agent`, `agent_status`, `name`, `cwd`, `foreground_cwd`, `workspace_id`, `tab_id`, `pane_id`, `terminal_id`, `focused`. Plus an `AgentStatus` enum (`Idle`, `Working`, `Blocked`, `Done`, `Unknown`) with an `Unknown` fallback for unrecognised values. | serde |
| `render.rs` | The `render` subcommand. Sets up ratatui + crossterm (alternate screen, raw mode), then a simple loop: fetch agents via `HerdrCli`, draw a **placeholder header + one line per agent** (name + status), poll-redraw every ~1–2s, quit on `q` / Ctrl-C, restore the terminal on exit. No animation, no sprites. | herdr, agent |
| `socket.rs` | **Spike-A scaffolding only.** A thin raw-socket helper: connect to `$HERDR_SOCKET_PATH`, send a single JSON-RPC request (`layout_export` / `layout_apply`), read the reply. Deliberately minimal and clearly commented as experiment support — it is *not* the Phase 1 event-subscription client. | serde_json |

**Why CLI-first + a thin raw socket** (decision): the file-viewer plugin proves
the CLI shell-out pattern is sufficient and highly testable for reads
(`agent list --json`), which is all the Phase 0 render pane needs. The raw
socket is required *only* because Spike A's `layout_apply` has no CLI wrapper. A
full JSON-over-socket client (event subscription, etc.) is deferred to Phase 1,
where live updates are actually in scope. This is the least code that answers
Phase 0's questions.

## 4. Manifest (`herdr-plugin.toml`)

```toml
id = "herdr-pets"
name = "herdr-pets"
version = "0.1.0"
description = "A herd of pixel-art pets for your herdr agents."
min_herdr_version = "0.7.0"
platforms = ["linux", "macos"]

[[build]]
platforms = ["linux", "macos"]
command = ["/bin/sh", "scripts/build.sh"]

[[panes]]
id = "pets"
title = "Pets"
placement = "split"        # Phase 0: manually opened. Full-width injection = Phase 2.
command = ["./target/release/herdr-pets", "render"]
```

- **Build step** is a 4-line `scripts/build.sh` that sources `~/.cargo/env` (if
  present) and runs `cargo build --release`. This is "plain cargo build" per the
  design decision — the script wrapper exists only because file-viewer learned
  herdr can launch without `~/.cargo/bin` on `PATH` (GUI / login-less launch).
  The full prebuilt-download + SHA-verify + fallback `fetch-or-build.sh` is
  release infrastructure and belongs to **Phase 4 (packaging)** — YAGNI now.
- **Pane** uses `placement = "split"` and is opened manually in Phase 0 via
  `herdr plugin pane open --plugin herdr-pets --entrypoint pets`. Full-width
  bottom placement and auto-injection are Phases 2–3. No `[[actions]]` entry is
  needed yet.

## 5. The two spikes

Both are run in a **throwaway scratch tab** (with 1–2 dummy panes) so any layout
mishap is isolated from the user's live ~20-agent session. Findings are written
back into this section.

### Spike A — full-width injection
**Question:** Can `layout_apply` (raw socket) insert a *new command pane*
running our command as the full-width bottom child (root vertical split) of a
tab that *already has multiple panes*?

**Method:**
1. In a scratch tab with 2+ panes, capture the current tree via `layout_export`
   (socket) / `herdr pane layout --current`.
2. Construct a `layout_apply` request wrapping the existing tree in a root
   vertical split with a new `LayoutPane { command, env }` as the bottom child,
   and send it over the socket.
3. Observe whether herdr **spawns the new command pane** or only rearranges
   existing panes / rejects the request.
4. Test the fallback: `herdr pane split --direction down` + `herdr pane move`.

**Finding:** _(to be filled in during implementation)_

Feeds **Phase 2** (full-width placement).

### Spike B — new-tab / bootstrap trigger
**Question:** Do `[[events]]` manifest hooks fire on `TabCreated` (and is there a
session-start / plugin-enable trigger)?

**Method:**
1. Add an experimental `[[events]]` hook to the manifest pointing at a trivial
   action that logs.
2. Re-link the plugin, create a new tab, and check `herdr plugin log list` for
   the hook firing. Probe session-start / plugin-enable the same way.
3. If nothing fires, confirm the polling fallback: `herdr tab list` polled every
   ~1–2s detects new tabs.

**Finding:** _(to be filled in during implementation)_

Feeds **Phase 3** (auto-injection: event-driven vs. polling).

## 6. Testing (TDD)

Write the failing test first for each shippable unit. Spikes are experiments
verified by their written findings, not by assertions.

- `tests/manifest.rs` — parse `herdr-plugin.toml`; assert required fields
  (`id`, `name`, `version`, `min_herdr_version`, `platforms`) and that the
  `[[panes]]` command path is `./target/release/herdr-pets`.
- `agent.rs` unit tests — deserialize a captured `herdr agent list --json`
  fixture into `Vec<Agent>`; assert `AgentStatus` parsing incl. the `Unknown`
  fallback.
- `render` snapshot test — inject a fake `CommandRunner` returning the fixture
  JSON; render into a ratatui `TestBackend` and snapshot (`insta`) the buffer
  showing the placeholder header + one line per agent.

## 7. Implementation tracks (subagent-driven)

Largely independent, dispatched after the plan is written:
- **Track A:** repo scaffold — `Cargo.toml`, `src/lib.rs`/`main.rs` skeleton,
  `herdr-plugin.toml`, `scripts/build.sh`, `LICENSE` (MIT), `.gitignore`, plus
  `tests/manifest.rs`.
- **Track B:** `herdr.rs` + `agent.rs` with the `HerdrCli`/`CommandRunner` seam
  and deserialization tests.
- **Track C:** `render.rs` + snapshot test (depends on B's types).

Then, sequentially (they need a live herd and a scratch tab):
- **Spike A**, then **Spike B**, with findings written into §5.

Finally: update the Phase tracker table in `docs/PLAN.md` (link this design + the
plan, set status).

## 8. Guardrails (from the handoff)

- Work on a **branch off `main`**, never on `main` directly.
- **Do not commit or push without the user asking**; local checkpoint commits may
  be proposed.
- Keep Phase 0 small — resist scope creep into Phases 1–4.
- If a spike result contradicts the design, update `GOAL.md` + `docs/PLAN.md`
  first and flag it to the user.
