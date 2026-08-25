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

/// How to render the detected agent kind next to a hover caption's name.
/// `Ascii` is the safe fallback for a terminal (or a user) that doesn't want
/// wide multi-cell emoji; `Off` shows no icon at all. There is deliberately
/// no `Auto` — emoji rendering support isn't something a terminal can be
/// reliably probed for, so the choice is a plain config knob, not detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentIconStyle {
    Emoji,
    Ascii,
    Off,
}

/// Every agent kind herdr's own config template enumerates as an accepted
/// `agent` value (its `cjk_ime_agents` allow-list comment) — the authoritative
/// set of strings herdr can detect and report, not a guess. `(kind, emoji,
/// ascii tag)`. Every emoji here is a single non-CJK codepoint outside any
/// variation-selector-dependent range, so `unicode-width` reports it as 2
/// cells without relying on a following U+FE0F (see `src/width.rs`).
const KNOWN_KINDS: &[(&str, &str, &str)] = &[
    ("claude", "🤖", "Cl"),
    ("codex", "🦊", "Cx"),
    ("gemini", "🐙", "Ge"),
    ("cursor", "👻", "Cu"),
    ("devin", "🌀", "Dv"),
    ("cline", "🚀", "Cn"),
    ("opencode", "🦉", "Oc"),
    ("copilot", "🐝", "Co"),
    ("kimi", "🦎", "Ki"),
    ("kiro", "🐢", "Kr"),
    ("droid", "🦄", "Dr"),
    ("amp", "🐬", "Am"),
    ("grok", "🦋", "Gr"),
    ("hermes", "🐧", "Hm"),
    ("kilo", "🦖", "Kl"),
    ("qodercli", "🐳", "Qc"),
    ("qoder", "🦁", "Qo"),
    ("pi", "🐸", "Pi"),
];

/// The generic fallback glyph for a non-empty `kind` this table doesn't
/// recognize yet (herdr can add a new detected kind after this ships) — kept
/// distinct from every entry in [`KNOWN_KINDS`] in both styles.
const FALLBACK_EMOJI: &str = "❓";
const FALLBACK_ASCII: &str = "?";

/// The glyph to show next to a hover caption's name for a detected agent
/// `kind`, in the given `style`. `None` when `kind` is absent or blank (no
/// agent detected — nothing to convey) or when `style` is `Off`.
pub fn kind_icon(kind: Option<&str>, style: AgentIconStyle) -> Option<&'static str> {
    if style == AgentIconStyle::Off {
        return None;
    }
    let kind = kind?.trim();
    if kind.is_empty() {
        return None;
    }
    let found = KNOWN_KINDS.iter().find(|(k, _, _)| *k == kind);
    Some(if style == AgentIconStyle::Ascii {
        found.map_or(FALLBACK_ASCII, |(_, _, tag)| tag)
    } else {
        found.map_or(FALLBACK_EMOJI, |(_, emoji, _)| emoji)
    })
}

/// One agent, as reported in `result.agents[]`. `agent` and `name` are absent
/// for `unknown`-status panes, so both are optional. `cwd` and
/// `foreground_cwd` are nullable in herdr's own schema, so they are optional
/// too; the label fallback chain handles their absence.
#[derive(Debug, Clone, Deserialize)]
pub struct Agent {
    #[serde(default)]
    pub agent: Option<String>,
    pub agent_status: AgentStatus,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub foreground_cwd: Option<String>,
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
    /// `cwd`), else the legacy [`label`](Agent::label). Both are nullable in
    /// herdr's schema, so either may be absent.
    fn folder_label(&self) -> String {
        fn basename(path: &str) -> Option<&str> {
            path.trim_end_matches('/')
                .rsplit('/')
                .find(|seg| !seg.is_empty())
        }
        self.foreground_cwd
            .as_deref()
            .and_then(basename)
            .or_else(|| self.cwd.as_deref().and_then(basename))
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
    #[serde(default)]
    agents: Vec<serde_json::Value>,
}

/// Parse the `herdr agent list` envelope into the agent vector.
///
/// Fails only when the envelope itself is unreadable. Each entry below that is
/// parsed on its own, so one agent in a shape we cannot read is skipped
/// instead of failing the whole list, matching how [`crate::snapshot`]
/// degrades.
pub fn parse_agent_list(json: &str) -> Result<Vec<Agent>, serde_json::Error> {
    let env: Envelope = serde_json::from_str(json)?;
    Ok(env
        .result
        .agents
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Agent>(v).ok())
        .collect())
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

    /// herdr's schema types `cwd`/`foreground_cwd` as nullable. A null must
    /// parse to `None`, not fail the agent.
    #[test]
    fn null_cwd_and_foreground_cwd_parse_to_none() {
        let json = r#"{"result":{"agents":[{"agent_status":"idle","cwd":null,"focused":false,"foreground_cwd":null,"pane_id":"p","revision":0,"tab_id":"t","terminal_id":"x","workspace_id":"w"}]}}"#;
        let a = parse_agent_list(json).unwrap();
        assert_eq!(a[0].cwd, None);
        assert_eq!(a[0].foreground_cwd, None);
    }

    /// The CLI path used to parse `agents[]` as one `Vec<Agent>`, so a single
    /// unreadable entry failed the whole list. It must now skip just that
    /// entry, matching how `parse_session_snapshot` degrades.
    #[test]
    fn a_single_unreadable_agent_is_skipped_not_fatal_to_the_whole_list() {
        let json = r#"{"result":{"agents":[
            {"agent_status":"idle","cwd":"/","focused":false,"foreground_cwd":"/",
             "pane_id":"good","revision":0,"tab_id":"t","terminal_id":"x","workspace_id":"w"},
            {"agent_status":"idle","focused":false,"foreground_cwd":"/",
             "pane_id":"missing-required-field","revision":0,"tab_id":"t","workspace_id":"w"}]}}"#;
        let a = parse_agent_list(json).unwrap();
        assert_eq!(a.len(), 1, "the entry missing terminal_id is dropped");
        assert_eq!(a[0].pane_id, "good");
    }

    #[test]
    fn label_prefers_name_then_agent_then_pane_id() {
        let a = parse_agent_list(FIXTURE).unwrap();
        assert_eq!(a[0].label(), "members-dev"); // has name
        assert_eq!(a[1].label(), "claude"); // no name, has agent
        assert_eq!(a[2].label(), "w1F:p3"); // neither -> pane_id
    }

    #[test]
    fn sidebar_label_joins_workspace_and_tab_as_a_breadcrumb() {
        let a = parse_agent_list(FIXTURE).unwrap();
        assert_eq!(
            a[0].sidebar_label(Some("herdr-herd"), Some("renderer")),
            "herdr-herd › renderer"
        );
    }

    #[test]
    fn sidebar_label_shows_one_piece_bare_when_the_other_is_missing() {
        let a = parse_agent_list(FIXTURE).unwrap();
        assert_eq!(a[0].sidebar_label(Some("herdr-herd"), None), "herdr-herd");
        assert_eq!(a[0].sidebar_label(None, Some("renderer")), "renderer");
        // Blank strings count as missing, not as an empty breadcrumb side.
        assert_eq!(a[0].sidebar_label(Some("  "), Some("renderer")), "renderer");
    }

    #[test]
    fn sidebar_label_falls_back_to_the_folder_basename_then_legacy() {
        // a[0].foreground_cwd = /Users/felix/projects/herdr-herd
        let a = parse_agent_list(FIXTURE).unwrap();
        assert_eq!(a[0].sidebar_label(None, None), "herdr-herd");

        // An agent with no folder path at all falls back to the legacy label.
        let mut bare = a[0].clone();
        bare.foreground_cwd = None;
        bare.cwd = None;
        // a[0] has name "members-dev", so legacy label() == "members-dev".
        assert_eq!(bare.sidebar_label(None, None), "members-dev");
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

    #[test]
    fn kind_icon_maps_known_kinds_to_distinct_glyphs_in_each_style() {
        assert_eq!(kind_icon(Some("claude"), AgentIconStyle::Emoji), Some("🤖"));
        assert_eq!(kind_icon(Some("codex"), AgentIconStyle::Emoji), Some("🦊"));
        assert_ne!(
            kind_icon(Some("claude"), AgentIconStyle::Emoji),
            kind_icon(Some("codex"), AgentIconStyle::Emoji)
        );
        assert_eq!(kind_icon(Some("claude"), AgentIconStyle::Ascii), Some("Cl"));
        assert_eq!(kind_icon(Some("codex"), AgentIconStyle::Ascii), Some("Cx"));
        assert_ne!(
            kind_icon(Some("claude"), AgentIconStyle::Ascii),
            kind_icon(Some("codex"), AgentIconStyle::Ascii)
        );
    }

    #[test]
    fn kind_icon_every_known_kind_has_a_distinct_glyph_per_style() {
        let emoji: Vec<_> = KNOWN_KINDS
            .iter()
            .map(|(k, _, _)| kind_icon(Some(k), AgentIconStyle::Emoji).unwrap())
            .collect();
        let ascii: Vec<_> = KNOWN_KINDS
            .iter()
            .map(|(k, _, _)| kind_icon(Some(k), AgentIconStyle::Ascii).unwrap())
            .collect();
        for glyphs in [&emoji, &ascii] {
            let unique: std::collections::HashSet<_> = glyphs.iter().collect();
            assert_eq!(
                unique.len(),
                glyphs.len(),
                "every known kind must render distinctly: {glyphs:?}"
            );
        }
    }

    #[test]
    fn kind_icon_falls_back_to_a_neutral_glyph_for_an_unrecognized_kind() {
        assert_eq!(
            kind_icon(Some("some-future-agent"), AgentIconStyle::Emoji),
            Some(FALLBACK_EMOJI)
        );
        assert_eq!(
            kind_icon(Some("some-future-agent"), AgentIconStyle::Ascii),
            Some(FALLBACK_ASCII)
        );
    }

    #[test]
    fn kind_icon_shows_nothing_when_no_agent_was_detected() {
        assert_eq!(kind_icon(None, AgentIconStyle::Emoji), None);
        assert_eq!(kind_icon(Some("   "), AgentIconStyle::Emoji), None);
    }

    #[test]
    fn kind_icon_off_style_shows_nothing_even_for_a_known_kind() {
        assert_eq!(kind_icon(Some("claude"), AgentIconStyle::Off), None);
    }
}
