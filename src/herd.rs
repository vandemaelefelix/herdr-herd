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

impl Herd {
    /// An empty herd.
    pub fn new() -> Self {
        Self { pets: Vec::new() }
    }

    /// Sync `self.pets` to `agents`, keyed by `terminal_id`: update survivors'
    /// status/label/`focused` flag, add new pets, and drop pets whose agent
    /// has departed.
    pub fn reconcile(&mut self, agents: &[Agent], species_count: usize) {
        for a in agents {
            if let Some(p) = self
                .pets
                .iter_mut()
                .find(|p| p.terminal_id == a.terminal_id)
            {
                p.status = a.agent_status;
                p.label = a.display_label();
                p.focused = a.focused;
            } else {
                let mut pet = Pet::new(
                    a.terminal_id.clone(),
                    identity_for(&a.terminal_id, species_count),
                    a.agent_status,
                );
                pet.label = a.display_label();
                pet.focused = a.focused;
                self.pets.push(pet);
            }
        }
        // Remove departed.
        self.pets
            .retain(|p| agents.iter().any(|a| a.terminal_id == p.terminal_id));
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
    fn reconcile_carries_the_focused_flag_onto_new_and_surviving_pets() {
        let mut h = Herd::new();
        let mut a = agent("a", AgentStatus::Idle);
        a.focused = true;
        h.reconcile(&[a], 1);
        assert!(
            h.pets[0].focused,
            "a fresh pet picks up focused from the agent"
        );

        // Focus moves to a new agent 'b'; 'a' survives but loses focus.
        let mut a2 = agent("a", AgentStatus::Idle);
        a2.focused = false;
        let mut b = agent("b", AgentStatus::Idle);
        b.focused = true;
        h.reconcile(&[a2, b], 1);
        let a_pet = h.pets.iter().find(|p| p.terminal_id == "a").unwrap();
        let b_pet = h.pets.iter().find(|p| p.terminal_id == "b").unwrap();
        assert!(
            !a_pet.focused,
            "surviving pet loses focus when it moves elsewhere"
        );
        assert!(b_pet.focused, "the newly focused agent's pet is focused");
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
