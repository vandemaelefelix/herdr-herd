---
name: rust-project-conventions
description: Use when writing any Rust in herdr-pets — starting a module, adding a function, or writing tests — and you want it to match house style. Gives doc-comment style, sentence-style test names, test layout, edition/toolchain discipline, and the minimal-dependency bias.
---

# herdr-pets Rust conventions

## When to use

Before adding a module, a public item, or a test. This is the connective tissue
the more specific Rust skills sit on top of ([[rust-error-handling]],
[[rust-testability-seams]], [[rust-serde-tolerant-parsing]],
[[rust-tui-snapshot-testing]]).

## Doc comments — say *why*

- Every module opens with a `//!` doc that states its job and any non-obvious
  constraint, e.g. *"unix-only here; platforms exclude Windows"* or *"Phase 0:
  … no animation — that is Phase 1."*
- Public items get a `///` doc. Prefer the reason or the contract over restating
  the signature: `/// Human label: prefer the user-set name, else the detected
  agent kind, else the stable pane_id.`
- Comment the surprising line inline (`// from_raw(256) => exit code 1 on unix`),
  not the obvious one.

## Tests — sentences, colocated, fixture-backed

- Name tests as full sentences describing the behaviour:
  `parses_statuses_including_unknown_and_blocked`,
  `label_prefers_name_then_agent_then_pane_id`,
  `run_json_errors_on_nonzero_exit`.
- Colocate unit tests in a `#[cfg(test)] mod tests` at the bottom of the file,
  `use super::*;`.
- Load fixtures with `include_str!(concat!(env!("CARGO_MANIFEST_DIR"),
  "/tests/fixtures/<file>"))` rather than hardcoding large literals; inline a
  small JSON string only for a one-off edge case.
- `expect("…")` in tests documents the invariant being assumed.

## Toolchain & dependencies

- **Edition 2024**, `rust-version = 1.96` — don't reach past what that pins.
- **Minimal dependencies.** The runtime deps are deliberate (`ratatui`,
  `crossterm`, `serde`, `serde_json`); dev-only tools stay in `[dev-dependencies]`
  (`insta`, `toml`). Justify any new crate against doing it with `std` first.
- Keep `unwrap`/`expect` out of non-test code (see [[rust-error-handling]]).

## Module boundaries

- One clear job per module: `agent` (model + parse), `herdr` (CLI seam),
  `render` (drawing + loop). New responsibilities get new modules wired in
  `src/lib.rs`, not bolted onto an existing one.
- Expose the useful type; keep helper/envelope structs private.

## Anti-patterns

- Restating the signature in a doc comment (`/// Gets the label.`) instead of the
  reasoning.
- Terse test names (`test_parse`, `test1`) — they don't say what broke.
- Adding a crate for something `std` already does cleanly.
- A module that has grown two unrelated jobs; split it.
