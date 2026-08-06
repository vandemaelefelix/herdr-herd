# Dev test harness — design

**Date:** 2026-08-06
**Status:** accepted

## Problem

Testing a herdr-herd change today means running it in the same herdr session the
user actually works in. Two things go wrong.

1. **Testing disturbs real work.** The controller injects strips into every
   eligible tab of whatever session it can reach. There is only one session, so
   a dev controller lands on top of live agents.
2. **You cannot tell which build you are looking at.** The installed plugin is
   pinned to a release commit; a local build may be many fixes ahead. The strip
   looks identical either way, so "is my fix in this pane?" is unanswerable by
   looking. Commit `7874012` removed a hand-rolled `◆v2-namerow` tag that
   existed precisely to answer it, because it was temp scaffolding with no
   shipping story.

## Goals

- A dedicated herdr session for testing, started by one command, that never
  touches the user's working session.
- A visible build identity in dev builds: version, commit, build time.
- That identity must be **absent from shipped builds**, not merely hidden.
- Strips in the test session are placed by the `herdr-herd` process itself
  (the controller), never by an operator running `plugin pane open` by hand.

## Non-goals

- Auto-starting the controller in any session other than the test one.
- Changing how the controller chooses tabs.
- A second registered plugin id (see [Decisions](#decisions)).

## Design

### Session isolation is already load-bearing

No new isolation mechanism is needed. Every path is scoped by
`$HERDR_SOCKET_PATH` today:

- `socket::socket_path` reads it (`src/socket.rs`).
- `herdr::LiveHerdr` shells out to the `herdr` CLI, which inherits it.
- `control::controller_lock_path` hashes the full socket path into the lock
  filename (`src/control.rs`), so two sessions get two independent controllers
  and neither blocks the other.

So the whole harness is: create a second session, point a controller at its
socket. The controller does not need to run *inside* the session. It is a
socket client that enumerates tabs and injects strips; strip panes it spawns
inherit the correct socket from that session's own server.

### `scripts/herd-test.sh`

Ordering constraint: the socket does not exist until the session does, and
`herdr --session <name>` blocks once it attaches. So:

1. `cargo build --release --features dev-marker`.
2. Background a waiter that polls `herdr session list --json` for the named
   session's `socket_path`, then runs `herdr-herd control` against it.
3. `exec herdr --session herd-test` in the foreground.

The socket path is **discovered**, never hardcoded, so the harness does not
depend on herdr's session-directory layout.

Nested herdr is disabled by default, so this script runs in a plain terminal
tab, not inside an existing herdr session. That is the intended usage anyway.

### Build marker

Two independent pieces:

**The stamp** comes from `build.rs`, which emits
`cargo:rustc-env=HERDR_HERD_BUILD=<short-sha>[+]<HH:MM:SS>`. The script emits
**no** `cargo:rerun-if-changed` directives: with none present, Cargo reruns the
build script whenever any file in the package changes, which is exactly the
freshness needed. Every rebuild restamps, so consecutive dev builds of the same
commit are still distinguishable. `+` marks a dirty working tree. Git or `date`
missing degrades to a placeholder rather than failing the build.

**Visibility** is a Cargo feature, `dev-marker`, off by default.
`marker::build_marker()` returns `Some(&'static str)` under the feature and
`None` without it. This is the only option where the marker code is not present
in a shipped binary at all: an env var would ship the code and could be flipped
on accidentally, and `cfg!(debug_assertions)` would force dev builds into the
debug profile, which we do not want for animation smoothness. Release CI passes
no extra features, so it excludes the marker with no CI change.

### Where it draws

The overlay lane above the sheep already exists in both renderers and is
rewritten every frame (`overlay_lane_row` in `src/kitty_render.rs`,
`draw_caption` in `src/render.rs`). Both existing occupants, the hover caption
and the `+N` overflow counter, are **right**-aligned. The marker takes the
**left** edge of the same lane, so the two never contend for the same columns.
The caption's available width shrinks by the marker's reserved width so a long
agent name truncates instead of overwriting the marker.

`marker::reserved_cols()` returns the lane columns the marker occupies, `0`
without the feature. Feeding that single number into both renderers keeps the
shipped layout byte-identical to today.

### Config isolation

`config::resolve_config_dir` asks `herdr plugin config-dir herdr-herd`, which
is global: dev and installed builds share one `config.toml`. Add a
`HERDR_HERD_CONFIG_DIR` env override, checked first, so the harness can point at
its own config dir when a test needs settings that must not leak into the user's
real session. Unset means today's behavior, which is the sensible default:
testing against your real settings is usually what you want.

## Decisions

- **No second plugin id.** An earlier sketch proposed linking the dev checkout
  as `herdr-herd-dev` for a separate config dir. Rejected: the plugin id lives
  in `herdr-plugin.toml`, so a second id means a duplicate manifest, and the dev
  build does not need to be a registered plugin at all. The controller runs the
  built binary directly and spawns `<self_exe> render`. `HERDR_HERD_CONFIG_DIR`
  buys the same config isolation for one env var.
- **Feature flag over env var** for marker visibility, so the code is absent
  from shipped builds. See [Build marker](#build-marker).
- **Marker on the left of the overlay lane**, caption and `+N` stay right. No
  new row: the strip is only a few rows tall and a row is expensive.

## Risks

- The `herdr` CLI may behave differently when invoked outside a managed pane.
  The script exports `HERDR_ENV=1` defensively; verify during implementation.
- `jq` is used to read `session list --json`. Acceptable dev-only dependency;
  the script fails with a clear message if it is missing.
- A stale controller from a previous run could linger. The socket-hashed lock
  makes a second controller for the same session exit cleanly, so re-running
  the script is safe.
