//! Deterministic, clockwork animation: every member's position and pose is a
//! **pure function** of `(terminal_id, status, state config, wall-clock time)`
//! — no accumulated per-process state, no randomness, no coordination between
//! panes. Any two `herdr-herd render` processes (one per tab/pane) calling
//! [`animate`] for the same agent at the same real moment get the identical
//! result, so the same agent's member reads as the same member wherever you look at
//! it — the whole point, since each pane is otherwise a fully independent
//! process with no shared memory.
//!
//! This replaces the old model (an RNG-driven random walk in `Herd::step`,
//! accumulated into `Member.x`/`Member.phase`): that was process-local state, so two
//! panes' independent RNG streams drifted apart from the first tick (real
//! per-process tick timing, not the shared seed, decided the outcome). A member's
//! "personality" (wander phase/speed, rest position, animation offset) is
//! instead derived once per call from [`crate::identity::unit_hash`], keyed by
//! its `terminal_id` — stable, and identical everywhere it's computed.
//!
//! Trade-off: the old pairwise "nudge working members apart so they don't
//! overlap" behavior is gone. It depended on the *current set of visible
//! members*, which differs per pane (different tab widths -> different overflow
//! capacity) — keeping it would have reintroduced the exact per-pane
//! divergence this module exists to remove. Occasional brief overlap between
//! two working members is the accepted cost.

use crate::agent::AgentStatus;
use crate::anim::{Offset, icon_wave_offset, motion_offset};
use crate::identity::unit_hash;
use crate::sprite::StateSpec;

/// A full walk-out-and-back cycle for a wandering (working) member. Slow enough
/// to stay easily clickable, per the herd's original design intent — a
/// gentle amble, not a run.
const WANDER_PERIOD_MS: f64 = 60_000.0;

/// Fraction of the period spent walking in *one* direction (so `2 *
/// WALK_FRACTION` of the period is walking overall). Paired with
/// `PAUSE_FRACTION` so `WALK_FRACTION + PAUSE_FRACTION == 0.5` — one walk
/// plus one pause is exactly half the cycle (there and back).
const WALK_FRACTION: f64 = 0.45;
/// Fraction of the period spent paused at *one* end. Short relative to
/// `WALK_FRACTION`, so a working member spends far more of its time walking
/// than standing still, and never stands still for long.
const PAUSE_FRACTION: f64 = 0.05;

/// Icon float cycle, matching the old `Member::ICON_CYCLE_MS`.
const ICON_CYCLE_MS: f64 = 1800.0;

/// Where a member was, and when, the instant it last left `Working` — captured
/// by `Herd::reconcile` and threaded into [`animate`] so a Working->non-Working
/// transition freezes the member in place instead of teleporting it to the
/// identity-derived rest position. `None` means this member has never been
/// observed making that transition (a fresh or late-attached pane), in which
/// case `animate` falls back to the old identity-derived rest position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    /// `x_fraction` sampled from `wander_position` at the instant this member
    /// left `Working`.
    pub frozen_x: f32,
    /// `now_ms` at the instant this member left `Working`.
    pub settled_at_ms: u64,
}

/// Total duration of Idle's one-shot "settle" (stand -> lie down) animation,
/// split evenly across however many frames the idle state has — ~1s per the
/// design brief, then holds on the last (fully-dozing) frame indefinitely.
const SETTLE_DURATION_MS: f64 = 1_000.0;

/// A wandering (`Working`) member's horizontal position/facing at `now_ms` — the
/// same computation [`animate`] uses for its `x_fraction`/`facing_left`,
/// factored out so `Herd::reconcile` can sample "where was this member when it
/// stopped working" without needing a `StateSpec` (position never depends on
/// which sprite frames a species has).
pub fn wander_position(terminal_id: &str, now_ms: u64) -> (f32, bool) {
    let phase0 = unit_hash("wander-phase", terminal_id) as f64;
    let period_ms =
        WANDER_PERIOD_MS * (0.75 + 0.5 * unit_hash("wander-period", terminal_id) as f64);
    let u = ((now_ms as f64 / period_ms) + phase0).rem_euclid(1.0);
    let (x_fraction, facing_left, _moving) = wander_segment(u);
    (x_fraction, facing_left)
}

/// A member's fully-resolved, ready-to-draw state at one instant.
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
    /// `true` if the member is facing/moving left.
    pub facing_left: bool,
    /// `true` if `x_fraction` is currently changing (a working member mid-walk,
    /// as opposed to paused at one end of its amble). Gates the walk-cycle
    /// leg animation below — see `animate`.
    pub moving: bool,
    /// The overlay icon's own float offset, in icon pixels.
    pub icon_offset: Offset,
}

/// Ease `0.0..1.0` in and out with zero derivative at both ends, so a walk
/// bout starts and stops with no velocity discontinuity against the pause
/// either side of it.
fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Explicit walk/pause rhythm for one full cycle, given fractional cycle
/// position `u` (`0.0..1.0`): walk to the far end, pause briefly, walk back,
/// pause briefly. Returns `(x_fraction, facing_left, moving)`. A deterministic
/// function of `u` alone — not a random walk — but still reads as "amble a
/// bit, rest a bit" because the pause segments are short relative to the walk
/// segments (see `WALK_FRACTION` / `PAUSE_FRACTION`).
fn wander_segment(u: f64) -> (f32, bool, bool) {
    let walk_out_ends = WALK_FRACTION;
    let pause_far_ends = WALK_FRACTION + PAUSE_FRACTION;
    let walk_back_ends = 2.0 * WALK_FRACTION + PAUSE_FRACTION;
    // Beyond walk_back_ends is the pause at the near end, through u == 1.0.
    if u < walk_out_ends {
        (smoothstep((u / WALK_FRACTION) as f32), false, true)
    } else if u < pause_far_ends {
        (1.0, false, false)
    } else if u < walk_back_ends {
        let local = (u - pause_far_ends) / WALK_FRACTION;
        (1.0 - smoothstep(local as f32), true, true)
    } else {
        (0.0, true, false)
    }
}

/// Resolve `terminal_id`'s animated state for `status`/`state` at `now_ms`
/// (milliseconds since the Unix epoch, or `0` under reduced motion — see
/// `render::run_loop`). `anchor` is this member's freeze anchor from
/// `Herd::reconcile` (see [`Anchor`]), or `None` if it's never been observed
/// leaving `Working`. Pure: same inputs, same output, always.
pub fn animate(
    terminal_id: &str,
    status: AgentStatus,
    state: &StateSpec,
    now_ms: u64,
    anchor: Option<Anchor>,
) -> Animated {
    let (x_fraction, facing_left, moving) = if status == AgentStatus::Working {
        let phase0 = unit_hash("wander-phase", terminal_id) as f64;
        // +/-25% period variation so working members don't all sweep in lockstep.
        let period_ms =
            WANDER_PERIOD_MS * (0.75 + 0.5 * unit_hash("wander-period", terminal_id) as f64);
        let u = ((now_ms as f64 / period_ms) + phase0).rem_euclid(1.0);
        wander_segment(u)
    } else {
        let rest_facing_left = unit_hash("rest-facing", terminal_id) < 0.5;
        // Freeze in place at the anchor captured when this member left Working —
        // no teleport to the identity rest spot. `Unknown` is exempted (stays
        // on the plain identity rest-x, unchanged) and a member with no anchor
        // (never observed leaving Working) falls back to that same rest-x.
        let rest = if status != AgentStatus::Unknown {
            anchor.map(|a| a.frozen_x)
        } else {
            None
        }
        .unwrap_or_else(|| unit_hash("rest-x", terminal_id));
        (rest, rest_facing_left, false)
    };

    let frame_count = state.frames.len();
    // Working-but-paused: the sheep has stopped ambling, so its legs hold a
    // single frame instead of cycling on a free-running clock — leg animation
    // is tied to actual horizontal movement, not wall-clock time alone (#11).
    let working_paused = status == AgentStatus::Working && !moving;
    // Static state (frame_ms == 0): no frame swap, no motion phase (matches the
    // old `advance` contract). Other statuses' frame_ms drives their own motion
    // (breathe/hop/shake/sway), unrelated to walking, so they ignore `moving`.
    let legs_frozen = state.frame_ms == 0 || working_paused;
    // Fixed wall-clock cadence for every status, including Working. A
    // distance-linked cadence (hop phase tied to cumulative horizontal
    // distance moved, so faster movement hopped faster) was tried here per
    // the design brief, but live testing on real hardware showed it read as
    // jittery rather than a clean walk — at ~12fps, a phase that speeds up
    // and slows down continuously doesn't sample smoothly. Reverted to the
    // brief's own explicit fallback.
    let phase = if legs_frozen {
        0.0
    } else {
        let cycle_ms = state.frame_ms as f64 * 2.0;
        let offset0 = unit_hash("anim-phase", terminal_id) as f64;
        ((now_ms as f64 / cycle_ms) + offset0).rem_euclid(1.0) as f32
    };
    // Walk frames are ordered stride-first (drawn airborne, on the hop's upbeat
    // — phase < 0.5) and planted-last (drawn grounded). A paused sheep rests on
    // the planted frame so it stands feet-down rather than frozen mid-stride
    // (#13 + #11); a static/single-frame state just uses its only frame.
    let frame_index = if frame_count <= 1 {
        0
    } else if working_paused {
        frame_count - 1
    } else if status == AgentStatus::Idle {
        // One-shot "settle" (stand -> lie down): steps through the idle
        // state's frames once, driven by time since this member's anchor was
        // captured, then holds on the last (fully-dozing) frame. No anchor
        // (never observed settling) skips straight to that resting frame —
        // there is no reference instant to animate the lie-down from.
        match anchor {
            Some(a) => {
                let elapsed_ms = now_ms.saturating_sub(a.settled_at_ms) as f64;
                let stage = (elapsed_ms / SETTLE_DURATION_MS * frame_count as f64) as usize;
                stage.min(frame_count - 1)
            }
            None => frame_count - 1,
        }
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
        moving,
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
    // test-blob's states are single-frame, so leg-cycle tests (which need a
    // real 2-frame walk cycle to observe frame_index change) use the real
    // sheep species instead.
    const SHEEP: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/sprites/sheep.sprite"));

    fn state(status: AgentStatus) -> StateSpec {
        parse_species(BLOB).unwrap().states.remove(&status).unwrap()
    }

    fn sheep_working_state() -> StateSpec {
        parse_species(SHEEP)
            .unwrap()
            .states
            .remove(&AgentStatus::Working)
            .unwrap()
    }

    fn sheep_idle_state() -> StateSpec {
        parse_species(SHEEP)
            .unwrap()
            .states
            .remove(&AgentStatus::Idle)
            .unwrap()
    }

    /// Upper bound on the jittered wander period (`WANDER_PERIOD_MS * 1.25`),
    /// so a scan of this many ms is guaranteed to cover at least one full
    /// walk/pause cycle for any terminal_id.
    const MAX_PERIOD_MS: u64 = 75_000;

    #[test]
    fn same_inputs_yield_the_identical_result_every_time() {
        // The whole point: two independent calls (standing in for two
        // independent panes) must agree exactly.
        let st = state(AgentStatus::Working);
        let a = animate("term_x", AgentStatus::Working, &st, 12_345, None);
        let b = animate("term_x", AgentStatus::Working, &st, 12_345, None);
        assert_eq!(a, b);
    }

    #[test]
    fn working_members_wander_over_time() {
        let st = state(AgentStatus::Working);
        let a = animate("term_x", AgentStatus::Working, &st, 0, None);
        let b = animate("term_x", AgentStatus::Working, &st, 5_000, None);
        assert_ne!(a.x_fraction, b.x_fraction, "position must change over time");
    }

    #[test]
    fn non_working_members_hold_a_fixed_position_over_time() {
        for status in [
            AgentStatus::Idle,
            AgentStatus::Done,
            AgentStatus::Blocked,
            AgentStatus::Unknown,
        ] {
            let st = state(status);
            let a = animate("term_x", status, &st, 0, None);
            let b = animate("term_x", status, &st, 60_000, None);
            assert_eq!(a.x_fraction, b.x_fraction, "{status:?} must not wander");
        }
    }

    #[test]
    fn x_fraction_stays_within_bounds() {
        let st = state(AgentStatus::Working);
        for ms in [0, 1_000, 4_999, 12_345, 999_999] {
            let a = animate("term_x", AgentStatus::Working, &st, ms, None);
            assert!((0.0..=1.0).contains(&a.x_fraction));
        }
    }

    #[test]
    fn different_agents_get_different_wander_phases() {
        let st = state(AgentStatus::Working);
        let a = animate("term_a", AgentStatus::Working, &st, 1_000, None);
        let b = animate("term_b", AgentStatus::Working, &st, 1_000, None);
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
        let a = animate("term_x", AgentStatus::Unknown, &st, 0, None);
        let b = animate("term_x", AgentStatus::Unknown, &st, 900, None);
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
        let a = animate("term_x", AgentStatus::Working, &st, 0, None);
        let b = animate("term_x", AgentStatus::Working, &st, 0, None);
        assert_eq!(
            a, b,
            "now_ms=0 is just another fixed instant — no special-casing needed"
        );
    }

    #[test]
    fn legs_hold_the_planted_frame_while_paused_and_cycle_while_walking() {
        let st = sheep_working_state();
        assert!(
            st.frames.len() >= 2,
            "fixture needs a real walk cycle to test frame gating"
        );
        // Walk frames are ordered stride-first, planted-last (see `animate`), so
        // a paused sheep must rest on the planted (feet-down) pose — not the
        // mid-stride frame, which would look like it froze in the air.
        let planted = st.frames.len() - 1;
        let mut saw_paused_instant = false;
        let mut saw_walking_with_a_non_planted_frame = false;
        for ms in (0..MAX_PERIOD_MS).step_by(50) {
            let a = animate("legs-test", AgentStatus::Working, &st, ms, None);
            if a.moving {
                saw_walking_with_a_non_planted_frame |= a.frame_index != planted;
            } else {
                assert_eq!(
                    a.frame_index, planted,
                    "paused member must hold the planted (standing) frame, ms={ms}"
                );
                saw_paused_instant = true;
            }
        }
        assert!(saw_paused_instant, "test must sample a paused instant");
        assert!(
            saw_walking_with_a_non_planted_frame,
            "test must sample a walking instant that shows the legs actually cycling"
        );
    }

    #[test]
    fn walking_leg_frame_stays_locked_to_the_hop() {
        // #13: while walking, the airborne part of the hop shows the mid-stride
        // (stride-first) frame and the grounded part shows the planted (last)
        // frame, so the sheep reads as really running rather than sliding.
        let st = sheep_working_state();
        let planted = st.frames.len() - 1;
        let stride = 0;
        let mut saw_airborne = false;
        let mut saw_grounded = false;
        for ms in (0..MAX_PERIOD_MS).step_by(15) {
            let a = animate("legs-test", AgentStatus::Working, &st, ms, None);
            if !a.moving {
                continue;
            }
            if a.offset.dy < 0.0 {
                assert_eq!(
                    a.frame_index, stride,
                    "airborne must show the mid-stride frame, ms={ms}"
                );
                saw_airborne = true;
            } else {
                assert_eq!(
                    a.frame_index, planted,
                    "grounded must show the planted frame, ms={ms}"
                );
                saw_grounded = true;
            }
        }
        assert!(
            saw_airborne && saw_grounded,
            "test must sample both airborne and grounded walking instants"
        );
    }

    #[test]
    fn wander_spends_more_time_walking_than_paused_with_short_pauses() {
        let st = sheep_working_state();
        for terminal_id in ["rhythm-a", "rhythm-b", "rhythm-c", "rhythm-d"] {
            let step_ms = 25u64;
            let mut moving = 0u32;
            let mut paused = 0u32;
            let mut longest_paused_run_ms = 0u64;
            let mut current_paused_run_ms = 0u64;
            for ms in (0..MAX_PERIOD_MS).step_by(step_ms as usize) {
                let a = animate(terminal_id, AgentStatus::Working, &st, ms, None);
                if a.moving {
                    moving += 1;
                    current_paused_run_ms = 0;
                } else {
                    paused += 1;
                    current_paused_run_ms += step_ms;
                    longest_paused_run_ms = longest_paused_run_ms.max(current_paused_run_ms);
                }
            }
            assert!(
                moving > paused,
                "{terminal_id}: should spend more time walking than paused: moving={moving} paused={paused}"
            );
            // A single pause segment is at most PAUSE_FRACTION of the longest
            // possible jittered period (MAX_PERIOD_MS) — well under half a
            // walk bout, so it reads as a brief rest, not standing still.
            assert!(
                longest_paused_run_ms <= 5_000,
                "{terminal_id}: a pause must stay short, got {longest_paused_run_ms}ms"
            );
        }
    }

    #[test]
    fn anchored_non_working_member_freezes_at_the_anchor_instead_of_teleporting() {
        let anchor = Anchor {
            frozen_x: 0.37,
            settled_at_ms: 1_000,
        };
        for status in [AgentStatus::Idle, AgentStatus::Done, AgentStatus::Blocked] {
            let st = state(status);
            let a = animate("term_x", status, &st, 1_000, Some(anchor));
            let b = animate("term_x", status, &st, 60_000, Some(anchor));
            assert_eq!(
                a.x_fraction, anchor.frozen_x,
                "{status:?} must render at the anchor's frozen_x"
            );
            assert_eq!(
                a.x_fraction, b.x_fraction,
                "{status:?} must stay put at the anchor regardless of elapsed time"
            );
        }
    }

    #[test]
    fn unanchored_non_working_member_falls_back_to_the_identity_rest_x() {
        let st = state(AgentStatus::Idle);
        let with_none = animate("term_x", AgentStatus::Idle, &st, 1_000, None);
        let without_arg = animate("term_x", AgentStatus::Idle, &st, 60_000, None);
        assert_eq!(
            with_none.x_fraction, without_arg.x_fraction,
            "no anchor observed yet -> stable identity rest-x, same as before anchors existed"
        );
    }

    #[test]
    fn unknown_status_ignores_the_anchor_and_keeps_the_identity_rest_x() {
        let st = state(AgentStatus::Unknown);
        let anchor = Anchor {
            frozen_x: 0.91,
            settled_at_ms: 0,
        };
        let anchored = animate("term_x", AgentStatus::Unknown, &st, 5_000, Some(anchor));
        let unanchored = animate("term_x", AgentStatus::Unknown, &st, 5_000, None);
        assert_eq!(
            anchored.x_fraction, unanchored.x_fraction,
            "Unknown stays on the plain identity rest-x even if it happens to carry an anchor"
        );
        assert_ne!(
            anchored.x_fraction, anchor.frozen_x,
            "Unknown must not use the anchor's frozen_x"
        );
    }

    #[test]
    fn idle_settle_sequence_steps_stand_sleep1_sleep2_doze_then_holds() {
        let st = sheep_idle_state();
        assert!(
            st.frames.len() >= 4,
            "fixture needs the traced stand/sleep.1/sleep.2/doze settle sequence"
        );
        let doze = st.frames.len() - 1;
        let anchor = Anchor {
            frozen_x: 0.2,
            settled_at_ms: 0,
        };
        let frame_at =
            |ms: u64| animate("settle-test", AgentStatus::Idle, &st, ms, Some(anchor)).frame_index;
        assert_eq!(
            frame_at(0),
            0,
            "starts on the standing frame the instant it settles"
        );
        assert_eq!(frame_at(300), 1, "sleep.1 partway through the settle");
        assert_eq!(frame_at(600), 2, "sleep.2 further along");
        assert_eq!(frame_at(1_000), doze, "fully dozing by ~1s");
        assert_eq!(
            frame_at(60_000),
            doze,
            "holds on doze indefinitely afterward"
        );
    }

    #[test]
    fn idle_settle_sequence_is_relative_to_when_this_member_settled() {
        // Two members settling at different wall-clock instants must each play
        // their own ~1s settle from their own settled_at_ms, not from t=0.
        let st = sheep_idle_state();
        let doze = st.frames.len() - 1;
        let anchor = Anchor {
            frozen_x: 0.5,
            settled_at_ms: 10_000,
        };
        assert_eq!(
            animate("settle-test", AgentStatus::Idle, &st, 10_000, Some(anchor)).frame_index,
            0
        );
        assert_eq!(
            animate("settle-test", AgentStatus::Idle, &st, 10_600, Some(anchor)).frame_index,
            2
        );
        assert_eq!(
            animate("settle-test", AgentStatus::Idle, &st, 20_000, Some(anchor)).frame_index,
            doze
        );
    }

    #[test]
    fn idle_without_an_observed_settle_shows_the_resting_doze_frame_directly() {
        // A fresh/late pane never saw the Working->Idle transition, so there
        // is no reference instant to animate a lie-down from — it should
        // just show the resting pose, not the standing start of a settle it
        // never observed happening.
        let st = sheep_idle_state();
        let doze = st.frames.len() - 1;
        assert_eq!(
            animate("settle-test", AgentStatus::Idle, &st, 0, None).frame_index,
            doze
        );
        assert_eq!(
            animate("settle-test", AgentStatus::Idle, &st, 999_999, None).frame_index,
            doze
        );
    }

    #[test]
    fn non_working_states_keep_animating_regardless_of_the_moving_gate() {
        // Non-working members never wander (`moving` is always false for them),
        // but their own motion (bounce/breathe/hop/sway) must still animate
        // freely on its own clock — the `moving` gate only freezes Working's
        // walk cycle.
        let st = state(AgentStatus::Blocked); // frame_ms=110, motion=bounce
        let a = animate("term_x", AgentStatus::Blocked, &st, 0, None);
        let b = animate("term_x", AgentStatus::Blocked, &st, 55, None);
        assert!(!a.moving && !b.moving, "non-working members never wander");
        assert_ne!(
            a.offset, b.offset,
            "blocked's bounce motion must keep animating regardless of the moving gate"
        );
    }
}
