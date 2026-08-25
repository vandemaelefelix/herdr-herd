//! The `session.snapshot` reply: one control-socket call that carries the whole
//! session: agents, workspace/tab labels, panes and per-tab layouts.
//!
//! This is the payload that lets the watcher and the controller stop shelling
//! out. `herdr agent list` + `workspace list` + `tab list` is three fork/execs
//! of an 18 MB binary (~203 ms wall, ~23 ms of CPU on the measured session);
//! the same data arrives here in a single request on the socket the plugin is
//! already connected to.
//!
//! Tolerant throughout: the payload is large
//! and gaining fields, so unknown keys are ignored, missing ones degrade, and a
//! single unreadable entry is skipped rather than failing the whole snapshot.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::agent::Agent;
use crate::control::{PaneRef, TabRef};

/// One `session.snapshot` reply, reduced to the parts the herd uses.
///
/// The tab and pane shapes are the controller's [`TabRef`]/[`PaneRef`]: herdr
/// reports the same fields in the snapshot as in the `tab list`/`pane list`
/// replies, so one definition serves both the socket and the CLI path.
#[derive(Debug, Clone, Default)]
pub struct SessionSnapshot {
    /// `snapshot.agents[]`. Entries herdr reports in a shape we cannot read are
    /// skipped, so one odd pane never costs the whole herd.
    pub agents: Vec<Agent>,
    /// `workspace_id → label`, blank labels dropped (they mean "no label" to
    /// [`Agent::sidebar_label`], not an empty breadcrumb side).
    pub workspace_labels: HashMap<String, String>,
    /// `tab_id → label`, blank labels dropped.
    pub tab_labels: HashMap<String, String>,
    /// `snapshot.tabs[]`.
    pub tabs: Vec<TabRef>,
    /// `snapshot.panes[]`.
    pub panes: Vec<PaneRef>,
    /// `tab_id → that tab's layout object`, in the shape
    /// [`crate::control::strip_target_from_layout`] reads.
    pub layouts: HashMap<String, Value>,
}

impl SessionSnapshot {
    /// Fill every agent's `hover_label` from this snapshot's own label maps.
    ///
    /// The watcher used to join `agent list` against two more CLI spawns to get
    /// here; the snapshot already carries both sides of the join, so the
    /// breadcrumb is always consistent with the agents it labels.
    pub fn resolve_hover_labels(&mut self) {
        for a in self.agents.iter_mut() {
            let ws = self
                .workspace_labels
                .get(&a.workspace_id)
                .map(String::as_str);
            let tab = self.tab_labels.get(&a.tab_id).map(String::as_str);
            a.hover_label = Some(a.sidebar_label(ws, tab));
        }
    }
}

/// Parse a `session.snapshot` reply into the parts the herd uses.
///
/// Fails only when the envelope itself is unreadable (junk, or an error reply
/// with no `result.snapshot`). That is the signal for the caller to fall back
/// to the CLI. Everything below the envelope degrades instead of failing.
pub fn parse_session_snapshot(json: &str) -> Result<SessionSnapshot, serde_json::Error> {
    let env: Envelope = serde_json::from_str(json)?;
    let raw = env.result.snapshot;
    Ok(SessionSnapshot {
        // Per-entry rather than `Vec<Agent>`: herdr types `cwd`/`foreground_cwd`
        // as nullable, so one pane reported without a working directory would
        // otherwise take the whole herd down with it.
        agents: raw
            .agents
            .into_iter()
            .filter_map(|v| serde_json::from_value::<Agent>(v).ok())
            .collect(),
        workspace_labels: raw
            .workspaces
            .iter()
            .filter_map(|w| Some((w.workspace_id.clone()?, non_blank(w.label.as_deref())?)))
            .collect(),
        tab_labels: raw
            .tabs
            .iter()
            .filter_map(|t| Some((t.tab_id.clone()?, non_blank(t.label.as_deref())?)))
            .collect(),
        tabs: raw
            .tabs
            .iter()
            .filter_map(|t| {
                Some(TabRef {
                    tab_id: t.tab_id.clone()?,
                    pane_count: t.pane_count.unwrap_or(0) as u32,
                })
            })
            .collect(),
        panes: raw
            .panes
            .iter()
            .filter_map(|p| {
                Some(PaneRef {
                    pane_id: p.pane_id.clone()?,
                    tab_id: p.tab_id.clone()?,
                    label: p.label.clone(),
                })
            })
            .collect(),
        layouts: raw
            .layouts
            .into_iter()
            .filter_map(|l| {
                let tab_id = l.get("tab_id")?.as_str()?.to_string();
                Some((tab_id, l))
            })
            .collect(),
    })
}

/// A trimmed, non-empty copy of `s`; `None` for absent or blank labels.
fn non_blank(s: Option<&str>) -> Option<String> {
    let t = s?.trim();
    (!t.is_empty()).then(|| t.to_string())
}

#[derive(Debug, Deserialize)]
struct Envelope {
    result: EnvelopeResult,
}

#[derive(Debug, Deserialize)]
struct EnvelopeResult {
    snapshot: RawSnapshot,
}

/// Only the arrays the herd reads. Every other key herdr sends (`splits`,
/// `protocol`, `focused_*`, anything added later) is parsed and discarded by
/// serde without allocating.
#[derive(Debug, Deserialize)]
struct RawSnapshot {
    #[serde(default)]
    agents: Vec<Value>,
    #[serde(default)]
    workspaces: Vec<RawWorkspace>,
    #[serde(default)]
    tabs: Vec<RawTab>,
    #[serde(default)]
    panes: Vec<RawPane>,
    #[serde(default)]
    layouts: Vec<Value>,
}

#[derive(Debug, Deserialize)]
struct RawWorkspace {
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    label: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawTab {
    #[serde(default)]
    tab_id: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    pane_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RawPane {
    #[serde(default)]
    pane_id: Option<String>,
    #[serde(default)]
    tab_id: Option<String>,
    #[serde(default)]
    label: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentStatus;

    const SNAPSHOT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/session-snapshot.json"
    ));

    fn parsed() -> SessionSnapshot {
        parse_session_snapshot(SNAPSHOT).expect("valid fixture")
    }

    #[test]
    fn parses_agents_from_the_snapshot_envelope() {
        let s = parsed();
        assert_eq!(
            s.agents.len(),
            3,
            "the fourth agent is missing terminal_id and unreadable, not merely null-cwd"
        );
        assert_eq!(s.agents[0].pane_id, "w1:p1");
        assert_eq!(s.agents[0].agent_status, AgentStatus::Working);
    }

    /// herdr may add statuses; an unrecognised one must not cost us the agent.
    #[test]
    fn an_unrecognised_agent_status_falls_back_to_unknown() {
        assert_eq!(parsed().agents[1].agent_status, AgentStatus::Unknown);
    }

    /// herdr types `cwd`/`foreground_cwd` as nullable. Reading them as `null`
    /// must parse to `None`, not make the agent unreadable.
    #[test]
    fn null_cwd_and_foreground_cwd_are_tolerated_not_dropped() {
        let json = r#"{"result":{"snapshot":{"agents":[
            {"agent_status":"idle","cwd":null,"focused":false,"foreground_cwd":null,
             "pane_id":"null-cwd","revision":0,"tab_id":"t","terminal_id":"x","workspace_id":"w"}]}}}"#;
        let s = parse_session_snapshot(json).expect("envelope is readable");
        assert_eq!(
            s.agents.len(),
            1,
            "a null cwd must not be treated as unreadable"
        );
        assert_eq!(s.agents[0].cwd, None);
        assert_eq!(s.agents[0].foreground_cwd, None);
    }

    /// An entry missing a genuinely required field (no `#[serde(default)]`)
    /// must skip just that agent, not the whole snapshot.
    #[test]
    fn an_agent_in_a_shape_we_cannot_read_is_skipped_not_fatal() {
        let json = r#"{"result":{"snapshot":{"agents":[
            {"agent_status":"idle","cwd":"/","focused":false,"foreground_cwd":"/",
             "pane_id":"bad","revision":0,"tab_id":"t","workspace_id":"w"},
            {"agent_status":"idle","cwd":"/","focused":false,"foreground_cwd":"/",
             "pane_id":"good","revision":0,"tab_id":"t","terminal_id":"x","workspace_id":"w"}]}}}"#;
        let s = parse_session_snapshot(json).expect("envelope is readable");
        assert_eq!(
            s.agents.len(),
            1,
            "the entry missing terminal_id is dropped"
        );
        assert_eq!(s.agents[0].pane_id, "good");
    }

    #[test]
    fn parses_workspace_and_tab_labels_by_id() {
        let s = parsed();
        assert_eq!(
            s.workspace_labels.get("w1").map(String::as_str),
            Some("herdr-herd")
        );
        assert_eq!(
            s.tab_labels.get("w1:t1").map(String::as_str),
            Some("renderer")
        );
        assert_eq!(
            s.tab_labels.get("w1:t2").map(String::as_str),
            Some("Lazygit")
        );
    }

    /// Blank and absent labels both mean "no label" to `sidebar_label`, so
    /// neither belongs in the map.
    #[test]
    fn blank_and_absent_labels_are_left_out_of_the_maps() {
        let s = parsed();
        assert!(
            !s.workspace_labels.contains_key("w2"),
            "blank label dropped"
        );
        assert!(
            !s.tab_labels.contains_key("w1:tCOL"),
            "absent label dropped"
        );
    }

    #[test]
    fn resolves_each_agents_breadcrumb_from_the_snapshots_own_labels() {
        let mut s = parsed();
        s.resolve_hover_labels();
        assert_eq!(
            s.agents[0].display_label(),
            "herdr-herd › renderer",
            "workspace › tab"
        );
        // w2 has a blank workspace label and no tab entry at all, so the agent
        // falls back down the chain to its folder basename.
        assert_eq!(s.agents[2].display_label(), "scratch");
    }

    #[test]
    fn parses_tabs_and_panes_the_controller_needs() {
        let s = parsed();
        assert_eq!(
            s.tabs,
            vec![
                TabRef {
                    tab_id: "w1:t1".into(),
                    pane_count: 2
                },
                TabRef {
                    tab_id: "w1:t2".into(),
                    pane_count: 2
                },
                TabRef {
                    tab_id: "w1:tCOL".into(),
                    pane_count: 1
                },
            ]
        );
        assert_eq!(s.panes.len(), 4);
        assert_eq!(
            s.panes[1],
            PaneRef {
                pane_id: "w1:pSTRIP".into(),
                tab_id: "w1:t1".into(),
                label: Some("herdr-herd".into()),
            }
        );
        assert_eq!(s.panes[0].label, None);
    }

    /// The layouts are the win for the controller: one snapshot replaces a
    /// `pane layout` spawn per candidate tab.
    #[test]
    fn indexes_every_tabs_layout_by_tab_id() {
        let s = parsed();
        let mut ids: Vec<&str> = s.layouts.keys().map(String::as_str).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["w1:t1", "w1:t2", "w1:tCOL"]);
        assert!(s.layouts["w1:t1"].get("panes").is_some());
    }

    #[test]
    fn an_unreadable_envelope_is_an_error_so_the_caller_can_fall_back() {
        assert!(parse_session_snapshot("not json").is_err());
        assert!(parse_session_snapshot(r#"{"result":{}}"#).is_err());
        assert!(
            parse_session_snapshot(r#"{"id":"x","error":{"message":"nope"}}"#).is_err(),
            "an error reply must not look like an empty herd"
        );
    }

    /// An empty session is a valid snapshot, not a failure: the herd is empty.
    #[test]
    fn an_empty_snapshot_parses_to_an_empty_herd() {
        let s = parse_session_snapshot(r#"{"result":{"snapshot":{}}}"#).expect("readable");
        assert!(s.agents.is_empty());
        assert!(s.tabs.is_empty());
        assert!(s.layouts.is_empty());
    }
}
