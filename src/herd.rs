//! The herd: the set of pets currently known, kept in sync with the live
//! agent snapshot. Reconciles by `terminal_id`: survivors keep their identity
//! and pick up status/label changes, new agents spawn a pet, departed agents
//! are dropped. Position and animation are not simulated here — see
//! `motion::animate`, a pure function of time computed fresh at draw time, so
//! every pane agrees without needing to share any of this state.

use crate::agent::Agent;
use crate::identity::identity_for;
use crate::pet::{Pet, priority};

/// A herd of pets, kept in sync with the live agent snapshot.
#[derive(Default)]
pub struct Herd {
    pub pets: Vec<Pet>,
}

/// An old→new status change detected for a surviving pet during `reconcile`.
/// Never emitted for a pet's first appearance — there is no prior status to
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
        Self { pets: Vec::new() }
    }

    /// Sync `self.pets` to `agents`, keyed by `terminal_id`: update survivors'
    /// status/label, add new pets, and drop pets whose agent has departed.
    /// Returns the old→new status changes seen on survivors — a freshly
    /// spawned pet has no prior status, so it never contributes one; this is
    /// what keeps the initial snapshot silent for sound notifications.
    pub fn reconcile(&mut self, agents: &[Agent], species_count: usize) -> Vec<StatusTransition> {
        let mut transitions = Vec::new();
        for a in agents {
            if let Some(p) = self
                .pets
                .iter_mut()
                .find(|p| p.terminal_id == a.terminal_id)
            {
                if p.status != a.agent_status {
                    transitions.push(StatusTransition {
                        terminal_id: a.terminal_id.clone(),
                        from: p.status,
                        to: a.agent_status,
                    });
                }
                p.status = a.agent_status;
                p.label = a.display_label();
            } else {
                let mut pet = Pet::new(
                    a.terminal_id.clone(),
                    identity_for(&a.terminal_id, species_count),
                    a.agent_status,
                );
                pet.label = a.display_label();
                self.pets.push(pet);
            }
        }
        // Remove departed.
        self.pets
            .retain(|p| agents.iter().any(|a| a.terminal_id == p.terminal_id));
        transitions
    }
}

/// Priority-ranked visibility: keep the highest-priority `capacity` pets
/// (ties by terminal_id for stability); return their indices + hidden count.
pub fn visible_and_hidden(pets: &[Pet], capacity: usize) -> (Vec<usize>, usize) {
    let mut idx: Vec<usize> = (0..pets.len()).collect();
    if pets.len() <= capacity {
        return (idx, 0);
    }
    idx.sort_by(|&a, &b| {
        priority(pets[b].status)
            .cmp(&priority(pets[a].status))
            .then_with(|| pets[a].terminal_id.cmp(&pets[b].terminal_id))
    });
    let hidden = pets.len() - capacity;
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
        );
        assert_eq!(h.pets.len(), 2);

        // 'a' changes status, 'b' leaves, 'c' joins.
        h.reconcile(
            &[
                agent("a", AgentStatus::Blocked),
                agent("c", AgentStatus::Idle),
            ],
            2,
        );
        let a = h.pets.iter().find(|p| p.terminal_id == "a").unwrap();
        assert_eq!(a.status, AgentStatus::Blocked);
        assert!(h.pets.iter().any(|p| p.terminal_id == "c"));
        assert!(!h.pets.iter().any(|p| p.terminal_id == "b"));
    }

    #[test]
    fn reconcile_preserves_identity_across_reconciles() {
        // A survivor's stable identity (species/hue) must not be re-rolled.
        let mut h = Herd::new();
        h.reconcile(&[agent("a", AgentStatus::Idle)], 3);
        let identity0 = h.pets[0].identity;
        h.reconcile(&[agent("a", AgentStatus::Working)], 3);
        assert_eq!(h.pets[0].identity, identity0);
    }

    #[test]
    fn reconcile_sets_and_updates_the_pet_label() {
        let mut h = Herd::new();
        let mut a = agent("a", AgentStatus::Idle);
        a.name = Some("backend".into());
        h.reconcile(&[a], 1);
        assert_eq!(h.pets[0].label, "backend");

        // A survivor renamed mid-session picks up the new label.
        let mut a2 = agent("a", AgentStatus::Idle);
        a2.name = Some("frontend".into());
        h.reconcile(&[a2], 1);
        assert_eq!(h.pets[0].label, "frontend");
    }

    #[test]
    fn reconcile_uses_the_resolved_hover_label_when_present() {
        let mut h = Herd::new();
        // A fresh pet takes the resolved breadcrumb, not the legacy "claude".
        let mut a = agent("a", AgentStatus::Working);
        a.hover_label = Some("herdr-pets › renderer".into());
        h.reconcile(&[a], 1);
        assert_eq!(h.pets[0].label, "herdr-pets › renderer");

        // A survivor whose breadcrumb changes (moved tab) picks up the new one.
        let mut a2 = agent("a", AgentStatus::Working);
        a2.hover_label = Some("herdr-pets › tests".into());
        h.reconcile(&[a2], 1);
        assert_eq!(h.pets[0].label, "herdr-pets › tests");
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
        );
        assert!(
            transitions.is_empty(),
            "a pet's first appearance is not a transition, even if already blocked"
        );
    }

    #[test]
    fn reconcile_reports_a_transition_when_a_survivor_changes_status() {
        let mut h = Herd::new();
        h.reconcile(&[agent("a", AgentStatus::Idle)], 1);
        let transitions = h.reconcile(&[agent("a", AgentStatus::Blocked)], 1);
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
        h.reconcile(&[agent("a", AgentStatus::Working)], 1);
        let transitions = h.reconcile(&[agent("a", AgentStatus::Working)], 1);
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
        );
        let mut transitions = h.reconcile(
            &[
                agent("a", AgentStatus::Blocked),
                agent("b", AgentStatus::Blocked),
                agent("c", AgentStatus::Working), // unchanged
            ],
            1,
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
    fn overflow_keeps_attention_states_and_drops_idle_first() {
        let pets = vec![
            crate::pet::Pet::new(
                "i".into(),
                crate::identity::identity_for("i", 2),
                AgentStatus::Idle,
            ),
            crate::pet::Pet::new(
                "b".into(),
                crate::identity::identity_for("b", 2),
                AgentStatus::Blocked,
            ),
            crate::pet::Pet::new(
                "w".into(),
                crate::identity::identity_for("w", 2),
                AgentStatus::Working,
            ),
        ];
        let (visible, hidden) = visible_and_hidden(&pets, 2);
        assert_eq!(hidden, 1);
        // the blocked and working pets must be the visible ones; idle dropped.
        let names: Vec<&str> = visible
            .iter()
            .map(|&i| pets[i].terminal_id.as_str())
            .collect();
        assert!(names.contains(&"b") && names.contains(&"w"));
        assert!(!names.contains(&"i"));
    }
}
