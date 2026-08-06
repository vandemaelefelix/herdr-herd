//! The dev build marker: which build is actually running in this strip.
//!
//! Gated behind the off-by-default `dev-marker` Cargo feature, so the marker
//! text and its layout cost are absent from a shipped binary rather than merely
//! hidden at runtime. The stamp itself comes from `build.rs`, which restamps on
//! every rebuild, so two dev builds of the same commit are distinguishable.

/// The marker text for this build: version, commit, and build time.
#[cfg(feature = "dev-marker")]
const MARKER: &str = concat!(
    "v",
    env!("CARGO_PKG_VERSION"),
    " ",
    env!("HERDR_HERD_BUILD")
);

/// The marker text for this build, or `None` in a shipped build.
#[cfg(feature = "dev-marker")]
pub fn build_marker() -> Option<&'static str> {
    Some(MARKER)
}

/// The marker text for this build, or `None` in a shipped build.
#[cfg(not(feature = "dev-marker"))]
pub fn build_marker() -> Option<&'static str> {
    None
}

/// Overlay-lane columns the marker occupies, including the gap that separates
/// it from anything right-aligned in the same lane. `0` in a shipped build, so
/// feeding this into the renderers leaves shipped layout byte-identical.
pub fn reserved_cols() -> u16 {
    match build_marker() {
        Some(m) => m.chars().count() as u16 + 1,
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(feature = "dev-marker"))]
    fn a_shipped_build_carries_no_marker_and_reserves_no_columns() {
        assert_eq!(build_marker(), None);
        assert_eq!(reserved_cols(), 0);
    }

    #[test]
    #[cfg(feature = "dev-marker")]
    fn a_dev_build_marker_names_the_crate_version() {
        let marker = build_marker().expect("a dev build has a marker");
        assert!(
            marker.contains(env!("CARGO_PKG_VERSION")),
            "marker should name the version so a stale build is obvious: {marker:?}"
        );
    }

    #[test]
    #[cfg(feature = "dev-marker")]
    fn a_dev_build_marker_is_one_short_single_line_string() {
        let marker = build_marker().expect("a dev build has a marker");
        assert!(!marker.is_empty(), "marker should not be empty");
        assert!(
            !marker.contains('\n'),
            "marker shares a single overlay row: {marker:?}"
        );
        assert!(
            marker.chars().count() <= 32,
            "marker must stay narrow enough for a slim strip: {marker:?}"
        );
    }

    #[test]
    #[cfg(feature = "dev-marker")]
    fn a_dev_build_reserves_the_marker_width_plus_a_separating_gap() {
        let marker = build_marker().expect("a dev build has a marker");
        assert_eq!(reserved_cols(), marker.chars().count() as u16 + 1);
    }
}
