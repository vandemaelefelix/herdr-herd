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
    /// This member wears the focus hat: the session's global "you are here"
    /// marker. Resolved by `Herd::reconcile` for the herd as a whole, never
    /// copied verbatim from `Agent::focused`, so at most one member is ever
    /// focused and the hat sticks to the last focused agent while a non-agent
    /// pane holds focus. Not derived here: `Member` has no access to the agent
    /// snapshot.
    pub focused: bool,
    /// Where/when this member settled out of `Working`, threaded into
    /// `motion::animate` so leaving `Working` freezes it in place (not a
    /// teleport to the identity rest position) and re-entering `Working` eases
    /// it back out from that spot. `None` until `Herd::reconcile` first observes
    /// it settle; re-stamped (not cleared) on re-entering `Working`. See
    /// [`crate::motion::Anchor`].
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
