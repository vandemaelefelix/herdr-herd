//! The full-width strip injector: measure the tab, export its layout tree,
//! wrap it in a root vertical split with a new pets pane as the bottom child,
//! and apply. Pure ratio/tree helpers live here; socket + env orchestration is
//! in [`place`]. See Phase 0 Spike A (design §5) for the verified wire protocol
//! (newline-delimited JSON-RPC, dotted methods, a command-leaf with no
//! `pane_id` spawns a fresh pane).

use std::io;

use serde_json::{Value, json};

use crate::herdr::HerdrCli;
use crate::socket;

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

/// Inject a full-width pets strip into the current tab. Reads `$HERDR_TAB_ID`
/// (the target) and `$HERDR_SOCKET_PATH` (the control socket); measures the tab
/// with `herdr pane layout --current` via `cli`, then `layout.export` +
/// `layout.apply` over the socket to place the strip. `self_exe` is the
/// absolute path to this binary; the bottom pane runs `<self_exe> render` in
/// `cwd`.
///
/// De-duplication (avoiding a second strip if one already exists) is a Phase 3
/// concern; this one-shot wraps whatever tree it exports.
pub fn place(cli: &dyn HerdrCli, self_exe: &str, cwd: &str) -> io::Result<()> {
    let tab_id = std::env::var("HERDR_TAB_ID").map_err(|_| {
        io::Error::other("HERDR_TAB_ID is not set — run `place` inside a herdr session")
    })?;
    let sock =
        socket::socket_path().ok_or_else(|| io::Error::other("HERDR_SOCKET_PATH is not set"))?;

    let layout_json = cli.run_json(&["pane", "layout", "--current"])?;
    let tab_rows = parse_tab_rows(&layout_json)?;
    let ratio = slim_ratio(tab_rows, TARGET_ROWS);

    let export_reply = socket::request_line(&sock, &export_request(&tab_id))?;
    check_reply(&export_reply)?;
    let tree = extract_export_root(&export_reply)?;

    let cmd = vec![self_exe.to_string(), "render".to_string()];
    let root = wrap_root(tree, ratio, &cmd, cwd);

    let apply_reply = socket::request_line(&sock, &apply_request(&tab_id, &root))?;
    check_reply(&apply_reply)?;
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
    fn slim_ratio_clamps_down_on_a_huge_tab() {
        // 1000-row tab, 7-row strip => 1 - 7/1000 = 0.993, clamped down to the 0.95 ceiling.
        assert_eq!(slim_ratio(1000, 7), 0.95);
    }

    #[test]
    fn slim_ratio_returns_a_sane_default_when_tab_rows_is_zero() {
        assert_eq!(slim_ratio(0, 7), 0.85);
    }

    #[test]
    fn wrap_root_puts_a_command_leaf_with_no_pane_id_at_the_bottom() {
        let tree = json!({"type": "pane", "pane_id": "w1:p1", "cwd": "/x"});
        let cmd = vec!["/abs/herdr-pets".to_string(), "render".to_string()];
        let root = wrap_root(tree.clone(), 0.89, &cmd, "/work");
        assert_eq!(root["type"], "split");
        assert_eq!(root["direction"], "down");
        let ratio = root["ratio"].as_f64().unwrap();
        assert!((ratio - 0.89).abs() < 1e-6, "got {ratio}");
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
    fn extract_export_root_errors_when_the_root_is_absent() {
        assert!(extract_export_root(r#"{"result":{"layout":{}}}"#).is_err());
    }

    #[test]
    fn export_request_builds_a_layout_export_request() {
        let line = export_request("w1:t1");
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["method"], "layout.export");
        assert_eq!(v["params"]["tab_id"], "w1:t1");
    }

    #[test]
    fn apply_request_builds_a_layout_apply_request_with_the_root() {
        let root = json!({"type": "split", "direction": "down", "ratio": 0.89});
        let line = apply_request("w1:t1", &root);
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["method"], "layout.apply");
        assert_eq!(v["params"]["tab_id"], "w1:t1");
        assert_eq!(v["params"]["root"], root);
    }

    #[test]
    fn check_reply_errors_on_an_error_envelope_and_passes_a_result() {
        assert!(check_reply(r#"{"error":{"code":"invalid_target"}}"#).is_err());
        assert!(check_reply(r#"{"result":{"ok":true}}"#).is_ok());
    }
}
