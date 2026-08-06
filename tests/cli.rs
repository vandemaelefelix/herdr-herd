//! CLI smoke: --version prints the crate version; unknown args fail with usage.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_herdr-herd"))
}

#[test]
fn version_flag_prints_version_and_succeeds() {
    let out = bin().arg("--version").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")), "got: {stdout}");
}

/// The strip shows the marker, but `--version` is what you reach for from a
/// shell, so it has to answer the same "which build is this?" question.
#[test]
#[cfg(feature = "dev-marker")]
fn version_flag_names_the_build_in_a_dev_build() {
    let marker = herdr_herd::marker::build_marker().expect("a dev build has a marker");
    let out = bin().arg("--version").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(marker), "got: {stdout}");
}

#[test]
fn no_subcommand_fails_with_usage() {
    let out = bin().output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("usage"), "got: {stderr}");
}
