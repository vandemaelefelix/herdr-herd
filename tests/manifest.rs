//! Validates herdr-plugin.toml has the fields herdr requires (herdr 0.7.0).

use toml::Value;

fn manifest() -> Value {
    let raw = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/herdr-plugin.toml"))
        .expect("herdr-plugin.toml must exist at repo root");
    raw.parse::<Value>()
        .expect("herdr-plugin.toml must be valid TOML")
}

#[test]
fn manifest_has_required_top_level_fields() {
    let m = manifest();
    assert_eq!(m.get("id").and_then(Value::as_str), Some("herdr-pets"));
    assert_eq!(m.get("name").and_then(Value::as_str), Some("herdr-pets"));
    assert_eq!(m.get("version").and_then(Value::as_str), Some("0.1.0"));
    assert_eq!(
        m.get("min_herdr_version").and_then(Value::as_str),
        Some("0.7.0")
    );
    let platforms: Vec<&str> = m
        .get("platforms")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(platforms, vec!["linux", "macos"]);
}

#[test]
fn manifest_pane_runs_the_release_binary_in_render_mode() {
    let m = manifest();
    let panes = m
        .get("panes")
        .and_then(Value::as_array)
        .expect("[[panes]] present");
    let pane = &panes[0];
    assert_eq!(pane.get("id").and_then(Value::as_str), Some("pets"));
    assert_eq!(pane.get("placement").and_then(Value::as_str), Some("split"));
    let cmd: Vec<&str> = pane
        .get("command")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(cmd, vec!["./target/release/herdr-pets", "render"]);
}
