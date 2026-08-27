---
name: rust-error-handling
description: Use when writing or reviewing Rust that can fail — I/O, subprocess calls, parsing, or any fallible function in this repo. Gives the project's error-handling rules: Result + `?`, io::Error::other, degrade at the UI boundary, no unwrap/expect outside tests.
---

# Rust error handling (herdr-pets conventions)

## When to use

Any function that can fail: shelling out to `herdr`, reading env, parsing JSON,
driving the terminal. Reach for this before writing a signature that returns a
value the caller can't trust, or before adding an `unwrap()`.

## The rules

1. **Fallible functions return `Result`.** Use the narrowest error type that
   fits — `io::Result<T>` for I/O and process work, `Result<T, serde_json::Error>`
   for parse-only functions. Do not reach for `anyhow`/`thiserror`; this crate
   stays on `std` errors.
2. **Propagate with `?`, don't handle early.** Let the boundary decide.
3. **Wrap opaque adapter failures with `io::Error::other`.** A non-zero exit or
   an unexpected shape becomes `io::Error::other("herdr exited non-zero")` — a
   message, not a panic.
4. **Degrade at the UI boundary.** The render loop must never crash the pane. A
   failed fetch or parse collapses to an empty herd, not an abort.
5. **`unwrap` / `expect` live only in `#[cfg(test)]`.** In tests they document
   an invariant (`expect("valid fixture")`). In production code they are a bug.
6. **Never `eprintln!` from the render process.** `render::run_loop` and
   anything running in its process (including the watcher thread spawned by
   `watcher::watch`) execute after raw mode + the alternate screen are
   entered — stderr shares the live TUI's tty, so a diagnostic printed there
   paints over the strip, stair-steps (raw mode sends a bare LF), and on the
   fatal path is discarded by `LeaveAlternateScreen` before anyone reads it
   (issue #84). `eprintln!` is fine in `main.rs` and `src/control.rs`: both
   run with normal stdio, outside any raw-mode guard. From inside the render
   process, latch degraded-boundary failures into [`crate::diag::Diagnostic`]
   instead — `run_loop` reads it every frame and draws it through the strip's
   own caption lane, the same one the hover name uses.

## The pattern

Propagate inside the adapter (`src/herdr.rs:50`):

```rust
fn run_json(&self, args: &[&str]) -> io::Result<String> {
    let out = self.runner.run(&self.program, args)?;   // propagate I/O error
    if !out.status.success() {
        return Err(io::Error::other("herdr exited non-zero"));  // opaque -> message
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
```

Degrade at the boundary (`src/render.rs:68`) — three fallible steps, one safe
fallback:

```rust
let agents = herdr
    .run_json(&["agent", "list"])
    .ok()
    .and_then(|s| parse_agent_list(&s).ok())
    .unwrap_or_default();   // no herd is a valid frame, not a crash
```

## Anti-patterns

- `let s = herdr.run_json(...).unwrap();` in the render loop — one hiccup from
  `herdr` kills the user's pane.
- Inventing an error enum + `thiserror` for a function that only ever fails one
  way. Match the existing `std`-error style first.
- Swallowing an error silently where the caller *could* act on it. Only degrade
  to a default at a genuine boundary (the draw loop), not in the middle of logic.
- `eprintln!("...")` anywhere reachable from `render::run` or `watcher::watch`
  — see rule 6. Use `diag.set(...)` there instead.
