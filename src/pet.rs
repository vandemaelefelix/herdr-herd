//! One pet: identity, live status, horizontal position, and an animation phase.
//! Priority drives both draw order (z-index) and overflow selection.

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

/// One pet: a stable identity plus the live, mutable state the renderer and
/// herd simulation update every tick.
#[derive(Debug, Clone)]
pub struct Pet {
    pub terminal_id: String,
    pub identity: Identity,
    pub status: AgentStatus,
    pub x: f32,
    pub target_x: f32,
    pub phase: f32,
}

impl Pet {
    /// Build a pet at rest at `x` (so `target_x` starts equal to `x`) with
    /// its animation phase at the start of the cycle.
    pub fn new(terminal_id: String, identity: Identity, status: AgentStatus, x: f32) -> Self {
        Self {
            terminal_id,
            identity,
            status,
            x,
            target_x: x,
            phase: 0.0,
        }
    }

    /// This pet's current draw/overflow priority, from its live status.
    pub fn z_priority(&self) -> u8 {
        priority(self.status)
    }

    /// Which of `frame_count` sprite frames the current phase selects.
    pub fn frame_index(&self, frame_count: usize) -> usize {
        if frame_count <= 1 {
            0
        } else {
            ((self.phase * frame_count as f32) as usize).min(frame_count - 1)
        }
    }

    /// Advance the animation phase by `dt_ms`, wrapping at 1.0.
    /// `frame_ms == 0` means a single static frame: phase stays pinned to 0.
    pub fn advance(&mut self, dt_ms: f32, frame_ms: u32) {
        if frame_ms == 0 {
            self.phase = 0.0;
            return;
        }
        // One full phase cycle spans `frame_ms` per implied frame; keep it simple:
        // advance proportionally and wrap.
        let cycle_ms = frame_ms as f32 * 2.0; // 2-frame default cadence
        self.phase = (self.phase + dt_ms / cycle_ms).rem_euclid(1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentStatus;
    use crate::identity::identity_for;

    fn pet(status: AgentStatus) -> Pet {
        Pet::new("term_x".into(), identity_for("term_x", 3), status, 0.0)
    }

    #[test]
    fn priority_orders_blocked_above_all_and_unknown_below() {
        assert!(priority(AgentStatus::Blocked) > priority(AgentStatus::Done));
        assert!(priority(AgentStatus::Done) > priority(AgentStatus::Working));
        assert!(priority(AgentStatus::Working) > priority(AgentStatus::Idle));
        assert!(priority(AgentStatus::Idle) > priority(AgentStatus::Unknown));
    }

    #[test]
    fn frame_index_cycles_with_phase() {
        let mut p = pet(AgentStatus::Working);
        p.phase = 0.0;
        assert_eq!(p.frame_index(2), 0);
        p.phase = 0.75;
        assert_eq!(p.frame_index(2), 1);
    }

    #[test]
    fn advance_wraps_phase() {
        let mut p = pet(AgentStatus::Idle);
        p.phase = 0.9;
        p.advance(600.0, 500); // 600ms over a 500ms frame cycle basis
        assert!((0.0..1.0).contains(&p.phase));
    }

    #[test]
    fn static_state_keeps_a_single_frame() {
        let mut p = pet(AgentStatus::Unknown);
        p.advance(1000.0, 0);
        assert_eq!(p.frame_index(1), 0);
    }
}
