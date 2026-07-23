# herdr-pets

**A herd of pixel-art pets for your herdr agents.** Every agent in herdr's
sidebar gets its own colored, pixelated pet in a slim strip along the bottom of
every tab. Each pet's behavior reflects its agent's live state — so you can feel
the state of your whole fleet at a glance, without reading a thing.

> Status: **in design.** This README describes what herdr-pets is meant to be.
> The project's north star lives in [GOAL.md](GOAL.md), the phased roadmap in
> [docs/PLAN.md](docs/PLAN.md); each phase's detailed design + plan land in
> `docs/superpowers/specs/`.

<!-- ![herdr-pets: a slim strip of pixel pets along the bottom of a herdr tab](assets/strip.png) -->

## What it is

herdr shows each agent's state (idle, working, blocked, done) in its sidebar.
herdr-pets turns that same information into a living scene: a small animal per
agent, always in view, whose mood mirrors the agent.

- 😴 **idle** — the pet dozes.
- 🏃 **working** — the pet runs and plays, happy and energetic.
- 🐕 **done** — the pet sits up alert and wags, with a calm `!`: *come see.*
- 😾 **blocked** — the pet turns red and jumps/paws for attention: *I need you now.*

Wherever you are in herdr, the strip is there, so a glance tells you: is anyone
stuck? is anyone finished? is everyone busy? is it quiet?

## How it works

- **One pet per agent** shown in the sidebar's *agents* section (scope
  configurable; by default it mirrors the sidebar).
- **Stable identity.** Each agent deterministically maps to the same pet — a
  species (cat, dog, fox, frog, bird, bunny…) and a color — so you learn to
  recognize your herd. Same agent, same pet, every time.
- **Live.** Pets react to state changes in near real-time via herdr's event
  stream.
- **Useful, not just cute.** No name labels cluttering the strip — instead,
  **hover** a pet to see its agent's name, and **click** it to jump straight to
  that agent's pane.
- **Universal rendering.** Sprites are drawn with half-block characters and
  24-bit color, so it looks pixel-art in every terminal, with no dependencies.

## Under the hood

A herdr plugin renders inside panes, so herdr-pets keeps a slim, full-width strip
pane pinned at the bottom of each tab and injects it automatically — on startup
and whenever a new tab appears. See [GOAL.md](GOAL.md#the-compromise-and-why-the-goal-still-holds)
for why this is the shape, and the spec for how it's built.

Two cooperating modes of one binary:

- `herdr-pets render` — runs in each strip pane; draws and animates the pets,
  handles hover/click.
- `herdr-pets control` — the watchdog that keeps a strip present in every tab.

Both read state from herdr's socket API; herdr is the single source of truth.

## Install

```sh
# For local development, from a checkout of this repo:
herdr plugin link .

# From the plugin registry (release): builds from source via this repo's
# `[[build]]` step in herdr-plugin.toml.
herdr plugin install <owner>/herdr-pets
```

herdr-pets requires **herdr ≥ 0.7.0**.

## Usage

herdr-pets ships two actions (run via herdr's action picker, or their
underlying commands directly):

- **`place-pets`** / `herdr-pets place` — on-demand: places a full-width pets
  strip in the current tab right now. This uses herdr's destructive pane
  rebuild to make room, so it's opt-in — run it when you want a strip and
  don't already have one.
- **`start-pets-controller`** / `herdr-pets control` — the always-on watchdog.
  Once started, it keeps a strip present across tabs automatically: it's
  non-destructive and only auto-injects into tabs that currently have a single
  pane, so it never rearranges or kills a tab with work already running in it.

## Configuration

herdr-pets reads an optional `config.toml` from its plugin config dir
(`herdr plugin config-dir herdr-pets`). Every key is optional and falls back to
an opinionated default:

| Key | Type | Default | Meaning |
|---|---|---|---|
| `enabled` | bool | `true` | Whether the `control` watchdog runs. |
| `strip_rows` | int | `7` | Strip height, in rows. |
| `sweep_interval_ms` | int | `3000` | Controller poll cadence (ms). |
| `reduced_motion` | bool | `false` | Calm pets — no wandering or bounce. |

`strip_rows` applies to the always-on `control` watchdog; the on-demand
`herdr-pets place` uses a fixed height.

Example `config.toml`:

    reduced_motion = true
    strip_rows = 6

## Development

Built in Rust. Development uses `herdr plugin link` for a fast iterate loop.
Details will follow in the spec and `CONTRIBUTING`.

## License

TBD.
