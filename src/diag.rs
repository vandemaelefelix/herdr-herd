//! A one-line diagnostic the render process can show without touching stdout
//! or stderr.
//!
//! The render process is a TUI: `render::run_loop` only runs after
//! `TerminalGuard::enter_raw`/`enter_screen`, so stderr shares the same tty as
//! the strip. An `eprintln!` from there (or from the watcher thread it shares
//! a process with) paints over the strip at the cursor position, stair-steps
//! because raw mode sends a bare LF with no carriage return, and — on the
//! fatal path — lands in the alternate screen and is discarded by
//! `LeaveAlternateScreen` before anyone reads it (issue #84).
//!
//! [`Diagnostic`] is the alternative: a shared latch that any thread in the
//! render process can set, and that `run_loop` reads every frame and draws
//! through the strip's own caption lane (`render::draw_caption`,
//! `kitty_render`'s caption path) instead. Both backends already draw an
//! opaque `Option<&str>` label there, so no new drawing code is needed —
//! only a place the label can come from besides the hovered member.
use std::sync::{Arc, Mutex};

/// A cheap-to-clone, thread-safe single-message latch. Every clone shares the
/// same slot, so one instance can be handed to `run_loop` and cloned into the
/// watcher thread ([`crate::watcher::watch`]) at construction time.
#[derive(Clone, Default)]
pub struct Diagnostic(Arc<Mutex<Option<String>>>);

impl Diagnostic {
    pub fn new() -> Self {
        Self::default()
    }

    /// Latch `msg`, unless a message is already latched. The first failure is
    /// the interesting one — later ones are usually just fallout of it — and
    /// a latch that kept overwriting itself every tick a still-broken source
    /// polls would fight [`Diagnostic::clear`] for no benefit.
    pub fn set(&self, msg: impl Into<String>) {
        let mut slot = self.lock();
        if slot.is_none() {
            *slot = Some(msg.into());
        }
    }

    /// Clear the latch, e.g. once a source that called [`Diagnostic::set`]
    /// observes the condition has recovered.
    pub fn clear(&self) {
        *self.lock() = None;
    }

    /// The latched message, if any.
    pub fn get(&self) -> Option<String> {
        self.lock().clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_latch_has_no_message() {
        assert_eq!(Diagnostic::new().get(), None);
    }

    #[test]
    fn set_latches_the_message() {
        let d = Diagnostic::new();
        d.set("socket subscribe failed");
        assert_eq!(d.get().as_deref(), Some("socket subscribe failed"));
    }

    #[test]
    fn a_second_set_does_not_overwrite_the_first() {
        let d = Diagnostic::new();
        d.set("first failure");
        d.set("second failure");
        assert_eq!(
            d.get().as_deref(),
            Some("first failure"),
            "the first failure is the interesting one"
        );
    }

    #[test]
    fn clear_lets_a_later_set_land() {
        let d = Diagnostic::new();
        d.set("transient failure");
        d.clear();
        assert_eq!(d.get(), None);
        d.set("next failure");
        assert_eq!(d.get().as_deref(), Some("next failure"));
    }

    #[test]
    fn clones_share_the_same_slot() {
        let d = Diagnostic::new();
        let clone = d.clone();
        clone.set("set from the clone");
        assert_eq!(d.get().as_deref(), Some("set from the clone"));
    }
}
