# Report: startup and shutdown reliability

Branch `fix/terminal-lifecycle`, two commits, not pushed, no PR.

`cargo test` 268 passed, `cargo clippy --all-targets -- -D warnings` clean,
`cargo fmt --check` clean.

## #27: kitty probe can hang forever

**Commit** `87865c9 fix(caps): bound the kitty probe so a silent terminal cannot hang the strip`

The fix is the shape the issue prescribed. `RealCaps::supports_kitty_graphics`
now spawns a worker thread that runs the blocking probe and waits on it with
`rx.recv_timeout(timeout + 100ms).unwrap_or(false)`. The read itself is
untouched and still blocking, per the warning in the old doc comment.

I also gave the probe a real seam, since it had none:

- `ProbeIo` (`write_query` + `read`) isolates the tty round-trip. `read` is
  documented as allowed to block forever, which is what a raw-mode stdin does.
- `TtyProbeIo` is the production impl (stdout out, stdin in). The
  `crossterm::event::poll` warning moved onto it, since that is where it applies.
- `RealCaps<I: ProbeIo = TtyProbeIo>` with `RealCaps::with_io(io, timeout)` for
  tests. `render::run`'s call site is unchanged (`RealCaps::new()`).
- The free function `probe(io, id, timeout)` holds the read loop; `RealCaps` only
  owns the bounding.

Tests added (`src/caps.rs`):

| Test | Property pinned |
| --- | --- |
| `probe_reports_unsupported_when_the_terminal_never_answers` | A stdin that never returns from `read` yields `false`, and does so in under 2s. This is the hard requirement. |
| `probe_confirms_support_when_the_kitty_reply_arrives` | The bounded path still reports `true` on a real kitty reply, i.e. the fix did not break detection. |
| `probe_reports_unsupported_when_only_the_device_attributes_reply_arrives` | The herdr-flag-off case: DA terminator ends the read immediately, answer is `false`. |
| `probe_reports_unsupported_when_the_query_cannot_be_written` | A failed stdout write short-circuits to `false` without reading. |

All four go through `probe_within`, which runs the call on its own thread and
`recv_timeout`s at 5s, so a regression fails the test instead of wedging
`cargo test`. Verified by mutation: reverting `recv_timeout` to `recv` makes
`probe_reports_unsupported_when_the_terminal_never_answers` fail in 5.02s.

The existing `fake_reports_configured_support` test is kept as-is. It only
exercises the test double, as the issue notes, but it is not wrong and the brief
says not to delete tests.

### Caveat worth knowing

The abandoned worker thread is detached and still owns stdin. If the terminal is
truly silent, it stays parked in `read` until *one* more byte arrives, notices
the deadline has passed, and exits. That one byte is consumed by the worker, not
by the event loop, so in the pathological case the user's first keypress after
startup can be swallowed. That is inherent to bounding a blocking read without
`O_NONBLOCK`, and it is a far better failure than a permanently blank pane. It is
documented at the call site. I did not try to make it airtight.

## #35: raw mode left on, no panic hook

**Commit** `45d21b4 fix(render): restore the terminal on every exit path, including panics`

New module `src/term.rs` (wired in `lib.rs`), because terminal lifecycle is its
own job rather than more surface on `render`.

- `TerminalControl` seam: `enable_raw` / `disable_raw` / `enter_screen` /
  `leave_screen`. `CrosstermControl` is the production impl; `leave_screen`
  bundles `LeaveAlternateScreen`, `DisableMouseCapture` and `Show`.
- `TerminalGuard<C = CrosstermControl>` tracks which of raw mode / alternate
  screen it actually entered and undoes them in `restore()`, which is idempotent
  and also called from `Drop`. Raw mode is restored first (same order as the old
  straight-line teardown), and the screen is restored even when disabling raw
  mode fails.
- `enter_screen` marks the state entered *before* calling through, because
  `execute!` applies its commands in order and can fail with the alternate screen
  already on.
- `install_panic_hook()` chains onto the previous hook behind a `Once`. It calls
  `restore_terminal_best_effort()`, which unconditionally disables raw mode,
  writes `kitty::delete_all()` and leaves the screen. The hook exists on top of
  `Drop` for two reasons: the default hook prints *before* unwinding drops the
  guard, so without it the panic message goes into the alternate screen and
  vanishes with it; and it covers panics outside a guard's reach.

`render::run` now installs the hook, drives the guard, and ends with
`result.and(guard.restore())`. That is the fix for the discarded `run_loop`
result: the loop's error wins, a restore error is reported only when the loop
succeeded.

Tests added (`src/term.rs`, all against a recording fake, no tty):

| Test | Property pinned |
| --- | --- |
| `guard_restores_raw_mode_when_entering_the_alternate_screen_fails` | The issue's exact repro shape: stdout fails after raw mode is on, and raw mode still comes off. |
| `guard_restores_on_unwind` | `catch_unwind` around a panicking closure holding a guard: `disable_raw` and `leave_screen` both run. |
| `guard_restores_the_screen_even_when_leaving_raw_mode_fails` | One failing restore step does not skip the other, and the error is still returned. |
| `guard_restores_only_what_was_entered` | Entering only raw mode restores only raw mode. |
| `failing_to_enable_raw_mode_leaves_nothing_to_restore` | No spurious `disable_raw` when `enable_raw` failed. |
| `a_guard_that_entered_nothing_restores_nothing` | Drop of an untouched guard is silent. |
| `restoring_twice_restores_once` | Explicit `restore()` then `Drop` does not double-restore. |

Verified by mutation: removing the body of `Drop` fails three of these.

## What the issues got wrong

Nothing material. Two small notes:

- #35 says a `Drop` guard plus a panic hook "covers the `?` paths and the panic
  path in one move". Strictly, `Drop` alone covers both; the hook's real job is
  ordering, getting the terminal back *before* the default hook prints, so the
  message survives. Implemented both, for that reason.
- #27's snippet passes `id` and `timeout` into `probe` but does not mention the
  seam. The seam is the part that made it testable, and it was the larger change.

## What is unproven

- **No live terminal check was run.** The brief's manual check
  (`cargo run --release -- render > /dev/full`) is not available: `/dev/full`
  does not exist on Darwin, and I was not going to exercise the render path
  against your working herdr session to improvise a substitute. So the real
  `CrosstermControl` path, the panic hook against a real tty, and the kitty
  `delete_all` on the panic path are all covered only by reading, not by running.
  Worth one manual pass on Linux, or by panicking the strip on purpose from a
  plain tty tab.
- **The probe's real stdin behaviour is unchanged and untested against a real
  terminal.** The tests prove the *bound* holds; they say nothing about whether
  a real Ghostty/kitty still answers within 150ms. Regression risk there is low
  since the read loop is byte-for-byte the same.
- **The swallowed-keypress caveat above is reasoned, not measured.** I have not
  reproduced a terminal that goes silent and then sends a keypress.
- **Signal deaths still leak.** SIGTERM/SIGINT-as-signal and SIGPIPE bypass both
  `Drop` and the panic hook. `run_loop` handles Ctrl-C as a key event in raw
  mode, so the common case is fine, but `kill` on the render process still leaves
  the terminal dirty. Out of scope for #35, which is about `?` paths and panics;
  a signal handler would be a separate change.
- **`control.rs`'s liveness check is untouched.** #27 notes that
  `renderer_is_running` counted a hung process as healthy. With the probe bounded
  there is no hang to detect, but the controller still cannot tell "alive" from
  "alive and useless". That is a different fix and was not in scope.
