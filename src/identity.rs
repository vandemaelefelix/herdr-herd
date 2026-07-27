//! Deterministic member identity: hash(terminal_id) -> (species, hue).
//!
//! Uses `terminal_id` (stable per terminal, survives the `pane_id` churn that
//! `layout.apply` causes — see Phase 0 Spike A). Independent salts keep species
//! and hue uncorrelated. `DefaultHasher` has fixed keys, so results are stable
//! across restarts of the same binary.

use std::hash::{Hash, Hasher};

/// A member's stable visual identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub species_index: usize,
    pub hue: u16,
}

fn hash_salted(salt: &str, value: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    salt.hash(&mut h);
    value.hash(&mut h);
    h.finish()
}

/// Map a terminal id to a stable `(species_index, hue)`.
pub fn identity_for(terminal_id: &str, species_count: usize) -> Identity {
    let species_index = if species_count == 0 {
        0
    } else {
        (hash_salted("species", terminal_id) % species_count as u64) as usize
    };
    let hue = (hash_salted("hue", terminal_id) % 360) as u16;
    Identity { species_index, hue }
}

/// A deterministic value in `0.0..1.0`, keyed by `(salt, terminal_id)`. Used to
/// derive an agent's stable "personality" constants (wander phase/speed, rest
/// position, animation offset) — every process computes the same value for the
/// same agent, with no shared state or coordination required.
pub fn unit_hash(salt: &str, terminal_id: &str) -> f32 {
    (hash_salted(salt, terminal_id) as f64 / u64::MAX as f64) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_terminal_id_yields_the_same_identity() {
        let a = identity_for("term_aaa", 3);
        let b = identity_for("term_aaa", 3);
        assert_eq!(a, b);
    }

    #[test]
    fn species_index_is_within_range() {
        for tid in ["term_a", "term_b", "term_c", "term_d", "term_e"] {
            assert!(identity_for(tid, 3).species_index < 3);
        }
    }

    #[test]
    fn hue_is_within_the_color_wheel() {
        for tid in ["term_a", "term_b", "term_c"] {
            assert!(identity_for(tid, 3).hue < 360);
        }
    }

    #[test]
    fn species_and_hue_are_independent() {
        // Two ids sharing a species should still usually differ in hue.
        let ids: Vec<_> = (0..40).map(|i| format!("term_{i}")).collect();
        let same_species: Vec<u16> = ids
            .iter()
            .map(|t| identity_for(t, 2))
            .filter(|i| i.species_index == 0)
            .map(|i| i.hue)
            .collect();
        let distinct: std::collections::HashSet<_> = same_species.iter().collect();
        assert!(distinct.len() > 1, "hue must vary within a single species");
    }

    #[test]
    fn zero_species_count_is_handled() {
        // Degenerate input must not panic (divide-by-zero guard).
        let id = identity_for("term_a", 0);
        assert_eq!(id.species_index, 0);
    }

    #[test]
    fn unit_hash_is_deterministic_and_bounded() {
        for _ in 0..3 {
            let v = unit_hash("wphase", "term_x");
            assert!((0.0..1.0).contains(&v));
            assert_eq!(v, unit_hash("wphase", "term_x"), "same inputs, same output");
        }
    }

    #[test]
    fn unit_hash_varies_by_salt_and_by_id() {
        assert_ne!(unit_hash("a", "term_x"), unit_hash("b", "term_x"));
        assert_ne!(unit_hash("a", "term_x"), unit_hash("a", "term_y"));
    }
}
