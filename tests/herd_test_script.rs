//! Dry-run checks for scripts/herd-test.sh: the launch plan for the dedicated
//! test session. `--print-plan` resolves the session name, socket lookup, and
//! controller argv without building, spawning, or touching a herdr server, so
//! the ordering logic is verified deterministically here.

use std::process::Command;

/// Run `herd-test.sh --print-plan`, optionally overriding the session name,
/// and return its stdout.
fn print_plan(session: Option<&str>) -> String {
    let dir = env!("CARGO_MANIFEST_DIR");
    let mut cmd = Command::new("sh");
    cmd.arg(format!("{dir}/scripts/herd-test.sh"))
        .arg("--print-plan")
        .current_dir(dir);
    if let Some(s) = session {
        cmd.env("HERDR_HERD_TEST_SESSION", s);
    } else {
        cmd.env_remove("HERDR_HERD_TEST_SESSION");
    }
    let out = cmd
        .env_remove("HERDR_HERD_CONFIG_DIR")
        .output()
        .expect("herd-test.sh should run under sh");
    assert!(out.status.success(), "--print-plan should exit 0");
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn the_default_session_is_a_dedicated_test_session_not_the_users_own() {
    let plan = print_plan(None);
    assert!(plan.contains("session: herd-test"), "got: {plan}");
}

#[test]
fn the_session_name_can_be_overridden() {
    let plan = print_plan(Some("herd-scratch"));
    assert!(plan.contains("session: herd-scratch"), "got: {plan}");
}

/// The whole point of the harness: the strip must show which build it is, so
/// the dev binary is always built with the marker feature on.
#[test]
fn the_binary_is_built_with_the_dev_marker_feature() {
    let plan = print_plan(None);
    assert!(plan.contains("--features dev-marker"), "got: {plan}");
}

/// The socket path must be discovered from herdr, never assumed, so the
/// harness does not depend on herdr's session-directory layout.
#[test]
fn the_socket_is_discovered_from_herdr_rather_than_assumed() {
    let plan = print_plan(None);
    assert!(plan.contains("herdr session list --json"), "got: {plan}");
    let script =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/herd-test.sh"))
            .unwrap();
    assert!(
        !script.contains("herdr.sock"),
        "the script must not hardcode a socket path"
    );
}

/// Strips are placed by the controller sweeping the session, never by an
/// operator running `plugin pane open` by hand.
#[test]
fn the_controller_places_the_strips() {
    let plan = print_plan(None);
    assert!(plan.contains("control"), "got: {plan}");
    let script =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/herd-test.sh"))
            .unwrap();
    assert!(
        !script.contains("plugin pane open"),
        "the script must not place panes itself"
    );
}

/// Test config must not write through to the config dir the user's real
/// session reads.
#[test]
fn the_test_session_gets_its_own_config_dir() {
    let plan = print_plan(None);
    let line = plan
        .lines()
        .find(|l| l.starts_with("config-dir:"))
        .unwrap_or_else(|| panic!("plan should name a config dir: {plan}"));
    assert!(
        !line.contains("/plugins/config/"),
        "must not point at the installed plugin's config dir: {line}"
    );
}
