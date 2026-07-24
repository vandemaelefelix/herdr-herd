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

/// A working pet's coarse amble rhythm: alternates between walking toward
/// `target_x` and a short stationary pause, so herd movement reads as
/// "wander a bit, rest a bit" rather than a jittery random walk. Owned by
/// `Pet` (like `target_x`) and driven each tick by `Herd::step`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WanderState {
    Walking,
    Paused,
}

/// One pet: a stable identity plus the live, mutable state the renderer and
/// herd simulation update every tick.
#[derive(Debug, Clone)]
pub struct Pet {
    pub terminal_id: String,
    pub identity: Identity,
    pub status: AgentStatus,
    pub label: String,
    pub x: f32,
    pub target_x: f32,
    pub phase: f32,
    pub facing_left: bool,
    /// Whether `x` is currently changing tick-to-tick. Drives the working
    /// walk-cycle gate in `advance`: legs animate only while this is true.
    pub moving: bool,
    /// Current phase of the walk/pause amble rhythm (`Herd::step` only).
    pub wander_state: WanderState,
    /// Milliseconds remaining in `wander_state` before `Herd::step` rolls the
    /// next phase. Starts at 0 (`Paused`) so a pet's very first working tick
    /// immediately rolls into `Walking` with a fresh target and duration.
    pub wander_timer_ms: f32,
}

impl Pet {
    /// Build a pet at rest at `x` (so `target_x` starts equal to `x`) with
    /// its animation phase at the start of the cycle.
    pub fn new(terminal_id: String, identity: Identity, status: AgentStatus, x: f32) -> Self {
        Self {
            terminal_id,
            identity,
            status,
            label: String::new(),
            x,
            target_x: x,
            phase: 0.0,
            facing_left: false,
            moving: false,
            wander_state: WanderState::Paused,
            wander_timer_ms: 0.0,
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
    /// For `Working`, the walk cycle also pins to 0 while `moving` is false —
    /// legs hold a single standing frame instead of cycling on a free-running
    /// clock (see `set_moving_from_dx`, which `Herd::step` drives from actual
    /// horizontal movement). Other statuses ignore `moving`: their frame_ms
    /// drives motion (breathe/hop/shake/sway), not a walk cycle.
    pub fn advance(&mut self, dt_ms: f32, frame_ms: u32) {
        let frozen = frame_ms == 0 || (self.status == AgentStatus::Working && !self.moving);
        if frozen {
            self.phase = 0.0;
            return;
        }
        // One full phase cycle spans `frame_ms` per implied frame; keep it simple:
        // advance proportionally and wrap.
        let cycle_ms = frame_ms as f32 * 2.0; // 2-frame default cadence
        self.phase = (self.phase + dt_ms / cycle_ms).rem_euclid(1.0);
    }

    /// Update facing from a horizontal delta; zero delta keeps the last facing
    /// so a pet that stops does not snap back to a default direction.
    pub fn set_facing_from_dx(&mut self, dx: f32) {
        if dx > 0.0 {
            self.facing_left = false;
        } else if dx < 0.0 {
            self.facing_left = true;
        }
    }

    /// Update whether this pet is currently moving from a horizontal delta;
    /// mirrors `set_facing_from_dx` and feeds the working walk-cycle gate in
    /// `advance`.
    pub fn set_moving_from_dx(&mut self, dx: f32) {
        self.moving = dx != 0.0;
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
        p.advance(600.0, 500); // cycle_ms = 500*2 = 1000; 0.9 + 600/1000 = 1.5 -> wraps to 0.5
        assert_eq!(p.phase, 0.5);
    }

    #[test]
    fn static_state_pins_phase_to_zero() {
        let mut p = pet(AgentStatus::Unknown);
        p.phase = 0.7; // start non-zero
        p.advance(1000.0, 0); // frame_ms == 0 => static
        assert_eq!(p.phase, 0.0); // proves it was pinned, not left at 0.7
    }

    #[test]
    fn facing_tracks_last_nonzero_direction() {
        let mut p = pet(AgentStatus::Working);
        assert!(
            !p.facing_left,
            "defaults to facing right (sprite art faces right)"
        );
        p.set_facing_from_dx(-2.0);
        assert!(p.facing_left, "moving left faces left");
        p.set_facing_from_dx(0.0);
        assert!(p.facing_left, "no movement keeps the last facing");
        p.set_facing_from_dx(3.0);
        assert!(!p.facing_left, "moving right faces right");
    }

    #[test]
    fn set_moving_from_dx_tracks_zero_and_nonzero_delta() {
        let mut p = pet(AgentStatus::Working);
        assert!(!p.moving, "starts stationary");
        p.set_moving_from_dx(3.0);
        assert!(p.moving, "nonzero delta is moving");
        p.set_moving_from_dx(0.0);
        assert!(!p.moving, "zero delta is stationary");
        p.set_moving_from_dx(-1.5);
        assert!(p.moving, "negative delta still counts as moving");
    }

    #[test]
    fn working_walk_cycle_freezes_on_a_standing_frame_when_not_moving() {
        let mut p = pet(AgentStatus::Working);
        p.phase = 0.3; // mid-cycle, to prove it gets pinned rather than left alone
        p.set_moving_from_dx(0.0);
        p.advance(1000.0, 150); // frame_ms > 0, but stationary
        assert_eq!(p.phase, 0.0, "stationary working pet holds a single frame");
    }

    #[test]
    fn working_walk_cycle_animates_while_moving() {
        let mut p = pet(AgentStatus::Working);
        p.set_moving_from_dx(3.0);
        p.phase = 0.0;
        p.advance(150.0, 150); // cycle_ms = 300; 0 + 150/300 = 0.5
        assert_eq!(p.phase, 0.5, "moving working pet keeps cycling frames");
    }

    #[test]
    fn non_working_states_animate_regardless_of_the_moving_flag() {
        let mut p = pet(AgentStatus::Idle);
        p.set_moving_from_dx(0.0); // idle pets never move; must not freeze their motion
        p.phase = 0.2;
        p.advance(260.0, 520); // cycle_ms = 1040; 0.2 + 260/1040 = 0.45
        assert_eq!(
            p.phase, 0.45,
            "non-working states are unaffected by the moving gate"
        );
    }
}
