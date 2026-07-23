//! Agent model: deserialize `herdr agent list` output.

use serde::Deserialize;

/// An agent's live status. `Unknown` is the fallback for panes with no detected
/// agent and for any status string herdr adds later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
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
}

impl Agent {
    /// Human label: prefer the user-set `name`, else the detected `agent`
    /// kind, else the stable `pane_id`.
    pub fn label(&self) -> String {
        self.name
            .clone()
            .or_else(|| self.agent.clone())
            .unwrap_or_else(|| self.pane_id.clone())
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
}
