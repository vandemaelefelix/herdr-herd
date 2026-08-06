//! Dry-run checks for scripts/build.sh: the uname -> target-triple -> URL map.
//! `--print-url` resolves the download URL without touching the network, so the
//! risky platform-detection logic is verified deterministically here.

use std::process::Command;

/// Run `build.sh --print-url` with a faked platform and return the trimmed URL.
fn print_url(os: &str, arch: &str) -> String {
    let dir = env!("CARGO_MANIFEST_DIR");
    let out = Command::new("sh")
        .arg(format!("{dir}/scripts/build.sh"))
        .arg("--print-url")
        .env("HERDR_HERD_FAKE_UNAME_S", os)
        .env("HERDR_HERD_FAKE_UNAME_M", arch)
        .current_dir(dir)
        .output()
        .expect("build.sh should run under sh");
    assert!(out.status.success(), "build.sh --print-url should exit 0");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn macos_arm64_maps_to_the_apple_silicon_asset_at_the_manifest_version() {
    let url = print_url("Darwin", "arm64");
    assert!(
        url.ends_with("/herdr-herd-aarch64-apple-darwin"),
        "got: {url}"
    );
    // Ties the URL's version to the manifest so version drift fails the gate.
    assert!(url.contains("/releases/download/v0.2.0/"), "got: {url}");
}

#[test]
fn macos_intel_maps_to_the_x86_64_apple_asset() {
    let url = print_url("Darwin", "x86_64");
    assert!(
        url.ends_with("/herdr-herd-x86_64-apple-darwin"),
        "got: {url}"
    );
}

#[test]
fn linux_x86_64_maps_to_the_gnu_asset() {
    let url = print_url("Linux", "x86_64");
    assert!(
        url.ends_with("/herdr-herd-x86_64-unknown-linux-gnu"),
        "got: {url}"
    );
}

#[test]
fn linux_arm64_maps_to_the_aarch64_gnu_asset() {
    let url = print_url("Linux", "aarch64");
    assert!(
        url.ends_with("/herdr-herd-aarch64-unknown-linux-gnu"),
        "got: {url}"
    );
}

#[test]
fn unsupported_platform_yields_no_url_so_install_falls_back_to_source() {
    let url = print_url("FreeBSD", "riscv64");
    assert!(url.is_empty(), "expected empty URL, got: {url}");
}
