# 🐑 herdr-herd

**Herd your herd.** Every AI agent in herdr becomes a pixel-art sheep grazing
along the bottom of your terminal — dozing when idle, sprinting when working, and
turning red and stamping when it's blocked and needs you. Your whole fleet's
state, felt at a glance.

> Your terminal already knows which agents need you. Now you can feel it.

<!-- ![herdr-herd: a slim strip of pixel sheep along the bottom of a herdr tab](assets/strip.png) -->

## What it is

herdr shows each agent's state (idle, working, blocked, done) in its sidebar.
herdr-herd turns that same information into a living scene: one little herd
animal per agent, always in view, whose mood mirrors the agent.

- 😴 **idle** — the sheep dozes.
- 🏃 **working** — the sheep runs and grazes, happy and energetic.
- ❗ **done** — the sheep sits up alert, with a calm `!`: *come see.*
- 😾 **blocked** — the sheep turns red and jumps/paws for attention: *I need you now.*

Wherever you are in herdr, the strip is there, so a glance tells you: is anyone
stuck? is anyone finished? is everyone busy? is it quiet?

## How it works

- **One sheep per agent** shown in the sidebar's *agents* section (scope
  configurable; by default it mirrors the sidebar).
- **Stable identity.** Each agent deterministically maps to the same herd
  member — a species (sheep or goat) and a color — so you learn to recognize
  your herd. Same agent, same sheep, every time.
- **Live.** The herd reacts to state changes in near real-time via herdr's
  event stream.
- **Useful, not just cute.** No name labels cluttering the strip — instead,
  **hover** a sheep to see its agent's name, and **click** it to jump straight
  to that agent's pane.
- **One hatted sheep: "you are here."** The agent in the focused pane wears a
  small red hat. herdr reports one focused pane per session and the strip is
  session-wide, so exactly one sheep is hatted at a time, in every tab, even
  when that agent lives in another workspace. Focusing something that is not an
  agent pane (the strip itself, a shell, an editor) leaves the hat on the agent
  you were last in, rather than dropping it from every sheep. See
  [GOAL.md](GOAL.md#the-focus-hat-one-global-you-are-here-marker) for the full
  semantics.
- **Universal rendering.** Sprites are drawn with half-block characters and
  24-bit color, so it looks pixel-art in every terminal, with no dependencies.

## Under the hood

A herdr plugin renders inside panes, so herdr-herd keeps a slim, full-width strip
pane pinned at the bottom of each tab and injects it automatically — on startup
and whenever a new tab appears. See [GOAL.md](GOAL.md#the-compromise-and-why-the-goal-still-holds)
for why this is the shape, and the spec for how it's built.

Two cooperating modes of one binary:

- `herdr-herd render` — runs in each strip pane; draws and animates the herd,
  handles hover/click.
- `herdr-herd control` — the watchdog that keeps a strip present in every tab.

Both read state from herdr's socket API; herdr is the single source of truth.

## Install

```sh
herdr plugin install vandemaelefelix/herdr-herd
```

That's it. herdr fetches a **prebuilt binary** for your platform (macOS on Apple
Silicon or Intel, Linux on x86-64 or arm64) — **no Rust toolchain required**. On
any other platform it falls back to building from source with `cargo`, so the
install still succeeds wherever Rust is available.

Requires **herdr ≥ 0.7.0**.

To pin a specific version, pass a tag: `herdr plugin install
vandemaelefelix/herdr-herd --ref v0.2.1`.

### From a local checkout (development)

```sh
herdr plugin link .
```

## Quickstart

After installing, start the watchdog once — it keeps a herd strip present in
every eligible tab:

```sh
herdr plugin action invoke herdr-herd start-herd-controller
```

(herdr fires no plugin-start hook, so the watchdog doesn't auto-start on a fresh
session — run this once per session, or from herdr's action picker.) For a
one-off strip in just the current tab, use the **`place-herd`** action instead.
See [Usage](#usage) for the difference between the two.

## Usage

herdr-herd ships two actions (run via herdr's action picker, or their
underlying commands directly):

- **`place-herd`** / `herdr-herd place` — on-demand: places a full-width herd
  strip in the current tab right now. This uses herdr's destructive pane
  rebuild to make room, so it's opt-in — run it when you want a strip and
  don't already have one.
- **`start-herd-controller`** / `herdr-herd control` — the always-on watchdog.
  Once started, it keeps a strip present across every tab in every workspace,
  automatically and non-destructively. It injects a full-width strip into any
  tab that has a full-width bottom pane — single-pane tabs and the common
  "content on top, full-width terminal/agent across the bottom" multi-pane
  layout — by splitting that bottom pane (which never kills its process). Tabs
  whose bottom edge is split into side-by-side columns have no full-width bottom
  pane, so a full-width strip there needs the destructive rebuild; use the
  on-demand **`place`** for those. The watchdog does not auto-start on a fresh
  herdr session (herdr fires no plugin-start hook) — start it once via the
  action above.

## Configuration

herdr-herd reads an optional `config.toml` from its plugin config dir
(`herdr plugin config-dir herdr-herd`). Every key is optional and falls back to
an opinionated default:

| Key | Type | Default | Meaning |
|---|---|---|---|
| `enabled` | bool | `true` | Whether the `control` watchdog runs. |
| `strip_rows` | int | `5` | Strip height, in rows. Slim on purpose: a herd is a glanceable status line, not the focus. |
| `sweep_interval_ms` | int | `3000` | Controller poll cadence (ms). |
| `reduced_motion` | bool | `false` | Calm herd — no wandering or bounce. |
| `renderer` | `auto` \| `kitty` \| `half-block` | `auto` | Which rendering backend to draw the herd with. |
| `member_scale` | int | `4` | Kitty-backend sprite scale (image px per sprite px); ignored by half-block. The default is roughly 1 transmitted pixel per screen pixel; raising it mostly costs transmission bandwidth and terminal memory. |
| `sounds_enabled` | bool | `false` | Master switch for notification sounds. Off out of the box. |
| `sound_<status>_enabled` | bool | `true` for `blocked`, else `false` | Per-status toggle. `<status>` is `idle`, `working`, `blocked`, or `done`. |
| `sound_<status>_path` | string | unset | Sound file to play on that status's transition. No bundled sounds ship, so a status plays nothing until you point it at a file. |

`strip_rows` applies to the always-on `control` watchdog; the on-demand
`herdr-herd place` uses a fixed height.

The kitty backend adapts its own band to whatever pane it's given, so `auto`
picking kitty (wherever the terminal supports it) is unaffected by
`strip_rows`. The half-block backend's band has a fixed pixel height, so at
the default 5 rows it shows the member cropped, losing headroom above the
head rather than the feet. A half-block user who wants the whole member drawn
uncropped should raise `strip_rows` to 10 (9 band rows + 1 overlay lane row).

Example `config.toml`:

    reduced_motion = true
    strip_rows = 6

### Notification sounds

A sound plays when a sheep's status *transitions* into a notifying state — not
on every render tick, and not for the initial snapshot when the strip first
picks up an agent that's already `blocked`. Only surviving agents that
actually change status trigger a sound.

Sounds are off by default (`sounds_enabled = false`); `blocked` is pre-armed
so turning on the master switch and pointing it at a file is enough to hear
it:

    sounds_enabled = true
    sound_blocked_path = /path/to/blocked.wav

`sound_done_path`, `sound_working_path`, and `sound_idle_path` work the same
way but stay off (`sound_<status>_enabled = false`) until you opt in.

Playback shells out to the platform's native player (`afplay` on macOS,
`paplay`/`aplay` on Linux) rather than pulling in an audio crate, matching
the minimal-dependency approach `src/herdr.rs` already uses for the `herdr`
CLI. A missing file or unavailable player is silently ignored — it never
crashes or blocks the strip. If several agents transition to the same status
in one tick, that status's sound plays once, not once per agent.

## Rendering

herdr-herd always works with the universal **half-block** renderer (`▀▄`
characters + 24-bit color) — no dependencies, no setup, correct everywhere.

On top of that, herdr-herd can draw the herd as small, crisp, full-detail images
via the **kitty graphics protocol**, an **experimental, opt-in upgrade** on
terminals that support it (e.g. Ghostty, kitty). Set `renderer = "auto"`
(the default) to use it automatically when available, falling back to
half-blocks everywhere else; or force it with `renderer = "kitty"` /
`renderer = "half-block"`.

The kitty upgrade has a prerequisite chain, since herdr itself gates it:

1. The outer terminal must support the kitty graphics protocol.
2. herdr must have the experimental flag enabled in
   `~/.config/herdr/config.toml`:
   ```toml
   [experimental]
   kitty_graphics = true
   ```
3. Run `herdr server reload-config`, then **detach and reattach** your herdr
   client — rendering support is negotiated client-side, so an existing
   session won't pick up the flag until you reattach.

If any of those aren't in place, `renderer = "auto"` silently falls back to
the half-block renderer — the strip always renders, just without the extra
crispness. (`renderer = "kitty"` is a forced override for testing/debugging:
it skips the probe and always draws via kitty escapes, so use `auto` unless
you know the prerequisites are met.)

## Development

Built in Rust. Development uses `herdr plugin link` for a fast iterate loop.
Details will follow in the spec and `CONTRIBUTING`.

### Testing changes in a dedicated session

Testing in the herdr session you actually work in means a dev controller lands
on top of live agents. Instead, run the herd in a session of its own:

```sh
sh scripts/herd-test.sh
```

Run it from a **plain terminal tab** — herdr refuses to nest by default, so this
cannot start from inside an existing herdr session. It builds the binary, opens
(or reattaches) a session named `herd-test`, and starts the controller against
that session's socket. Everything is scoped by `HERDR_SOCKET_PATH`, including
the controller's own lock, so your working session is never touched.

Strips are placed by the controller sweeping the session, exactly as they are
in real use — never by opening panes by hand. Give the test session single-pane
tabs: automatic injection needs a full-width bottom pane to split.

Its controller log is at `target/herd-test-controller.log`, its config dir is
`.herd-test/config` (so settings you try out do not leak into the installed
plugin's global config), and `HERDR_HERD_TEST_SESSION` overrides the session
name.

### Hot reload

You do not need to restart anything after a change. Each sweep the controller
compares the binary on disk against the one it started from; when they differ it
closes every strip it injected and re-execs itself, so the controller is running
the new build too. The next sweep re-injects the strips from the new binary.

So the loop is just:

```sh
cargo build --release --features dev-marker
```

Within about one sweep interval (`sweep_interval_ms`, 3s by default) the strips
blink and come back on the new build. Watch the marker's timestamp change to
confirm. This applies to renderer *and* controller changes, since the controller
replaces itself.

Reload only ever closes strips the controller injected (labelled `herdr-herd`).
A strip opened by hand from the manifest keeps running, because the sweep cannot
always put one back — a tab whose bottom edge is split into columns has no
full-width pane to split.

Each sweep also reaps duplicates: if a tab somehow ends up with more than one
strip, every strip after the first is closed, so a tab holds exactly one. The
main way that used to happen is now closed off too — an injected pane that
cannot be labelled is removed rather than left as an orphan the next sweep
cannot see.

### Knowing which build you are looking at

`scripts/herd-test.sh` builds with the `dev-marker` feature, which draws a
marker at the left of each strip's overlay lane:

```
v0.2.1 75d494c 17:52:10
```

Version, commit, and build time; `*` means the working tree was dirty. The time
changes on every rebuild, so after a fix you can tell at a glance whether the
strip in front of you contains it. `herdr-herd --version` prints the same
string.

The feature is **off by default**, so the marker and its layout cost are absent
from a shipped binary rather than hidden at runtime. Release builds pass no
extra features. `cargo test` covers the shipped layout; `cargo test --features
dev-marker` covers the marker on top of it.

## License

MIT — see [LICENSE](LICENSE).
