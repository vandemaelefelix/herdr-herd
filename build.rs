//! Captures a short git-commit marker for `dev-build-marker` (see Cargo.toml)
//! at compile time, so it's baked into the binary rather than read at
//! runtime — the whole point is telling apart already-running processes.
//! Always runs (cheap), regardless of whether the feature is enabled; the
//! env var it sets is simply unread when the feature is off.

use std::process::Command;

fn main() {
    let sha = Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);

    let marker = if dirty { format!("{sha}-dirty") } else { sha };
    println!("cargo:rustc-env=HERDR_PETS_BUILD_SHA={marker}");
    println!("cargo:rerun-if-changed=.git/HEAD");
}
