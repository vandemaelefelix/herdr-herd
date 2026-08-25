# Project Goal — the North Star

This document is the reference we resolve questions against. When a design or
implementation decision is unclear, it should be answered by asking: *does this
serve the goal below?* If a proposed feature doesn't, it's out.

## Mission (one line)

**Herd your herd of agents: give every agent in herdr's sidebar a living,
colored, pixel-art sheep whose behavior reflects its state — always visible, at
a glance, wherever you are.**

## The intent

Herdr's sidebar already tells you each agent's state (idle, working, blocked,
done) as text and dots. That works, but it's cognitive — you *read* it. The herd
turns that same information into something you *feel* at a glance: a flock of
little animals whose mood mirrors your fleet. A sleeping sheep means "nothing to
do here." One jumping and turning red means "this one needs you, now." You
should be able to know the state of your whole herd without reading a single
word.

It should also be **useful, not just decoration**: the strip is a fast way to
see and reach any agent in your fleet.

## The non-negotiable goal

1. **One sheep per agent shown in the sidebar's "agents" section.** The herd
   mirrors that panel (which is scoped by herdr's `agent_panel_scope`).
2. **Always visible.** You should not have to summon it or switch to it. As you
   move around herdr, the herd is there.
3. **Behavior reflects live state.** Each sheep's animation is driven by its
   agent's real status, updated in near real-time.
4. **Stable identity.** A given agent is always the same sheep (same species,
   same color), so you learn to recognize your herd.

## The compromise (and why the goal still holds)

A herdr plugin cannot paint over herdr's own chrome (the sidebar and tab bar are
herdr's, and there is no global persistent overlay for plugins). Everything a
plugin draws lives in a **pane**.

So "always visible everywhere" is achieved by **injecting a slim, full-width sheep
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

## The focus hat: one global "you are here" marker

Exactly one sheep in the strip wears a small red hat. It answers one question:
*which agent am I working with right now?* The decided semantics, locked
2026-08-19:

- **Session-wide scope.** Every strip lists every agent in the herdr session,
  across all workspaces and tabs (`herdr agent list` is fetched unfiltered).
  This is deliberate, not a bug. The strip is a fast way to see and reach *any*
  agent in the fleet, so it shows the whole fleet.
- **One hat, globally.** Because the herd is session-wide, the hat is a global
  marker: exactly one hatted sheep exists across the whole session, whichever
  tab or workspace you happen to be looking at. Every strip agrees on which
  sheep it is. `Herd::reconcile` enforces this rather than trusting herdr's
  per-agent flag, so a future herdr that reported focus per workspace or per tab
  could never hat the whole herd.
- **Focus is pane-level, not tab-level.** herdr reports exactly one focused pane
  for the whole session (verified live on herdr 0.8.0: 1 focused of 45 panes, 1
  focused of 17 agents). Four agents sharing one tab therefore produce one hat,
  on the selected pane's agent.
- **Sticky on non-agent panes.** When the focused pane is not an agent pane (the
  strip itself, a shell, an editor), herdr reports zero focused agents. The hat
  then stays on the agent that most recently held focus instead of vanishing
  from every sheep. Clicking the strip must not make the marker blink out. The
  memory is dropped when that agent leaves the herd, so a hat never lingers on a
  dead sheep, and a genuine focus change replaces it immediately.

Two consequences are accepted:

1. A strip in tab A will hat an agent living in workspace B, and the agent
   running in the pane directly above a strip often has no hat.
2. Stickiness is per render process. Each strip is its own process and remembers
   only the focus changes it has observed, so a strip that starts *after* the
   last focus change has nothing to remember and shows no hat until the next
   focus event, while older strips show one. herdr's snapshot carries no
   "focused last" field to recover it from, so this cannot be fixed inside the
   plugin. Session-wide scope means every strip otherwise sees identical data,
   so this is the only way two strips can disagree.

## Design principles

- **Glanceable over detailed.** The win is instant emotional read of the whole
  herd. Anything that requires reading or hunting works against this.
- **Stable, recognizable identity.** Deterministic agent → (species, color).
  Never random per session.
- **Universal first.** Must work in every terminal (half-block sprites, 24-bit
  color). Fancier rendering (e.g. Kitty graphics) is only ever an opt-in upgrade
  on top, never a requirement.
- **Useful, not merely cute.** Click a sheep to jump to its agent; hover to see its
  name. The strip earns its space.
- **Unobtrusive.** Slim, calm by default, never steals focus, never interrupts
  work. It should be possible to forget it's a pane.
- **Opinionated defaults, few knobs.** Sensible out of the box; configurable
  where it matters (on/off, scope, height/motion, palette/behavior).

## Non-goals (YAGNI)

- Not a dashboard, not a metrics/telemetry surface, not a log viewer.
- Not a general herdr UI framework — it does one thing.
- No gameplay/tamagotchi mechanics (feeding, leveling, currency). Sheep reflect
  agents; they are not a game.
- No requirement to modify herdr itself. It is a plugin, full stop.
- No cross-machine/remote sync of sheep state beyond what herdr already exposes.

## Locked decisions (from the design conversation)

| Question | Decision |
|---|---|
| Where sheep render | Slim full-width strip pane, auto-injected into every tab |
| Which agents | Mirrors the sidebar "agents" section; scope configurable (default: mirror) |
| Blocked sheep | Red, angry face, jumping/pawing for attention |
| Done sheep | Sits up alert, wags, calm `!` "come see" |
| Idle / working | Sleeping / happy-running |
| Identity | Deterministic hash(agent) → species + color |
| Labels | None by default; name on hover, jump on click |
| Rendering | Half-block Unicode + 24-bit color (Kitty sprites = future opt-in) |
| Interactivity | Click sheep → focus its agent's pane |
| Herd scope | Session-wide: every strip shows every agent in the session |
| Focus hat | Exactly one globally, on the focused *pane*'s agent; sticky on non-agent panes |
| Config | on/off + auto-inject, scope, height + motion, palette + per-state behavior |
| Language | Rust |

## How to use this doc

- Before adding a feature: does it serve the mission and principles? If not, cut it.
- When two designs compete: pick the one that is more glanceable and less obtrusive.
- If the compromise ever feels like it's fighting the goal, revisit here first.
