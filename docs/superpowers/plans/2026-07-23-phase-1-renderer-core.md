# Phase 1 — The pets (renderer core): Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Phase 0 placeholder with a real, animated, deterministically-colored pixel-pet renderer: one manually-opened strip shows the live herd, each agent a half-block sprite whose behavior tracks its status, updated near-real-time.

**Architecture:** Sprites are data (role-painted text, embedded + overridable). A pure engine turns `hash(terminal_id)` into `(species, hue)`, tints role-based sprites, and animates them via named motion primitives. A free-roaming herd simulation (wander + separation, priority z-index/overflow) is drawn as half-blocks by a ~10–12 fps render thread. A std-only background watcher thread subscribes to the herdr socket and pushes debounced `agent list` snapshots over an `mpsc` channel; it degrades to polling if the socket is unavailable.

**Tech Stack:** Rust (edition 2024, rust-version 1.96), `ratatui` 0.30, `crossterm` 0.29, `serde`/`serde_json`, std threads + `mpsc`; dev `insta`. **No new third-party dependencies.**

## Global Constraints

- Rust **edition = "2024"**, **rust-version = "1.96"**. Follow the repo Rust skills (error-handling: `Result`/`?`, `io::Error::other`, no `unwrap`/`expect` outside tests; testability-seams: trait + Real/Fake; serde tolerant parsing; tui snapshot testing; project conventions: sentence-style test names, doc comments).
- **No new crates.** Hashing uses `std::hash` (`DefaultHasher`); randomness uses a small in-repo LCG. If a task seems to need a crate, stop and flag it.
- **Identity input is `terminal_id`** (never `pane_id` — it churns on `layout.apply` per Phase 0 Spike A). Independent salts for species vs. hue.
- **Strip height is fixed at 6 rows (12 px).** No config surface this phase.
- **Blocked never recolors the coat** — alarm is motion + a red overlay on top of the agent's own hue.
- **Sprites embedded via `include_str!`** + optional `$HERDR_PETS_SPRITES` override dir. Every species defines all five states (`idle`, `working`, `done`, `blocked`, `unknown`).
- **Tests are hermetic:** no real process spawn, no real socket, no real-I/O on threads. Use the `HerdrCli`/`CommandRunner` fakes, a `SocketClient` fake, a clock seam, and a seeded RNG.
- **Git:** branch `feat/phase-1-renderer-core` (already created off `feat/phase-0-foundations`; rebase onto `main` once Phase 0 merges). Conventional Commits. **Do not push.** Local commit at each task's final step. End commit messages with the `Claude-Session:` trailer.
- **Scope discipline:** no mouse, no full-width injection, no controller, no user config, no reduced-motion (Phases 2–4).

Module dependency order (a module only depends on earlier ones): `identity` → `anim` → `sprite` → `palette` → `pet` → `herd` → `render`; `socket` → `watcher`; `main` wires all.

---

### Task 1: Spike 1 — verify the event subscription

**Files:**
- Modify: `docs/superpowers/specs/2026-07-23-phase-1-renderer-core-design.md` (fill §8 "Finding").

**This is an experiment, not TDD.** Run against a live herdr session in an **isolated scratch tab**. It de-risks the one real unknown before the watcher is built; the design already degrades to polling if events misbehave.

- [ ] **Step 1: Open a persistent socket and discover the subscribe shape**

```bash
echo "$HERDR_SOCKET_PATH"
# Keep a connection open and watch it. Use a line-oriented tool:
#   socat - UNIX-CONNECT:"$HERDR_SOCKET_PATH"     (preferred, interactive)
#   or: nc -U "$HERDR_SOCKET_PATH"
# Send (one line each), observing replies:
#   {"id":"s1","method":"events.subscribe","params":{}}
#   {"id":"s1","method":"events.subscribe","params":{"events":["*"]}}
# Use the Spike-A method-enumeration trick if the shape is rejected:
#   {"id":"x","method":"bogus.method"}   -> error message lists valid methods
```
Record the exact accepted `events.subscribe` request and the reply/ack envelope.

- [ ] **Step 2: Drive agent status changes and watch for events**

With the subscription open, in another pane change an agent's status (start/stop work, or `herdr agent wait --status …` against a real agent) and observe whether events arrive on the socket. Note the **event envelope shape** and which event names fire on status change vs. agent add/remove.

- [ ] **Step 3: Confirm the polling fallback**

```bash
herdr agent list      # snapshot; change an agent's status; run again — diff reflects it
```
Confirm `agent list` reflects status changes within ~1–2 s (the safety-net path).

- [ ] **Step 4: Tear down the scratch tab and write the finding**

Replace §8 "Finding" with: the working `events.subscribe` request, the event envelope shape, which events fire on status change, and whether the watcher can be event-driven or must lean on polling. If it contradicts the design, update `GOAL.md`/`docs/PLAN.md` and flag to the user (it should not — design degrades gracefully).

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/specs/2026-07-23-phase-1-renderer-core-design.md GOAL.md docs/PLAN.md
git commit -m "docs(phase-1): record Spike 1 (event subscription) findings"
```

---

### Task 2: Deterministic identity

**Files:**
- Create: `src/identity.rs`
- Modify: `src/lib.rs` (add `pub mod identity;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `herdr_pets::identity::Identity { pub species_index: usize, pub hue: u16 }` (`Copy`, `PartialEq`, `Eq`, `Debug`).
  - `herdr_pets::identity::identity_for(terminal_id: &str, species_count: usize) -> Identity`.

- [ ] **Step 1: Write the failing test** — append to `src/identity.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_terminal_id_yields_the_same_identity() {
        let a = identity_for("term_aaa", 3);
        let b = identity_for("term_aaa", 3);
        assert_eq!(a, b);
    }

    #[test]
    fn species_index_is_within_range() {
        for tid in ["term_a", "term_b", "term_c", "term_d", "term_e"] {
            assert!(identity_for(tid, 3).species_index < 3);
        }
    }

    #[test]
    fn hue_is_within_the_color_wheel() {
        for tid in ["term_a", "term_b", "term_c"] {
            assert!(identity_for(tid, 3).hue < 360);
        }
    }

    #[test]
    fn species_and_hue_are_independent() {
        // Two ids sharing a species should still usually differ in hue.
        let ids: Vec<_> = (0..40).map(|i| format!("term_{i}")).collect();
        let same_species: Vec<u16> = ids.iter()
            .map(|t| identity_for(t, 2))
            .filter(|i| i.species_index == 0)
            .map(|i| i.hue)
            .collect();
        let distinct: std::collections::HashSet<_> = same_species.iter().collect();
        assert!(distinct.len() > 1, "hue must vary within a single species");
    }

    #[test]
    fn zero_species_count_is_handled() {
        // Degenerate input must not panic (divide-by-zero guard).
        let id = identity_for("term_a", 0);
        assert_eq!(id.species_index, 0);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib identity`
Expected: FAIL — `Identity`/`identity_for` not found.

- [ ] **Step 3: Write minimal implementation** — top of `src/identity.rs`

```rust
//! Deterministic pet identity: hash(terminal_id) -> (species, hue).
//!
//! Uses `terminal_id` (stable per terminal, survives the `pane_id` churn that
//! `layout.apply` causes — see Phase 0 Spike A). Independent salts keep species
//! and hue uncorrelated. `DefaultHasher` has fixed keys, so results are stable
//! across restarts of the same binary.

use std::hash::{Hash, Hasher};

/// A pet's stable visual identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub species_index: usize,
    pub hue: u16,
}

fn hash_salted(salt: &str, value: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    salt.hash(&mut h);
    value.hash(&mut h);
    h.finish()
}

/// Map a terminal id to a stable `(species_index, hue)`.
pub fn identity_for(terminal_id: &str, species_count: usize) -> Identity {
    let species_index = if species_count == 0 {
        0
    } else {
        (hash_salted("species", terminal_id) % species_count as u64) as usize
    };
    let hue = (hash_salted("hue", terminal_id) % 360) as u16;
    Identity { species_index, hue }
}
```

Add to `src/lib.rs`: `pub mod identity;`

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib identity`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add src/identity.rs src/lib.rs
git commit -m "feat(identity): deterministic hash(terminal_id) -> species + hue"
```

---

### Task 3: Animation primitives & overlay specs

**Files:**
- Create: `src/anim.rs`
- Modify: `src/lib.rs` (add `pub mod anim;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `Rgb(pub u8, pub u8, pub u8)` (`Copy`, `PartialEq`, `Debug`).
  - `Motion` enum `{ None, Breathe, Hop, Shake, Sway, Wander }`; `MotionSpec { pub motions: Vec<Motion> }`.
  - `parse_motion(s: &str) -> Result<MotionSpec, String>` (`"hop+wander"` → both; unknown → `Err`).
  - `Offset { pub dx: f32, pub dy: f32 }`; `motion_offset(spec: &MotionSpec, phase: f32) -> Offset` (`phase` in `0.0..1.0`; pixel offsets; `Wander` contributes zero here — the herd owns horizontal roam).
  - `has_wander(spec: &MotionSpec) -> bool`.
  - `Overlay { None, Bubble(String), Badge(String) }`; `OverlayColor { Default, Accent, Literal(Rgb) }`; `OverlaySpec { pub kind: Overlay, pub color: OverlayColor }`; `parse_overlay(s: &str) -> Result<OverlaySpec, String>`.

- [ ] **Step 1: Write the failing test** — append to `src/anim.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_motion_reads_single_and_composed() {
        assert_eq!(parse_motion("hop").unwrap().motions, vec![Motion::Hop]);
        let m = parse_motion("hop+wander").unwrap().motions;
        assert!(m.contains(&Motion::Hop) && m.contains(&Motion::Wander));
        assert_eq!(parse_motion("none").unwrap().motions, vec![Motion::None]);
    }

    #[test]
    fn parse_motion_rejects_unknown() {
        assert!(parse_motion("moonwalk").is_err());
    }

    #[test]
    fn breathe_offset_is_vertical_and_bounded() {
        let spec = parse_motion("breathe").unwrap();
        for p in [0.0, 0.25, 0.5, 0.75] {
            let o = motion_offset(&spec, p);
            assert_eq!(o.dx, 0.0);
            assert!(o.dy.abs() <= 1.5, "breathe stays gentle");
        }
    }

    #[test]
    fn hop_offset_never_goes_below_ground() {
        let spec = parse_motion("hop").unwrap();
        for p in [0.0, 0.5, 1.0] {
            assert!(motion_offset(&spec, p).dy <= 0.0, "hop only lifts (negative = up)");
        }
    }

    #[test]
    fn wander_is_detected_and_adds_no_local_offset() {
        let spec = parse_motion("wander").unwrap();
        assert!(has_wander(&spec));
        let o = motion_offset(&spec, 0.5);
        assert_eq!((o.dx, o.dy), (0.0, 0.0));
    }

    #[test]
    fn parse_overlay_reads_kind_and_color() {
        let b = parse_overlay("bubble:Zz").unwrap();
        assert_eq!(b.kind, Overlay::Bubble("Zz".into()));
        assert_eq!(b.color, OverlayColor::Default);

        let badge = parse_overlay("badge:!").unwrap();
        assert_eq!(badge.kind, Overlay::Badge("!".into()));

        assert_eq!(parse_overlay("badge:! color=accent").unwrap().color, OverlayColor::Accent);
        assert_eq!(
            parse_overlay("badge:! color=#e62d23").unwrap().color,
            OverlayColor::Literal(Rgb(0xe6, 0x2d, 0x23))
        );
        assert_eq!(parse_overlay("none").unwrap().kind, Overlay::None);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib anim`
Expected: FAIL — items not found.

- [ ] **Step 3: Write minimal implementation** — top of `src/anim.rs`

```rust
//! Named motion primitives and overlay specs — the "animation config" library
//! that `.sprite` files reference. All motion is a pure function of a phase in
//! 0.0..1.0, so it is deterministic and testable. Horizontal roaming (`Wander`)
//! is owned by the herd simulation, not by per-pet offsets.

use std::f32::consts::TAU;

/// An 8-bit-per-channel color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion { None, Breathe, Hop, Shake, Sway, Wander }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MotionSpec { pub motions: Vec<Motion> }

/// Parse `"hop"`, `"hop+wander"`, `"none"`. Unknown token -> Err.
pub fn parse_motion(s: &str) -> Result<MotionSpec, String> {
    let mut motions = Vec::new();
    for tok in s.split('+').map(str::trim).filter(|t| !t.is_empty()) {
        let m = match tok {
            "none" => Motion::None,
            "breathe" => Motion::Breathe,
            "hop" => Motion::Hop,
            "shake" => Motion::Shake,
            "sway" => Motion::Sway,
            "wander" => Motion::Wander,
            other => return Err(format!("unknown motion '{other}'")),
        };
        motions.push(m);
    }
    if motions.is_empty() {
        return Err("empty motion".into());
    }
    Ok(MotionSpec { motions })
}

pub fn has_wander(spec: &MotionSpec) -> bool {
    spec.motions.contains(&Motion::Wander)
}

/// A pixel offset applied to a sprite before blitting. Negative `dy` = up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Offset { pub dx: f32, pub dy: f32 }

/// Sum the local (non-wander) motions at `phase` (0.0..1.0).
pub fn motion_offset(spec: &MotionSpec, phase: f32) -> Offset {
    let t = phase * TAU;
    let mut o = Offset { dx: 0.0, dy: 0.0 };
    for m in &spec.motions {
        match m {
            Motion::None | Motion::Wander => {}
            Motion::Breathe => o.dy += -0.5 * (1.0 - t.cos()) / 2.0, // gentle rise/settle, <=0.5
            Motion::Hop => o.dy += -2.0 * (t.sin()).max(0.0),        // lifts on the upbeat
            Motion::Shake => { o.dx += 1.2 * (t * 2.0).sin(); o.dy += -1.5 * (t * 2.0).sin().abs(); }
            Motion::Sway => o.dx += 1.0 * t.sin(),
        }
    }
    o
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay { None, Bubble(String), Badge(String) }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayColor { Default, Accent, Literal(Rgb) }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlaySpec { pub kind: Overlay, pub color: OverlayColor }

/// Parse `"bubble:Zz"`, `"badge:! color=accent"`, `"badge:! color=#e62d23"`, `"none"`.
pub fn parse_overlay(s: &str) -> Result<OverlaySpec, String> {
    let mut kind = Overlay::None;
    let mut color = OverlayColor::Default;
    for (i, part) in s.split_whitespace().enumerate() {
        if i == 0 {
            kind = match part.split_once(':') {
                Some(("bubble", g)) => Overlay::Bubble(g.to_string()),
                Some(("badge", g)) => Overlay::Badge(g.to_string()),
                None if part == "none" => Overlay::None,
                _ => return Err(format!("bad overlay '{part}'")),
            };
        } else if let Some(c) = part.strip_prefix("color=") {
            color = parse_color(c)?;
        }
    }
    Ok(OverlaySpec { kind, color })
}

fn parse_color(c: &str) -> Result<OverlayColor, String> {
    if c == "accent" {
        return Ok(OverlayColor::Accent);
    }
    let hex = c.strip_prefix('#').ok_or_else(|| format!("bad color '{c}'"))?;
    if hex.len() != 6 {
        return Err(format!("bad color '{c}'"));
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| format!("bad color '{c}'"));
    Ok(OverlayColor::Literal(Rgb(byte(0)?, byte(2)?, byte(4)?)))
}
```

Add to `src/lib.rs`: `pub mod anim;`

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib anim`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add src/anim.rs src/lib.rs
git commit -m "feat(anim): motion primitives and overlay specs"
```

---

### Task 4: Sprite format — roles, parsing, registry, validation

**Files:**
- Create: `src/sprite.rs`
- Create: `sprites/test-blob.sprite` (a tiny fixture species used only by tests)
- Modify: `src/lib.rs` (add `pub mod sprite;`)

**Interfaces:**
- Consumes: `anim::{MotionSpec, OverlaySpec, parse_motion, parse_overlay}`, `agent::AgentStatus`.
- Produces:
  - `Role` enum `{ Transparent, Outline, Eye, Skin, Horn, CoatLight, CoatMid, CoatShadow, Accent }` (`Copy`).
  - `role_from_char(c: char) -> Option<Role>`.
  - `Frame { pub w: usize, pub h: usize, pub cells: Vec<Role> }` (row-major, `cells.len() == w*h`).
  - `StateSpec { pub frames: Vec<Frame>, pub frame_ms: u32, pub motion: MotionSpec, pub overlay: OverlaySpec, pub dim: bool, pub ghost: bool }`.
  - `Species { pub name: String, pub states: std::collections::BTreeMap<AgentStatus, StateSpec> }`; `Species::size(&self) -> (usize, usize)` (w,h of any frame).
  - `parse_species(src: &str) -> Result<Species, String>`.
  - `embedded_species() -> Vec<Species>` (parses each embedded `.sprite`; the validation test guards correctness).
  - `load_species() -> Vec<Species>` (embedded, then merged/overridden by `$HERDR_PETS_SPRITES/*.sprite` by name; a bad override file is skipped with an `eprintln!` warning, never fatal).

> `AgentStatus` needs `Ord` + `Copy` for the `BTreeMap` key. It already derives `Copy`; add `PartialOrd, Ord` to its derive list in `src/agent.rs` in Step 3 (a one-line, behavior-preserving change).

- [ ] **Step 1: Write the fixture** — `sprites/test-blob.sprite`

```text
name = TestBlob

[idle]    frame_ms=500 motion=breathe overlay=bubble:Zz
.MM.
MMMM
M##M
.MM.

[working] frame_ms=140 motion=hop+wander overlay=none
.MM.
MMMM
MMMM
.MM.

[done]    frame_ms=900 motion=hop overlay=badge:! color=accent
.MM.
MMMM
MMMM
.MM.

[blocked] frame_ms=110 motion=shake overlay=badge:! color=#e62d23
.MM.
M##M
MMMM
.MM.

[unknown] frame_ms=0 motion=sway overlay=bubble:? ghost=true
.MM.
MMMM
MMMM
.MM.
```

- [ ] **Step 2: Write the failing test** — append to `src/sprite.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentStatus;

    const BLOB: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/sprites/test-blob.sprite"));

    #[test]
    fn parses_name_states_and_frame_grid() {
        let sp = parse_species(BLOB).expect("valid fixture");
        assert_eq!(sp.name, "TestBlob");
        assert_eq!(sp.states.len(), 5);
        let idle = &sp.states[&AgentStatus::Idle];
        assert_eq!(idle.frames.len(), 2);
        assert_eq!((idle.frames[0].w, idle.frames[0].h), (4, 4));
        assert_eq!(idle.frame_ms, 500);
        assert!(idle.frames[0].cells.iter().any(|c| *c == Role::CoatMid));
        assert!(idle.frames[0].cells.iter().any(|c| *c == Role::Outline));
    }

    #[test]
    fn unknown_state_carries_ghost_and_overlay_config() {
        let sp = parse_species(BLOB).unwrap();
        let u = &sp.states[&AgentStatus::Unknown];
        assert!(u.ghost);
    }

    #[test]
    fn illegal_symbol_is_a_parse_error() {
        let bad = "name = X\n[idle] frame_ms=1 motion=none overlay=none\n.Z.\n";
        assert!(parse_species(bad).is_err());
    }

    #[test]
    fn ragged_frame_is_a_parse_error() {
        let bad = "name=X\n[idle] frame_ms=1 motion=none overlay=none\nMM\nMMM\n";
        assert!(parse_species(bad).is_err());
    }

    #[test]
    fn role_from_char_covers_the_legend() {
        assert_eq!(role_from_char('#'), Some(Role::Outline));
        assert_eq!(role_from_char('M'), Some(Role::CoatMid));
        assert_eq!(role_from_char('.'), Some(Role::Transparent));
        assert_eq!(role_from_char(' '), Some(Role::Transparent));
        assert_eq!(role_from_char('Z'), None);
    }

    // The guard: every shipped species is well-formed. Fails CI on a bad sprite.
    #[test]
    fn every_embedded_species_is_valid() {
        let all = embedded_species();
        assert!(!all.is_empty());
        for sp in &all {
            for st in [AgentStatus::Idle, AgentStatus::Working, AgentStatus::Done,
                       AgentStatus::Blocked, AgentStatus::Unknown] {
                let spec = sp.states.get(&st)
                    .unwrap_or_else(|| panic!("{} missing state {st:?}", sp.name));
                assert!(!spec.frames.is_empty(), "{} {st:?} has no frames", sp.name);
                let (w, h) = (spec.frames[0].w, spec.frames[0].h);
                assert!(h <= 12, "{} taller than the 6-row budget", sp.name);
                for f in &spec.frames {
                    assert_eq!((f.w, f.h), (w, h), "{} {st:?} frame size drift", sp.name);
                }
            }
        }
    }
}
```

- [ ] **Step 3: Write minimal implementation** — top of `src/sprite.rs`

```rust
//! Sprite data format: role-painted text, one file per animal, all five states
//! and their frames inside. Roles (not colors) so tinting + theming are free.
//! Loaded embedded by default; `$HERDR_PETS_SPRITES` overrides by name.

use std::collections::BTreeMap;

use crate::agent::AgentStatus;
use crate::anim::{MotionSpec, OverlaySpec, parse_motion, parse_overlay};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role { Transparent, Outline, Eye, Skin, Horn, CoatLight, CoatMid, CoatShadow, Accent }

pub fn role_from_char(c: char) -> Option<Role> {
    Some(match c {
        '.' | ' ' => Role::Transparent,
        '#' => Role::Outline,
        'e' => Role::Eye,
        'p' => Role::Skin,
        'h' => Role::Horn,
        'L' => Role::CoatLight,
        'M' => Role::CoatMid,
        'S' => Role::CoatShadow,
        'a' => Role::Accent,
        _ => return None,
    })
}

#[derive(Debug, Clone)]
pub struct Frame { pub w: usize, pub h: usize, pub cells: Vec<Role> }

#[derive(Debug, Clone)]
pub struct StateSpec {
    pub frames: Vec<Frame>,
    pub frame_ms: u32,
    pub motion: MotionSpec,
    pub overlay: OverlaySpec,
    pub dim: bool,
    pub ghost: bool,
}

#[derive(Debug, Clone)]
pub struct Species {
    pub name: String,
    pub states: BTreeMap<AgentStatus, StateSpec>,
}

impl Species {
    pub fn size(&self) -> (usize, usize) {
        self.states.values().next().and_then(|s| s.frames.first())
            .map(|f| (f.w, f.h)).unwrap_or((0, 0))
    }
}

fn status_from_key(k: &str) -> Option<AgentStatus> {
    Some(match k {
        "idle" => AgentStatus::Idle,
        "working" => AgentStatus::Working,
        "done" => AgentStatus::Done,
        "blocked" => AgentStatus::Blocked,
        "unknown" => AgentStatus::Unknown,
        _ => return None,
    })
}

fn kv<'a>(header: &'a str, key: &str) -> Option<&'a str> {
    header.split_whitespace().find_map(|tok| tok.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

fn parse_frame(lines: &[&str]) -> Result<Frame, String> {
    let w = lines[0].chars().count();
    let mut cells = Vec::with_capacity(w * lines.len());
    for line in lines {
        if line.chars().count() != w {
            return Err("ragged frame (rows differ in width)".into());
        }
        for c in line.chars() {
            cells.push(role_from_char(c).ok_or_else(|| format!("illegal symbol '{c}'"))?);
        }
    }
    Ok(Frame { w, h: lines.len(), cells })
}

/// Parse one `.sprite` file into a `Species`.
pub fn parse_species(src: &str) -> Result<Species, String> {
    let mut name = String::new();
    let mut states = BTreeMap::new();
    let mut cur_status: Option<AgentStatus> = None;
    let mut cur_header = String::new();
    let mut frames: Vec<Frame> = Vec::new();
    let mut buf: Vec<&str> = Vec::new();

    // Helper to flush the accumulated frame block into the current state.
    fn flush(buf: &mut Vec<&str>, frames: &mut Vec<Frame>) -> Result<(), String> {
        if !buf.is_empty() {
            frames.push(parse_frame(buf)?);
            buf.clear();
        }
        Ok(())
    }
    fn commit(states: &mut BTreeMap<AgentStatus, StateSpec>, status: Option<AgentStatus>,
              header: &str, frames: &mut Vec<Frame>) -> Result<(), String> {
        if let Some(st) = status {
            if frames.is_empty() { return Err(format!("state {st:?} has no frames")); }
            let frame_ms = kv(header, "frame_ms").and_then(|v| v.parse().ok()).unwrap_or(0);
            let motion = parse_motion(kv(header, "motion").unwrap_or("none"))?;
            let overlay = parse_overlay(kv(header, "overlay").unwrap_or("none"))?;
            let dim = kv(header, "dim") == Some("true");
            let ghost = kv(header, "ghost") == Some("true");
            states.insert(st, StateSpec { frames: std::mem::take(frames), frame_ms, motion, overlay, dim, ghost });
        }
        Ok(())
    }

    for raw in src.lines() {
        let line = raw.trim_end();
        if let Some(rest) = line.strip_prefix("name") {
            if let Some(v) = rest.trim_start().strip_prefix('=') { name = v.trim().to_string(); }
            continue;
        }
        if line.trim_start().starts_with('[') {
            flush(&mut buf, &mut frames)?;
            commit(&mut states, cur_status, &cur_header, &mut frames)?;
            let close = line.find(']').ok_or("missing ] in state header")?;
            let key = &line[line.find('[').unwrap() + 1..close];
            cur_status = Some(status_from_key(key).ok_or_else(|| format!("unknown state '{key}'"))?);
            cur_header = line[close + 1..].trim().to_string();
            continue;
        }
        if line.trim().is_empty() {
            flush(&mut buf, &mut frames)?;
            continue;
        }
        if cur_status.is_some() {
            buf.push(line);
        }
    }
    flush(&mut buf, &mut frames)?;
    commit(&mut states, cur_status, &cur_header, &mut frames)?;

    if name.is_empty() { return Err("missing name".into()); }
    Ok(Species { name, states })
}

/// Embedded sprite sources. Add one line per new animal.
const EMBEDDED: &[&str] = &[
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/sprites/sheep.sprite")),
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/sprites/goat.sprite")),
];

/// Parse the embedded sprites. Guarded by `every_embedded_species_is_valid`.
pub fn embedded_species() -> Vec<Species> {
    EMBEDDED.iter().filter_map(|src| parse_species(src).ok()).collect()
}

/// Embedded species, with any `$HERDR_PETS_SPRITES/*.sprite` overriding by name.
pub fn load_species() -> Vec<Species> {
    let mut out = embedded_species();
    if let Some(dir) = std::env::var_os("HERDR_PETS_SPRITES") {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let path = e.path();
                if path.extension().and_then(|x| x.to_str()) != Some("sprite") { continue; }
                match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|s| parse_species(&s)) {
                    Ok(sp) => {
                        out.retain(|x| x.name != sp.name);
                        out.push(sp);
                    }
                    Err(err) => eprintln!("herdr-pets: skipping sprite {path:?}: {err}"),
                }
            }
        }
    }
    out
}
```

> **Note:** `embedded_species()` references `sprites/sheep.sprite` and `sprites/goat.sprite` via `include_str!`. Those files are authored in **Task 12**. To keep this task compiling and testable in isolation, temporarily point `EMBEDDED` at only `sprites/test-blob.sprite`; Task 12 swaps in the real sheep + goat and re-points `EMBEDDED`. (The `every_embedded_species_is_valid` guard passes against `test-blob` now and against the real art later.)

Also in `src/agent.rs` Step 3: add `PartialOrd, Ord` to the `AgentStatus` derive (`#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]`).

Add to `src/lib.rs`: `pub mod sprite;`

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib sprite` (with `EMBEDDED` pointed at `test-blob` only, per the note)
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add src/sprite.rs src/agent.rs src/lib.rs sprites/test-blob.sprite
git commit -m "feat(sprite): role-based .sprite parser, registry, and validation guard"
```

---

### Task 5: Palette — role → color, tint, theme, overrides

**Files:**
- Create: `src/palette.rs`
- Modify: `src/lib.rs` (add `pub mod palette;`)

**Interfaces:**
- Consumes: `sprite::Role`, `anim::Rgb`.
- Produces:
  - `Theme { Dark, Light }` (`Copy`).
  - `StateStyle { pub dim: bool, pub ghost: bool }` (`Copy`; `StateStyle::none()`).
  - `role_color(role: Role, hue: u16, theme: Theme, style: StateStyle) -> Option<Rgb>` (`None` = transparent).
  - `hsl_to_rgb(h: f32, s: f32, l: f32) -> Rgb` (h in degrees, s/l in 0.0..1.0).

- [ ] **Step 1: Write the failing test** — append to `src/palette.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sprite::Role;

    #[test]
    fn transparent_role_has_no_color() {
        assert_eq!(role_color(Role::Transparent, 120, Theme::Dark, StateStyle::none()), None);
    }

    #[test]
    fn coat_roles_track_the_hue() {
        // Different hues produce different coat colors.
        let a = role_color(Role::CoatMid, 20, Theme::Dark, StateStyle::none());
        let b = role_color(Role::CoatMid, 200, Theme::Dark, StateStyle::none());
        assert!(a.is_some() && a != b);
    }

    #[test]
    fn coat_light_is_lighter_than_shadow() {
        let l = role_color(Role::CoatLight, 200, Theme::Dark, StateStyle::none()).unwrap();
        let s = role_color(Role::CoatShadow, 200, Theme::Dark, StateStyle::none()).unwrap();
        let sum = |c: Rgb| c.0 as u32 + c.1 as u32 + c.2 as u32;
        assert!(sum(l) > sum(s));
    }

    #[test]
    fn skin_and_eye_ignore_hue() {
        assert_eq!(
            role_color(Role::Skin, 10, Theme::Dark, StateStyle::none()),
            role_color(Role::Skin, 300, Theme::Dark, StateStyle::none())
        );
    }

    #[test]
    fn ghost_desaturates_the_coat_toward_grey() {
        let normal = role_color(Role::CoatMid, 200, Theme::Dark, StateStyle::none()).unwrap();
        let ghost = role_color(Role::CoatMid, 200, Theme::Dark, StateStyle { dim: false, ghost: true }).unwrap();
        let spread = |c: Rgb| (c.0.max(c.1).max(c.2) as i32 - c.0.min(c.1).min(c.2) as i32);
        assert!(spread(ghost) < spread(normal), "ghost should be greyer");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib palette`
Expected: FAIL — items not found.

- [ ] **Step 3: Write minimal implementation** — top of `src/palette.rs`

```rust
//! Role -> color: coat roles tint to the agent's hue; skin/eye/outline are
//! fixed (outline + neutrals are theme-aware). `dim` and `ghost` are engine
//! state overrides applied here so no sprite bakes them in.

use crate::anim::Rgb;
use crate::sprite::Role;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme { Dark, Light }

#[derive(Debug, Clone, Copy)]
pub struct StateStyle { pub dim: bool, pub ghost: bool }
impl StateStyle { pub fn none() -> Self { Self { dim: false, ghost: false } } }

/// HSL (degrees, 0..1, 0..1) -> RGB.
pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Rgb {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0), 1 => (x, c, 0.0), 2 => (0.0, c, x),
        3 => (0.0, x, c), 4 => (x, 0.0, c), _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let to = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Rgb(to(r1), to(g1), to(b1))
}

pub fn role_color(role: Role, hue: u16, theme: Theme, style: StateStyle) -> Option<Rgb> {
    let h = hue as f32;
    // coat saturation/lightness steps; ghost flattens sat, dim lowers both.
    let (mut sat, light) = match role {
        Role::CoatLight => (0.52, 0.86),
        Role::CoatMid => (0.48, 0.66),
        Role::CoatShadow => (0.46, 0.52),
        Role::Accent => (0.72, 0.55),
        Role::Outline => return Some(match theme { Theme::Dark => Rgb(18, 18, 18), Theme::Light => Rgb(28, 28, 28) }),
        Role::Eye => return Some(Rgb(20, 20, 20)),
        Role::Skin => return Some(Rgb(0xe7, 0xad, 0x86)),
        Role::Horn => return Some(Rgb(0xdc, 0xcb, 0xa6)),
        Role::Transparent => return None,
    };
    let mut light = light;
    if style.ghost { sat = 0.06; }
    if style.dim { sat *= 0.5; light = (light * 0.82).min(0.7); }
    Some(hsl_to_rgb(h, sat, light))
}
```

Add to `src/lib.rs`: `pub mod palette;`

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib palette`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add src/palette.rs src/lib.rs
git commit -m "feat(palette): role->color tint with theme and dim/ghost overrides"
```

---

### Task 6: Pet — state, priority, animation phase

**Files:**
- Create: `src/pet.rs`
- Modify: `src/lib.rs` (add `pub mod pet;`)

**Interfaces:**
- Consumes: `identity::Identity`, `agent::AgentStatus`.
- Produces:
  - `priority(status: AgentStatus) -> u8` (blocked=5, done=4, working=3, idle=2, unknown=1).
  - `Pet { pub terminal_id: String, pub identity: Identity, pub status: AgentStatus, pub x: f32, pub target_x: f32, pub phase: f32 }`.
  - `Pet::new(terminal_id, identity, status, x) -> Pet`.
  - `Pet::z_priority(&self) -> u8`.
  - `Pet::frame_index(&self, frame_count: usize) -> usize` (from `phase`).
  - `Pet::advance(&mut self, dt_ms: f32, frame_ms: u32)` (advances `phase`, wrapping at 1.0; `frame_ms == 0` means a single static frame).

- [ ] **Step 1: Write the failing test** — append to `src/pet.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentStatus;
    use crate::identity::identity_for;

    fn pet(status: AgentStatus) -> Pet {
        Pet::new("term_x".into(), identity_for("term_x", 3), status, 0.0)
    }

    #[test]
    fn priority_orders_blocked_above_all_and_unknown_below() {
        assert!(priority(AgentStatus::Blocked) > priority(AgentStatus::Done));
        assert!(priority(AgentStatus::Done) > priority(AgentStatus::Working));
        assert!(priority(AgentStatus::Working) > priority(AgentStatus::Idle));
        assert!(priority(AgentStatus::Idle) > priority(AgentStatus::Unknown));
    }

    #[test]
    fn frame_index_cycles_with_phase() {
        let mut p = pet(AgentStatus::Working);
        p.phase = 0.0;
        assert_eq!(p.frame_index(2), 0);
        p.phase = 0.75;
        assert_eq!(p.frame_index(2), 1);
    }

    #[test]
    fn advance_wraps_phase() {
        let mut p = pet(AgentStatus::Idle);
        p.phase = 0.9;
        p.advance(600.0, 500); // 600ms over a 500ms frame cycle basis
        assert!((0.0..1.0).contains(&p.phase));
    }

    #[test]
    fn static_state_keeps_a_single_frame() {
        let mut p = pet(AgentStatus::Unknown);
        p.advance(1000.0, 0);
        assert_eq!(p.frame_index(1), 0);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib pet`
Expected: FAIL — items not found.

- [ ] **Step 3: Write minimal implementation** — top of `src/pet.rs`

```rust
//! One pet: identity, live status, horizontal position, and an animation phase.
//! Priority drives both draw order (z-index) and overflow selection.

use crate::agent::AgentStatus;
use crate::identity::Identity;

pub fn priority(status: AgentStatus) -> u8 {
    match status {
        AgentStatus::Blocked => 5,
        AgentStatus::Done => 4,
        AgentStatus::Working => 3,
        AgentStatus::Idle => 2,
        AgentStatus::Unknown => 1,
    }
}

#[derive(Debug, Clone)]
pub struct Pet {
    pub terminal_id: String,
    pub identity: Identity,
    pub status: AgentStatus,
    pub x: f32,
    pub target_x: f32,
    pub phase: f32,
}

impl Pet {
    pub fn new(terminal_id: String, identity: Identity, status: AgentStatus, x: f32) -> Self {
        Self { terminal_id, identity, status, x, target_x: x, phase: 0.0 }
    }

    pub fn z_priority(&self) -> u8 { priority(self.status) }

    pub fn frame_index(&self, frame_count: usize) -> usize {
        if frame_count <= 1 { 0 } else { ((self.phase * frame_count as f32) as usize).min(frame_count - 1) }
    }

    /// Advance the animation phase. `frame_ms == 0` => static (phase pinned to 0).
    pub fn advance(&mut self, dt_ms: f32, frame_ms: u32) {
        if frame_ms == 0 { self.phase = 0.0; return; }
        // One full phase cycle spans `frame_ms` per implied frame; keep it simple:
        // advance proportionally and wrap.
        let cycle_ms = frame_ms as f32 * 2.0; // 2-frame default cadence
        self.phase = (self.phase + dt_ms / cycle_ms).rem_euclid(1.0);
    }
}
```

Add to `src/lib.rs`: `pub mod pet;`

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib pet`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/pet.rs src/lib.rs
git commit -m "feat(pet): pet model with priority, phase, and frame indexing"
```

---

### Task 7: Herd — reconcile, roam+separation, overflow

**Files:**
- Create: `src/herd.rs`
- Modify: `src/lib.rs` (add `pub mod herd;`)

**Interfaces:**
- Consumes: `pet::{Pet, priority}`, `agent::Agent`, `identity::identity_for`.
- Produces:
  - `Lcg` — `Lcg::new(seed: u64)`, `impl Rng for Lcg`; trait `Rng { fn next_unit(&mut self) -> f32; }`.
  - `Herd { pub pets: Vec<Pet> }`; `Herd::new()`.
  - `Herd::reconcile(&mut self, agents: &[Agent], species_count: usize, strip_w: f32, rng: &mut dyn Rng)` — add new pets (by `terminal_id`) at a random x, drop departed ones, update status of survivors (position/phase preserved).
  - `Herd::step(&mut self, dt_ms: f32, strip_w: f32, pet_w: f32, rng: &mut dyn Rng)` — pick roam targets by status, ease toward them, apply pairwise separation, clamp to `[0, strip_w - pet_w]`.
  - `visible_and_hidden(pets: &[Pet], capacity: usize) -> (Vec<usize>, usize)` — indices of visible pets (priority-ranked, ties by terminal_id) and the hidden count.

- [ ] **Step 1: Write the failing test** — append to `src/herd.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, AgentStatus};

    fn agent(tid: &str, status: AgentStatus) -> Agent {
        Agent {
            agent: Some("claude".into()), agent_status: status, name: None,
            cwd: "/".into(), foreground_cwd: "/".into(), workspace_id: "w".into(),
            tab_id: "t".into(), pane_id: "p".into(), terminal_id: tid.into(),
            revision: 0, focused: false,
        }
    }

    #[test]
    fn reconcile_adds_updates_and_removes_by_terminal_id() {
        let mut h = Herd::new();
        let mut rng = Lcg::new(1);
        h.reconcile(&[agent("a", AgentStatus::Idle), agent("b", AgentStatus::Working)], 2, 200.0, &mut rng);
        assert_eq!(h.pets.len(), 2);

        // 'a' changes status, 'b' leaves, 'c' joins.
        h.pets[0].x = 42.0; // survivor position must be preserved
        h.reconcile(&[agent("a", AgentStatus::Blocked), agent("c", AgentStatus::Idle)], 2, 200.0, &mut rng);
        let a = h.pets.iter().find(|p| p.terminal_id == "a").unwrap();
        assert_eq!(a.status, AgentStatus::Blocked);
        assert_eq!(a.x, 42.0, "survivor keeps position");
        assert!(h.pets.iter().any(|p| p.terminal_id == "c"));
        assert!(!h.pets.iter().any(|p| p.terminal_id == "b"));
    }

    #[test]
    fn step_keeps_pets_within_bounds() {
        let mut h = Herd::new();
        let mut rng = Lcg::new(7);
        h.reconcile(&(0..6).map(|i| agent(&format!("t{i}"), AgentStatus::Working)).collect::<Vec<_>>(), 2, 100.0, &mut rng);
        for _ in 0..200 { h.step(50.0, 100.0, 20.0, &mut rng); }
        for p in &h.pets {
            assert!(p.x >= 0.0 && p.x <= 80.0, "x={} out of bounds", p.x);
        }
    }

    #[test]
    fn overflow_keeps_attention_states_and_drops_idle_first() {
        let pets = vec![
            crate::pet::Pet::new("i".into(), crate::identity::identity_for("i", 2), AgentStatus::Idle, 0.0),
            crate::pet::Pet::new("b".into(), crate::identity::identity_for("b", 2), AgentStatus::Blocked, 0.0),
            crate::pet::Pet::new("w".into(), crate::identity::identity_for("w", 2), AgentStatus::Working, 0.0),
        ];
        let (visible, hidden) = visible_and_hidden(&pets, 2);
        assert_eq!(hidden, 1);
        // the blocked and working pets must be the visible ones; idle dropped.
        let names: Vec<&str> = visible.iter().map(|&i| pets[i].terminal_id.as_str()).collect();
        assert!(names.contains(&"b") && names.contains(&"w"));
        assert!(!names.contains(&"i"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib herd`
Expected: FAIL — items not found.

- [ ] **Step 3: Write minimal implementation** — top of `src/herd.rs`

```rust
//! The herd: a free-roaming collection of pets. Reconciles against agent
//! snapshots by terminal_id (survivors keep position + phase), roams with a
//! gentle separation force, and selects a priority-ranked visible set on
//! overflow. All randomness is an injected LCG so the simulation is testable.

use crate::agent::Agent;
use crate::identity::identity_for;
use crate::pet::{Pet, priority};

/// Minimal injected RNG (no `rand` dependency).
pub trait Rng { fn next_unit(&mut self) -> f32; } // returns 0.0..1.0

pub struct Lcg { state: u64 }
impl Lcg { pub fn new(seed: u64) -> Self { Self { state: seed.wrapping_add(0x9E37_79B9_7F4A_7C15) } } }
impl Rng for Lcg {
    fn next_unit(&mut self) -> f32 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.state >> 33) as f32) / (1u64 << 31) as f32
    }
}

#[derive(Default)]
pub struct Herd { pub pets: Vec<Pet> }

impl Herd {
    pub fn new() -> Self { Self { pets: Vec::new() } }

    pub fn reconcile(&mut self, agents: &[Agent], species_count: usize, strip_w: f32, rng: &mut dyn Rng) {
        // Update survivors / add new.
        for a in agents {
            if let Some(p) = self.pets.iter_mut().find(|p| p.terminal_id == a.terminal_id) {
                p.status = a.agent_status;
            } else {
                let x = rng.next_unit() * strip_w.max(1.0);
                self.pets.push(Pet::new(
                    a.terminal_id.clone(),
                    identity_for(&a.terminal_id, species_count),
                    a.agent_status,
                    x,
                ));
            }
        }
        // Remove departed.
        self.pets.retain(|p| agents.iter().any(|a| a.terminal_id == p.terminal_id));
    }

    pub fn step(&mut self, dt_ms: f32, strip_w: f32, pet_w: f32, rng: &mut dyn Rng) {
        let dt = dt_ms / 1000.0;
        let max_x = (strip_w - pet_w).max(0.0);
        for p in &mut self.pets {
            // Working roams widely; idle/done drift a little; blocked holds.
            let roam = match p.status {
                crate::agent::AgentStatus::Working => 1.0,
                crate::agent::AgentStatus::Blocked => 0.0,
                _ => 0.35,
            };
            if rng.next_unit() < roam * dt * 0.6 {
                p.target_x = rng.next_unit() * max_x;
            }
            let speed = if p.status == crate::agent::AgentStatus::Working { 22.0 } else { 7.0 };
            let dx = p.target_x - p.x;
            p.x += dx.signum() * dx.abs().min(speed * dt);
        }
        // Pairwise separation.
        let min_gap = pet_w * 0.55;
        let n = self.pets.len();
        for i in 0..n {
            for j in (i + 1)..n {
                let gap = self.pets[j].x - self.pets[i].x;
                if gap.abs() < min_gap {
                    let push = (min_gap - gap.abs()) * 0.5 * dt;
                    let dir = if gap >= 0.0 { 1.0 } else { -1.0 };
                    self.pets[i].x -= push * dir;
                    self.pets[j].x += push * dir;
                }
            }
        }
        for p in &mut self.pets { p.x = p.x.clamp(0.0, max_x); }
    }
}

/// Priority-ranked visibility: keep the highest-priority `capacity` pets
/// (ties by terminal_id for stability); return their indices + hidden count.
pub fn visible_and_hidden(pets: &[Pet], capacity: usize) -> (Vec<usize>, usize) {
    let mut idx: Vec<usize> = (0..pets.len()).collect();
    if pets.len() <= capacity {
        return (idx, 0);
    }
    idx.sort_by(|&a, &b| {
        priority(pets[b].status).cmp(&priority(pets[a].status))
            .then_with(|| pets[a].terminal_id.cmp(&pets[b].terminal_id))
    });
    let hidden = pets.len() - capacity;
    idx.truncate(capacity);
    (idx, hidden)
}
```

Add to `src/lib.rs`: `pub mod herd;`

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib herd`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/herd.rs src/lib.rs
git commit -m "feat(herd): reconcile, roam+separation, priority overflow"
```

---

### Task 8: Render — half-block blit of the herd

**Files:**
- Modify: `src/render.rs` (replace the placeholder `draw`/`status_glyph`; keep the run-loop shell, adapt it in Task 11)
- Modify: `src/lib.rs` (no change; `render` already public)

**Interfaces:**
- Consumes: `herd::{Herd, visible_and_hidden}`, `pet::Pet`, `sprite::{Species, Role, Frame}`, `palette::{role_color, Theme, StateStyle}`, `anim::{motion_offset, Overlay, OverlayColor}`, `agent::AgentStatus`.
- Produces:
  - `PixelBuf { w: usize, h: usize, px: Vec<Option<Rgb>> }`; `PixelBuf::new(w,h)`; `PixelBuf::blit(frame, colors, x, y)`.
  - `draw_pixels(frame: &mut ratatui::Frame, area: Rect, buf: &PixelBuf)` — emit the pixel buffer as `▀` half-block cells.
  - `draw_herd(frame: &mut ratatui::Frame, herd: &Herd, species: &[Species], theme: Theme)` — the full strip: blit visible pets (priority z-order), overlay bubbles/badges, and a `+N` marker.
  - `PET_PX_H: usize = 12` (6 rows).

- [ ] **Step 1: Write the failing snapshot test** — append to `src/render.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, AgentStatus};
    use crate::herd::{Herd, Lcg};
    use crate::palette::Theme;
    use crate::sprite::parse_species;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    const BLOB: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/sprites/test-blob.sprite"));

    fn agent(tid: &str, s: AgentStatus) -> Agent {
        Agent { agent: None, agent_status: s, name: None, cwd: "/".into(), foreground_cwd: "/".into(),
            workspace_id: "w".into(), tab_id: "t".into(), pane_id: "p".into(),
            terminal_id: tid.into(), revision: 0, focused: false }
    }

    fn fixed_herd(states: &[AgentStatus]) -> Herd {
        let mut h = Herd::new();
        let mut rng = Lcg::new(1);
        let agents: Vec<_> = states.iter().enumerate()
            .map(|(i, s)| agent(&format!("t{i}"), *s)).collect();
        h.reconcile(&agents, 1, 120.0, &mut rng);
        // Freeze positions + phase for a deterministic snapshot.
        for (i, p) in h.pets.iter_mut().enumerate() {
            p.x = 4.0 + i as f32 * 16.0;
            p.target_x = p.x;
            p.phase = 0.0;
        }
        h
    }

    #[test]
    fn renders_each_state_in_the_strip() {
        use AgentStatus::*;
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[Idle, Working, Done, Blocked, Unknown]);
        let mut terminal = Terminal::new(TestBackend::new(90, 6)).unwrap();
        terminal.draw(|f| draw_herd(f, &herd, &species, Theme::Dark)).unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn renders_overflow_counter() {
        use AgentStatus::*;
        let species = vec![parse_species(BLOB).unwrap()];
        let herd = fixed_herd(&[Idle; 20]);
        let mut terminal = Terminal::new(TestBackend::new(40, 6)).unwrap();
        terminal.draw(|f| draw_herd(f, &herd, &species, Theme::Dark)).unwrap();
        insta::assert_snapshot!(terminal.backend());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib render`
Expected: FAIL — `draw_herd`/`PixelBuf` not found.

- [ ] **Step 3: Write minimal implementation** — replace the top of `src/render.rs` (keep the `run`/`run_loop` functions for Task 11; delete `status_glyph` and the old `draw`)

```rust
//! Half-block renderer: blit the roaming herd into a pixel buffer, emit it as
//! `▀` cells (fg = top pixel, bg = bottom pixel), then overlay state bubbles/
//! badges and a `+N` counter.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;

use crate::anim::{motion_offset, Overlay, OverlayColor, Rgb};
use crate::herd::{visible_and_hidden, Herd};
use crate::palette::{role_color, StateStyle, Theme};
use crate::pet::priority;
use crate::sprite::Species;

pub const PET_PX_H: usize = 12;

pub struct PixelBuf { pub w: usize, pub h: usize, pub px: Vec<Option<Rgb>> }

impl PixelBuf {
    pub fn new(w: usize, h: usize) -> Self { Self { w, h, px: vec![None; w * h] } }

    pub fn set(&mut self, x: i32, y: i32, c: Rgb) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            self.px[y as usize * self.w + x as usize] = Some(c);
        }
    }
}

fn to_color(c: Rgb) -> Color { Color::Rgb(c.0, c.1, c.2) }

/// Emit the pixel buffer as half-block cells into `area` (top-left aligned).
pub fn draw_pixels(frame: &mut Frame, area: Rect, buf: &PixelBuf) {
    let rows = buf.h.div_ceil(2);
    for ry in 0..rows {
        for x in 0..buf.w {
            let top = buf.px[(ry * 2) * buf.w + x];
            let bot = if ry * 2 + 1 < buf.h { buf.px[(ry * 2 + 1) * buf.w + x] } else { None };
            let cx = area.x + x as u16;
            let cy = area.y + ry as u16;
            if cx >= area.right() || cy >= area.bottom() { continue; }
            let (ch, style) = match (top, bot) {
                (None, None) => continue,
                (Some(t), Some(b)) => ('▀', Style::default().fg(to_color(t)).bg(to_color(b))),
                (Some(t), None) => ('▀', Style::default().fg(to_color(t))),
                (None, Some(b)) => ('▄', Style::default().fg(to_color(b))),
            };
            frame.buffer_mut().set_string(cx, cy, ch.to_string(), style);
        }
    }
}

/// Draw the whole strip: visible pets (priority z-order), overlays, `+N`.
pub fn draw_herd(frame: &mut Frame, herd: &Herd, species: &[Species], theme: Theme) {
    let area = frame.area();
    let strip_w = area.width as usize;
    let mut buf = PixelBuf::new(strip_w, PET_PX_H);

    let pet_w = species.first().map(|s| s.size().0).unwrap_or(12);
    let capacity = (strip_w / (pet_w * 3 / 4).max(1)).max(1);
    let (visible, hidden) = visible_and_hidden(&herd.pets, capacity);

    // z-order: lowest priority first so blocked draws last (on top).
    let mut order = visible.clone();
    order.sort_by_key(|&i| priority(herd.pets[i].status));

    for &i in &order {
        let pet = &herd.pets[i];
        let Some(sp) = species.get(pet.identity.species_index).or_else(|| species.first()) else { continue };
        let Some(state) = sp.states.get(&pet.status) else { continue };
        let fi = pet.frame_index(state.frames.len());
        let fr = &state.frames[fi];
        let style = StateStyle { dim: state.dim, ghost: state.ghost };
        let off = motion_offset(&state.motion, pet.phase);
        let ox = (pet.x + off.dx).round() as i32;
        let oy = (off.dy).round() as i32; // ground-aligned; dy<=0 lifts
        for y in 0..fr.h {
            for x in 0..fr.w {
                if let Some(c) = role_color(fr.cells[y * fr.w + x], pet.identity.hue, theme, style) {
                    buf.set(ox + x as i32, oy + y as i32, c);
                }
            }
        }
    }
    draw_pixels(frame, area, &buf);

    // Overlays (bubbles/badges) as text cells above each visible pet.
    for &i in &order {
        let pet = &herd.pets[i];
        let Some(sp) = species.get(pet.identity.species_index).or_else(|| species.first()) else { continue };
        let Some(state) = sp.states.get(&pet.status) else { continue };
        let (glyph, kind) = match &state.overlay.kind {
            Overlay::Bubble(g) => (g.clone(), 'b'),
            Overlay::Badge(g) => (g.clone(), 'a'),
            Overlay::None => continue,
        };
        let color = match state.overlay.color {
            OverlayColor::Literal(c) => to_color(c),
            OverlayColor::Accent => Color::Rgb(0xe6, 0xc8, 0x77),
            OverlayColor::Default => Color::Gray,
        };
        let _ = kind;
        let cx = area.x + (pet.x.round() as u16).saturating_add(3).min(area.width.saturating_sub(glyph.len() as u16));
        frame.buffer_mut().set_span(cx, area.y, &Span::styled(glyph, Style::default().fg(color)), area.width);
    }

    if hidden > 0 {
        let label = format!("+{hidden}");
        let x = area.right().saturating_sub(label.len() as u16 + 1);
        frame.buffer_mut().set_span(x, area.y + area.height / 2,
            &Span::styled(label, Style::default().fg(Color::DarkGray)), label.len() as u16);
    }
}
```

- [ ] **Step 4: Run tests; accept snapshots**

Run: `cargo test --lib render` → new snapshots pending.
Run: `cargo insta accept` (review first with `cargo insta review` if desired).
Run: `cargo test --lib render`
Expected: PASS (2 tests). Snapshots land in `src/snapshots/`.

- [ ] **Step 5: Commit**

```bash
git add src/render.rs src/snapshots/
git commit -m "feat(render): half-block herd renderer with overlays and overflow"
```

---

### Task 9: Socket — persistent line-delimited JSON-RPC client

**Files:**
- Modify: `src/socket.rs` (grow the Phase 0 one-shot helper into a persistent client behind a trait)

**Interfaces:**
- Consumes: nothing (std only).
- Produces:
  - `SocketClient` trait: `fn send_line(&mut self, line: &str) -> std::io::Result<()>`, `fn recv_line(&mut self) -> std::io::Result<String>` (one framed line, trailing newline stripped).
  - `RealSocket { .. }`; `RealSocket::connect(path: &Path) -> io::Result<RealSocket>` implementing `SocketClient` over `UnixStream` with an internal `BufReader`.
  - Keep `socket_path()` (unchanged). Keep the Phase 0 `request()` (still used by nothing critical; leave for compatibility) OR remove if unused — leave it.
  - `subscribe_request() -> String` — the verified `events.subscribe` line from Spike 1 (default `{"id":"pets","method":"events.subscribe","params":{}}` until Spike 1 refines it).

- [ ] **Step 1: Write the failing test** — append to `src/socket.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    #[test]
    fn real_socket_sends_and_receives_framed_lines() {
        let path = std::env::temp_dir().join(format!("herdr-pets-rt-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn({
            let path = path.clone();
            move || {
                let (conn, _) = listener.accept().unwrap();
                let mut r = BufReader::new(conn.try_clone().unwrap());
                let mut w = conn;
                let mut got = String::new();
                r.read_line(&mut got).unwrap();
                w.write_all(b"{\"event\":\"ok\"}\n").unwrap();
                let _ = std::fs::remove_file(&path);
                got
            }
        });
        let mut c = RealSocket::connect(&path).unwrap();
        c.send_line("{\"id\":\"x\",\"method\":\"events.subscribe\",\"params\":{}}").unwrap();
        let reply = c.recv_line().unwrap();
        assert_eq!(reply, "{\"event\":\"ok\"}");
        let got = server.join().unwrap();
        assert!(got.contains("events.subscribe"));
    }

    #[test]
    fn subscribe_request_is_valid_json_line() {
        let s = subscribe_request();
        assert!(s.contains("events.subscribe"));
        assert!(!s.contains('\n'));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib socket`
Expected: FAIL — `RealSocket`/`SocketClient`/`subscribe_request` not found.

- [ ] **Step 3: Write minimal implementation** — add to `src/socket.rs` (keep existing `socket_path`/`request`)

```rust
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

/// A persistent, line-delimited JSON-RPC connection (see Phase 0 Spike A: the
/// control socket speaks newline-delimited JSON-RPC with dotted method names).
pub trait SocketClient {
    fn send_line(&mut self, line: &str) -> std::io::Result<()>;
    fn recv_line(&mut self) -> std::io::Result<String>;
}

pub struct RealSocket {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

impl RealSocket {
    pub fn connect(path: &Path) -> std::io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Self { writer: stream, reader })
    }
}

impl SocketClient for RealSocket {
    fn send_line(&mut self, line: &str) -> std::io::Result<()> {
        self.writer.write_all(line.as_bytes())?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()
    }
    fn recv_line(&mut self) -> std::io::Result<String> {
        let mut s = String::new();
        let n = self.reader.read_line(&mut s)?;
        if n == 0 {
            return Err(std::io::Error::other("socket closed"));
        }
        Ok(s.trim_end_matches(['\r', '\n']).to_string())
    }
}

/// The verified `events.subscribe` request line (refine per Spike 1).
pub fn subscribe_request() -> String {
    r#"{"id":"pets","method":"events.subscribe","params":{}}"#.to_string()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib socket`
Expected: PASS (3 tests — includes the Phase 0 test).

- [ ] **Step 5: Commit**

```bash
git add src/socket.rs
git commit -m "feat(socket): persistent line-delimited JSON-RPC SocketClient"
```

---

### Task 10: Watcher — background snapshot source

**Files:**
- Create: `src/watcher.rs`
- Modify: `src/lib.rs` (add `pub mod watcher;`)

**Interfaces:**
- Consumes: `socket::{SocketClient, subscribe_request}`, `herdr::HerdrCli`, `agent::{Agent, parse_agent_list}`.
- Produces:
  - `Clock` trait: `fn now_ms(&self) -> u64`; `RealClock` (via `std::time::Instant` from a stored origin).
  - `fn watch(cli: Box<dyn HerdrCli + Send>, socket: Option<Box<dyn SocketClient + Send>>, clock: Box<dyn Clock + Send>, tx: std::sync::mpsc::Sender<Vec<Agent>>, slow_ms: u64, debounce_ms: u64) -> std::thread::JoinHandle<()>` — spawns the watcher loop (used by `main`).
  - `fn run_watch_once(...)` — a **pure, non-threaded** step used by tests: given a fake socket that yields a fixed number of events and a fake cli, drive the debounce/poll logic and return the sequence of snapshots it would push. (Exact signature below.)

> To keep the watcher testable without threads, the debounce/poll decision logic lives in a pure helper the thread wraps. Tests exercise the helper; the thread is a thin loop.

- [ ] **Step 1: Write the failing test** — append to `src/watcher.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentStatus;
    use crate::herdr::{CommandRunner, LiveHerdr};
    use std::ffi::OsStr;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{ExitStatus, Output};

    const LIST: &str = r#"{"result":{"agents":[{"agent_status":"idle","cwd":"/","focused":false,"foreground_cwd":"/","pane_id":"p","revision":0,"tab_id":"t","terminal_id":"x","workspace_id":"w"}]}}"#;

    struct FakeRunner;
    impl CommandRunner for FakeRunner {
        fn run(&self, _p: &OsStr, _a: &[&str]) -> std::io::Result<Output> {
            Ok(Output { status: ExitStatus::from_raw(0), stdout: LIST.as_bytes().to_vec(), stderr: vec![] })
        }
    }

    // A fake socket that emits N event lines then blocks forever (returns Err).
    struct FakeSocket { remaining: usize }
    impl crate::socket::SocketClient for FakeSocket {
        fn send_line(&mut self, _l: &str) -> std::io::Result<()> { Ok(()) }
        fn recv_line(&mut self) -> std::io::Result<String> {
            if self.remaining == 0 { return Err(std::io::Error::other("done")); }
            self.remaining -= 1;
            Ok(r#"{"event":"agent.status_changed"}"#.into())
        }
    }

    #[test]
    fn debounce_coalesces_a_burst_into_one_refetch() {
        let cli = LiveHerdr::with_runner("herdr", FakeRunner);
        // 5 events arriving within the debounce window => 1 snapshot.
        let snaps = drain_events(&cli, &mut FakeSocket { remaining: 5 }, 250, |_| 0 /* all same tick */);
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].len(), 1);
        assert_eq!(snaps[0][0].agent_status, AgentStatus::Idle);
    }

    #[test]
    fn separated_events_produce_separate_refetches() {
        let cli = LiveHerdr::with_runner("herdr", FakeRunner);
        let mut tick = 0u64;
        let snaps = drain_events(&cli, &mut FakeSocket { remaining: 3 }, 250, move |_| { tick += 1000; tick });
        assert_eq!(snaps.len(), 3);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib watcher`
Expected: FAIL — `drain_events`/`watch`/`Clock` not found.

- [ ] **Step 3: Write minimal implementation** — top of `src/watcher.rs`

```rust
//! Background watcher: subscribe to socket events; on any event, debounced-
//! refetch `herdr agent list` and push a snapshot. A slow interval refetch is
//! the safety net; socket failure degrades to poll-only. All timing is behind a
//! clock seam so the debounce/coalesce logic is unit-testable without threads.

use std::sync::mpsc::Sender;
use std::thread::JoinHandle;

use crate::agent::{parse_agent_list, Agent};
use crate::herdr::HerdrCli;
use crate::socket::{subscribe_request, SocketClient};

pub trait Clock { fn now_ms(&self) -> u64; }

pub struct RealClock { origin: std::time::Instant }
impl RealClock { pub fn new() -> Self { Self { origin: std::time::Instant::now() } } }
impl Default for RealClock { fn default() -> Self { Self::new() } }
impl Clock for RealClock { fn now_ms(&self) -> u64 { self.origin.elapsed().as_millis() as u64 } }

fn refetch(cli: &dyn HerdrCli) -> Option<Vec<Agent>> {
    cli.run_json(&["agent", "list"]).ok().and_then(|s| parse_agent_list(&s).ok())
}

/// Test seam: consume all events a socket yields, applying the debounce rule,
/// and return the snapshots that would be pushed. `event_time(i)` supplies the
/// clock reading (ms) for the i-th event so tests can place events in/out of
/// the debounce window.
pub fn drain_events(
    cli: &dyn HerdrCli,
    socket: &mut dyn SocketClient,
    debounce_ms: u64,
    mut event_time: impl FnMut(usize) -> u64,
) -> Vec<Vec<Agent>> {
    let _ = socket.send_line(&subscribe_request());
    let mut snaps = Vec::new();
    let mut last_fetch: Option<u64> = None;
    let mut i = 0;
    while let Ok(_line) = socket.recv_line() {
        let t = event_time(i);
        i += 1;
        let due = match last_fetch { Some(prev) => t.saturating_sub(prev) >= debounce_ms, None => true };
        if due {
            if let Some(s) = refetch(cli) { snaps.push(s); }
            last_fetch = Some(t);
        }
    }
    snaps
}

/// Spawn the real watcher thread. On socket errors it degrades to a slow poll.
pub fn watch(
    cli: Box<dyn HerdrCli + Send>,
    mut socket: Option<Box<dyn SocketClient + Send>>,
    clock: Box<dyn Clock + Send>,
    tx: Sender<Vec<Agent>>,
    slow_ms: u64,
    debounce_ms: u64,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        // Initial snapshot.
        if let Some(s) = refetch(cli.as_ref()) { let _ = tx.send(s); }
        let mut last = clock.now_ms();
        if let Some(sock) = socket.as_mut() {
            let _ = sock.send_line(&subscribe_request());
        }
        loop {
            let mut fired = false;
            if let Some(sock) = socket.as_mut() {
                match sock.recv_line() {
                    Ok(_line) => {
                        let now = clock.now_ms();
                        if now.saturating_sub(last) >= debounce_ms {
                            if let Some(s) = refetch(cli.as_ref()) { if tx.send(s).is_err() { return; } }
                            last = now;
                        }
                        fired = true;
                    }
                    Err(_) => { socket = None; } // degrade to polling
                }
            }
            if !fired {
                std::thread::sleep(std::time::Duration::from_millis(slow_ms));
                if let Some(s) = refetch(cli.as_ref()) { if tx.send(s).is_err() { return; } }
                last = clock.now_ms();
            }
        }
    })
}
```

Add to `src/lib.rs`: `pub mod watcher;`

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib watcher`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/watcher.rs src/lib.rs
git commit -m "feat(watcher): debounced event-driven snapshot source with poll fallback"
```

---

### Task 11: Render loop + binary wiring

**Files:**
- Modify: `src/render.rs` (replace `run`/`run_loop` with the two-clock loop consuming a snapshot channel + a herd)
- Modify: `src/main.rs` (spawn the watcher, connect the socket if present, run the loop)

**Interfaces:**
- Consumes: `herd::{Herd, Lcg}`, `watcher::{watch, RealClock}`, `socket::{socket_path, RealSocket}`, `herdr::LiveHerdr`, `sprite::load_species`, `agent::Agent`.
- Produces:
  - `render::run(rx: std::sync::mpsc::Receiver<Vec<Agent>>, species: Vec<Species>, theme: Theme) -> io::Result<()>` — the render thread: ~12 fps tick, drain snapshots→reconcile, step herd, draw, quit on `q`/Ctrl-C, restore terminal.

- [ ] **Step 1: Write the failing test** — append to `src/render.rs`

```rust
    #[test]
    fn reconcile_then_draw_shows_the_incoming_herd() {
        // A focused integration check: feed one snapshot, reconcile, draw, snapshot.
        use crate::agent::AgentStatus::*;
        use crate::herd::{Herd, Lcg};
        let species = vec![crate::sprite::parse_species(BLOB).unwrap()];
        let mut herd = Herd::new();
        let mut rng = Lcg::new(3);
        herd.reconcile(&[agent("a", Working), agent("b", Blocked)], 1, 60.0, &mut rng);
        for (i, p) in herd.pets.iter_mut().enumerate() { p.x = 4.0 + i as f32 * 18.0; p.target_x = p.x; }
        let mut terminal = Terminal::new(TestBackend::new(60, 6)).unwrap();
        terminal.draw(|f| draw_herd(f, &herd, &species, Theme::Dark)).unwrap();
        insta::assert_snapshot!(terminal.backend());
    }
```

(The two-clock `run` loop itself is exercised by the manual dev-loop check in Step 5; it is a thin wrapper over the already-tested `draw_herd` + `Herd::step`/`reconcile`.)

- [ ] **Step 2: Run test to verify it fails, then accept snapshot**

Run: `cargo test --lib render` → new snapshot pending → `cargo insta accept` → re-run → PASS.

- [ ] **Step 3: Replace `run`/`run_loop` in `src/render.rs`**

```rust
use std::io;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::agent::Agent;
use crate::herd::{Herd, Lcg};
use crate::sprite::Species;

/// Render thread: ~12 fps tick. Drains snapshots, reconciles, steps the herd,
/// draws, and quits on `q`/Ctrl-C. Restores the terminal on exit.
pub fn run(rx: Receiver<Vec<Agent>>, species: Vec<Species>, theme: Theme) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    let res = run_loop(&mut terminal, rx, &species, theme);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    res
}

fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    rx: Receiver<Vec<Agent>>,
    species: &[Species],
    theme: Theme,
) -> io::Result<()>
where io::Error: From<B::Error> {
    let tick = Duration::from_millis(83); // ~12 fps
    let species_count = species.len().max(1);
    let mut herd = Herd::new();
    let mut rng = Lcg::new(0xC0FFEE);
    let mut last = Instant::now();
    loop {
        while let Ok(agents) = rx.try_recv() {
            let w = terminal.size()?.width as f32;
            herd.reconcile(&agents, species_count, w, &mut rng);
        }
        let now = Instant::now();
        let dt_ms = (now - last).as_millis() as f32;
        last = now;
        let w = terminal.size()?.width as f32;
        let pet_w = species.first().map(|s| s.size().0).unwrap_or(12) as f32;
        herd.step(dt_ms, w, pet_w, &mut rng);
        for p in herd.pets.iter_mut() {
            let fm = species.get(p.identity.species_index).or_else(|| species.first())
                .and_then(|s| s.states.get(&p.status)).map(|st| st.frame_ms).unwrap_or(0);
            p.advance(dt_ms, fm);
        }
        terminal.draw(|f| draw_herd(f, &herd, species, theme))?;

        if event::poll(tick)? {
            if let Event::Key(k) = event::read()? {
                let quit = k.code == KeyCode::Char('q')
                    || (k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL));
                if quit { return Ok(()); }
            }
        }
    }
}
```

- [ ] **Step 4: Rewrite `src/main.rs`**

```rust
use std::process::ExitCode;
use std::sync::mpsc;

use herdr_pets::herdr::LiveHerdr;
use herdr_pets::palette::Theme;
use herdr_pets::socket::{socket_path, RealSocket, SocketClient};
use herdr_pets::sprite::load_species;
use herdr_pets::watcher::{watch, RealClock};
use herdr_pets::render;

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("render") => {
            let species = load_species();
            let (tx, rx) = mpsc::channel();
            let cli = Box::new(LiveHerdr::from_env());
            let socket: Option<Box<dyn SocketClient + Send>> = socket_path()
                .and_then(|p| RealSocket::connect(&p).ok())
                .map(|s| Box::new(s) as Box<dyn SocketClient + Send>);
            let _watcher = watch(cli, socket, Box::new(RealClock::new()), tx, 2500, 250);
            match render::run(rx, species, Theme::Dark) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => { eprintln!("herdr-pets: {e}"); ExitCode::FAILURE }
            }
        }
        Some("--version") | Some("-V") => {
            println!("herdr-pets {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => { eprintln!("usage: herdr-pets render"); ExitCode::FAILURE }
    }
}
```

- [ ] **Step 5: Verify the suite + manual dev-loop check**

Run: `cargo test` — whole suite green. `cargo build --release`.
```bash
herdr plugin link .    # if not already linked
herdr plugin pane open --plugin herdr-pets --entrypoint pets
```
Expected: a strip of roaming pets, one per live agent, animating and tracking status; `q` closes. Note anything surprising.

- [ ] **Step 6: Commit**

```bash
git add src/render.rs src/main.rs src/snapshots/
git commit -m "feat: two-clock render loop wired to the watcher snapshot channel"
```

---

### Task 12: Author the sheep + proof species

**Files:**
- Create: `sprites/sheep.sprite` (all five states, multi-frame)
- Create: `sprites/goat.sprite` (proof species; clearly different silhouette)
- Modify: `src/sprite.rs` (point `EMBEDDED` at `sheep.sprite` + `goat.sprite`; drop the temporary `test-blob`-only wiring — `test-blob` stays for unit tests via its own `include_str!`)

**This task produces data, verified by the Task 4 validation guard** (`every_embedded_species_is_valid`) and the render snapshots.

- [ ] **Step 1: Author `sprites/sheep.sprite`**

Use the approved reference style at **12 px tall (6 rows)**: bold `#` outline, blocky castellated wool in `L`/`M`/`S`, `p` skin face + hooves, `e` eye, facing right. Base the `idle` grid on the brainstorm's 16×13 sheep (translate `K→#`, cream/`C→M`, grey/`G→S`, highlights→`L`, peach/`P→p`, eye→`e`). Provide at least 2 frames for `idle` (wool bob) and `working` (leg gait); 1 frame is fine for `done`/`blocked`/`unknown`. Header config per state:

```text
name = Sheep

[idle]    frame_ms=520 motion=breathe overlay=bubble:Zz
… frame 1 …

… frame 2 …

[working] frame_ms=150 motion=hop+wander overlay=none
… frame 1 …

… frame 2 …

[done]    frame_ms=1400 motion=hop overlay=badge:! color=accent
… frame …

[blocked] frame_ms=120 motion=shake overlay=badge:! color=#e62d23
… frame …

[unknown] frame_ms=0 motion=sway overlay=bubble:? ghost=true
… frame …
```

- [ ] **Step 2: Author `sprites/goat.sprite`**

Same legend and 6-row budget, but a distinct silhouette (slimmer body, `h` horns, `p` beard) so the plug-and-play claim is visibly true. All five states; the same header shape as the sheep.

- [ ] **Step 3: Point `EMBEDDED` at the real sprites**

In `src/sprite.rs`, set:
```rust
const EMBEDDED: &[&str] = &[
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/sprites/sheep.sprite")),
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/sprites/goat.sprite")),
];
```

- [ ] **Step 4: Run the guard + render snapshots**

Run: `cargo test --lib sprite` → `every_embedded_species_is_valid` PASSES for both real species.
Run: `cargo test` → whole suite green (render snapshots still use `test-blob`, so they don't churn).
Optional: `cargo build --release && herdr plugin pane open …` to eyeball the real sheep/goat in a live strip.

- [ ] **Step 5: Commit**

```bash
git add sprites/sheep.sprite sprites/goat.sprite src/sprite.rs
git commit -m "feat(sprite): author the sheep and a proof goat species"
```

---

### Task 13: Close out Phase 1

**Files:**
- Modify: `docs/PLAN.md` (Phase tracker row for Phase 1).

- [ ] **Step 1: Update the Phase tracker**

In `docs/PLAN.md`, set the Phase 1 row `Status` to `Done` and link the design + plan:

```markdown
| 1 | The pets (renderer core) | Done | [design](superpowers/specs/2026-07-23-phase-1-renderer-core-design.md) | [plan](superpowers/plans/2026-07-23-phase-1-renderer-core.md) |
```

- [ ] **Step 2: Verify the whole suite + a clean build**

Run: `cargo test && cargo build --release`
Expected: all tests pass; release binary builds.

- [ ] **Step 3: Commit**

```bash
git add docs/PLAN.md
git commit -m "docs(phase-1): mark Phase 1 done and link design + plan"
```

- [ ] **Step 4: Report to the user**

Summarize what landed, the Spike 1 finding, confirm nothing was pushed, and ask whether to push `feat/phase-1-renderer-core` and/or open a PR (and whether to first merge/rebase relative to Phase 0's PR #1).

---

## Self-Review

**Spec coverage:**
- Identity `hash(terminal_id)` (spec §2, §9) → Task 2. ✅
- Plug-and-play sprite format: roles, one-file-per-animal, embed + override, validation (spec §3) → Task 4 (+ authoring in Task 12). ✅
- Rendering: half-block, tint, theme, overrides (spec §4.1–4.2) → Tasks 5, 8. ✅
- State→animation config library (spec §4.3) → Task 3 (parse/primitives) + consumed in Task 8. ✅
- Herd: free-roam, separation, priority z-index, priority overflow, reconcile (spec §5) → Tasks 6, 7, 8. ✅
- Live updates: two clocks, watcher thread, debounce, slow poll, degrade, seams (spec §6) → Tasks 9, 10, 11. ✅
- Spike 1 (spec §8) → Task 1. ✅
- Bestiary: sheep + one proof species (spec §7) → Task 12. ✅
- Testing approach (spec §10) → each task's Step 1/2 + snapshot tasks. ✅
- Module plan (spec §9) → Tasks map 1:1 to modules. ✅
- Guardrails / scope (spec §12) → Global Constraints. ✅

**Placeholder scan:** The only intentional fill-ins are Spike 1's *finding* (Task 1, an experiment deliverable, like Phase 0) and the sprite *pixel art* (Task 12, data verified by the Task 4 guard). All code steps contain complete, runnable code. Task 4 carries an explicit note about temporarily pointing `EMBEDDED` at `test-blob` until Task 12 authors the real art, so every task compiles in isolation.

**Type consistency:** `Identity{species_index,hue}`, `Rgb`, `Motion`/`MotionSpec`/`motion_offset`/`has_wander`, `Overlay`/`OverlayColor`/`OverlaySpec`, `Role`/`role_from_char`/`Frame`/`StateSpec`/`Species`/`parse_species`/`embedded_species`/`load_species`, `role_color`/`Theme`/`StateStyle`, `Pet`/`priority`/`z_priority`/`frame_index`/`advance`, `Herd`/`Rng`/`Lcg`/`reconcile`/`step`/`visible_and_hidden`, `PixelBuf`/`draw_pixels`/`draw_herd`/`PET_PX_H`, `SocketClient`/`RealSocket`/`subscribe_request`, `Clock`/`RealClock`/`drain_events`/`watch`, `render::run` — names and signatures are consistent across the Producer/Consumer blocks and their uses in later tasks. `AgentStatus` gains `PartialOrd, Ord` (Task 4) for the `BTreeMap` key.
