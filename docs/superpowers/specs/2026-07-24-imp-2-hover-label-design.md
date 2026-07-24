# Improvement 2 — Richer hover label (design)

**Date:** 2026-07-24
**Kind:** post-roadmap improvement
**Status:** approved (autonomous run — brainstorming gate waived), pre-implementation
**Resolves against:** [`GOAL.md`](../../../GOAL.md) — "Useful, not merely cute… hover to
see its name."

## 1. Problem & goal

Hover today shows `Agent::label()` = `name || agent || pane_id`. The maintainer
always runs Claude, so `agent` is almost always the literal `"claude"` and
`name` is usually unset ⇒ **every pet hovers as "claude"**, useless for telling
agents apart.

**Goal:** hover shows the same thing herdr's left sidebar shows for that agent —
its **workspace → tab breadcrumb** — so a hover instantly says *which* agent and
*what* it's doing.

## 2. Grounding facts (verified live, herdr 0.7.0, 2026-07-24)

`herdr agent list` gives only ids (`workspace_id`, `tab_id`) plus `agent`/`name`/
`cwd`/`foreground_cwd`. The human labels live on other objects:

- `herdr workspace list` → each workspace has a **`label`** (e.g. `"herdr-pets"`,
  `"vbrb-pinb"`, `"Home folder"`) — the sidebar's workspace header.
- `herdr tab list` → each tab has a **`label`** (e.g. `"Monorepo UI package"`,
  `"XML implementaion"`, `"Diff"`, `"Lazygit"`) — the per-tab row in the sidebar.

So the sidebar entry for an agent reads **`<workspace label> › <tab label>`**.
`agent list` alone can't produce it — it must be **joined** by `workspace_id`
and `tab_id`. (This is the "source the fuller identity from the socket data"
the request calls for.)

Data flow: a background **watcher** (owns the `herdr` CLI seam + debounce) fetches
`agent list` and pushes `Vec<Agent>` down a channel; the render loop calls
`Herd::reconcile`, which sets `Pet.label = agent.label()`; the hover caption
renders `Pet.label`. Label resolution therefore belongs in the **watcher**
(it already holds the CLI and is debounced to ~250 ms / 2.5 s — not per-frame).

## 3. Design

The hover label mirrors the sidebar breadcrumb:

```
hover = "<workspace-label> › <tab-label>"
```

with a resilient fallback chain (any piece may be missing):

| workspace label | tab label | result |
|---|---|---|
| yes | yes | `"<ws> › <tab>"` |
| yes | no | `"<ws>"` |
| no | yes | `"<tab>"` |
| no | no | basename(`foreground_cwd`) → basename(`cwd`) → legacy `label()` |

**Why workspace › tab (not the agent kind/name):** it is literally what the
sidebar shows and is maximally discriminating — workspace tells the project, tab
tells the task ("XML implementaion"). The generic agent kind (`"claude"`) is the
useless string we're replacing; a user-set `name`, when present, is already what
herdr uses as that tab's label, so the tab label subsumes it. Keeping the join a
small **pure function** makes the exact format trivial to tweak later.

**Units:**

| Unit | Responsibility |
|---|---|
| `agent.rs` (extend) | `Agent` gains `#[serde(default)] hover_label: Option<String>` (not in the JSON — resolved post-fetch). `Agent::display_label()` returns `hover_label` if set, else the legacy `label()`. A pure `sidebar_label(ws: Option<&str>, tab: Option<&str>) -> String` builds the breadcrumb + folder/legacy fallback. |
| `sidebar.rs` (new) | Tolerant serde for `workspace list` / `tab list` → `id → label` maps (`parse_workspace_labels`, `parse_tab_labels`). std + serde only. |
| `watcher.rs` (extend) | `refetch` resolves labels: fetch `agent list`, plus `workspace list` + `tab list` (each best-effort — failure ⇒ empty map ⇒ fallbacks), build the maps, set each agent's `hover_label`. |
| `herd.rs` (reconcile) | `p.label = a.display_label()` (was `a.label()`). Survivors update their label too (already do). |

`Agent::label()` stays as the legacy/last-resort fallback (still unit-tested).
Adding a field only touches the two test `agent()` helper constructors.

## 4. Error handling

Every enrichment is best-effort (`rust-error-handling`, degrade at the boundary):
a failed/missing `workspace list` or `tab list` ⇒ empty map ⇒ the fallback chain
⇒ never worse than today, never a crash. Serde is tolerant (`#[serde(default)]`,
skip malformed entries), matching the repo's `rust-serde-tolerant-parsing`.

## 5. Testing (TDD — failing test first)

- `agent::sidebar_label`: all four join rows; folder-basename fallback; legacy
  fallback when everything is absent. (Write first — they fail until the fn
  exists.)
- `agent::display_label`: returns `hover_label` when set, else `label()`.
- `sidebar::parse_workspace_labels` / `parse_tab_labels`: map extraction from
  real-shaped envelopes; tolerant of missing `label`; ignores malformed.
- `herd`: extend `reconcile_sets_and_updates_the_pet_label` — an agent whose
  `hover_label` is set shows *that*, and a survivor picks up a changed
  `hover_label`.
- Watcher CLI wiring is thin glue (fetch + map + assign) — covered by the pure
  tests above; not separately mocked beyond existing watcher tests.

## 6. Verification (live)

With the strip open, hover a pet and confirm the caption reads e.g.
`herdr-pets › <tab>` rather than `claude`. (Live hover verified via the same
synthetic-PTY path Phase 2 used, if needed.)

## 7. Guardrails

- Branch `feat/pets-hover-label` off the Improvement-1 tip (stacked); never
  commit to `main`.
- Keep it to label resolution — no changes to draw/placement/controller.
- Gate green before done.
