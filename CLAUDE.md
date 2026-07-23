# herdr-pets — project conventions

A herdr plugin that gives every agent a pixel-art pet. See [`GOAL.md`](GOAL.md)
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
`feat(render): draw agent list in the pets pane`,
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
