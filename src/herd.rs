//! The herd: a free-roaming collection of pets. Reconciles against agent
//! snapshots by terminal_id (survivors keep position + phase), roams with a
//! gentle separation force, and selects a priority-ranked visible set on
//! overflow. All randomness is an injected LCG so the simulation is testable.

use crate::agent::Agent;
use crate::identity::identity_for;
use crate::pet::{Pet, priority};

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
    /// a random x, update survivors' status (preserving position and
    /// animation phase), and drop pets whose agent has departed.
    pub fn reconcile(
        &mut self,
        agents: &[Agent],
        species_count: usize,
        strip_w: f32,
        rng: &mut dyn Rng,
    ) {
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
                let x = rng.next_unit() * strip_w.max(1.0);
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

    /// Advance the roam simulation by `dt_ms`: pick new wander targets by
    /// status, ease toward them, apply pairwise separation, and clamp every
    /// pet to `[0, strip_w - pet_w]`.
    pub fn step(&mut self, dt_ms: f32, strip_w: f32, pet_w: f32, rng: &mut dyn Rng) {
        let dt = dt_ms / 1000.0;
        let max_x = (strip_w - pet_w).max(0.0);
        for p in &mut self.pets {
            // Working roams widely; idle/done drift a little; blocked holds.
            let roam = match p.status {
                crate::agent::AgentStatus::Working => 1.0,
                crate::agent::AgentStatus::Blocked => 0.0,
                _ => 0.35,
            };
            if rng.next_unit() < roam * dt * 0.6 {
                p.target_x = rng.next_unit() * max_x;
            }
            let speed = if p.status == crate::agent::AgentStatus::Working {
                22.0
            } else {
                7.0
            };
            let dx = p.target_x - p.x;
            p.x += dx.signum() * dx.abs().min(speed * dt);
        }
        // Pairwise separation.
        let min_gap = pet_w * 0.55;
        let n = self.pets.len();
        for i in 0..n {
            for j in (i + 1)..n {
                let gap = self.pets[j].x - self.pets[i].x;
                if gap.abs() < min_gap {
                    let push = (min_gap - gap.abs()) * 0.5 * dt;
                    let dir = if gap >= 0.0 { 1.0 } else { -1.0 };
                    self.pets[i].x -= push * dir;
                    self.pets[j].x += push * dir;
                }
            }
        }
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
    fn reconcile_sets_and_updates_the_pet_label() {
        let mut h = Herd::new();
        let mut rng = Lcg::new(1);
        let mut a = agent("a", AgentStatus::Idle);
        a.name = Some("backend".into());
        h.reconcile(&[a], 1, 100.0, &mut rng);
        assert_eq!(h.pets[0].label, "backend");

        // A survivor renamed mid-session picks up the new label.
        let mut a2 = agent("a", AgentStatus::Idle);
        a2.name = Some("frontend".into());
        h.reconcile(&[a2], 1, 100.0, &mut rng);
        assert_eq!(h.pets[0].label, "frontend");
    }

    #[test]
    fn reconcile_uses_the_resolved_hover_label_when_present() {
        let mut h = Herd::new();
        let mut rng = Lcg::new(1);
        // A fresh pet takes the resolved breadcrumb, not the legacy "claude".
        let mut a = agent("a", AgentStatus::Working);
        a.hover_label = Some("herdr-pets › renderer".into());
        h.reconcile(&[a], 1, 100.0, &mut rng);
        assert_eq!(h.pets[0].label, "herdr-pets › renderer");

        // A survivor whose breadcrumb changes (moved tab) picks up the new one.
        let mut a2 = agent("a", AgentStatus::Working);
        a2.hover_label = Some("herdr-pets › tests".into());
        h.reconcile(&[a2], 1, 100.0, &mut rng);
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
