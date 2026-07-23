//! The full-width strip injector: measure the tab, export its layout tree,
//! wrap it in a root vertical split with a new pets pane as the bottom child,
//! and apply. Pure ratio/tree helpers live here; socket + env orchestration is
//! in [`place`]. See Phase 0 Spike A (design §5) for the verified wire protocol
//! (newline-delimited JSON-RPC, dotted methods, a command-leaf with no
//! `pane_id` spawns a fresh pane).

use std::io;

use serde_json::{Value, json};

/// Rows the strip should occupy: pets take 6 half-block rows, plus 1 caption.
pub const TARGET_ROWS: u16 = 7;

/// The split ratio that leaves the bottom `target_rows` for the strip on a tab
/// `tab_rows` tall: `1 - target/tab`. Clamped to `[0.3, 0.95]` so a tiny tab
/// still keeps a usable top region and a huge tab still yields a real strip.
pub fn slim_ratio(tab_rows: u16, target_rows: u16) -> f32 {
    if tab_rows == 0 {
        return 0.85;
    }
    let r = 1.0 - (target_rows as f32 / tab_rows as f32);
    r.clamp(0.3, 0.95)
}

/// Wrap `tree` (a `layout.export` root) in a root `down` split whose bottom
/// child is a new command pane running `cmd` in `cwd`. The bottom leaf carries
/// a `command` and **no `pane_id`** — that is how herdr spawns a fresh pane
/// (Spike A). The existing tree is preserved verbatim as the top child.
pub fn wrap_root(tree: Value, ratio: f32, cmd: &[String], cwd: &str) -> Value {
    json!({
        "type": "split",
        "direction": "down",
        "ratio": ratio,
        "first": tree,
        "second": {
            "type": "pane",
            "command": cmd,
            "cwd": cwd,
        }
    })
}

/// Extract `result.layout.area.height` (the tab's total row count) from a
/// `herdr pane layout --current` CLI JSON envelope.
pub fn parse_tab_rows(cli_json: &str) -> io::Result<u16> {
    let v: Value = serde_json::from_str(cli_json).map_err(io::Error::other)?;
    v.get("result")
        .and_then(|r| r.get("layout"))
        .and_then(|l| l.get("area"))
        .and_then(|a| a.get("height"))
        .and_then(Value::as_u64)
        .and_then(|h| u16::try_from(h).ok())
        .ok_or_else(|| io::Error::other("no result.layout.area.height in pane layout output"))
}

/// Extract the recursive `result.layout.root` tree from a socket
/// `layout.export` reply, ready to feed to [`wrap_root`].
pub fn extract_export_root(reply: &str) -> io::Result<Value> {
    let v: Value = serde_json::from_str(reply).map_err(io::Error::other)?;
    v.get("result")
        .and_then(|r| r.get("layout"))
        .and_then(|l| l.get("root"))
        .cloned()
        .ok_or_else(|| io::Error::other("no result.layout.root in layout.export reply"))
}

/// Build the `layout.export` request line for `tab_id`.
pub fn export_request(tab_id: &str) -> String {
    json!({"id": "pets-place", "method": "layout.export", "params": {"tab_id": tab_id}}).to_string()
}

/// Build the `layout.apply` request line placing `root` on `tab_id`.
pub fn apply_request(tab_id: &str, root: &Value) -> String {
    json!({"id": "pets-place", "method": "layout.apply", "params": {"tab_id": tab_id, "root": root}})
        .to_string()
}

/// Error if a JSON-RPC reply carries an `error` object; otherwise `Ok`.
pub fn check_reply(reply: &str) -> io::Result<()> {
    let v: Value = serde_json::from_str(reply).map_err(io::Error::other)?;
    if let Some(err) = v.get("error") {
        return Err(io::Error::other(format!(
            "herdr rejected the request: {err}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slim_ratio_leaves_the_target_rows_for_the_strip() {
        // 64-row tab, 7-row strip => 1 - 7/64.
        let r = slim_ratio(64, 7);
        assert!((r - (1.0 - 7.0 / 64.0)).abs() < 1e-6, "got {r}");
    }

    #[test]
    fn slim_ratio_clamps_up_on_a_tiny_tab() {
        // 8-row tab, 7-row strip => 1 - 7/8 = 0.125, clamped up to the 0.3 floor.
        assert_eq!(slim_ratio(8, 7), 0.3);
    }

    #[test]
    fn wrap_root_puts_a_command_leaf_with_no_pane_id_at_the_bottom() {
        let tree = json!({"type": "pane", "pane_id": "w1:p1", "cwd": "/x"});
        let cmd = vec!["/abs/herdr-pets".to_string(), "render".to_string()];
        let root = wrap_root(tree.clone(), 0.89, &cmd, "/work");
        assert_eq!(root["type"], "split");
        assert_eq!(root["direction"], "down");
        assert_eq!(
            root["first"], tree,
            "existing tree preserved verbatim on top"
        );
        let bottom = &root["second"];
        assert_eq!(bottom["type"], "pane");
        assert_eq!(bottom["command"], json!(["/abs/herdr-pets", "render"]));
        assert_eq!(bottom["cwd"], "/work");
        assert!(
            bottom.get("pane_id").is_none(),
            "a fresh pane must carry no pane_id"
        );
    }

    #[test]
    fn parse_tab_rows_reads_the_area_height() {
        let j = r#"{"result":{"layout":{"area":{"height":64,"width":214,"x":40,"y":1}}}}"#;
        assert_eq!(parse_tab_rows(j).unwrap(), 64);
    }

    #[test]
    fn parse_tab_rows_errors_when_height_is_absent() {
        assert!(parse_tab_rows(r#"{"result":{"layout":{}}}"#).is_err());
    }

    #[test]
    fn extract_export_root_returns_the_recursive_tree() {
        let reply = r#"{"result":{"type":"layout_export","layout":{"tab_id":"w1:t1","root":{"type":"pane","pane_id":"w1:p1","cwd":"/x"}}}}"#;
        let root = extract_export_root(reply).unwrap();
        assert_eq!(root["type"], "pane");
        assert_eq!(root["pane_id"], "w1:p1");
    }

    #[test]
    fn check_reply_errors_on_an_error_envelope_and_passes_a_result() {
        assert!(check_reply(r#"{"error":{"code":"invalid_target"}}"#).is_err());
        assert!(check_reply(r#"{"result":{"ok":true}}"#).is_ok());
    }
}
