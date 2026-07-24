//! The herd: a free-roaming collection of pets. Reconciles against agent
//! snapshots by terminal_id (survivors keep position + phase), roams with a
//! gentle separation force, and selects a priority-ranked visible set on
//! overflow. All randomness is an injected LCG so the simulation is testable.

use crate::agent::Agent;
use crate::identity::identity_for;
use crate::pet::{Pet, WanderState, priority};

/// Horizontal speed while a working pet is actively walking (pixels/sec).
const WALK_SPEED: f32 = 9.0;
/// Randomized amble distance range (px) for one walk bout. The bout's
/// duration is derived from this distance and `WALK_SPEED` (not rolled
/// independently), so a "Walking" pet is in motion for its entire bout
/// instead of arriving early and standing idle while still nominally
/// "walking".
const WALK_STEP_PX_RANGE: (f32, f32) = (20.0, 90.0);
/// Randomized pause duration range (ms): short, so a pet never stands still
/// for long.
const PAUSE_MS_RANGE: (f32, f32) = (150.0, 450.0);

/// A value in `range.0..range.1`, drawn from the injected `Rng`.
fn rand_range(rng: &mut dyn Rng, range: (f32, f32)) -> f32 {
    range.0 + rng.next_unit() * (range.1 - range.0)
}

/// Minimal injected RNG (no `rand` dependency).
pub trait Rng {
    /// Next pseudo-random value in `0.0..1.0`.
    fn next_unit(&mut self) -> f32;
}

/// A tiny linear-congruential generator: deterministic given a seed, so herd
/// simulation tests can assert on exact outcomes.
pub struct Lcg {
    state: u64,
}

impl Lcg {
    /// Build a generator from a seed. Seeds are salted so `0` isn't a
    /// degenerate starting state.
    pub fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_add(0x9E37_79B9_7F4A_7C15),
        }
    }
}

impl Rng for Lcg {
    fn next_unit(&mut self) -> f32 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.state >> 33) as f32) / (1u64 << 31) as f32
    }
}

/// A free-roaming herd of pets, kept in sync with the live agent snapshot.
#[derive(Default)]
pub struct Herd {
    pub pets: Vec<Pet>,
}

impl Herd {
    /// An empty herd.
    pub fn new() -> Self {
        Self { pets: Vec::new() }
    }

    /// Sync `self.pets` to `agents`, keyed by `terminal_id`: add new pets at
    /// a random x within the walkable strip (`[0, strip_w - pet_w]`, the same
    /// bound `step` clamps to), update survivors' status (preserving
    /// position and animation phase), and drop pets whose agent has departed.
    pub fn reconcile(
        &mut self,
        agents: &[Agent],
        species_count: usize,
        strip_w: f32,
        pet_w: f32,
        rng: &mut dyn Rng,
    ) {
        let max_x = (strip_w - pet_w).max(0.0);
        // Update survivors / add new.
        for a in agents {
            if let Some(p) = self
                .pets
                .iter_mut()
                .find(|p| p.terminal_id == a.terminal_id)
            {
                p.status = a.agent_status;
                p.label = a.display_label();
            } else {
                let x = rng.next_unit() * max_x;
                let mut pet = Pet::new(
                    a.terminal_id.clone(),
                    identity_for(&a.terminal_id, species_count),
                    a.agent_status,
                    x,
                );
                pet.label = a.display_label();
                self.pets.push(pet);
            }
        }
        // Remove departed.
        self.pets
            .retain(|p| agents.iter().any(|a| a.terminal_id == p.terminal_id));
    }

    /// Advance the roam simulation by `dt_ms`: working pets alternate between
    /// walking a randomized short distance and a short pause (an explicit
    /// walk/pause rhythm, biased toward walking — see `WALK_STEP_PX_RANGE` /
    /// `PAUSE_MS_RANGE`), and separate from other working pets; non-working
    /// pets hold their `x` exactly. Every pet — working or not — is then
    /// clamped to `[0, strip_w - pet_w]`, so a pane resize (or any pet placed
    /// outside that range) can't leave one stuck off-screen.
    pub fn step(&mut self, dt_ms: f32, strip_w: f32, pet_w: f32, rng: &mut dyn Rng) {
        let dt = dt_ms / 1000.0;
        let max_x = (strip_w - pet_w).max(0.0);
        for p in &mut self.pets {
            // Only working pets roam horizontally; everyone else holds position
            // (they still animate in place via motion_offset).
            if p.status != crate::agent::AgentStatus::Working {
                continue;
            }
            p.wander_timer_ms -= dt_ms;
            if p.wander_timer_ms <= 0.0 {
                match p.wander_state {
                    WanderState::Walking => {
                        p.wander_state = WanderState::Paused;
                        p.wander_timer_ms = rand_range(rng, PAUSE_MS_RANGE);
                    }
                    WanderState::Paused => {
                        p.wander_state = WanderState::Walking;
                        // Pick a direction with room to move — at a strip
                        // edge, a coin-flip direction can point into the
                        // wall, clamp to zero distance, and disguise another
                        // pause as a "Walking" bout (defeating the short-
                        // pause guarantee).
                        let room_pos = (max_x - p.x).max(0.0);
                        let room_neg = p.x.max(0.0);
                        let dir = if room_pos <= 0.0 {
                            -1.0
                        } else if room_neg <= 0.0 {
                            1.0
                        } else if rng.next_unit() < 0.5 {
                            -1.0
                        } else {
                            1.0
                        };
                        let room = if dir > 0.0 { room_pos } else { room_neg };
                        let travel = rand_range(rng, WALK_STEP_PX_RANGE).min(room);
                        p.target_x = p.x + dir * travel;
                        // Duration matches the (possibly room-clamped) travel
                        // distance at WALK_SPEED, so the bout ends exactly
                        // when the pet arrives — never idling mid-"walk".
                        p.wander_timer_ms = (travel / WALK_SPEED) * 1000.0;
                    }
                }
            }
            let speed = if p.wander_state == WanderState::Walking {
                WALK_SPEED
            } else {
                0.0
            };
            let dx = p.target_x - p.x;
            let applied = dx.signum() * dx.abs().min(speed * dt);
            p.x += applied;
            p.set_facing_from_dx(applied);
            p.set_moving_from_dx(applied);
        }
        // Pairwise separation only nudges working pets — a non-working pet's
        // x must stay exactly at its pre-step value, so it neither pushes
        // nor gets pushed.
        let min_gap = pet_w * 0.55;
        let n = self.pets.len();
        for i in 0..n {
            for j in (i + 1)..n {
                let gap = self.pets[j].x - self.pets[i].x;
                if gap.abs() < min_gap {
                    let push = (min_gap - gap.abs()) * 0.5 * dt;
                    let dir = if gap >= 0.0 { 1.0 } else { -1.0 };
                    if self.pets[i].status == crate::agent::AgentStatus::Working {
                        self.pets[i].x -= push * dir;
                    }
                    if self.pets[j].status == crate::agent::AgentStatus::Working {
                        self.pets[j].x += push * dir;
                    }
                }
            }
        }
        // Unconditional: every pet is clamped to the walkable strip, working
        // or not. `reconcile` already bounds spawns to `[0, max_x]`, so this
        // is normally a no-op for non-working pets — it only bites if the
        // strip shrinks (pane resize) out from under a pet that isn't
        // actively roaming to correct itself.
        for p in &mut self.pets {
            p.x = p.x.clamp(0.0, max_x);
        }
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
        let mut rng = Lcg::new(1);
        h.reconcile(
            &[
                agent("a", AgentStatus::Idle),
                agent("b", AgentStatus::Working),
            ],
            2,
            200.0,
            16.0,
            &mut rng,
        );
        assert_eq!(h.pets.len(), 2);

        // 'a' changes status, 'b' leaves, 'c' joins.
        h.pets[0].x = 42.0; // survivor position must be preserved
        h.reconcile(
            &[
                agent("a", AgentStatus::Blocked),
                agent("c", AgentStatus::Idle),
            ],
            2,
            200.0,
            16.0,
            &mut rng,
        );
        let a = h.pets.iter().find(|p| p.terminal_id == "a").unwrap();
        assert_eq!(a.status, AgentStatus::Blocked);
        assert_eq!(a.x, 42.0, "survivor keeps position");
        assert!(h.pets.iter().any(|p| p.terminal_id == "c"));
        assert!(!h.pets.iter().any(|p| p.terminal_id == "b"));
    }

    #[test]
    fn step_keeps_pets_within_bounds() {
        let mut h = Herd::new();
        let mut rng = Lcg::new(7);
        h.reconcile(
            &(0..6)
                .map(|i| agent(&format!("t{i}"), AgentStatus::Working))
                .collect::<Vec<_>>(),
            2,
            100.0,
            20.0,
            &mut rng,
        );
        for _ in 0..200 {
            h.step(50.0, 100.0, 20.0, &mut rng);
        }
        for p in &h.pets {
            assert!(p.x >= 0.0 && p.x <= 80.0, "x={} out of bounds", p.x);
        }
    }

    #[test]
    fn working_pet_alternates_short_pauses_between_longer_walks() {
        let mut h = Herd::new();
        let mut rng = Lcg::new(3);
        h.reconcile(
            &[agent("w", AgentStatus::Working)],
            1,
            300.0,
            16.0,
            &mut rng,
        );

        let dt = 20.0_f32;
        let mut walking_ticks = 0u32;
        let mut paused_ticks = 0u32;
        let mut longest_paused_run_ms = 0.0_f32;
        let mut current_paused_run_ms = 0.0_f32;
        for _ in 0..3000 {
            let before = h.pets[0].x;
            h.step(dt, 300.0, 16.0, &mut rng);
            if h.pets[0].x != before {
                walking_ticks += 1;
                current_paused_run_ms = 0.0;
            } else {
                paused_ticks += 1;
                current_paused_run_ms += dt;
                longest_paused_run_ms = longest_paused_run_ms.max(current_paused_run_ms);
            }
        }

        assert!(
            walking_ticks > paused_ticks,
            "should spend more time walking than paused: walk={walking_ticks} pause={paused_ticks}"
        );
        assert!(
            longest_paused_run_ms <= 500.0,
            "a pause must stay short, got a {longest_paused_run_ms}ms stretch of no movement"
        );
    }

    #[test]
    fn working_pet_resumes_walking_after_a_pause_using_the_injected_rng() {
        // Deterministic under a fixed seed: same seed => identical trajectory.
        let mut h1 = Herd::new();
        let mut rng1 = Lcg::new(42);
        h1.reconcile(
            &[agent("w", AgentStatus::Working)],
            1,
            300.0,
            16.0,
            &mut rng1,
        );
        let mut h2 = Herd::new();
        let mut rng2 = Lcg::new(42);
        h2.reconcile(
            &[agent("w", AgentStatus::Working)],
            1,
            300.0,
            16.0,
            &mut rng2,
        );

        for _ in 0..500 {
            h1.step(20.0, 300.0, 16.0, &mut rng1);
            h2.step(20.0, 300.0, 16.0, &mut rng2);
        }
        assert_eq!(
            h1.pets[0].x, h2.pets[0].x,
            "same seed must reproduce the same walk/pause trajectory"
        );
    }

    #[test]
    fn only_working_pets_roam_horizontally() {
        let mut h = Herd::new();
        let mut rng = Lcg::new(11);
        h.reconcile(
            &[
                agent("idle", AgentStatus::Idle),
                agent("done", AgentStatus::Done),
                agent("blk", AgentStatus::Blocked),
            ],
            1,
            200.0,
            16.0,
            &mut rng,
        );
        let before: Vec<f32> = h.pets.iter().map(|p| p.x).collect();
        for _ in 0..200 {
            h.step(50.0, 200.0, 16.0, &mut rng);
        }
        for (p, x0) in h.pets.iter().zip(before) {
            assert_eq!(p.x, x0, "{} must not roam when not working", p.terminal_id);
        }
    }

    #[test]
    fn reconcile_sets_and_updates_the_pet_label() {
        let mut h = Herd::new();
        let mut rng = Lcg::new(1);
        let mut a = agent("a", AgentStatus::Idle);
        a.name = Some("backend".into());
        h.reconcile(&[a], 1, 100.0, 16.0, &mut rng);
        assert_eq!(h.pets[0].label, "backend");

        // A survivor renamed mid-session picks up the new label.
        let mut a2 = agent("a", AgentStatus::Idle);
        a2.name = Some("frontend".into());
        h.reconcile(&[a2], 1, 100.0, 16.0, &mut rng);
        assert_eq!(h.pets[0].label, "frontend");
    }

    #[test]
    fn reconcile_uses_the_resolved_hover_label_when_present() {
        let mut h = Herd::new();
        let mut rng = Lcg::new(1);
        // A fresh pet takes the resolved breadcrumb, not the legacy "claude".
        let mut a = agent("a", AgentStatus::Working);
        a.hover_label = Some("herdr-pets › renderer".into());
        h.reconcile(&[a], 1, 100.0, 16.0, &mut rng);
        assert_eq!(h.pets[0].label, "herdr-pets › renderer");

        // A survivor whose breadcrumb changes (moved tab) picks up the new one.
        let mut a2 = agent("a", AgentStatus::Working);
        a2.hover_label = Some("herdr-pets › tests".into());
        h.reconcile(&[a2], 1, 100.0, 16.0, &mut rng);
        assert_eq!(h.pets[0].label, "herdr-pets › tests");
    }

    #[test]
    fn overflow_keeps_attention_states_and_drops_idle_first() {
        let pets = vec![
            crate::pet::Pet::new(
                "i".into(),
                crate::identity::identity_for("i", 2),
                AgentStatus::Idle,
                0.0,
            ),
            crate::pet::Pet::new(
                "b".into(),
                crate::identity::identity_for("b", 2),
                AgentStatus::Blocked,
                0.0,
            ),
            crate::pet::Pet::new(
                "w".into(),
                crate::identity::identity_for("w", 2),
                AgentStatus::Working,
                0.0,
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
