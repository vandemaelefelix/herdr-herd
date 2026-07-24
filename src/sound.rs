//! Notification sounds: play a short clip when a pet's status transitions
//! into a notifying state (e.g. `blocked`), behind a trait seam so tests
//! never make noise.
//!
//! Playback shells out to the OS-native player (`afplay` on macOS,
//! `paplay`/`aplay` on Linux) rather than pulling in a Rust audio crate —
//! consistent with this repo's minimal-dependency bias (see `src/herdr.rs`,
//! which does the same for the `herdr` CLI). The tradeoff is platform
//! specificity, but the plugin manifest already only targets
//! `linux`/`macos`, so that's not a real cost here.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use crate::config::SoundConfig;
use crate::herd::StatusTransition;

/// The substitution point the app depends on: play a sound file. A `Result`
/// is returned for the Real impl's own bookkeeping; callers must always
/// treat playback as best-effort (see [`play_all`]) and never let a failure
/// interrupt rendering.
pub trait SoundPlayer {
    fn play(&self, path: &Path) -> io::Result<()>;
}

/// Real playback: shell out to the first platform player that spawns
/// successfully, detached from the caller so a multi-second clip never
/// blocks the render loop.
pub struct SystemSoundPlayer;

impl SoundPlayer for SystemSoundPlayer {
    fn play(&self, path: &Path) -> io::Result<()> {
        let mut last_err = None;
        for player in candidate_players(std::env::consts::OS) {
            match spawn_detached(player, path) {
                Ok(()) => return Ok(()),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err
            .unwrap_or_else(|| io::Error::other("no sound player available for this platform")))
    }
}

/// Spawn `player path`, then reap it on a background thread so the child
/// never lingers as a zombie and the caller never waits on it.
fn spawn_detached(player: &str, path: &Path) -> io::Result<()> {
    let mut child: Child = Command::new(player)
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Candidate player programs for `os` (pass `std::env::consts::OS` in
/// production), tried in order until one spawns successfully. Empty for any
/// platform outside the plugin manifest's `linux`/`macos` targets.
fn candidate_players(os: &str) -> &'static [&'static str] {
    match os {
        "macos" => &["afplay"],
        "linux" => &["paplay", "aplay"],
        _ => &[],
    }
}

/// Which sound files (if any) to play for a batch of transitions: applies
/// the master switch and each status's own toggle/path, then de-bounces to
/// at most one sound per distinct target status — so five agents flipping to
/// `blocked` in the same tick plays the blocked sound once, not a burst of
/// five overlapping copies. Order follows first occurrence in `transitions`.
pub fn sounds_to_play(transitions: &[StatusTransition], cfg: &SoundConfig) -> Vec<PathBuf> {
    if !cfg.enabled {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for t in transitions {
        if !seen.insert(t.to) {
            continue;
        }
        let Some(setting) = cfg.for_status(t.to) else {
            continue;
        };
        if !setting.enabled {
            continue;
        }
        if let Some(path) = &setting.path {
            out.push(path.clone());
        }
    }
    out
}

/// Play every path in `paths`, ignoring individual failures: a missing file
/// or an unavailable player must never crash or block rendering.
pub fn play_all(player: &dyn SoundPlayer, paths: &[PathBuf]) {
    for p in paths {
        let _ = player.play(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentStatus;
    use std::cell::RefCell;
    use std::path::PathBuf;

    fn transition(terminal_id: &str, from: AgentStatus, to: AgentStatus) -> StatusTransition {
        StatusTransition {
            terminal_id: terminal_id.into(),
            from,
            to,
        }
    }

    /// A `SoundSetting` armed with `path`, for building test `SoundConfig`s.
    fn armed(path: &str) -> crate::config::SoundSetting {
        crate::config::SoundSetting {
            enabled: true,
            path: Some(PathBuf::from(path)),
        }
    }

    #[test]
    fn no_sounds_when_the_master_switch_is_off() {
        let cfg = SoundConfig {
            enabled: false,
            blocked: armed("/tmp/blocked.wav"),
            ..SoundConfig::default()
        };
        let transitions = [transition("a", AgentStatus::Idle, AgentStatus::Blocked)];
        assert!(sounds_to_play(&transitions, &cfg).is_empty());
    }

    #[test]
    fn no_sound_when_the_status_is_disabled() {
        let cfg = SoundConfig {
            enabled: true,
            blocked: crate::config::SoundSetting {
                enabled: false,
                path: Some(PathBuf::from("/tmp/blocked.wav")),
            },
            ..SoundConfig::default()
        };
        let transitions = [transition("a", AgentStatus::Idle, AgentStatus::Blocked)];
        assert!(sounds_to_play(&transitions, &cfg).is_empty());
    }

    #[test]
    fn no_sound_when_enabled_but_no_path_configured() {
        let cfg = SoundConfig {
            enabled: true,
            blocked: crate::config::SoundSetting {
                enabled: true,
                path: None,
            },
            ..SoundConfig::default()
        };
        let transitions = [transition("a", AgentStatus::Idle, AgentStatus::Blocked)];
        assert!(sounds_to_play(&transitions, &cfg).is_empty());
    }

    #[test]
    fn plays_the_configured_sound_for_an_enabled_transition() {
        let cfg = SoundConfig {
            enabled: true,
            blocked: armed("/tmp/blocked.wav"),
            ..SoundConfig::default()
        };
        let transitions = [transition("a", AgentStatus::Idle, AgentStatus::Blocked)];
        assert_eq!(
            sounds_to_play(&transitions, &cfg),
            vec![PathBuf::from("/tmp/blocked.wav")]
        );
    }

    #[test]
    fn never_plays_for_the_unknown_status() {
        let cfg = SoundConfig {
            enabled: true,
            idle: armed("/tmp/idle.wav"),
            ..SoundConfig::default()
        };
        let transitions = [transition("a", AgentStatus::Working, AgentStatus::Unknown)];
        assert!(sounds_to_play(&transitions, &cfg).is_empty());
    }

    #[test]
    fn debounces_a_burst_of_agents_hitting_the_same_status_to_one_sound() {
        let cfg = SoundConfig {
            enabled: true,
            blocked: armed("/tmp/blocked.wav"),
            ..SoundConfig::default()
        };
        let transitions = [
            transition("a", AgentStatus::Working, AgentStatus::Blocked),
            transition("b", AgentStatus::Idle, AgentStatus::Blocked),
            transition("c", AgentStatus::Working, AgentStatus::Blocked),
        ];
        assert_eq!(
            sounds_to_play(&transitions, &cfg),
            vec![PathBuf::from("/tmp/blocked.wav")],
            "one sound for the whole burst, not one per agent"
        );
    }

    #[test]
    fn plays_distinct_sounds_for_distinct_target_statuses_in_one_batch() {
        let cfg = SoundConfig {
            enabled: true,
            blocked: armed("/tmp/blocked.wav"),
            done: armed("/tmp/done.wav"),
            ..SoundConfig::default()
        };
        let transitions = [
            transition("a", AgentStatus::Working, AgentStatus::Blocked),
            transition("b", AgentStatus::Working, AgentStatus::Done),
        ];
        assert_eq!(
            sounds_to_play(&transitions, &cfg),
            vec![
                PathBuf::from("/tmp/blocked.wav"),
                PathBuf::from("/tmp/done.wav"),
            ]
        );
    }

    #[test]
    fn candidate_players_covers_the_manifest_platforms() {
        assert_eq!(candidate_players("macos"), &["afplay"]);
        assert_eq!(candidate_players("linux"), &["paplay", "aplay"]);
        assert_eq!(candidate_players("windows"), &[] as &[&str]);
    }

    struct Fake {
        calls: RefCell<Vec<PathBuf>>,
        fail: bool,
    }

    impl SoundPlayer for Fake {
        fn play(&self, path: &Path) -> io::Result<()> {
            self.calls.borrow_mut().push(path.to_path_buf());
            if self.fail {
                Err(io::Error::other("boom"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn play_all_invokes_the_player_for_every_path() {
        let fake = Fake {
            calls: RefCell::new(Vec::new()),
            fail: false,
        };
        let paths = vec![PathBuf::from("/a.wav"), PathBuf::from("/b.wav")];
        play_all(&fake, &paths);
        assert_eq!(*fake.calls.borrow(), paths);
    }

    #[test]
    fn play_all_ignores_individual_failures_and_keeps_going() {
        let fake = Fake {
            calls: RefCell::new(Vec::new()),
            fail: true,
        };
        let paths = vec![PathBuf::from("/a.wav"), PathBuf::from("/b.wav")];
        play_all(&fake, &paths); // must not panic
        assert_eq!(fake.calls.borrow().len(), 2, "both paths were attempted");
    }
}
