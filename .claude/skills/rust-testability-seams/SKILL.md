---
name: rust-testability-seams
description: Use when code touches the outside world — spawning processes, reading env, hitting the network or clock — and you want it testable. Gives this repo's trait + Real/Fake dependency-injection pattern so tests never touch the real world.
---

# Rust testability seams (herdr-pets conventions)

## When to use

Whenever a unit would otherwise call `std::process::Command`, `std::env`, a
socket, or the filesystem directly. Introduce a seam *before* writing the logic,
so the logic is testable from the first line.

## The pattern

Two layers of trait, each with a real impl and a test double. This is the
project's signature pattern — see `src/herdr.rs`.

1. **Outer boundary trait** — what the app consumes, referenced as `&dyn Trait`
   so callers are decoupled from the concrete adapter:

```rust
pub trait HerdrCli {
    fn run_json(&self, args: &[&str]) -> io::Result<String>;
}
```

2. **Inner seam trait** — isolates the one unavoidable side effect (spawning),
   kept generic (`<R: CommandRunner>`) so it stays zero-cost in production:

```rust
pub trait CommandRunner {
    fn run(&self, program: &OsStr, args: &[&str]) -> io::Result<Output>;
}

pub struct RealRunner;                     // production: real Command
impl CommandRunner for RealRunner { /* Command::new(program).args(args).output() */ }

pub struct LiveHerdr<R: CommandRunner = RealRunner> { program: OsString, runner: R }
```

3. **Fake in tests** — implement the *inner* seam, drive the *outer* behaviour,
   and assert without spawning anything (`src/herdr.rs:74`):

```rust
struct Fake { stdout: String, raw_status: i32 }
impl CommandRunner for Fake {
    fn run(&self, _p: &OsStr, _a: &[&str]) -> io::Result<Output> {
        Ok(Output { status: ExitStatus::from_raw(self.raw_status),
                    stdout: self.stdout.clone().into_bytes(), stderr: Vec::new() })
    }
}

#[test]
fn run_json_errors_on_nonzero_exit() {
    let h = LiveHerdr::with_runner("herdr", Fake { stdout: String::new(), raw_status: 256 });
    assert!(h.run_json(&["agent", "list"]).is_err());
}
```

## Rules

- **Seam at the lowest level.** Fake the process runner, not the whole app, so
  tests exercise real parsing/branching logic and only stub the side effect.
- **App code depends on the trait, not the struct.** Functions take
  `herdr: &dyn HerdrCli` (see `render::run`), never `LiveHerdr` directly.
- **Provide a `with_runner`-style constructor** so tests inject the fake and a
  `from_env`-style one for production wiring.
- **Generic inner seam, `dyn` outer boundary.** Generics keep the hot path
  monomorphized; `dyn` at the boundary keeps signatures simple.

## Anti-patterns

- Calling `Command::new(...)` directly inside logic you want to test — now the
  test needs a real `herdr` on `PATH`.
- Reading `std::env::var` deep in a function instead of resolving it once at a
  seam (see `resolve_program`) and passing the value in.
- A test double that reimplements business logic. A double returns canned data;
  the logic under test stays real.
