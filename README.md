# herdr-herd

**Herd your herd of agents.** Every agent in herdr's sidebar becomes a colored,
pixel-art sheep in a slim strip along the bottom of every tab. Each sheep's
behavior reflects its agent's live state — so you can feel the state of your
whole flock at a glance, without reading a thing.

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
vandemaelefelix/herdr-herd --ref v0.1.0`.

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
| `strip_rows` | int | `5` | Strip height, in rows (1 icon lane + 3 pixel rows + 1 caption). |
| `sweep_interval_ms` | int | `3000` | Controller poll cadence (ms). |
| `reduced_motion` | bool | `false` | Calm herd — no wandering or bounce. |
| `renderer` | `auto` \| `kitty` \| `half-block` | `auto` | Which rendering backend to draw the herd with. |
| `member_scale` | int | `7` | Kitty-backend sprite scale (image px per sprite px); ignored by half-block. |
| `sounds_enabled` | bool | `false` | Master switch for notification sounds. Off out of the box. |
| `sound_<status>_enabled` | bool | `true` for `blocked`, else `false` | Per-status toggle. `<status>` is `idle`, `working`, `blocked`, or `done`. |
| `sound_<status>_path` | string | unset | Sound file to play on that status's transition. No bundled sounds ship, so a status plays nothing until you point it at a file. |

`strip_rows` applies to the always-on `control` watchdog; the on-demand
`herdr-herd place` uses a fixed height.

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

## License

MIT — see [LICENSE](LICENSE).
