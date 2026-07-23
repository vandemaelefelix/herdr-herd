//! Deterministic pet identity: hash(terminal_id) -> (species, hue).
//!
//! Uses `terminal_id` (stable per terminal, survives the `pane_id` churn that
//! `layout.apply` causes — see Phase 0 Spike A). Independent salts keep species
//! and hue uncorrelated. `DefaultHasher` has fixed keys, so results are stable
//! across restarts of the same binary.

use std::hash::{Hash, Hasher};

/// A pet's stable visual identity.
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
        let same_species: Vec<u16> = ids.iter()
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
}
