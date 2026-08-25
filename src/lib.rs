//! herdr-herd — a herd of pixel-art sheep for your herdr agents, one per agent.
//!
//! Phase 0: foundations. Modules are added task-by-task.

// `marker` is imported by the integration tests (`tests/cli.rs`); the rest of
// this list is `pub` only where `main.rs` needs it directly, since the binary
// target is a separate crate from cargo's point of view and cannot see
// `pub(crate)` items here. Everything else is `pub(crate)`, so clippy's
// dead-code lint actually runs on it instead of exempting it as public API.
pub(crate) mod agent;
pub(crate) mod anim;
pub(crate) mod base64;
pub(crate) mod caps;
pub mod config;
pub mod control;
pub(crate) mod herd;
pub mod herdr;
pub(crate) mod icon;
pub(crate) mod identity;
pub(crate) mod kitty;
pub(crate) mod kitty_ids;
pub(crate) mod kitty_render;
pub(crate) mod lock;
pub mod marker;
pub(crate) mod member;
pub(crate) mod motion;
pub mod palette;
pub mod place;
pub(crate) mod raster;
pub mod render;
pub(crate) mod sidebar;
pub(crate) mod snapshot;
pub mod socket;
pub mod sound;
pub mod sprite;
pub(crate) mod term;
pub mod watcher;
