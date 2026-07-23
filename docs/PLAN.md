# herdr-pets — Global Plan

The high-level roadmap. This document stays **shallow on purpose**: it names the
phases, their goals, and how they fit together. It does *not* contain detailed
design or task breakdowns — each phase gets its **own design + plan doc** when we
pick it up (see [How we work](#how-we-work)).

Anchor documents:
- [GOAL.md](../GOAL.md) — the north star and locked decisions. Read first.
- [README.md](../README.md) — what the product is.
- This file — the phase map.

## Architecture in one picture

```
                 ┌─────────────────────────────────────────┐
                 │              herdr server                 │
                 │        (single source of truth)           │
                 └───────────────▲───────────────▲───────────┘
                     socket API   │               │  socket API
        ┌────────────────────────┘               └────────────────────────┐
        │                                                                   │
┌───────┴───────────┐                                        ┌──────────────┴──────────────┐
│  herdr-pets render │  (one per strip pane)                  │      herdr-pets control      │  (one, watchdog)
│  • agent list      │                                        │  • enumerate tabs            │
│  • status events   │─▶ animate pets                         │  • inject/keep strip in each │
│  • hover / click   │─▶ agent focus                          │  • respawn on close          │
└────────────────────┘                                        └──────────────────────────────┘
```

One Rust binary, two subcommands. Everything reads state from herdr's socket
(`$HERDR_SOCKET_PATH`); the only write is `agent focus` on a pet click. See
GOAL.md for *why* it's a strip-per-tab and not a global overlay.

## Phases

Each phase is independently reviewable and leaves the project in a working
state. Order matters: later phases build on earlier ones.

### Phase 0 — Foundations & spikes
**Goal:** a plugin that installs and runs, plus the two unknowns de-risked.
- Rust skeleton, `herdr-plugin.toml`, build/fetch script, working `herdr plugin
  link` dev loop.
- A socket-client module (list agents, subscribe to events).
- A minimal `render` pane that draws a placeholder and prints the real agent list.
- **Spike A:** can `layout_apply` inject a *new command pane* full-width at the
  bottom of an already-split tab? (Fallback: `pane split` + `pane move`.)
- **Spike B:** do `[[events]]` hooks fire on `TabCreated` / session-start? (Fallback:
  controller polls `tab list`.)

**Exit:** plugin installs, opens a pane showing your live herd; both spikes answered
and their findings written down (they shape Phases 2–3).

### Phase 1 — The pets (renderer core)
**Goal:** real, animated pets in one manually-opened strip.
- Deterministic identity: `hash(agent)` → species + color.
- Half-block sprite engine + per-state animations (idle/working/done/blocked/unknown).
- Wandering movement; crowding/overflow handling for a large herd.
- Live updates: event subscription + slow periodic refresh.

**Exit:** open one strip, see your actual agents as correct, animated pets that
change with state.

### Phase 2 — Interactivity & placement
**Goal:** the strip is full-width and clickable.
- Mouse: hover → name, click → `agent focus`.
- Full-width bottom placement via root-split wrapping (uses Spike A's outcome).

**Exit:** a full-width strip sits beneath a multi-pane tab; clicking a pet focuses
its agent.

### Phase 3 — Always everywhere (controller / watchdog)
**Goal:** strips appear and stay in every tab, automatically.
- `control` mode: inject into all existing tabs on startup; inject into new tabs
  (event hook or polling fallback from Spike B); respawn on close; re-assert
  placement; single-owner lockfile; session-restore de-dup; bootstrap story.

**Exit:** enable the plugin → a strip is present in every tab and returns if closed.

### Phase 4 — Config & polish
**Goal:** configurable, documented, installable.
- Config surface: enable/auto-inject, scope, height + motion, palette + per-state
  behavior.
- Palette/theme alignment, `reduced-motion`, `+N` overflow refinement.
- Packaging/release (fetch-or-build, CI), user docs.
- *Stretch:* Kitty-graphics sprite upgrade (opt-in, half-block stays the default).

**Exit:** a tagged release installable via `herdr plugin install`, with docs.

## Dependency order

```
Phase 0 ─▶ Phase 1 ─▶ Phase 2 ─▶ Phase 3 ─▶ Phase 4
   │           (Spike A feeds Phase 2, Spike B feeds Phase 3)
   └──────────────────────────────┘
```

Phases 1 and 2 can partly overlap (identity/sprites vs. placement), but 2's
placement depends on Spike A, and 3 depends on 2 being in place.

## How we work

- Each phase gets its **own design doc** (`docs/superpowers/specs/`) and **plan
  doc** when we start it — that's where the depth goes. This global plan only
  points at them.
- Implementation is **test-driven** (write the failing test first) and
  **subagent-driven** (independent tasks dispatched to subagents), per the
  superpowers skills.
- Every phase ends in a working, reviewable state before the next begins.
- If a phase reveals something that changes the shape of the project, update
  GOAL.md and this file before proceeding.

## Phase tracker

| Phase | Name | Status | Design | Plan |
|---|---|---|---|---|
| 0 | Foundations & spikes | Done | [design](superpowers/specs/2026-07-23-phase-0-foundations-design.md) | [plan](superpowers/plans/2026-07-23-phase-0-foundations.md) |
| 1 | The pets (renderer core) | Done | [design](superpowers/specs/2026-07-23-phase-1-renderer-core-design.md) | [plan](superpowers/plans/2026-07-23-phase-1-renderer-core.md) |
| 2 | Interactivity & placement | Not started | — | — |
| 3 | Always everywhere (controller) | Not started | — | — |
| 4 | Config & polish | Not started | — | — |
