//! Agent model: deserialize `herdr agent list` output.

use serde::Deserialize;

/// An agent's live status. `Unknown` is the fallback for panes with no detected
/// agent and for any status string herdr adds later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    #[serde(other)]
    Unknown,
}

/// One agent, as reported in `result.agents[]`. `agent` and `name` are absent
/// for `unknown`-status panes, so both are optional.
#[derive(Debug, Clone, Deserialize)]
pub struct Agent {
    #[serde(default)]
    pub agent: Option<String>,
    pub agent_status: AgentStatus,
    #[serde(default)]
    pub name: Option<String>,
    pub cwd: String,
    pub foreground_cwd: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub pane_id: String,
    pub terminal_id: String,
    #[serde(default)]
    pub revision: i64,
    pub focused: bool,
    /// The sidebar breadcrumb (`workspace › tab`), resolved after the fetch by
    /// joining against `workspace list` / `tab list`. Not present in the
    /// `agent list` JSON, so it defaults to `None` and is filled in by the
    /// watcher; `display_label` falls back to `label()` when it is absent.
    #[serde(default, skip)]
    pub hover_label: Option<String>,
}

impl Agent {
    /// Legacy human label: prefer the user-set `name`, else the detected
    /// `agent` kind, else the stable `pane_id`. Kept as the last-resort
    /// fallback for [`display_label`] / [`sidebar_label`].
    pub fn label(&self) -> String {
        self.name
            .clone()
            .or_else(|| self.agent.clone())
            .unwrap_or_else(|| self.pane_id.clone())
    }

    /// What hover shows: the resolved [`hover_label`](Agent::hover_label) (the
    /// sidebar breadcrumb) when set, else the legacy [`label`](Agent::label).
    pub fn display_label(&self) -> String {
        self.hover_label.clone().unwrap_or_else(|| self.label())
    }

    /// Build the sidebar breadcrumb from this agent's workspace + tab labels,
    /// exactly as herdr's left sidebar reads it: `"<workspace> › <tab>"`. Empty
    /// or missing pieces degrade gracefully — one piece alone is shown bare, and
    /// with neither we fall back to the working-directory basename, then to the
    /// legacy [`label`](Agent::label). Blank strings count as missing.
    pub fn sidebar_label(&self, ws_label: Option<&str>, tab_label: Option<&str>) -> String {
        fn clean(s: Option<&str>) -> Option<&str> {
            s.map(str::trim).filter(|s| !s.is_empty())
        }
        match (clean(ws_label), clean(tab_label)) {
            (Some(ws), Some(tab)) => format!("{ws} › {tab}"),
            (Some(ws), None) => ws.to_string(),
            (None, Some(tab)) => tab.to_string(),
            (None, None) => self.folder_label(),
        }
    }

    /// Last-resort location label: the basename of `foreground_cwd` (else
    /// `cwd`), else the legacy [`label`](Agent::label).
    fn folder_label(&self) -> String {
        fn basename(path: &str) -> Option<&str> {
            path.trim_end_matches('/')
                .rsplit('/')
                .find(|seg| !seg.is_empty())
        }
        basename(&self.foreground_cwd)
            .or_else(|| basename(&self.cwd))
            .map(str::to_string)
            .unwrap_or_else(|| self.label())
    }
}

#[derive(Debug, Deserialize)]
struct Envelope {
    result: EnvelopeResult,
}

#[derive(Debug, Deserialize)]
struct EnvelopeResult {
    agents: Vec<Agent>,
}

/// Parse the `herdr agent list` envelope into the agent vector.
pub fn parse_agent_list(json: &str) -> Result<Vec<Agent>, serde_json::Error> {
    let env: Envelope = serde_json::from_str(json)?;
    Ok(env.result.agents)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/agent-list.json"
    ));

    #[test]
    fn parses_all_agents_from_the_envelope() {
        let agents = parse_agent_list(FIXTURE).expect("valid fixture");
        assert_eq!(agents.len(), 4);
    }

    #[test]
    fn parses_statuses_including_unknown_and_blocked() {
        let a = parse_agent_list(FIXTURE).unwrap();
        assert_eq!(a[0].agent_status, AgentStatus::Working);
        assert_eq!(a[1].agent_status, AgentStatus::Idle);
        assert_eq!(a[2].agent_status, AgentStatus::Unknown);
        assert_eq!(a[3].agent_status, AgentStatus::Blocked);
    }

    #[test]
    fn optional_agent_and_name_are_none_when_absent() {
        let a = parse_agent_list(FIXTURE).unwrap();
        assert_eq!(a[2].agent, None);
        assert_eq!(a[2].name, None);
        assert_eq!(a[1].name, None);
    }

    #[test]
    fn unrecognised_status_falls_back_to_unknown() {
        let json = r#"{"result":{"agents":[{"agent_status":"wat","cwd":"/","focused":false,"foreground_cwd":"/","pane_id":"p","revision":0,"tab_id":"t","terminal_id":"x","workspace_id":"w"}]}}"#;
        let a = parse_agent_list(json).unwrap();
        assert_eq!(a[0].agent_status, AgentStatus::Unknown);
    }

    #[test]
    fn label_prefers_name_then_agent_then_pane_id() {
        let a = parse_agent_list(FIXTURE).unwrap();
        assert_eq!(a[0].label(), "pets-dev"); // has name
        assert_eq!(a[1].label(), "claude"); // no name, has agent
        assert_eq!(a[2].label(), "w1F:p3"); // neither -> pane_id
    }

    #[test]
    fn sidebar_label_joins_workspace_and_tab_as_a_breadcrumb() {
        let a = parse_agent_list(FIXTURE).unwrap();
        assert_eq!(
            a[0].sidebar_label(Some("herdr-pets"), Some("renderer")),
            "herdr-pets › renderer"
        );
    }

    #[test]
    fn sidebar_label_shows_one_piece_bare_when_the_other_is_missing() {
        let a = parse_agent_list(FIXTURE).unwrap();
        assert_eq!(a[0].sidebar_label(Some("herdr-pets"), None), "herdr-pets");
        assert_eq!(a[0].sidebar_label(None, Some("renderer")), "renderer");
        // Blank strings count as missing, not as an empty breadcrumb side.
        assert_eq!(a[0].sidebar_label(Some("  "), Some("renderer")), "renderer");
    }

    #[test]
    fn sidebar_label_falls_back_to_the_folder_basename_then_legacy() {
        // a[0].foreground_cwd = /Users/felix/projects/herdr-pets
        let a = parse_agent_list(FIXTURE).unwrap();
        assert_eq!(a[0].sidebar_label(None, None), "herdr-pets");

        // An agent with no folder path at all falls back to the legacy label.
        let mut bare = a[0].clone();
        bare.foreground_cwd = String::new();
        bare.cwd = String::new();
        // a[0] has name "pets-dev", so legacy label() == "pets-dev".
        assert_eq!(bare.sidebar_label(None, None), "pets-dev");
    }

    #[test]
    fn display_label_prefers_the_resolved_hover_label() {
        let mut a = parse_agent_list(FIXTURE).unwrap().remove(1); // "claude", no name
        assert_eq!(
            a.display_label(),
            "claude",
            "falls back to legacy when unresolved"
        );
        a.hover_label = Some("home › shell".to_string());
        assert_eq!(a.display_label(), "home › shell");
    }
}
