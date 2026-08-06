# Dev test harness — implementation plan

Design: [2026-08-06-dev-test-harness-design.md](../specs/2026-08-06-dev-test-harness-design.md)

Test-driven throughout: each task names its failing test first.

## Task 1 — Build stamp and feature flag

**Test:** `src/marker.rs` unit tests.
- Without `dev-marker`: `build_marker()` is `None` and `reserved_cols()` is `0`.
- With `dev-marker`: `build_marker()` is `Some`, non-empty, single-line, and
  contains the crate version; `reserved_cols()` is greater than `0` and matches
  the rendered text width plus its margin.

**Code:**
- `Cargo.toml`: `[features] dev-marker = []`.
- `build.rs`: resolve short sha (`git rev-parse --short HEAD`), dirty flag
  (`git status --porcelain`), and `date +%H:%M:%S`; emit
  `cargo:rustc-env=HERDR_HERD_BUILD=...`. Emit no `rerun-if-changed`. Degrade to
  a placeholder on any failure.
- `src/marker.rs`: `build_marker()`, `reserved_cols()`, wired into `lib.rs`.
- `main.rs`: `--version` appends the stamp when the feature is on.

**Gate:** `cargo test` and `cargo test --features dev-marker` both green.

## Task 2 — Draw the marker

**Test (half-block, `src/render.rs`):**
- A caption long enough to reach the left edge is truncated at
  `reserved_cols()`, leaving the marker columns untouched.
- Existing snapshots are unchanged without the feature.

**Test (kitty, `src/kitty_render.rs`):**
- With the feature, `draw` emits a cursor-positioning escape at column 1 of the
  overlay lane carrying the marker text.
- Without the feature, the emitted escapes are byte-identical to today.

**Code:** marker drawing in `draw_overlay_text` and the half-block lane path;
caption width reduced by `reserved_cols()` in both.

**Gate:** full suite green under both feature settings; snapshots unchanged in
the default build.

## Task 3 — Config dir override

**Test:** `src/config.rs` — `HERDR_HERD_CONFIG_DIR` set returns that path
without shelling out to herdr; unset falls through to the existing resolution.

**Code:** check the env var first in `resolve_config_dir`. Keep the herdr CLI
seam behind it so the existing tests still cover the fallback.

## Task 4 — `scripts/herd-test.sh`

**Test:** `tests/herd_test_script.rs`, mirroring `tests/build_script.rs`'s
dry-run pattern. A `--print-plan` flag prints the resolved session name, socket
lookup command, and controller argv without launching anything, so the ordering
logic is verified without a live herdr server.
- The script builds with `--features dev-marker`.
- It discovers the socket via `herdr session list --json`, never a literal path.
- The controller is started with `HERDR_SOCKET_PATH` set to the discovered
  socket.
- It fails with a clear message when `jq` is absent.

**Code:** the script per the design, plus a `herd-test` alias suggestion in the
docs.

## Task 5 — Docs

- README: a short "Testing changes" section pointing at the script and
  explaining how to read the marker.
- `docs/decisions.md`: record the feature-flag and no-second-plugin-id calls.

## Out of scope

- Releasing the three unreleased fixes on `main`. Separate change; noted so it
  is not forgotten.
