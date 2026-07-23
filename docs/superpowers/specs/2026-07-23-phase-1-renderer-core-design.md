# Phase 1 — The pets (renderer core) (design)

**Date:** 2026-07-23
**Phase:** 1 of 5 (see [`docs/PLAN.md`](../../PLAN.md))
**Status:** approved, pre-implementation
**Resolves against:** [`GOAL.md`](../../../GOAL.md)
**Builds on:** [Phase 0 design](2026-07-23-phase-0-foundations-design.md)

## 1. Goal & exit criteria

Replace the Phase 0 placeholder with the **real, animated pet renderer**: open one
manually-launched strip and see your actual agents as correct, animated,
deterministically-colored pets whose behavior tracks their live status.

**Exit criteria:**
- Opening the pane (`herdr plugin pane open --plugin herdr-pets --entrypoint pets`)
  draws a **roaming herd of half-block sprites**, one pet per agent, not the
  placeholder text lines.
- Each agent maps to a **stable** `(species, color)` via `hash(terminal_id)` —
  the same agent is the same pet across restarts.
- Pets **animate per state** (idle / working / done / blocked / unknown) with the
  behaviors in §5, and the animation **changes when the agent's status changes**,
  in near-real-time.
- The herd **updates live**: an event-driven watcher refetches on change, with a
  slow periodic refresh as a safety net; the pane degrades to polling if the
  socket is unavailable and never crashes.
- Adding a new species is **plug-and-play** — proven by shipping a second species
  (§7) and a validation test that guards the sprite format.
- All new logic is covered by hermetic tests (no real process/socket/threads-with-I/O).

**Explicitly out of scope** (later phases): mouse hover/click (Phase 2),
full-width auto-injection (Phase 2), the `control` watchdog (Phase 3), any user
config surface and `reduced-motion` (Phase 4). The strip height is **fixed at 6
rows** in Phase 1.

> **Note on "configurable":** the sprite and state-animation *data formats* are
> intentionally data-driven and extensible (§3–§4). That is a file format, not a
> user-facing config surface — it does not violate the Phase 4 scope line.

## 2. Locked design decisions

These were resolved in the Phase 1 brainstorm (with live half-block mockups) and
are the contract this phase implements.

| Area | Decision |
|---|---|
| Identity input | `hash(terminal_id)`; independent salts for species vs. hue |
| Species ↔ color | species = coarse silhouette, hue = fine per-agent identity |
| Coat coloring | **fully tinted** wool (the agent's hue), not cream-with-accent |
| Sprite art style | bold black outline, blocky/castellated wool, cream + peach face/hooves, facing right (reference-style) |
| Strip height | **6 rows** (12 px), half-block, 24-bit color |
| Sprite storage | embedded (`include_str!`) + optional `$HERDR_PETS_SPRITES` override dir |
| Sprite format | one file per animal, semantic-role legend, all states+frames inside |
| State behaviors | idle=sleep, working=run+roam, done=hop+`!`, blocked=shake+red `!` (**hue kept**), unknown=ghost+`?` |
| Alarm rule | blocked signals via motion + red bubble **on top of** the agent's own hue — never recolors the coat |
| Cue rule | idle/unknown use a high-contrast **bubble** (`Zz` / `?`), not bare faint text |
| Herd layout | **free-roaming** horizontal wander + soft separation; no fixed order |
| Overlap | **priority z-index** (blocked > done > working > idle > unknown) decides who draws in front |
| Overflow | **priority-ranked** `+N`: idle/unknown collapse first; attention states always shown |
| Live updates | event-driven watcher: any event → debounced refetch of `herdr agent list`; slow poll fallback |
| Concurrency | std-only background thread + `mpsc` snapshots; no async runtime |
| Bestiary | sheep (all 5 states) + **one** proof species |

## 3. Sprites — the plug-and-play data format

The design goal the user set: *"if I ever want to add a new sprite/animal, it is
very plug and play."* Sprites are therefore **data, not code**.

### 3.1 Semantic-role legend

An author paints a frame with role symbols; they never write a colour. The
engine resolves roles → colours (§4.2), so tinting and light/dark theming come
for free to *every* sprite.

| Symbol | Role | Resolved colour |
|---|---|---|
| `.` (or space) | transparent | — |
| `#` | outline | fixed near-black (theme-aware) |
| `e` | eye | fixed dark |
| `p` | skin | fixed peach (face, hooves, nose) |
| `h` | horn / bone / beak | fixed bone (for goat/deer/etc.) |
| `L` | coat — light | hashed hue, high lightness |
| `M` | coat — mid (base) | hashed hue, mid lightness |
| `S` | coat — shadow | hashed hue, low lightness |
| `a` | accent | hashed hue, saturated (collars/spots — reserved) |

Unknown symbols are a **parse error** (caught by the validation test), not a
silent skip — a typo should fail loudly.

### 3.2 File format (one file per animal)

`sprites/<name>.sprite` — a small line-oriented text format:

```text
name = Sheep

[idle]    frame_ms=520  motion=breathe        overlay=bubble:Zz  dim=false
....###..####...
...#MMM##MMMM#..
   … 13 rows total (frame 1) …

....###..####...
   … frame 2 (wool bobs) …

[working] frame_ms=140  motion=hop+wander
   … frame 1 …

   … frame 2 …

[done]    frame_ms=1500 motion=hop            overlay=badge:! color=accent
   … 1 frame …

[blocked] frame_ms=110  motion=shake          overlay=badge:! color=#e62d23
   … frame(s) …

[unknown] frame_ms=0    motion=sway           overlay=bubble:? ghost=true
   … 1 frame …
```

Rules:
- A `[state]` header opens a block; the header line carries the state's
  **animation config** (§4.3).
- Frames follow, **separated by a blank line**; each frame is a rectangular grid
  of legend symbols.
- All frames in a file share one `W×H` (so animation doesn't jitter). Different
  *species* may differ in size (alpaca tall, cow wide).
- Every species must define **all five states**: `idle`, `working`, `done`,
  `blocked`, `unknown`.

### 3.3 Loading & registry

- **Default:** each shipped `.sprite` is embedded with `include_str!` and listed
  in a one-line registry (`const SPRITES: &[&str] = &[…]`). A single
  self-contained binary; hermetic tests; no runtime file-not-found.
- **Override:** if `$HERDR_PETS_SPRITES` points at a directory, `*.sprite` files
  there are loaded at startup and **override/extend** the embedded set by `name`.
  This is the hot-reload authoring path — drop a file, relaunch the pane, no
  rebuild. Missing/broken override files degrade to the embedded set with a
  logged warning (never a crash).
- **To add an animal:** create `sprites/<name>.sprite`, add one `include_str!`
  line. The validation test (below) then guards it.

### 3.4 Validation (a guard test)

A single test iterates every registered species and asserts: all five states
present; within a species every frame is the same `W×H`; height ≤ the 6-row
budget (12 px); only legal legend symbols; each state's `motion`/`overlay`
config parses. A malformed sprite fails CI, not the strip.

## 4. Rendering engine

### 4.1 Half-block blitting

Each terminal cell renders `▀` (upper half-block): the cell's **foreground** is
the top pixel, its **background** the bottom pixel. So `H` pixel rows → `⌈H/2⌉`
terminal rows; the 12-px sprite is 6 rows. Transparent pixels leave the cell
unset (strip background shows through). Sprites are blitted at an integer `(x,y)`
into a pixel buffer, which is then emitted as half-block cells.

### 4.2 Role → colour (tint + theme)

`palette(hue, theme)` maps roles to colours:
- `L/M/S/a` → `hsl(hue, …)` at fixed lightness/saturation steps (the agent's
  coat, fully tinted).
- `#/e/p/h` → fixed colours; `#` (outline) and the strip neutrals are
  **theme-aware** (slightly different in light vs. dark terminals — detected
  best-effort; safe default otherwise).
- **State overrides** are applied by the engine, not baked into sprites:
  `ghost` desaturates all roles (unknown); `dim` lowers coat lightness/saturation
  (idle option); the blocked **red bubble/badge** is an overlay drawn *near* the
  pet — the coat palette is untouched (the alarm-keeps-identity rule).

### 4.3 Motion primitives & overlays (the state-config library)

The engine ships a named library the sprite files reference:
- **motion:** `none`, `breathe`, `hop`, `shake`, `sway`, `wander` — composable
  (`hop+wander`) and optionally parameterized (`hop:height=30`). `breathe/hop/
  shake/sway` are per-pet transforms on the sprite; `wander` feeds the herd
  simulation (§5). All are pure functions of `(phase, params)` → offset, so they
  are deterministic and testable.
- **overlay:** `none`, `bubble:<glyph>`, `badge:<glyph>` with an optional
  `color` (`accent` = hue-derived, or a literal like `#e62d23`). Bubbles/badges
  are drawn as small high-contrast marks above the pet.
- **modifiers:** `dim`, `ghost` (see §4.2).

Adding a new animation = reference a different primitive / tweak params in the
`.sprite` file; a genuinely new motion = add one primitive to the library and
reference it. A 6th state is a new `[state]` block once the status model grows.

## 5. The herd (layout, motion, crowding)

Per the user's direction, the herd is **free-roaming**, not slotted.

- **Wander:** each pet holds a pixel `x` and picks wander targets over time.
  `working` pets roam actively; `idle` barely move; `blocked` hold position
  (they shake in place, demanding attention). Motion is horizontal — the 6-row
  strip has no vertical room to roam.
- **Separation:** a gentle pairwise force nudges pets apart when closer than a
  min-gap, so they graze rather than clump. Occasional overlap is allowed.
- **Priority z-index:** on overlap, the higher-priority pet draws in front,
  order `blocked > done > working > idle > unknown`. A blocked pet is never
  hidden behind a dozing one.
- **Overflow:** when the strip can't fit the herd, selection is **priority-
  ranked** — blocked/done/working stay visible; idle/unknown collapse into a
  `+N` counter first. (Because blocked outranks everything, "never miss an
  alarm" falls out for free. `+N` refinement is a Phase 4 concern.)
- **Reconciliation:** when a new agent snapshot arrives, the herd is reconciled
  by identity — existing pets **keep their position and animation phase**, their
  state is updated, new agents spawn, departed agents are removed. Identity, not
  list order, keys a pet.

The wander + separation step is a **pure function** of
`(pets, dt, rng)` → new positions, so it is unit-testable with an injected,
seeded RNG and fixed `dt`.

## 6. Live updates — two clocks, isolated I/O

The Phase 0 render loop was `fetch → draw → poll(1.5s)`. Phase 1 splits the
concerns because animation needs a steady tick independent of data arrival.

### 6.1 Render thread (main)

A fixed **~10–12 fps** tick: advance animation + wander (§5), draw the herd,
drain any new snapshots from the channel and reconcile (§5), check for quit
(`q` / Ctrl-C). It **never blocks on I/O** — reads use `try_recv`; input uses a
zero-timeout `event::poll`. ratatui's buffer diffing keeps redraws cheap.

### 6.2 Watcher thread (background)

Owns the persistent socket and the herd's data:
1. Connect to `$HERDR_SOCKET_PATH`, send `events.subscribe` (newline-delimited
   JSON-RPC, dotted method — confirmed by Phase 0 Spike A / Phase 1 Spike 1).
2. On **any** relevant event, mark dirty; a **~250 ms debounce** coalesces bursts
   into a single refetch of `herdr agent list` (via the existing `HerdrCli`),
   which is parsed by the existing tolerant parser and pushed as a `Vec<Agent>`
   snapshot down an `mpsc` channel.
3. A **slow ~2–3 s interval** refetch runs regardless, as a safety net for
   missed events.
4. On socket error/absence: **degrade to poll-only** and reconnect with backoff;
   the render thread keeps drawing the last snapshot. Never panics the pane.

*Why "any event → refetch" rather than parsing event payloads:* it depends only
on the one already-tested parser and the documented `agent list` shape, not on
undocumented event payloads. Status changes are infrequent, so the extra
shell-out is negligible.

### 6.3 Seams (so tests never touch the real world)

- `SocketClient` trait — `connect` / `send_line` / `recv_line` over a long-lived
  connection. `RealSocketClient` (`UnixStream`, line-buffered) and a `Fake` that
  replays scripted event lines. Grows `socket.rs` from its Phase 0 one-shot form.
- `HerdrCli` — reused unchanged for the `agent list` refetch.
- A **clock seam** (trait returning "now" + a sleep) so debounce, slow-poll, and
  backoff timing are deterministic in tests.
- The render loop consumes an abstract snapshot `Receiver` — tests feed it
  scripted `Vec<Agent>` and snapshot the drawn buffer; no thread or socket
  required.

## 7. Bestiary scope

Ship **sheep** (fully polished: all five states, multi-frame) plus **one** proof
species (goat or alpaca — chosen at authoring time for a clearly different
silhouette) to demonstrate the format is genuinely plug-and-play. Every further
animal is then pure data.

## 8. Spike 1 — verify the event subscription (first task)

Phase 0 confirmed `events.subscribe` *exists* but verified neither the subscribe
request shape nor that agent **status changes** emit events (it only exercised
`tab.created`). Before building the watcher:

**Question:** What exact `events.subscribe` request does the socket accept, and
does changing an agent's status produce an event on that subscription?

**Method:** Against a live session (isolated scratch tab), open a persistent
socket connection, send candidate `events.subscribe` requests (starting from the
method-enumeration trick from Spike A), and watch for events while driving an
agent through status changes (e.g. `herdr agent wait` / real agent activity).
Record the working request, the event envelope shape, and which events fire.

**Fallback (already designed-in):** if status changes *don't* emit usable events,
the slow-poll path keeps the herd correct; the watcher simply relies on polling
and the event layer is best-effort. Record the finding in this section; update
`GOAL.md` / `docs/PLAN.md` only if it contradicts the design (it should not — the
design degrades gracefully).

**Finding:** _(to be filled in during implementation)_

## 9. Module plan

Grows the existing `src/` (Phase 0: `agent`, `herdr`, `render`, `socket`, `lib`,
`main`). Each module has one clear job and a fakeable boundary.

| Module | Responsibility | Depends on |
|---|---|---|
| `identity.rs` | `hash(terminal_id)` → `Identity { species_index, hue }`; independent salts; deterministic. | agent |
| `sprite.rs` | Parse the `.sprite` format; `Frame`, `Animation`, `StateSpec`, `Species`; the registry; embed + `$HERDR_PETS_SPRITES` override loading; the validation guard. | — |
| `palette.rs` | Role → colour: hue tint, theme-aware neutrals, `dim`/`ghost` overrides. | sprite |
| `anim.rs` | Motion-primitive library + overlay specs; pure `(phase, params)` → offset/mark. | sprite |
| `pet.rs` | One pet: identity, current state, position/velocity, animation phase, z-priority. Pure per-pet sim step. | identity, sprite, anim |
| `herd.rs` | The collection: reconcile a snapshot → pets (preserve identity/position), wander+separation step (pure, seeded RNG), overflow selection by priority. | pet, agent |
| `render.rs` | Half-block blit of the herd + bubbles/badges + `+N`; the render-thread tick loop; consumes the snapshot channel. | herd, palette, anim |
| `socket.rs` | Grows into the real persistent `SocketClient` (trait + Real/Fake), line-delimited JSON-RPC, `events.subscribe`. | serde_json |
| `watcher.rs` | Background thread: subscribe + on-event debounced refetch + slow poll + reconnect/degrade; pushes snapshots. | socket, herdr, agent, clock seam |
| `main.rs` | Parse subcommand; spawn watcher; run the render loop; restore terminal. | render, watcher, herdr |

New third-party dependencies: **none expected** (std threads + `mpsc`, existing
`serde`/`serde_json`/`ratatui`/`crossterm`, dev `insta`). If a tiny hashing
helper is wanted, prefer a `std::hash`-based approach over a new crate.

## 10. Testing (TDD)

Write the failing test first for each unit. Everything is deterministic:
injected seeded RNG, a clock seam, pure sim/animation functions, fixed
positions/frame indices for snapshots.

- **identity** — same `terminal_id` → same `(species, hue)`; different ids spread
  across species and hues; salts make species/hue independent.
- **sprite** — parse a fixture `.sprite` (states, frames, config); the validation
  guard (all states, consistent dims, legal symbols, parseable config) passes for
  shipped sprites and fails for a crafted-bad one; embedded registry loads; the
  `$HERDR_PETS_SPRITES` override replaces by name and degrades on a bad file.
- **palette / anim** — role→colour incl. `dim`/`ghost`; motion primitives return
  expected offsets at sampled phases.
- **pet / herd** — reconcile keeps identity/position across snapshots, spawns/
  removes correctly; overflow selection drops idle/unknown before attention
  states; z-order sorts by priority; separation math pushes apart under min-gap
  (pure, seeded).
- **render** — `insta` snapshots via `TestBackend`: the herd at fixed positions/
  frame for each state, the blocked red bubble, the ghost unknown, and an
  overflow (`+N`) case.
- **watcher** — fake `SocketClient` replays events → assert a debounced/coalesced
  refetch, the slow-poll fallback fires without events, and a socket error
  degrades to poll-only without panic (fake clock drives timing).
- **cli** — `render` still dispatches; `--version` unchanged.

No test spawns a real process, opens the real socket, or does real-I/O on a
thread.

## 11. Implementation tracks (subagent-driven)

Sequenced by dependency; independent leaves dispatched in parallel where noted.
The plan doc (`docs/superpowers/plans/`) breaks these into TDD tasks.

1. **Spike 1** (§8) — verify event subscription; write the finding. *(first)*
2. **identity** + **sprite/palette/anim** (data + pure engine) — largely
   parallel; no I/O.
3. **pet** + **herd** (sim + reconcile + overflow) — depends on 2.
4. **render** (blit + tick loop + snapshots) — depends on 3.
5. **socket** (real client) + **watcher** (thread) — depends on Spike 1 + 2's
   agent types.
6. **main** wiring + manual dev-loop check against a live herd.
7. Author the **sheep** (all states) and the **proof species**; run the
   validation guard.
8. Update the Phase tracker in `docs/PLAN.md`.

## 12. Guardrails (from the handoff)

- Work on a **branch off `main`**, never on `main` directly. *(Base note: PR #1
  — Phase 0 — was still open at Phase 1 start, so this branch is based on
  `feat/phase-0-foundations`; rebase onto `main` once Phase 0 merges.)*
- **Do not commit or push without the user asking**; local checkpoint commits may
  be proposed.
- Keep Phase 1 within scope — resist creep into Phases 2–4 (mouse, injection,
  controller, config).
- If Spike 1 contradicts the design, update `GOAL.md` + `docs/PLAN.md` first and
  flag it to the user.
