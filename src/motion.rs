//! Deterministic, clockwork animation: every pet's position and pose is a
//! **pure function** of `(terminal_id, status, state config, wall-clock time)`
//! — no accumulated per-process state, no randomness, no coordination between
//! panes. Any two `herdr-pets render` processes (one per tab/pane) calling
//! [`animate`] for the same agent at the same real moment get the identical
//! result, so the same agent's pet reads as the same pet wherever you look at
//! it — the whole point, since each pane is otherwise a fully independent
//! process with no shared memory.
//!
//! This replaces the old model (an RNG-driven random walk in `Herd::step`,
//! accumulated into `Pet.x`/`Pet.phase`): that was process-local state, so two
//! panes' independent RNG streams drifted apart from the first tick (real
//! per-process tick timing, not the shared seed, decided the outcome). A pet's
//! "personality" (wander phase/speed, rest position, animation offset) is
//! instead derived once per call from [`crate::identity::unit_hash`], keyed by
//! its `terminal_id` — stable, and identical everywhere it's computed.
//!
//! Trade-off: the old pairwise "nudge working pets apart so they don't
//! overlap" behavior is gone. It depended on the *current set of visible
//! pets*, which differs per pane (different tab widths -> different overflow
//! capacity) — keeping it would have reintroduced the exact per-pane
//! divergence this module exists to remove. Occasional brief overlap between
//! two working pets is the accepted cost.

use std::f32::consts::TAU;

use crate::agent::AgentStatus;
use crate::anim::{Offset, icon_wave_offset, motion_offset};
use crate::identity::unit_hash;
use crate::sprite::StateSpec;

/// A full sweep-and-back cycle for a wandering (working) pet. Slow enough to
/// stay easily clickable, per the herd's original design intent — a gentle
/// amble, not a run.
const WANDER_PERIOD_MS: f64 = 60_000.0;

/// Icon float cycle, matching the old `Pet::ICON_CYCLE_MS`.
const ICON_CYCLE_MS: f64 = 1800.0;

/// A pet's fully-resolved, ready-to-draw state at one instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Animated {
    /// Horizontal position as a fraction of the walkable width (`0.0..1.0`).
    /// A fraction, not a pixel, because different panes have different
    /// widths — each renderer multiplies by its own local `max_x`.
    pub x_fraction: f32,
    /// Body motion offset (breathe/hop/bounce/sway), in sprite pixels.
    pub offset: Offset,
    /// Which of the state's frames to draw (walk-cycle leg swap etc.).
    pub frame_index: usize,
    /// `true` if the pet is facing/moving left.
    pub facing_left: bool,
    /// The overlay icon's own float offset, in icon pixels.
    pub icon_offset: Offset,
}

/// Resolve `terminal_id`'s animated state for `status`/`state` at `now_ms`
/// (milliseconds since the Unix epoch, or `0` under reduced motion — see
/// `render::run_loop`). Pure: same inputs, same output, always.
pub fn animate(terminal_id: &str, status: AgentStatus, state: &StateSpec, now_ms: u64) -> Animated {
    let (x_fraction, facing_left) = if status == AgentStatus::Working {
        let phase0 = unit_hash("wander-phase", terminal_id) as f64;
        // +/-25% period variation so working pets don't all sweep in lockstep.
        let period_ms =
            WANDER_PERIOD_MS * (0.75 + 0.5 * unit_hash("wander-period", terminal_id) as f64);
        let t = (((now_ms as f64 / period_ms) + phase0).rem_euclid(1.0) as f32) * TAU;
        (0.5 + 0.5 * t.sin(), t.cos() < 0.0)
    } else {
        // Not wandering: a fixed, identity-derived resting spot and facing.
        let rest = unit_hash("rest-x", terminal_id);
        let rest_facing_left = unit_hash("rest-facing", terminal_id) < 0.5;
        (rest, rest_facing_left)
    };

    let frame_count = state.frames.len();
    let phase = if state.frame_ms == 0 {
        0.0 // static state: no frame swap, no motion phase (matches the old `advance` contract)
    } else {
        let cycle_ms = state.frame_ms as f64 * 2.0;
        let offset0 = unit_hash("anim-phase", terminal_id) as f64;
        ((now_ms as f64 / cycle_ms) + offset0).rem_euclid(1.0) as f32
    };
    let frame_index = if frame_count <= 1 {
        0
    } else {
        ((phase * frame_count as f32) as usize).min(frame_count - 1)
    };
    let offset = motion_offset(&state.motion, phase);

    let icon_phase0 = unit_hash("icon-phase", terminal_id) as f64;
    let icon_phase = ((now_ms as f64 / ICON_CYCLE_MS) + icon_phase0).rem_euclid(1.0) as f32;
    let icon_offset = icon_wave_offset(icon_phase);

    Animated {
        x_fraction,
        offset,
        frame_index,
        facing_left,
        icon_offset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sprite::parse_species;

    const BLOB: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/sprites/test-blob.sprite"
    ));

    fn state(status: AgentStatus) -> StateSpec {
        parse_species(BLOB).unwrap().states.remove(&status).unwrap()
    }

    #[test]
    fn same_inputs_yield_the_identical_result_every_time() {
        // The whole point: two independent calls (standing in for two
        // independent panes) must agree exactly.
        let st = state(AgentStatus::Working);
        let a = animate("term_x", AgentStatus::Working, &st, 12_345);
        let b = animate("term_x", AgentStatus::Working, &st, 12_345);
        assert_eq!(a, b);
    }

    #[test]
    fn working_pets_wander_over_time() {
        let st = state(AgentStatus::Working);
        let a = animate("term_x", AgentStatus::Working, &st, 0);
        let b = animate("term_x", AgentStatus::Working, &st, 5_000);
        assert_ne!(a.x_fraction, b.x_fraction, "position must change over time");
    }

    #[test]
    fn non_working_pets_hold_a_fixed_position_over_time() {
        for status in [
            AgentStatus::Idle,
            AgentStatus::Done,
            AgentStatus::Blocked,
            AgentStatus::Unknown,
        ] {
            let st = state(status);
            let a = animate("term_x", status, &st, 0);
            let b = animate("term_x", status, &st, 60_000);
            assert_eq!(a.x_fraction, b.x_fraction, "{status:?} must not wander");
        }
    }

    #[test]
    fn x_fraction_stays_within_bounds() {
        let st = state(AgentStatus::Working);
        for ms in [0, 1_000, 4_999, 12_345, 999_999] {
            let a = animate("term_x", AgentStatus::Working, &st, ms);
            assert!((0.0..=1.0).contains(&a.x_fraction));
        }
    }

    #[test]
    fn different_agents_get_different_wander_phases() {
        let st = state(AgentStatus::Working);
        let a = animate("term_a", AgentStatus::Working, &st, 1_000);
        let b = animate("term_b", AgentStatus::Working, &st, 1_000);
        assert_ne!(
            a.x_fraction, b.x_fraction,
            "distinct agents shouldn't move in lockstep"
        );
    }

    #[test]
    fn static_state_pins_motion_phase_but_not_the_icon() {
        // `unknown` has frame_ms=0 (static), but the icon must still float.
        let st = state(AgentStatus::Unknown);
        assert_eq!(st.frame_ms, 0);
        let a = animate("term_x", AgentStatus::Unknown, &st, 0);
        let b = animate("term_x", AgentStatus::Unknown, &st, 900);
        assert_eq!(
            a.offset, b.offset,
            "body motion is pinned for a static state"
        );
        assert_ne!(
            a.icon_offset, b.icon_offset,
            "the overlay icon keeps floating regardless"
        );
    }

    #[test]
    fn reduced_motion_freezes_at_now_ms_zero() {
        let st = state(AgentStatus::Working);
        let a = animate("term_x", AgentStatus::Working, &st, 0);
        let b = animate("term_x", AgentStatus::Working, &st, 0);
        assert_eq!(
            a, b,
            "now_ms=0 is just another fixed instant — no special-casing needed"
        );
    }
}
