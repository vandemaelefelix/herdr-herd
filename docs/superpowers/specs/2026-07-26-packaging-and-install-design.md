# Packaging & install — ship herdr-pets as an easy-to-install plugin (design)

**Date:** 2026-07-26
**Status:** approved (brainstorming), pre-implementation
**Resolves against:** [`GOAL.md`](../../../GOAL.md) — "Universal first", "Opinionated
defaults, few knobs", and the Phase 4 exit criterion *"a tagged release installable
via `herdr plugin install`, with docs."*

## 1. Goal & exit criteria

All five phases are implemented. What remains is making the plugin **genuinely
easy for other people to install** and giving the README **clear, present-tense
install instructions**. This closes the packaging/release item that Phase 4
deliberately deferred (see `docs/decisions.md`).

The blocker today: `herdr plugin install vandemaelefelix/herdr-pets` works, but
its `[[build]]` step runs `cargo build --release`, so **every installer needs a
Rust toolchain**. Many herdr users won't have `cargo`, so a source-only install
just fails for them.

**Exit criteria:**
- Installing on a supported platform requires **no Rust toolchain**: a prebuilt
  binary is downloaded from a GitHub Release.
- On any unsupported platform/arch, install **still succeeds** by falling back to
  `cargo build --release` (today's behaviour) — never a hard failure.
- A tagged GitHub Release (`v0.1.0`) exists with binaries attached, produced by a
  reproducible CI workflow (not hand-built).
- `README.md` leads with a single-line install command, states "no Rust needed"
  and the herdr ≥ 0.7.0 requirement, has a Quickstart, and no longer claims
  "in design" or "License: TBD".
- The existing PR gate (`ci.yml`: fmt/clippy/test) is unchanged and still green.

## 2. Scope (and deliberate non-goals)

**In scope:** a release workflow, a fetch-or-build `scripts/build.sh`, cutting
`v0.1.0`, a README refresh, and a manifest tidy.

**Non-goals (unchanged from GOAL.md / Phase 4):** no new config knobs, no new
features, no Kitty-sprite work, no `scope`/palette customization. This is
packaging only. A README screenshot/GIF is desirable but **out of scope for
automation** — it requires a live terminal capture the maintainer supplies; the
README keeps a clearly-marked image slot for it.

## 3. Chosen approach: fetch prebuilt, source-build fallback

Rejected alternatives:
- **Source build only** — simplest, zero new infra, but fails the core goal:
  every installer needs Rust.
- **Prebuilt only, no fallback** — drops the free robustness of the fallback;
  hard-fails on any target we didn't publish. Strictly worse than fetch-or-build.

Fetch-or-build satisfies "no toolchain for the common case" **and** "never
hard-fails", from one design. The only cost is a one-time, standard release
workflow.

## 4. Components

### 4.1 Release workflow — `.github/workflows/release.yml` (new)

- **Trigger:** push of a tag matching `v*` (e.g. `v0.1.0`).
- **Matrix** — four targets, each cross-compiled and its raw binary uploaded to
  the Release for the tag:

  | Target triple | Runner | Notes |
  |---|---|---|
  | `aarch64-apple-darwin` | `macos-latest` (arm64) | native |
  | `x86_64-apple-darwin` | `macos-latest` | `--target` cross, same host |
  | `x86_64-unknown-linux-gnu` | `ubuntu-latest` | native |
  | `aarch64-unknown-linux-gnu` | `ubuntu-latest` | via `cross` (or arm runner) |

- **Asset naming:** `herdr-pets-<target-triple>` (raw executable, no archive) so
  `build.sh` can construct the URL deterministically. Example:
  `herdr-pets-aarch64-apple-darwin`.
- **Publish:** attach all four assets to the GitHub Release for the tag (create
  the release if it doesn't exist). Toolchain pinned to the repo's Rust version
  (1.96), matching `ci.yml`.
- `ci.yml` (fmt/clippy/test on push/PR) is **untouched** — it remains the gate;
  `release.yml` is a separate, tag-only job.

### 4.2 `scripts/build.sh` (rewrite: fetch-or-build)

Invoked by herdr's `[[build]]` step from the freshly-checked-out plugin dir. New
flow:

1. Resolve the version string from `herdr-plugin.toml` (`version = "X.Y.Z"`),
   the single source of truth for which Release to fetch. Because herdr checks
   out the plugin at a ref, the `build.sh` and the `version` it reads always
   belong to the same tag.
2. Detect platform via `uname -s` / `uname -m` and map to a target triple:
   - `Darwin`+`arm64` → `aarch64-apple-darwin`
   - `Darwin`+`x86_64` → `x86_64-apple-darwin`
   - `Linux`+`x86_64` → `x86_64-unknown-linux-gnu`
   - `Linux`+`aarch64`/`arm64` → `aarch64-unknown-linux-gnu`
   - anything else → **skip to fallback**.
3. Download
   `https://github.com/vandemaelefelix/herdr-pets/releases/download/v<version>/herdr-pets-<target>`
   with `curl -fsSL` (fail on HTTP error) to a temp path, `chmod +x`, and move it
   to `target/release/herdr-pets` — exactly where the manifest's pane/action
   commands already point.
4. **Fallback:** if the target is unknown, `curl` fails, or the downloaded file
   isn't a runnable executable, fall through to `cargo build --release` (keeping
   the existing `[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"` guard for
   GUI/login-less launches).
5. Portability: `#!/bin/sh` POSIX only (no bash-isms); rely on `uname`, `curl`,
   `mkdir`, `chmod`, `mv` — all present on stock macOS/Linux. Emit a one-line
   note to stderr saying whether it fetched or built, so `herdr plugin log` shows
   which path ran.

**Failure semantics:** a prebuilt fetch that *partially* succeeds (e.g. truncated
download) must not shadow a working build — verify the binary before trusting it,
else fall back. The script exits non-zero only if **both** fetch and build fail.

### 4.3 Manifest tidy — `herdr-plugin.toml`

- Remove stale "Phase 0 / Phases 2-3" comments.
- Keep `id`/`name`/`version`/`description`/`min_herdr_version`/`platforms`.
- Keep the `[[build]]` step pointing at `scripts/build.sh` (now fetch-or-build).
- Keep the `pets` pane (manual `herdr plugin pane open`) and both actions
  (`place-pets`, `start-pets-controller`), with accurate present-tense titles.
- `version` stays `0.1.0` and is the value `build.sh` reads.

### 4.4 README refresh

- Delete the "Status: **in design**" blockquote.
- **Install** section rewritten:
  - Lead: `herdr plugin install vandemaelefelix/herdr-pets`.
  - State: no Rust toolchain needed on macOS/Linux (prebuilt); source-build
    fallback otherwise; requires **herdr ≥ 0.7.0**.
  - Dev path: `herdr plugin link .` from a checkout.
- **Quickstart** (new, short): after install, run the `start-pets-controller`
  action once per session to get strips in every eligible tab (the controller has
  no auto-start hook — documented limit). Mention `place-pets` for a one-off
  strip.
- **License** section: state **MIT** (the `LICENSE` file already is MIT); drop
  "TBD".
- Keep the existing How-it-works / Configuration / Rendering / Notification-sound
  sections as-is (they're accurate); leave the commented image slot for a
  maintainer-supplied screenshot.

### 4.5 Cut the release (`v0.1.0`)

After the PR merges to `main`: tag `v0.1.0` and push it, which fires
`release.yml` and produces the installable Release. This tag/push is a **manual
maintainer step** (the repo's convention is no autonomous commits/pushes; the
harness also blocks autonomous merges) — the spec/plan documents the exact
commands, but the human runs them.

## 5. Data flow (install on an end-user machine)

```
herdr plugin install vandemaelefelix/herdr-pets
   └─ herdr clones repo @ default branch (or --ref vX.Y.Z)
        └─ runs [[build]] → scripts/build.sh
             ├─ read version from herdr-plugin.toml
             ├─ detect target triple
             ├─ curl release asset  ──success──▶ chmod +x → target/release/herdr-pets
             └─ (unknown target / curl fail / bad file) ──▶ cargo build --release
   └─ manifest panes/actions run ./target/release/herdr-pets {render|place|control}
```

herdr stays the single source of truth at runtime; packaging changes nothing
about how the binary talks to the socket.

## 6. Error handling

- Follows the repo's rule (see `.claude/skills/rust-error-handling`): degrade at
  the boundary, never crash the install. `build.sh` treats every failure of the
  preferred path as a reason to try the next, and only errors out when nothing
  works.
- A missing/renamed Release asset degrades to source build (so a maintainer who
  forgets to attach one target still ends up with a working install where Rust is
  present).
- No secrets, no network writes; the only network read is an unauthenticated
  GitHub Releases download over HTTPS.

## 7. Testing & verification

- **`build.sh` target-mapping** — the risky logic is the `uname → triple` map and
  the fetch/fallback decision. Factor that into a shell function or keep it
  linear but exercise it: a lightweight test (shell or a Rust `tests/` harness
  invoking the script with a stubbed `curl`/`PATH`) asserts each `uname` pair maps
  to the right triple and that an unreachable URL falls back to build. Match the
  repo's existing `tests/` style where practical; if a full seam is
  disproportionate, at minimum a `--print-target` dry-run mode covered by one
  test.
- **Manifest still parses** — the repo already has `tests/manifest.rs`; ensure it
  still passes after the tidy (extend it only if the tidy changes parsed fields).
- **Workflow** — validate `release.yml` builds all four targets on a throwaway
  pre-release tag (e.g. `v0.1.0-rc.1`) before cutting `v0.1.0`, so a broken matrix
  is caught off the real tag. This is a maintainer verification step.
- **End-to-end** — on the maintainer's macOS: after the release is cut,
  `herdr plugin install vandemaelefelix/herdr-pets` in a scratch context pulls the
  prebuilt binary (confirm via `herdr plugin log`: "fetched", not "built") and the
  strip renders. Documented as a manual post-release check.
- Existing gate (`cargo fmt --check` + `clippy -D warnings` + `cargo test`) stays
  green throughout.

## 8. Open risks / flagged, not blocking

- **Linux arm64** needs `cross` or an arm runner — one extra moving part. If it's
  fragile in CI, drop that one matrix target; `aarch64` Linux users then hit the
  source-build fallback (still works where Rust is present). Decision recorded so
  it's a conscious trade, not a silent gap.
- **Screenshot/GIF** — out of scope to automate; README keeps a marked slot for a
  maintainer capture.
- **Version drift** — `build.sh` reads the version from `herdr-plugin.toml`; that
  file, `Cargo.toml`, and the git tag must agree at release time. The plan calls
  this out as a single pre-tag checklist item.
