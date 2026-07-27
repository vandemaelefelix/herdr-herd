# herdr-herd — project conventions

A herdr plugin that gives every agent a pixel-art sheep. See [`GOAL.md`](GOAL.md)
(north star + locked decisions), [`docs/PLAN.md`](docs/PLAN.md) (phase roadmap),
and per-phase specs in `docs/superpowers/specs/`.

## Git conventions

### Branch names

`<type>/<short-kebab-description>` — e.g. `feat/phase-0-foundations`,
`chore/ci-setup`, `docs/readme-tidy`.

Types: `feat`, `fix`, `chore`, `docs`, `refactor`, `test`, `perf`, `ci`.

Always branch off `main`; never commit directly to `main`.

### Commit messages (Conventional Commits)

`<type>(<optional-scope>): <description>` — e.g.
`docs(phase-0): add foundations & spikes design spec`,
`feat(render): draw agent list in the herd pane`,
`chore: scaffold cargo project`.

- Same type vocabulary as branches, plus `build`, `style`, `revert`.
- Imperative mood, lower-case description, no trailing period.
- Scope is optional; use the phase or module when it clarifies
  (`phase-0`, `render`, `herdr`, `manifest`).
- Breaking changes: `type!: …` or a `BREAKING CHANGE:` footer.

### Committing & pushing

- **Do not commit or push without the user asking.** Propose commits; the user
  controls when work lands and when it is pushed.
- Local checkpoint commits on a feature branch are fine to propose as we go.

## Rust skills

Project-tuned Rust skills live in `.claude/skills/` and are auto-listed in the
Skill tool. Use the relevant one before writing Rust here, so new code matches
the patterns already in `src/`:

- `rust-error-handling` — `Result`/`?`, `io::Error::other`, degrade at the UI
  boundary, no `unwrap`/`expect` outside tests.
- `rust-testability-seams` — the trait + `Real`/`Fake` dependency-injection
  pattern so tests never touch the real world.
- `rust-serde-tolerant-parsing` — deserializing external CLI JSON (envelopes,
  `rename_all`, `#[serde(other)]`, `#[serde(default)]`).
- `rust-tui-snapshot-testing` — ratatui + `insta` + `TestBackend` snapshots.
- `rust-project-conventions` — doc-comment style, sentence-style test names,
  toolchain/dependency discipline.
