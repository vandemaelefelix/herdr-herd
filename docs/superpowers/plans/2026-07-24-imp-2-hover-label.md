# Improvement 2 — Richer hover label (plan)

Spec: [`2026-07-24-imp-2-hover-label-design.md`](../specs/2026-07-24-imp-2-hover-label-design.md)

**Execution note.** One coherent feature (a join + wiring). Implemented directly
with TDD — the pure pieces (label join, list parsing) are unit-tested first; the
watcher wiring is thin glue over them. Not decomposed to subagents.

## Tasks (red → green each)

1. **Label join (pure).** In `agent.rs`, write failing tests for
   `Agent::sidebar_label(ws, tab)`: the four breadcrumb rows, folder-basename
   fallback, legacy fallback. Implement to green. Add `hover_label:
   Option<String>` (`#[serde(default)]`) + `display_label()`; fix the two test
   `agent()` helpers.

2. **List parsing (pure).** New `sidebar.rs`: failing tests for
   `parse_workspace_labels` / `parse_tab_labels` (id→label maps, tolerant).
   Implement to green. Register `mod sidebar;` in `lib.rs`.

3. **reconcile uses it.** `herd.rs`: `p.label = a.display_label()`; extend the
   label test so a set `hover_label` wins and a survivor picks up a change.

4. **Watcher wiring.** `watcher.rs`: `refetch` also fetches `workspace list` +
   `tab list` (best-effort), builds maps, sets each agent's `hover_label`.

5. **Gate + commit + PR** off `feat/pets-hover-label`.
