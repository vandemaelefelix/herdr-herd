//! Stamps the build identity into `HERDR_HERD_BUILD` for the dev marker.
//!
//! Deliberately emits **no** `cargo:rerun-if-changed` directives: with none
//! present, Cargo reruns this script whenever any file in the package changes,
//! which is exactly the freshness the marker needs. Two dev builds of the same
//! commit still differ by their timestamp.
//!
//! Every lookup degrades to a placeholder rather than failing the build, so a
//! source tarball with no `.git` still compiles.

use std::process::Command;

fn main() {
    println!("cargo:rustc-env=HERDR_HERD_BUILD={}", stamp());
}

/// `<short-sha>[*] <HH:MM:SS>` — `*` marks an uncommitted working tree, so a
/// marker never claims a clean commit that does not match what is running.
/// (`*` rather than `+`, which the overflow counter already owns in the same
/// overlay lane.)
fn stamp() -> String {
    let sha = run("git", &["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "nogit".to_string());
    let dirty = run("git", &["status", "--porcelain"])
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let time = run("date", &["+%H:%M:%S"]).unwrap_or_else(|| "??:??:??".to_string());
    let flag = if dirty { "*" } else { "" };
    format!("{sha}{flag} {time}")
}

/// Trimmed stdout of a successful command, or `None` on any failure.
fn run(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}
