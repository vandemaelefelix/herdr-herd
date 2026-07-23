# Handoff — start planning Phase 0

**To:** the next agent picking up herdr-pets.
**From:** the design/brainstorming session (2026-07-23).
**Your mission:** produce the **design + plan for Phase 0** (Foundations & spikes),
then implement it TDD + subagent-driven. You are *not* being asked to build the
whole product — just Phase 0.

---

## 1. Read these first (in order)

1. [`GOAL.md`](../../GOAL.md) — the north star + a table of every locked design
   decision. **Everything resolves against this.** Do not re-open settled
   decisions unless something forces it.
2. [`README.md`](../../README.md) — what the product is.
3. [`docs/PLAN.md`](../PLAN.md) — the 5-phase roadmap. You own **Phase 0** only.
4. This handoff — the herdr technical facts already discovered (§5) save you hours.

## 2. Where things stand

- Repo: `github.com/vandemaelefelix/herdr-pets` (private, personal account
  `vandemaelefelix`). `origin/main` exists with only docs — **no code yet.**
- Design phase is **done**; all high-level decisions are locked in GOAL.md.
- License is **TBD** (lean MIT to match the file-viewer plugin, but not decided).
- Nothing has been built or spiked yet.

## 3. Your task, concretely

Follow the superpowers flow for Phase 0 as its own sub-project:

1. **`superpowers:brainstorming`** — brainstorm *Phase 0 specifics only* (not the
   whole product; that's designed). Open questions to settle are in §6.
2. Write the Phase 0 **design doc** → `docs/superpowers/specs/YYYY-MM-DD-phase-0-foundations-design.md`.
3. **`superpowers:writing-plans`** — write the Phase 0 **plan doc**.
4. Implement with **`superpowers:test-driven-development`** (failing test first)
   and **`superpowers:subagent-driven-development`** (dispatch independent tasks).
5. Update the **Phase tracker** table in `docs/PLAN.md` (link the design + plan,
   set status) as you go.

## 4. Phase 0 scope (from PLAN.md — do not exceed it)

**Goal:** a plugin that installs and runs, plus the two unknowns de-risked.
- Rust skeleton, `herdr-plugin.toml`, build/fetch script, working `herdr plugin
  link` dev loop.
- A socket-client module (list agents, subscribe to events).
- A minimal `render` pane that draws a placeholder and prints the real agent list.
- **Spike A** and **Spike B** (see §5.4), with findings written down.

**Exit:** plugin installs via `herdr plugin link`, opens a pane showing the live
herd; both spikes answered and documented (they shape Phases 2–3).

Explicitly **out of scope** for Phase 0: real sprites, animation, identity
hashing, mouse, auto-injection, config. Those are Phases 1–4.

## 5. herdr technical reference (already discovered — trust but re-verify with `--help`)

Environment: **herdr 0.7.0**, macOS (darwin), zsh. herdr is a tmux-like
multiplexer for AI agents. We are running *inside* a herdr-managed pane, so the
`herdr` CLI drives this session. Use the **`using-herdr` skill** for orientation.

### 5.1 The plugin model
A plugin is a git repo with a `herdr-plugin.toml` manifest + a program herdr runs.
Manifest sections (confirmed against the installed file-viewer plugin):
- `[[build]]` — `{ command, platforms }`, run at install time.
- `[[panes]]` — `{ id, title, placement, command }`. `command` is relative to the
  plugin root (e.g. `["./target/release/herdr-pets"]`).
- `[[actions]]` — `{ id, title, description, command, platforms }`, bindable to a
  keybinding in `~/.config/herdr/config.toml`.
- `[[events]]` — **exists** (`PluginManifestEventHook { on, action, ... }`) but the
  file-viewer doesn't use it, and *which* event names fire hooks is **unconfirmed**
  → that's **Spike B**.
- Required manifest fields: `id`, `name`, `version`, `min_herdr_version`, `platforms`.

Plugin CLI: `herdr plugin install <owner>/<repo>`, `herdr plugin link <path>`
(dev), `herdr plugin list [--json]`, `herdr plugin enable/disable`,
`herdr plugin config-dir <id>`, `herdr plugin pane open|focus|close`,
`herdr plugin action list|invoke`, `herdr plugin log list`.

### 5.2 Pane placements
`herdr plugin pane open --plugin ID --entrypoint ID --placement overlay|split|tab|zoomed`.
From the binary: **overlay** panes "target the active pane" and belong to a tab
(not a persistent global layer — confirmed: a plugin cannot paint over herdr's own
chrome). That's *why* the design injects a strip per tab (see GOAL.md).

### 5.3 The data (source of truth = herdr socket, `$HERDR_SOCKET_PATH`)
- `herdr agent list --json` → array of agents with: `agent`, `agent_status`
  (`idle|working|blocked|done|unknown`), `name`, `cwd`, `foreground_cwd`,
  `workspace_id`, `tab_id`, `pane_id`, `terminal_id`, `focused`.
- `herdr agent focus <target>` → focus an agent's pane (this is the click action
  in later phases; target = name, label, or pane id).
- **Event subscription exists** over the socket: `Subscription::PaneAgentStatusChanged`
  and `PaneOutputMatched` (params `EventsSubscribeParams` / `EventsWaitParams`).
  The CLI exposes `herdr wait agent-status <pane-id> --status ...` and
  `herdr wait output ...`; richer subscribe is socket-level. **Note:** the
  subscription enum appears limited to those two match types — lifecycle events
  like `TabCreated` may NOT be subscribable the same way (relevant to Spike B).
- Layout: `herdr pane layout [--current]` returns the tab's layout tree
  (`{panes, splits}` with `direction`/`ratio`/`rect`). Socket has `layout_export`
  + `layout_apply` (`LayoutNode::Split`, `LayoutPane { command, env }`) — **no CLI
  wrapper**, so talk to the socket directly. Whether `layout_apply` can *spawn a
  new command pane* into an already-split tab is **Spike A**.
- Tabs: `herdr tab list|create|get|focus|close`. Panes: `herdr pane split
  --direction right|down`, `pane move`, `pane close`, `pane run`, `pane
  report-metadata` (can set `custom_status` / `state_labels` on a pane).

### 5.4 The two spikes (the point of Phase 0)
- **Spike A — full-width injection.** Can `layout_apply` (socket) insert a *new*
  pane running our command as the full-width bottom child (root vertical split) of
  a tab that *already has multiple panes*? If not, fallback = `pane split
  --direction down` (works cleanly only when the tab has one pane) + `pane move` to
  reposition. Write down which works. Feeds **Phase 2**.
- **Spike B — new-tab / bootstrap trigger.** Do `[[events]]` manifest hooks fire on
  `TabCreated` (and is there a session-start/plugin-enable trigger)? If yes →
  auto-inject + controller bootstrap are event-driven. If no → the controller
  polls `herdr tab list` every ~1–2s. Write down the answer. Feeds **Phase 3**.

### 5.5 Reference plugin to study
The **file-viewer** plugin is installed locally and is the canonical example (Rust,
same era, well-documented):
`/Users/felix/.config/herdr/plugins/github/herdr-file-viewer-c993314e2614/`
Read its `herdr-plugin.toml`, `scripts/fetch-or-build.sh`, `ARCHITECTURE.md`,
`CONTEXT.md`, and `tests/` layout — mirror its conventions (manifest shape,
build/fetch script, snapshot tests, security posture for untrusted repos).

### 5.6 Rendering (for later phases, FYI)
Sprites = half-block `▀▄█` + 24-bit color (universal). herdr *also* supports the
Kitty graphics protocol (`experimental.kitty_graphics`) — that's a **Phase 4
stretch** only, not now.

## 6. Open decisions to settle in Phase 0 brainstorming

- Rust crates: `ratatui` + `crossterm` (TUI/mouse/truecolor) — confirm; socket
  client (hand-rolled JSON over the unix socket vs. a helper). Study how
  file-viewer talks to herdr.
- Binary shape: one binary with `render` / `control` subcommands (per GOAL/PLAN) —
  confirm the CLI surface for Phase 0 (probably just `render` + a placeholder).
- Build/fetch script strategy (prebuilt download + cargo fallback, like file-viewer)
  vs. plain `cargo build` for now.
- Manifest `min_herdr_version = "0.7.0"`, `platforms` (macos + linux to start?).
- How to run/verify the spikes without disrupting the user's live ~20-agent session
  (use a throwaway tab/workspace).

## 7. Guardrails & conventions

- **Don't push or commit without the user asking** (they set up the repo and want
  to control commits). Local commits for your own checkpoints are fine to *propose*.
- Work on a **branch off `main`**, not on `main` directly. Per the user's global
  setup, if isolation is wanted use the **native worktree mechanism** (`EnterWorktree`
  / `claude --worktree`), never raw `git worktree add`.
- Keep Phase 0 small. Resist scope creep into Phases 1–4.
- If a spike result contradicts the design, update `GOAL.md` + `docs/PLAN.md`
  before continuing, and flag it to the user.
- User is `vandemaelefelix` (they/them until told otherwise); personal repo, private.
