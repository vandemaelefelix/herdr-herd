//! The herd: the set of members currently known, kept in sync with the live
//! agent snapshot. Reconciles by `terminal_id`: survivors keep their identity
//! and pick up status/label changes, new agents spawn a member, departed agents
//! are dropped. Position and animation are not simulated here — see
//! `motion::animate`, a pure function of time computed fresh at draw time, so
//! every pane agrees without needing to share any of this state.

use crate::agent::{Agent, AgentStatus};
use crate::identity::identity_for;
use crate::member::{Member, priority};
use crate::motion::{Anchor, wander_position};

/// A herd of members, kept in sync with the live agent snapshot.
#[derive(Default)]
pub struct Herd {
    pub members: Vec<Member>,
}

/// An old→new status change detected for a surviving member during `reconcile`.
/// Never emitted for a member's first appearance — there is no prior status to
/// transition from, so the initial snapshot never produces one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusTransition {
    pub terminal_id: String,
    pub from: crate::agent::AgentStatus,
    pub to: crate::agent::AgentStatus,
}

impl Herd {
    /// An empty herd.
    pub fn new() -> Self {
        Self {
            members: Vec::new(),
        }
    }

    /// Sync `self.members` to `agents`, keyed by `terminal_id`: update survivors'
    /// status/label/`focused` flag, add new members, and drop members whose agent
    /// has departed. Returns the old→new status changes seen on survivors — a
    /// freshly spawned member has no prior status, so it never contributes one;
    /// this is what keeps the initial snapshot silent for sound notifications.
    ///
    /// `now_ms` is the current wall-clock instant (see `render::run_loop`):
    /// when a survivor leaves `Working`, its freeze anchor is captured here —
    /// `frozen_x` sampled from `motion::wander_position` at this exact
    /// instant, `settled_at_ms` set to it — so `motion::animate` can hold the
    /// member there instead of teleporting it to the identity rest position.
    /// Re-entering `Working` clears the anchor; a transition between two
    /// non-Working statuses leaves an existing anchor untouched (it persists
    /// until the member works again). A member's first-ever appearance already
    /// non-Working has no anchor to capture (there's no prior Working instant
    /// to sample) — accepted per-pane cosmetic tradeoff: a pane that only sees
    /// the member post-transition can't know where it was, so it falls back to
    /// the identity rest position, same as before anchors existed.
    pub fn reconcile(
        &mut self,
        agents: &[Agent],
        species_count: usize,
        now_ms: u64,
    ) -> Vec<StatusTransition> {
        let mut transitions = Vec::new();
        for a in agents {
            if let Some(p) = self
                .members
                .iter_mut()
                .find(|p| p.terminal_id == a.terminal_id)
            {
                if p.status != a.agent_status {
                    transitions.push(StatusTransition {
                        terminal_id: a.terminal_id.clone(),
                        from: p.status,
                        to: a.agent_status,
                    });
                    if p.status == AgentStatus::Working {
                        let (frozen_x, _facing_left) = wander_position(&p.terminal_id, now_ms);
                        p.anchor = Some(Anchor {
                            frozen_x,
                            settled_at_ms: now_ms,
                        });
                    } else if a.agent_status == AgentStatus::Working {
                        p.anchor = None;
                    }
                }
                p.status = a.agent_status;
                p.label = a.display_label();
                p.focused = a.focused;
            } else {
                let mut member = Member::new(
                    a.terminal_id.clone(),
                    identity_for(&a.terminal_id, species_count),
                    a.agent_status,
                );
                member.label = a.display_label();
                member.focused = a.focused;
                self.members.push(member);
            }
        }
        // Remove departed.
        self.members
            .retain(|p| agents.iter().any(|a| a.terminal_id == p.terminal_id));
        transitions
    }
}

/// Priority-ranked visibility: keep the highest-priority `capacity` members
/// (ties by terminal_id for stability); return their indices + hidden count.
pub fn visible_and_hidden(members: &[Member], capacity: usize) -> (Vec<usize>, usize) {
    let mut idx: Vec<usize> = (0..members.len()).collect();
    if members.len() <= capacity {
        return (idx, 0);
    }
    idx.sort_by(|&a, &b| {
        // Focus wins outright: the selected agent's member must never be
        // dropped from the visible set, whatever its status — otherwise its
        // sheep (and the focus hat) disappear when the strip overflows. Below
        // focus, keep the highest-priority statuses (attention states first),
        // ties by terminal_id for stability.
        members[b]
            .focused
            .cmp(&members[a].focused)
            .then_with(|| priority(members[b].status).cmp(&priority(members[a].status)))
            .then_with(|| members[a].terminal_id.cmp(&members[b].terminal_id))
    });
    let hidden = members.len() - capacity;
    idx.truncate(capacity);
    (idx, hidden)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, AgentStatus};

    fn agent(tid: &str, status: AgentStatus) -> Agent {
        Agent {
            agent: Some("claude".into()),
            agent_status: status,
            name: None,
            cwd: "/".into(),
            foreground_cwd: "/".into(),
            workspace_id: "w".into(),
            tab_id: "t".into(),
            pane_id: "p".into(),
            terminal_id: tid.into(),
            revision: 0,
            focused: false,
            hover_label: None,
        }
    }

    #[test]
    fn reconcile_adds_updates_and_removes_by_terminal_id() {
        let mut h = Herd::new();
        h.reconcile(
            &[
                agent("a", AgentStatus::Idle),
                agent("b", AgentStatus::Working),
            ],
            2,
            0,
        );
        assert_eq!(h.members.len(), 2);

        // 'a' changes status, 'b' leaves, 'c' joins.
        h.reconcile(
            &[
                agent("a", AgentStatus::Blocked),
                agent("c", AgentStatus::Idle),
            ],
            2,
            0,
        );
        let a = h.members.iter().find(|p| p.terminal_id == "a").unwrap();
        assert_eq!(a.status, AgentStatus::Blocked);
        assert!(h.members.iter().any(|p| p.terminal_id == "c"));
        assert!(!h.members.iter().any(|p| p.terminal_id == "b"));
    }

    #[test]
    fn reconcile_preserves_identity_across_reconciles() {
        // A survivor's stable identity (species/hue) must not be re-rolled.
        let mut h = Herd::new();
        h.reconcile(&[agent("a", AgentStatus::Idle)], 3, 0);
        let identity0 = h.members[0].identity;
        h.reconcile(&[agent("a", AgentStatus::Working)], 3, 0);
        assert_eq!(h.members[0].identity, identity0);
    }

    #[test]
    fn reconcile_sets_and_updates_the_member_label() {
        let mut h = Herd::new();
        let mut a = agent("a", AgentStatus::Idle);
        a.name = Some("backend".into());
        h.reconcile(&[a], 1, 0);
        assert_eq!(h.members[0].label, "backend");

        // A survivor renamed mid-session picks up the new label.
        let mut a2 = agent("a", AgentStatus::Idle);
        a2.name = Some("frontend".into());
        h.reconcile(&[a2], 1, 0);
        assert_eq!(h.members[0].label, "frontend");
    }

    #[test]
    fn reconcile_carries_the_focused_flag_onto_new_and_surviving_members() {
        let mut h = Herd::new();
        let mut a = agent("a", AgentStatus::Idle);
        a.focused = true;
        h.reconcile(&[a], 1, 0);
        assert!(
            h.members[0].focused,
            "a fresh member picks up focused from the agent"
        );

        // Focus moves to a new agent 'b'; 'a' survives but loses focus.
        let mut a2 = agent("a", AgentStatus::Idle);
        a2.focused = false;
        let mut b = agent("b", AgentStatus::Idle);
        b.focused = true;
        h.reconcile(&[a2, b], 1, 0);
        let a_member = h.members.iter().find(|p| p.terminal_id == "a").unwrap();
        let b_member = h.members.iter().find(|p| p.terminal_id == "b").unwrap();
        assert!(
            !a_member.focused,
            "surviving member loses focus when it moves elsewhere"
        );
        assert!(
            b_member.focused,
            "the newly focused agent's member is focused"
        );
    }

    #[test]
    fn reconcile_uses_the_resolved_hover_label_when_present() {
        let mut h = Herd::new();
        // A fresh member takes the resolved breadcrumb, not the legacy "claude".
        let mut a = agent("a", AgentStatus::Working);
        a.hover_label = Some("herdr-herd › renderer".into());
        h.reconcile(&[a], 1, 0);
        assert_eq!(h.members[0].label, "herdr-herd › renderer");

        // A survivor whose breadcrumb changes (moved tab) picks up the new one.
        let mut a2 = agent("a", AgentStatus::Working);
        a2.hover_label = Some("herdr-herd › tests".into());
        h.reconcile(&[a2], 1, 0);
        assert_eq!(h.members[0].label, "herdr-herd › tests");
    }

    #[test]
    fn reconcile_reports_no_transitions_for_the_initial_snapshot() {
        let mut h = Herd::new();
        let transitions = h.reconcile(
            &[
                agent("a", AgentStatus::Blocked),
                agent("b", AgentStatus::Done),
            ],
            2,
            0,
        );
        assert!(
            transitions.is_empty(),
            "a member's first appearance is not a transition, even if already blocked"
        );
    }

    #[test]
    fn reconcile_reports_a_transition_when_a_survivor_changes_status() {
        let mut h = Herd::new();
        h.reconcile(&[agent("a", AgentStatus::Idle)], 1, 0);
        let transitions = h.reconcile(&[agent("a", AgentStatus::Blocked)], 1, 0);
        assert_eq!(
            transitions,
            vec![StatusTransition {
                terminal_id: "a".into(),
                from: AgentStatus::Idle,
                to: AgentStatus::Blocked,
            }]
        );
    }

    #[test]
    fn reconcile_reports_no_transition_when_status_is_unchanged() {
        let mut h = Herd::new();
        h.reconcile(&[agent("a", AgentStatus::Working)], 1, 0);
        let transitions = h.reconcile(&[agent("a", AgentStatus::Working)], 1, 0);
        assert!(transitions.is_empty());
    }

    #[test]
    fn reconcile_reports_one_transition_per_surviving_agent_that_changed() {
        let mut h = Herd::new();
        h.reconcile(
            &[
                agent("a", AgentStatus::Working),
                agent("b", AgentStatus::Working),
                agent("c", AgentStatus::Working),
            ],
            1,
            0,
        );
        let mut transitions = h.reconcile(
            &[
                agent("a", AgentStatus::Blocked),
                agent("b", AgentStatus::Blocked),
                agent("c", AgentStatus::Working), // unchanged
            ],
            1,
            0,
        );
        transitions.sort_by(|x, y| x.terminal_id.cmp(&y.terminal_id));
        assert_eq!(
            transitions,
            vec![
                StatusTransition {
                    terminal_id: "a".into(),
                    from: AgentStatus::Working,
                    to: AgentStatus::Blocked,
                },
                StatusTransition {
                    terminal_id: "b".into(),
                    from: AgentStatus::Working,
                    to: AgentStatus::Blocked,
                },
            ]
        );
    }

    #[test]
    fn reconcile_anchors_a_member_leaving_working_at_its_wander_position_and_instant() {
        let mut h = Herd::new();
        h.reconcile(&[agent("a", AgentStatus::Working)], 1, 0);
        assert_eq!(h.members[0].anchor, None, "still working -> no anchor yet");

        h.reconcile(&[agent("a", AgentStatus::Idle)], 1, 12_345);
        let (expected_x, _) = crate::motion::wander_position("a", 12_345);
        assert_eq!(
            h.members[0].anchor,
            Some(Anchor {
                frozen_x: expected_x,
                settled_at_ms: 12_345,
            })
        );
    }

    #[test]
    fn reconcile_clears_the_anchor_on_re_entering_working() {
        let mut h = Herd::new();
        h.reconcile(&[agent("a", AgentStatus::Working)], 1, 0);
        h.reconcile(&[agent("a", AgentStatus::Idle)], 1, 1_000);
        assert!(h.members[0].anchor.is_some());

        h.reconcile(&[agent("a", AgentStatus::Working)], 1, 2_000);
        assert_eq!(
            h.members[0].anchor, None,
            "re-entering Working clears the anchor"
        );
    }

    #[test]
    fn reconcile_keeps_the_anchor_across_non_working_to_non_working_changes() {
        let mut h = Herd::new();
        h.reconcile(&[agent("a", AgentStatus::Working)], 1, 0);
        h.reconcile(&[agent("a", AgentStatus::Idle)], 1, 1_000);
        let anchor = h.members[0].anchor;
        assert!(anchor.is_some());

        // Idle -> Blocked much later: the anchor must persist unchanged, not
        // re-sample at the new instant.
        h.reconcile(&[agent("a", AgentStatus::Blocked)], 1, 99_999);
        assert_eq!(
            h.members[0].anchor, anchor,
            "a non-working -> non-working change must not touch the anchor"
        );
    }

    #[test]
    fn reconcile_leaves_a_freshly_seen_non_working_member_unanchored() {
        // First-ever appearance already non-working: no prior Working instant
        // was ever observed, so there's nothing to anchor to.
        let mut h = Herd::new();
        h.reconcile(&[agent("a", AgentStatus::Idle)], 1, 5_000);
        assert_eq!(h.members[0].anchor, None);
    }

    #[test]
    fn the_focused_member_stays_visible_even_over_capacity_and_low_priority() {
        // capacity 1: an unfocused Blocked (highest priority) vs a focused Idle
        // (lowest). Focus must win the slot — otherwise the selected agent's
        // sheep, and its focus hat, vanish from an overflowing strip. This is
        // the "the hat isn't showing on my selected agent" bug.
        let blocked = crate::member::Member::new(
            "aaa".into(),
            crate::identity::identity_for("aaa", 2),
            AgentStatus::Blocked,
        );
        let mut focused_idle = crate::member::Member::new(
            "zzz".into(),
            crate::identity::identity_for("zzz", 2),
            AgentStatus::Idle,
        );
        focused_idle.focused = true;
        let members = vec![blocked, focused_idle];
        let (visible, hidden) = visible_and_hidden(&members, 1);
        assert_eq!(hidden, 1);
        assert_eq!(
            visible,
            vec![1],
            "the focused member must take the visible slot over an unfocused higher-priority one"
        );
    }

    #[test]
    fn overflow_keeps_attention_states_and_drops_idle_first() {
        let members = vec![
            crate::member::Member::new(
                "i".into(),
                crate::identity::identity_for("i", 2),
                AgentStatus::Idle,
            ),
            crate::member::Member::new(
                "b".into(),
                crate::identity::identity_for("b", 2),
                AgentStatus::Blocked,
            ),
            crate::member::Member::new(
                "w".into(),
                crate::identity::identity_for("w", 2),
                AgentStatus::Working,
            ),
        ];
        let (visible, hidden) = visible_and_hidden(&members, 2);
        assert_eq!(hidden, 1);
        // the blocked and working members must be the visible ones; idle dropped.
        let names: Vec<&str> = visible
            .iter()
            .map(|&i| members[i].terminal_id.as_str())
            .collect();
        assert!(names.contains(&"b") && names.contains(&"w"));
        assert!(!names.contains(&"i"));
    }
}
