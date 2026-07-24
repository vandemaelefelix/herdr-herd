//! Sidebar-label lookups: parse `herdr workspace list` / `herdr tab list` into
//! `id → label` maps so an agent's hover caption can read the same
//! `workspace › tab` breadcrumb herdr shows in its left sidebar. Tolerant
//! (Value-navigation, like [`crate::control`]'s parsers): entries missing an id
//! or a label are skipped, a malformed envelope yields an empty map — the caller
//! degrades to its fallbacks, never crashes.

use std::collections::HashMap;

use serde_json::Value;

/// Extract `workspace_id → label` from a `herdr workspace list` envelope.
/// Unknown shape or missing fields ⇒ empty map (best-effort).
pub fn parse_workspace_labels(list_json: &str) -> HashMap<String, String> {
    labels_from(list_json, "workspaces", "workspace_id")
}

/// Extract `tab_id → label` from a `herdr tab list` envelope.
/// Unknown shape or missing fields ⇒ empty map (best-effort).
pub fn parse_tab_labels(list_json: &str) -> HashMap<String, String> {
    labels_from(list_json, "tabs", "tab_id")
}

/// Shared extractor: `result.<array_key>[].{id_key, label}` → map. Entries
/// without both an id and a non-empty label are skipped.
fn labels_from(list_json: &str, array_key: &str, id_key: &str) -> HashMap<String, String> {
    let Ok(v) = serde_json::from_str::<Value>(list_json) else {
        return HashMap::new();
    };
    let Some(items) = v
        .get("result")
        .and_then(|r| r.get(array_key))
        .and_then(Value::as_array)
    else {
        return HashMap::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let id = item.get(id_key)?.as_str()?;
            let label = item.get("label")?.as_str()?.trim();
            if label.is_empty() {
                return None;
            }
            Some((id.to_string(), label.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_workspace_labels_by_id() {
        let j = r#"{"result":{"workspaces":[
            {"workspace_id":"w1","label":"vbrb-pinb","number":1},
            {"workspace_id":"w1T","label":"herdr-pets","number":5}]}}"#;
        let m = parse_workspace_labels(j);
        assert_eq!(m.get("w1").map(String::as_str), Some("vbrb-pinb"));
        assert_eq!(m.get("w1T").map(String::as_str), Some("herdr-pets"));
    }

    #[test]
    fn parses_tab_labels_by_id() {
        let j = r#"{"result":{"tabs":[
            {"tab_id":"w1:t8","label":"Lazygit"},
            {"tab_id":"w1:t11","label":"Monorepo UI package"}]}}"#;
        let m = parse_tab_labels(j);
        assert_eq!(m.get("w1:t8").map(String::as_str), Some("Lazygit"));
        assert_eq!(
            m.get("w1:t11").map(String::as_str),
            Some("Monorepo UI package")
        );
    }

    #[test]
    fn skips_entries_missing_id_or_label_and_tolerates_junk() {
        let j = r#"{"result":{"workspaces":[
            {"workspace_id":"w1","label":"ok"},
            {"workspace_id":"w2"},
            {"label":"no-id"},
            {"workspace_id":"w3","label":"   "}]}}"#;
        let m = parse_workspace_labels(j);
        assert_eq!(m.len(), 1, "only the well-formed, non-blank entry survives");
        assert_eq!(m.get("w1").map(String::as_str), Some("ok"));
    }

    #[test]
    fn malformed_envelope_yields_an_empty_map() {
        assert!(parse_workspace_labels("not json").is_empty());
        assert!(parse_tab_labels(r#"{"result":{}}"#).is_empty());
    }
}
