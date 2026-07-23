//! The controller/watchdog: keep a slim full-width pet strip in every eligible
//! tab via a non-destructive `pane split` (never `layout.apply`, which kills
//! processes — Phase 3 spike). Pure sweep logic here; the loop + I/O are thin
//! shells over the `herdr` CLI seam. See the Phase 3 design spec.

use std::collections::HashSet;
use std::io;

use serde_json::Value;

use crate::herdr::HerdrCli;
use crate::place::{TARGET_ROWS, parse_tab_rows, slim_ratio};

// NOTE: Task 4 adds the imports its code needs (`std::path::{Path, PathBuf}`,
// `std::time::Duration`, `crate::{lock, socket}`).

/// The pane label the controller stamps on each strip so later sweeps (and a
/// fresh controller after a restart) recognise it and never stack a second one.
pub const STRIP_LABEL: &str = "herdr-pets";

/// A tab as seen by `herdr tab list`.
#[derive(Debug, Clone, PartialEq)]
pub struct TabRef {
    pub tab_id: String,
    pub pane_count: u32,
}

/// A pane as seen by `herdr pane list` (label is set only on some panes).
#[derive(Debug, Clone, PartialEq)]
pub struct PaneRef {
    pub pane_id: String,
    pub tab_id: String,
    pub label: Option<String>,
}

/// `true` if a pane label marks it as a pets strip — the controller's own
/// marker or the manifest pane title (so a `place`/manual strip is deduped too).
pub fn is_strip_label(label: &str) -> bool {
    label == STRIP_LABEL || label == "Pets"
}

/// Parse `herdr tab list` into `TabRef`s (skips malformed entries tolerantly).
pub fn parse_tabs(list_json: &str) -> io::Result<Vec<TabRef>> {
    let v: Value = serde_json::from_str(list_json).map_err(io::Error::other)?;
    let tabs = v
        .get("result")
        .and_then(|r| r.get("tabs"))
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("no result.tabs in tab list output"))?;
    Ok(tabs
        .iter()
        .filter_map(|t| {
            Some(TabRef {
                tab_id: t.get("tab_id")?.as_str()?.to_string(),
                pane_count: t.get("pane_count").and_then(Value::as_u64).unwrap_or(0) as u32,
            })
        })
        .collect())
}

/// Parse `herdr pane list` into `PaneRef`s (`label` present only when set).
pub fn parse_panes(list_json: &str) -> io::Result<Vec<PaneRef>> {
    let v: Value = serde_json::from_str(list_json).map_err(io::Error::other)?;
    let panes = v
        .get("result")
        .and_then(|r| r.get("panes"))
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("no result.panes in pane list output"))?;
    Ok(panes
        .iter()
        .filter_map(|p| {
            Some(PaneRef {
                pane_id: p.get("pane_id")?.as_str()?.to_string(),
                tab_id: p.get("tab_id")?.as_str()?.to_string(),
                label: p.get("label").and_then(Value::as_str).map(String::from),
            })
        })
        .collect())
}

/// The set of tab ids that already contain a strip pane (by label marker).
pub fn tabs_with_strip(panes: &[PaneRef]) -> HashSet<String> {
    panes
        .iter()
        .filter(|p| p.label.as_deref().is_some_and(is_strip_label))
        .map(|p| p.tab_id.clone())
        .collect()
}

/// A tab is eligible for non-destructive full-width injection iff it has exactly
/// one pane and no strip yet. (Multi-pane tabs can't get a full-width strip
/// without the destructive `layout.apply`, so they are left to on-demand `place`.)
pub fn needs_strip(tab: &TabRef, with_strip: &HashSet<String>) -> bool {
    tab.pane_count == 1 && !with_strip.contains(&tab.tab_id)
}

/// The `(tab_id, root_pane_id)` pairs to inject this sweep: each eligible tab
/// paired with its sole pane (the split target).
pub fn plan_injections(tabs: &[TabRef], panes: &[PaneRef]) -> Vec<(String, String)> {
    let with_strip = tabs_with_strip(panes);
    tabs.iter()
        .filter(|t| needs_strip(t, &with_strip))
        .filter_map(|t| {
            let pane = panes.iter().find(|p| p.tab_id == t.tab_id)?;
            Some((t.tab_id.clone(), pane.pane_id.clone()))
        })
        .collect()
}

/// Non-destructively place a slim full-width pets strip below `root_pane_id`
/// (the sole pane of a single-pane tab): measure the tab, split down at the slim
/// ratio, run the renderer in the new pane, and stamp the de-dup label. Uses
/// `pane split` (NOT `layout.apply`) so the existing pane's process survives.
pub fn inject_strip(cli: &dyn HerdrCli, root_pane_id: &str, self_exe: &str) -> io::Result<()> {
    let layout_json = cli.run_json(&["pane", "layout", "--pane", root_pane_id])?;
    let rows = parse_tab_rows(&layout_json)?;
    let ratio_arg = format!("{:.4}", slim_ratio(rows, TARGET_ROWS));
    let split_reply = cli.run_json(&[
        "pane",
        "split",
        root_pane_id,
        "--direction",
        "down",
        "--ratio",
        &ratio_arg,
        "--no-focus",
    ])?;
    let strip_pane = parse_split_pane_id(&split_reply)?;
    let render_cmd = format!("{self_exe} render");
    cli.run_json(&["pane", "run", &strip_pane, &render_cmd])?;
    cli.run_json(&["pane", "rename", &strip_pane, STRIP_LABEL])?;
    Ok(())
}

/// Extract `result.pane.pane_id` from a `herdr pane split` reply.
fn parse_split_pane_id(reply: &str) -> io::Result<String> {
    let v: Value = serde_json::from_str(reply).map_err(io::Error::other)?;
    v.get("result")
        .and_then(|r| r.get("pane"))
        .and_then(|p| p.get("pane_id"))
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| io::Error::other("no result.pane.pane_id in pane split reply"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeCli {
        calls: RefCell<Vec<Vec<String>>>,
    }
    impl FakeCli {
        fn new() -> Self {
            FakeCli {
                calls: RefCell::new(Vec::new()),
            }
        }
    }
    impl HerdrCli for FakeCli {
        fn run_json(&self, args: &[&str]) -> io::Result<String> {
            self.calls
                .borrow_mut()
                .push(args.iter().map(|s| s.to_string()).collect());
            match args {
                ["tab", "list"] => {
                    Ok(r#"{"result":{"tabs":[{"tab_id":"w1:t1","pane_count":1}]}}"#.into())
                }
                ["pane", "list"] => {
                    Ok(r#"{"result":{"panes":[{"pane_id":"w1:p1","tab_id":"w1:t1"}]}}"#.into())
                }
                ["pane", "layout", ..] => {
                    Ok(r#"{"result":{"layout":{"area":{"height":64}}}}"#.into())
                }
                ["pane", "split", ..] => Ok(r#"{"result":{"pane":{"pane_id":"w1:pNEW"}}}"#.into()),
                _ => Ok(r#"{"result":{}}"#.into()),
            }
        }
    }

    #[test]
    fn inject_strip_splits_runs_and_labels_in_order() {
        let cli = FakeCli::new();
        inject_strip(&cli, "w1:p1", "/abs/herdr-pets").unwrap();
        let calls = cli.calls.borrow();
        assert_eq!(calls[0], vec!["pane", "layout", "--pane", "w1:p1"]);
        // slim_ratio(64, 7) = 1 - 7/64 = 0.890625 -> "{:.4}" = "0.8906"
        assert_eq!(
            calls[1],
            vec![
                "pane",
                "split",
                "w1:p1",
                "--direction",
                "down",
                "--ratio",
                "0.8906",
                "--no-focus"
            ]
        );
        assert_eq!(
            calls[2],
            vec!["pane", "run", "w1:pNEW", "/abs/herdr-pets render"]
        );
        assert_eq!(calls[3], vec!["pane", "rename", "w1:pNEW", "herdr-pets"]);
    }

    fn tab(id: &str, panes: u32) -> TabRef {
        TabRef {
            tab_id: id.into(),
            pane_count: panes,
        }
    }
    fn pane(id: &str, tab: &str, label: Option<&str>) -> PaneRef {
        PaneRef {
            pane_id: id.into(),
            tab_id: tab.into(),
            label: label.map(String::from),
        }
    }

    #[test]
    fn parse_tabs_reads_id_and_pane_count() {
        let j = r#"{"result":{"tabs":[
            {"tab_id":"w1:t1","pane_count":1,"label":"a"},
            {"tab_id":"w1:t2","pane_count":3,"label":"b"}]}}"#;
        let tabs = parse_tabs(j).unwrap();
        assert_eq!(tabs, vec![tab("w1:t1", 1), tab("w1:t2", 3)]);
    }

    #[test]
    fn parse_panes_reads_optional_label() {
        let j = r#"{"result":{"panes":[
            {"pane_id":"w1:p1","tab_id":"w1:t1"},
            {"pane_id":"w1:p2","tab_id":"w1:t1","label":"herdr-pets"}]}}"#;
        let panes = parse_panes(j).unwrap();
        assert_eq!(panes[0], pane("w1:p1", "w1:t1", None));
        assert_eq!(panes[1], pane("w1:p2", "w1:t1", Some("herdr-pets")));
    }

    #[test]
    fn is_strip_label_matches_the_marker_and_the_manifest_title() {
        assert!(is_strip_label("herdr-pets"));
        assert!(is_strip_label("Pets"));
        assert!(!is_strip_label("claude"));
    }

    #[test]
    fn tabs_with_strip_collects_tabs_that_hold_a_marked_pane() {
        let panes = vec![
            pane("w1:p1", "w1:t1", None),
            pane("w1:p2", "w1:t1", Some("herdr-pets")),
            pane("w1:p3", "w1:t2", Some("claude")),
        ];
        let with = tabs_with_strip(&panes);
        assert!(with.contains("w1:t1"));
        assert!(!with.contains("w1:t2"), "an agent pane is not a strip");
    }

    #[test]
    fn needs_strip_is_true_only_for_a_single_pane_tab_without_one() {
        let mut with = HashSet::new();
        assert!(
            needs_strip(&tab("w1:t1", 1), &with),
            "single-pane, no strip"
        );
        assert!(
            !needs_strip(&tab("w1:t2", 2), &with),
            "multi-pane is skipped"
        );
        with.insert("w1:t1".to_string());
        assert!(!needs_strip(&tab("w1:t1", 1), &with), "already has a strip");
    }

    #[test]
    fn plan_injections_pairs_eligible_tabs_with_their_sole_pane() {
        let tabs = vec![tab("w1:t1", 1), tab("w1:t2", 2), tab("w1:t3", 1)];
        let panes = vec![
            pane("w1:p1", "w1:t1", None), // eligible
            pane("w1:p2", "w1:t2", None), // multi-pane, skipped
            pane("w1:p9", "w1:t2", None),
            pane("w1:pA", "w1:t3", Some("Pets")), // already stripped
        ];
        let plan = plan_injections(&tabs, &panes);
        assert_eq!(plan, vec![("w1:t1".to_string(), "w1:p1".to_string())]);
    }
}
