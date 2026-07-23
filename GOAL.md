# Project Goal — the North Star

This document is the reference we resolve questions against. When a design or
implementation decision is unclear, it should be answered by asking: *does this
serve the goal below?* If a proposed feature doesn't, it's out.

## Mission (one line)

**Give every agent in herdr's sidebar a living, colored, pixel-art pet whose
behavior reflects its state — always visible, at a glance, wherever you are.**

## The intent

Herdr's sidebar already tells you each agent's state (idle, working, blocked,
done) as text and dots. That works, but it's cognitive — you *read* it. Pets
turn that same information into something you *feel* at a glance: a herd of
little animals whose mood mirrors your fleet. A sleeping pet means "nothing to
do here." A pet jumping and turning red means "this one needs you, now." You
should be able to know the state of your whole herd without reading a single
word.

It should also be **useful, not just decoration**: the strip is a fast way to
see and reach any agent in your fleet.

## The non-negotiable goal

1. **One pet per agent shown in the sidebar's "agents" section.** The set of
   pets mirrors that panel (which is scoped by herdr's `agent_panel_scope`).
2. **Always visible.** You should not have to summon it or switch to it. As you
   move around herdr, the pets are there.
3. **Behavior reflects live state.** A pet's animation is driven by its agent's
   real status, updated in near real-time.
4. **Stable identity.** A given agent is always the same pet (same species, same
   color), so you learn to recognize your herd.

## The compromise (and why the goal still holds)

A herdr plugin cannot paint over herdr's own chrome (the sidebar and tab bar are
herdr's, and there is no global persistent overlay for plugins). Everything a
plugin draws lives in a **pane**.

So "always visible everywhere" is achieved by **injecting a slim, full-width pet
strip pane at the bottom of every tab** — automatically, on startup and for new
tabs. This is a *rendering-location* compromise, not a change of intent: the
strip still shows the same agents the sidebar shows, still always in view, still
reflecting live state. The goal above is unchanged.

### Injection must never disturb running work (verified 2026-07-23, Phase 3 spike)

A hard constraint discovered while building Phase 3: herdr's `layout.apply`
(the full-tab rewrite that Phase 2's on-demand `place` uses) **re-materialises
every pane in the tab, killing the process running in each** (verified: a marker
process was SIGHUP-killed by an injection). So `layout.apply` can *never* be used
for **automatic** injection — doing so across every tab would kill every running
agent, violating the "unobtrusive / never interrupt work" principle above.

The safe primitive is an **incremental `pane split` (down)**, which preserves the
existing pane's process — but it only yields a **full-width** strip on a
**single-pane** tab (on a multi-pane tab it splits just one column). Therefore
automatic injection is **non-destructive and scoped to single-pane tabs**, which
covers **every newly created tab** (single-pane at creation) and any existing
single-pane tab. Pre-existing **multi-pane** tabs are left to the **on-demand
`place`** command (Phase 2), where the user knowingly accepts the rebuild. This
keeps "always everywhere" true for the natural forward workflow while never
killing work. See `docs/decisions.md` and the Phase 3 design spec.

## Design principles

- **Glanceable over detailed.** The win is instant emotional read of the whole
  herd. Anything that requires reading or hunting works against this.
- **Stable, recognizable identity.** Deterministic agent → (species, color).
  Never random per session.
- **Universal first.** Must work in every terminal (half-block sprites, 24-bit
  color). Fancier rendering (e.g. Kitty graphics) is only ever an opt-in upgrade
  on top, never a requirement.
- **Useful, not merely cute.** Click a pet to jump to its agent; hover to see its
  name. The strip earns its space.
- **Unobtrusive.** Slim, calm by default, never steals focus, never interrupts
  work. It should be possible to forget it's a pane.
- **Opinionated defaults, few knobs.** Sensible out of the box; configurable
  where it matters (on/off, scope, height/motion, palette/behavior).

## Non-goals (YAGNI)

- Not a dashboard, not a metrics/telemetry surface, not a log viewer.
- Not a general herdr UI framework — it does one thing.
- No gameplay/tamagotchi mechanics (feeding, leveling, currency). Pets reflect
  agents; they are not a game.
- No requirement to modify herdr itself. It is a plugin, full stop.
- No cross-machine/remote sync of pet state beyond what herdr already exposes.

## Locked decisions (from the design conversation)

| Question | Decision |
|---|---|
| Where pets render | Slim full-width strip pane, auto-injected into every tab |
| Which agents | Mirrors the sidebar "agents" section; scope configurable (default: mirror) |
| Blocked pet | Red, angry face, jumping/pawing for attention |
| Done pet | Sits up alert, wags, calm `!` "come see" |
| Idle / working | Sleeping / happy-running |
| Identity | Deterministic hash(agent) → species + color |
| Labels | None by default; name on hover, jump on click |
| Rendering | Half-block Unicode + 24-bit color (Kitty sprites = future opt-in) |
| Interactivity | Click pet → focus its agent's pane |
| Config | on/off + auto-inject, scope, height + motion, palette + per-state behavior |
| Language | Rust |

## How to use this doc

- Before adding a feature: does it serve the mission and principles? If not, cut it.
- When two designs compete: pick the one that is more glanceable and less obtrusive.
- If the compromise ever feels like it's fighting the goal, revisit here first.
