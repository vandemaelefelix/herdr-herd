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
    pub label: String,
    pub x: f32,
    pub target_x: f32,
    pub phase: f32,
    pub facing_left: bool,
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

    /// Update facing from a horizontal delta; zero delta keeps the last facing
    /// so a pet that stops does not snap back to a default direction.
    pub fn set_facing_from_dx(&mut self, dx: f32) {
        if dx > 0.0 {
            self.facing_left = false;
        } else if dx < 0.0 {
            self.facing_left = true;
        }
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
    fn working_frame_leg_pose_matches_hop_airborne_window() {
        use crate::anim::motion_offset;
        use crate::sprite::{self, Role};

        let sheep = sprite::embedded_species()
            .into_iter()
            .find(|s| s.name == "Sheep")
            .expect("sheep species is embedded");
        let working = &sheep.states[&AgentStatus::Working];
        assert_eq!(working.frames.len(), 2, "working is a two-frame walk cycle");

        let leg_rows = |cells: &[Role], w: usize, h: usize| cells[(h - 2) * w..].to_vec();
        let legend = |rows: &[&str]| -> Vec<Role> {
            rows.iter()
                .flat_map(|r| r.chars())
                .map(|c| sprite::role_from_char(c).expect("legend char"))
                .collect()
        };
        let diagonal_legs = legend(&[".#MM#..#MS#.....", "..##....##......"]);
        let straight_legs = legend(&["..#MM#..#MM#....", "...##....##....."]);

        let mut p = pet(AgentStatus::Working);
        // phase 0.0 is the start of the hop's rise (sin == 0 there too, but it's
        // still inside the airborne half of the cycle) — see Motion::Hop.
        for &(phase, want_diagonal) in &[(0.0, true), (0.25, true), (0.5, false), (0.75, false)] {
            p.phase = phase;
            let frame = &working.frames[p.frame_index(working.frames.len())];
            let got = leg_rows(&frame.cells, frame.w, frame.h);
            let expected = if want_diagonal {
                &diagonal_legs
            } else {
                &straight_legs
            };
            assert_eq!(
                &got,
                expected,
                "phase {phase}: expected {} legs",
                if want_diagonal { "diagonal" } else { "straight" }
            );
        }

        // Sanity: whenever the hop is actually lifting, it must be within the
        // diagonal-leg half of the cycle (never the straight-leg half).
        for phase in [0.1, 0.25, 0.4] {
            assert!(
                motion_offset(&working.motion, phase).dy < 0.0,
                "phase {phase} should be airborne"
            );
        }
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
}
