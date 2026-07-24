//! One pet: identity, live status, and label. Priority drives both draw order
//! (z-index) and overflow selection. Position and animation are *not* stored
//! here — they're resolved fresh every draw by `motion::animate`, a pure
//! function of `(terminal_id, status, wall-clock time)`, so every pane's
//! independent process renders the exact same agent identically.

use crate::agent::AgentStatus;
use crate::identity::Identity;

/// Map a status to its draw/overflow priority: higher draws on top and is
/// kept first when the herd overflows the available width.
pub fn priority(status: AgentStatus) -> u8 {
    match status {
        AgentStatus::Blocked => 5,
        AgentStatus::Done => 4,
        AgentStatus::Working => 3,
        AgentStatus::Idle => 2,
        AgentStatus::Unknown => 1,
    }
}

/// One pet: a stable identity plus the live status/label the herd simulation
/// updates on reconcile.
#[derive(Debug, Clone)]
pub struct Pet {
    pub terminal_id: String,
    pub identity: Identity,
    pub status: AgentStatus,
    pub label: String,
}

impl Pet {
    /// Build a pet with an empty label (filled in by `reconcile`).
    pub fn new(terminal_id: String, identity: Identity, status: AgentStatus) -> Self {
        Self {
            terminal_id,
            identity,
            status,
            label: String::new(),
        }
    }

    /// This pet's current draw/overflow priority, from its live status.
    pub fn z_priority(&self) -> u8 {
        priority(self.status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentStatus;
    use crate::identity::identity_for;

    fn pet(status: AgentStatus) -> Pet {
        Pet::new("term_x".into(), identity_for("term_x", 3), status)
    }

    #[test]
    fn priority_orders_blocked_above_all_and_unknown_below() {
        assert!(priority(AgentStatus::Blocked) > priority(AgentStatus::Done));
        assert!(priority(AgentStatus::Done) > priority(AgentStatus::Working));
        assert!(priority(AgentStatus::Working) > priority(AgentStatus::Idle));
        assert!(priority(AgentStatus::Idle) > priority(AgentStatus::Unknown));
    }

    #[test]
    fn z_priority_matches_the_free_function() {
        assert_eq!(
            pet(AgentStatus::Blocked).z_priority(),
            priority(AgentStatus::Blocked)
        );
    }
}
