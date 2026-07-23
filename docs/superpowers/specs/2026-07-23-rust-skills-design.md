# Rust skills for agents (design)

**Date:** 2026-07-23
**Status:** approved, pre-implementation
**Resolves against:** [`CLAUDE.md`](../../../CLAUDE.md), the codebase conventions

## 1. Goal & exit criteria

Give every agent working in this repo a small set of **project-tuned Rust
skills** so the Rust they write matches the patterns already established here,
without adding heavyweight machinery.

**Exit criteria:**
- Five `SKILL.md` files exist under `.claude/skills/<name>/`, each with valid
  frontmatter (`name`, `description`) and content grounded in this repo's code.
- Every skill is auto-discoverable: it appears in the Skill tool listing for a
  fresh agent in this repo (no hooks or router required).
- Each `description` is a sharp "Use when …" trigger so the right skill surfaces
  for the right task.
- `CLAUDE.md` gains a short "Rust skills" pointer section.

**Explicitly out of scope:** the actionbook keyword-hook system (~400 triggers),
a router skill, the three-layer "cognitive" taxonomy, domain extensions, and any
external plugin dependency. We deliberately keep only the flat, checked-in skill
files.

## 2. Approach & rationale

Chosen approach: **a lean, curated set authored against this project's code**,
seeded by the best ideas from
[`actionbook/rust-skills`](https://github.com/actionbook/rust-skills) but
dropping its hook/router/plugin machinery.

Rejected alternatives:
- *Install actionbook as a plugin* — comprehensive but heavy for a single-binary
  CLI; pulls in an external dependency and a keyword-hook system that would fire
  noisily on a small codebase.
- *Vendor a subset of actionbook as-is* — more content but generic; it would not
  reflect this repo's signature patterns (trait seams, tolerant serde, snapshot
  TUI tests), which is the whole point.

Why checked-in and project-scoped: multiple agents collaborate on this repo via
handoffs. Committing the skills to `.claude/skills/` means every agent — current
and future — inherits the same conventions, versioned alongside the code they
describe.

## 3. The five skills

Each skill lives at `.claude/skills/<name>/SKILL.md`. All are grounded in
concrete code cited from `src/`.

1. **`rust-error-handling`** — `Result` + `?` propagation, `io::Error::other`
   for adapter failures, degrade-at-the-UI-boundary
   (`.ok().and_then(...).unwrap_or_default()` in the render loop), and the rule
   that `unwrap`/`expect` appear only in tests. Grounds on `src/herdr.rs`,
   `src/render.rs`.
2. **`rust-testability-seams`** — the trait + `Real`/`Fake` dependency-injection
   pattern: an outer boundary trait the app consumes (`&dyn HerdrCli`), an inner
   seam (`CommandRunner`) with a `RealRunner` for production and a `Fake` for
   tests, so tests never spawn a process. Grounds on `src/herdr.rs`.
3. **`rust-serde-tolerant-parsing`** — deserializing external CLI JSON:
   envelope-unwrapping structs, `#[serde(rename_all = "lowercase")]`,
   a `#[serde(other)]` catch-all variant, `#[serde(default)]` for optional
   fields, and pushing tolerance into the type. Grounds on `src/agent.rs`.
4. **`rust-tui-snapshot-testing`** — testing ratatui output with `insta` +
   `TestBackend`, keeping renders deterministic (e.g. `status_glyph`), and
   restoring the terminal on exit in the live loop. Grounds on `src/render.rs`.
5. **`rust-project-conventions`** — the connective tissue: `//!`/`///` docs that
   explain *why*, test names written as full sentences, `#[cfg(test)] mod tests`
   with `include_str!` fixtures, edition 2024 / `rust-version` discipline, and a
   minimal-dependencies bias.

## 4. Skill file shape

Each `SKILL.md` follows the standard skill format:

```markdown
---
name: <kebab-case-name>
description: Use when <trigger> — <what it gives you>.
---

# <Title>

## When to use
...

## The pattern
<short prose + a code snippet drawn from this repo>

## Rules
- ...

## Anti-patterns
- ...
```

Content principles:
- **Short and high-signal.** Each skill fits on a screen or two; it points at the
  real file (`src/herdr.rs:11`) rather than restating everything.
- **Show, don't lecture.** One representative snippet per skill, lifted or
  distilled from actual code.
- **Prescriptive.** Rules and anti-patterns, not a survey of options.

## 5. Discovery mechanism

Claude Code auto-lists every `.claude/skills/*/SKILL.md` in the Skill tool for
any agent operating in this repo. No hook, router, or manifest entry is needed.
Discovery quality is therefore entirely a function of the `description` field —
each is written as a "Use when …" trigger.

`CLAUDE.md` gains a short **"## Rust skills"** section pointing at the set, added
as an append to minimize conflict with the concurrently-running agent on this
branch.

## 6. Constraints & guardrails

- **New files only** for the skills themselves (`.claude/skills/**`), so no
  collision with the other agent live on this branch.
- **`CLAUDE.md`** is edited by appending a section, not rewriting existing
  content.
- **No commit without the user asking** (per `CLAUDE.md`): work is proposed as a
  checkpoint commit; the user controls when it lands.
