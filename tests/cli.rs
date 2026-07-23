//! CLI smoke: --version prints the crate version; unknown args fail with usage.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_herdr-pets"))
}

#[test]
fn version_flag_prints_version_and_succeeds() {
    let out = bin().arg("--version").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")), "got: {stdout}");
}

#[test]
fn no_subcommand_fails_with_usage() {
    let out = bin().output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("usage"), "got: {stderr}");
}
