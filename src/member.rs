//! One member: identity, live status, label, and focus. Priority drives both
//! draw order (z-index) and overflow selection. Position and animation are
//! *not* stored here — they're resolved fresh every draw by
//! `motion::animate`, a pure function of `(terminal_id, status, wall-clock
//! time)`, so every pane's independent process renders the exact same agent
//! identically.

use crate::agent::AgentStatus;
use crate::identity::Identity;
use crate::motion::Anchor;

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

/// One member: a stable identity plus the live status/label the herd simulation
/// updates on reconcile.
#[derive(Debug, Clone)]
pub struct Member {
    pub terminal_id: String,
    pub identity: Identity,
    pub status: AgentStatus,
    pub label: String,
    /// Mirrors `Agent::focused`: this member's owning agent is the current one,
    /// so the renderer draws a focus hat on it. Set by `Herd::reconcile`, not
    /// derived here — `Member` has no access to the agent snapshot.
    pub focused: bool,
    /// Where/when this member was last seen leaving `Working`, threaded into
    /// `motion::animate` so a Working->non-Working transition freezes it in
    /// place instead of teleporting to the identity rest position. `None`
    /// until `Herd::reconcile` observes that transition; cleared again on
    /// re-entering `Working`. See [`crate::motion::Anchor`].
    pub anchor: Option<Anchor>,
}

impl Member {
    /// Build a member with an empty label (filled in by `reconcile`), unfocused
    /// and unanchored until `Herd::reconcile` says otherwise.
    pub fn new(terminal_id: String, identity: Identity, status: AgentStatus) -> Self {
        Self {
            terminal_id,
            identity,
            status,
            label: String::new(),
            focused: false,
            anchor: None,
        }
    }

    /// This member's current draw/overflow priority, from its live status.
    pub fn z_priority(&self) -> u8 {
        priority(self.status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentStatus;
    use crate::identity::identity_for;

    fn member(status: AgentStatus) -> Member {
        Member::new("term_x".into(), identity_for("term_x", 3), status)
    }

    #[test]
    fn new_member_starts_unfocused() {
        assert!(
            !member(AgentStatus::Idle).focused,
            "focus is only granted by Herd::reconcile, never by default"
        );
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
            member(AgentStatus::Blocked).z_priority(),
            priority(AgentStatus::Blocked)
        );
    }
}
