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

> Not yet released. Planned install paths:

```sh
# From the plugin registry (release):
herdr plugin install <owner>/herdr-pets

# For local development:
herdr plugin link /path/to/herdr-pets
```

herdr-pets requires **herdr ≥ 0.7.0**.

## Configuration

Opinionated defaults, a few knobs (shipped as `config.example.toml`):

- **enable / auto-inject** — turn it off globally, or inject into every tab vs.
  only tabs you opt into.
- **scope** — `mirror-sidebar` (default) · `all` · `current-workspace` ·
  `current-tab`.
- **height & motion** — strip height in rows (default 3), animation speed, and a
  calm `reduced-motion` mode.
- **palette & behavior** — how pet colors are generated (herdr-theme-aligned or
  vivid), and optional overrides of the state→behavior mapping.

## Development

Built in Rust. Development uses `herdr plugin link` for a fast iterate loop.
Details will follow in the spec and `CONTRIBUTING`.

## License

TBD.
