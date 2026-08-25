//! Notification sounds: play a short clip when a member's status transitions
//! into a notifying state (e.g. `blocked`), behind a trait seam so tests
//! never make noise.
//!
//! **Sound is claimed once per transition per session, not once per pane.** A
//! session runs one render process per pane, each with its own watcher and its
//! own reconcile, so all of them observe the same transition within
//! milliseconds of each other. Left alone that is N overlapping copies of one
//! clip and N process spawns. [`play_claimed`] asks a [`TransitionClaim`]
//! first, and the panes that lose it stay silent. The claim lives on the
//! filesystem ([`FileTransitionClaim`]) because the panes are separate
//! processes. A pane that loses skips immediately: it never blocks and never
//! waits, because the render loop runs at ~12 fps. Playback stays best-effort
//! throughout, so a failed claim, a missing claim directory or a read-only
//! filesystem all degrade to silence, never to an error reaching the render
//! loop.
//!
//! Playback shells out to the OS-native player (`afplay` on macOS,
//! `paplay`/`aplay` on Linux) rather than pulling in a Rust audio crate —
//! consistent with this repo's minimal-dependency bias (see `src/herdr.rs`,
//! which does the same for the `herdr` CLI). The tradeoff is platform
//! specificity, but the plugin manifest already only targets
//! `linux`/`macos`, so that's not a real cost here.

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::fs::{File, OpenOptions, TryLockError};
use std::hash::{Hash, Hasher};
use std::io;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Once, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

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
        let mut errors = Vec::new();
        for player in candidate_players(std::env::consts::OS) {
            match spawn_detached(player, path) {
                Ok(()) => return Ok(()),
                // Keep every candidate's error, not just the last one: on
                // Linux a `paplay` failure (the informative one, e.g. no
                // PulseAudio socket) would otherwise be discarded the moment
                // the `aplay` fallback also fails.
                Err(e) => errors.push(format!("{player}: {e}")),
            }
        }
        Err(playback_error(errors))
    }
}

/// The final playback error to report, given every candidate player's own
/// message: joined, so the first (often the most informative) one survives
/// alongside the rest, or a generic message when there was nothing to even
/// attempt (an unsupported OS, whose [`candidate_players`] list is empty).
fn playback_error(errors: Vec<String>) -> io::Error {
    io::Error::other(if errors.is_empty() {
        "no sound player available for this platform".to_string()
    } else {
        errors.join("; ")
    })
}

/// Spawn `player path`, then hand it to the session's single reaper thread so
/// the child never lingers as a zombie and the caller never waits on it. One
/// thread for the whole process, not one per clip (#74): a clip lives a
/// second or two, and a session can play many of them.
fn spawn_detached(player: &str, path: &Path) -> io::Result<()> {
    let child: Child = Command::new(player)
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // The reaper thread never exits (its receiver stays alive as long as the
    // process does), so a send failure would mean it panicked. Either way, a
    // child this loses track of is no worse than the old per-clip thread
    // dying before `wait()` — a transient zombie, not a crash.
    let _ = reaper().send(child);
    Ok(())
}

/// The session's single reaper thread, started on first use. It parks on
/// `rx` and `wait()`s each child as it arrives, replacing what used to be a
/// dedicated `std::thread::spawn` per clip.
fn reaper() -> &'static Sender<Child> {
    static REAPER: OnceLock<Sender<Child>> = OnceLock::new();
    REAPER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Child>();
        std::thread::spawn(move || {
            for mut child in rx {
                let _ = child.wait();
            }
        });
        tx
    })
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
        if let Some(path) = armed_path(cfg, t.to) {
            out.push(path.clone());
        }
    }
    out
}

/// The file configured for `status`, if it is armed and pointed at one. Shared
/// by [`sounds_to_play`] and the claim gate so both agree on what "has a sound"
/// means. The gate leans on it to keep a quiet config off the filesystem.
fn armed_path(cfg: &SoundConfig, status: crate::agent::AgentStatus) -> Option<&PathBuf> {
    let setting = cfg.for_status(status)?;
    if !setting.enabled {
        return None;
    }
    setting.path.as_ref()
}

/// Play every path in `paths`, ignoring individual failures: a missing file
/// or an unavailable player must never crash or block rendering. This is the
/// unconditional primitive; app code goes through [`play_claimed`] so N panes
/// watching one transition do not each play it.
pub fn play_all(player: &dyn SoundPlayer, paths: &[PathBuf]) {
    for p in paths {
        if let Err(e) = player.play(p) {
            warn_once(&e);
        }
    }
}

/// Tell the user, once per process, that a sound failed to play. Playback
/// stays best-effort either way (see the module doc), but silence with no
/// explanation (#74) is a worse default than one stderr line the first time
/// it happens: enough to point at a missing player without spamming a pane
/// that keeps failing the same way.
fn warn_once(err: &io::Error) {
    static WARNED: Once = Once::new();
    WARNED.call_once(|| {
        eprintln!("herdr-herd: sound playback failed and will stay silent for this pane: {err}");
    });
}

/// Play the sounds for `transitions` that this process claims, ignoring the
/// rest. This is the whole fan-out fix in one call: every pane in the session
/// sees the same transition and calls this, and exactly one of them makes a
/// noise.
///
/// The order matters. Config gating comes first, so a quiet install (the
/// default, `config::SoundConfig::default`) never touches the filesystem;
/// claiming comes next, per transition, so panes that batched the same events
/// differently still agree on who owns each one; de-bouncing to one sound per
/// status comes last, so a burst of agents is one sound for the winner rather
/// than one per member.
pub fn play_claimed(
    player: &dyn SoundPlayer,
    claim: &dyn TransitionClaim,
    transitions: &[StatusTransition],
    cfg: &SoundConfig,
) {
    let mine = claimed_transitions(claim, transitions, cfg);
    play_all(player, &sounds_to_play(&mine, cfg));
}

/// The transitions in `transitions` whose sound this process owns: the ones
/// with a sound configured at all, that `claim` handed to us.
fn claimed_transitions(
    claim: &dyn TransitionClaim,
    transitions: &[StatusTransition],
    cfg: &SoundConfig,
) -> Vec<StatusTransition> {
    if !cfg.enabled {
        return Vec::new();
    }
    transitions
        .iter()
        .filter(|t| armed_path(cfg, t.to).is_some())
        .filter(|t| claim.claim(t))
        .cloned()
        .collect()
}

/// A claim store for this herdr session, or a claim that never grants anything
/// if one cannot be created. Silence is the right degradation: without a shared
/// store there is no way to tell which pane owns a transition, and N panes each
/// playing is the bug being fixed.
pub fn session_claim() -> Box<dyn TransitionClaim> {
    let dir = claim_dir();
    match std::fs::create_dir_all(&dir) {
        Ok(()) => Box::new(FileTransitionClaim::new(
            dir,
            Box::new(SystemEpochClock),
            CLAIM_WINDOW_MS,
        )),
        Err(_) => Box::new(NeverClaims),
    }
}

/// Where this session's claims live: beside the herdr socket if there is one,
/// else the system temp dir. The directory name embeds a hash of the full
/// socket path, so two herdr sessions sharing a socket parent directory keep
/// separate claims, since one session's sounds must not mute another's. Mirrors
/// `control::controller_lock_path`, which scopes the controller lock the same
/// way. Nothing sweeps the directory: it holds at most one small file per
/// (member, status) pair the session ever saw, and panes reuse those files
/// rather than adding to them.
fn claim_dir() -> PathBuf {
    crate::socket::socket_path()
        .and_then(|p| {
            let parent = p.parent()?.to_path_buf();
            let mut hasher = DefaultHasher::new();
            p.to_string_lossy().hash(&mut hasher);
            Some(parent.join(format!("herdr-herd-sounds-{:x}", hasher.finish())))
        })
        .unwrap_or_else(|| std::env::temp_dir().join("herdr-herd-sounds"))
}

/// The fallback claim: grants nothing, so playback degrades to silence when the
/// claim store is unusable.
struct NeverClaims;

impl TransitionClaim for NeverClaims {
    fn claim(&self, _t: &StatusTransition) -> bool {
        false
    }
}

/// A source of wall-clock milliseconds since the unix epoch, behind a seam so
/// tests never touch the real clock. Wall clock rather than the monotonic
/// [`crate::watcher::Clock`]: the stamp a claim leaves behind is read by *other*
/// processes, which share an epoch but not a monotonic origin.
pub trait EpochClock {
    fn now_ms(&self) -> u64;
}

/// The real clock. A pre-epoch system clock reads as 0, which is stale by
/// construction, so the worst a broken clock buys is an extra sound.
pub struct SystemEpochClock;

impl EpochClock for SystemEpochClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// Decides whether *this* process is the one that plays a given transition.
/// Every pane runs its own watcher and its own reconcile, so all of them see
/// the same transition within a moment of each other; the claim is what keeps
/// one event to one sound (see [`play_claimed`]).
pub trait TransitionClaim {
    /// `true` if this process may play the sound for `t`. A loser skips
    /// silently: it never blocks and never retries, because the render loop
    /// runs at ~12 fps and must not stall on sound.
    fn claim(&self, t: &StatusTransition) -> bool;
}

/// How long one claimed transition stays claimed. Panes poll independently
/// (the watcher's slow refetch is 2500 ms), so the last pane to notice a
/// transition can be that far behind the first; the window has to outlast that
/// spread or the straggler plays a second copy. The price of a window this
/// coarse is that the same member re-entering the same status inside it is
/// silent, which is a de-bounce most people would ask for anyway.
pub const CLAIM_WINDOW_MS: u64 = 3_000;

/// A claim recorded on the filesystem, so it holds across the separate
/// processes one herdr session spawns. Each transition identity gets a small
/// file whose contents are the wall-clock stamp of the last sound played for
/// it; a pane wins only if that stamp is missing or older than `window_ms`.
///
/// The file is not a lock held across the sound, it is a *record that the sound
/// happened*. That is what closes the race: all N panes observe the transition
/// within milliseconds, and a bare try_lock/release window is far too narrow
/// for the stragglers to notice. A late pane reads the stamp instead of racing
/// for a lock that was taken and released long before it looked. `try_lock`
/// still guards the read-then-write, so two panes taking over the same expired
/// claim in the same instant cannot both win.
pub struct FileTransitionClaim {
    dir: PathBuf,
    clock: Box<dyn EpochClock>,
    window_ms: u64,
}

impl FileTransitionClaim {
    /// Claims recorded in `dir`, each held for `window_ms`. `dir` must already
    /// exist: a missing or unwritable one degrades to silence rather than to an
    /// error (see [`session_claim`]).
    pub fn new(dir: PathBuf, clock: Box<dyn EpochClock>, window_ms: u64) -> Self {
        Self {
            dir,
            clock,
            window_ms,
        }
    }

    /// The claim attempt with its I/O errors intact. [`TransitionClaim::claim`]
    /// is the caller-facing half and swallows them.
    fn try_claim(&self, t: &StatusTransition) -> io::Result<bool> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(self.dir.join(claim_file_name(t)))?;
        match file.try_lock() {
            Ok(()) => {}
            // Another pane is deciding this very transition right now. It will
            // either play it or find it already played; either way, not ours.
            Err(TryLockError::WouldBlock) => return Ok(false),
            Err(TryLockError::Error(e)) => return Err(e),
        }
        let now = self.clock.now_ms();
        if let Some(last) = read_stamp(&file)? {
            // abs_diff, not a subtraction: a stamp from the future (a clock
            // stepped backwards between two panes) is stale, not eternal.
            if now.abs_diff(last) < self.window_ms {
                return Ok(false);
            }
        }
        write_stamp(&file, now)?;
        Ok(true)
    }
}

impl TransitionClaim for FileTransitionClaim {
    fn claim(&self, t: &StatusTransition) -> bool {
        self.try_claim(t).unwrap_or(false)
    }
}

/// The file that records one transition's identity: which member, and which
/// status it reached. Deliberately *not* the status it came from: two panes
/// polling at different moments can disagree about that for the same event
/// (one sees idle→working→blocked, the other only idle→blocked), and the sound
/// is chosen by the target status anyway. Hashed so an arbitrary terminal id
/// can never escape into the path.
fn claim_file_name(t: &StatusTransition) -> String {
    let mut hasher = DefaultHasher::new();
    t.terminal_id.hash(&mut hasher);
    t.to.hash(&mut hasher);
    format!("{:x}.claim", hasher.finish())
}

/// The stamp in `file`, or `None` if it is empty or unreadable as a number.
/// A corrupt claim must be takeable, not a permanent mute.
fn read_stamp(mut file: &File) -> io::Result<Option<u64>> {
    let mut raw = Vec::new();
    file.read_to_end(&mut raw)?;
    Ok(String::from_utf8_lossy(&raw).trim().parse().ok())
}

/// Overwrite `file` with `now_ms`, the instant this claim's sound is played.
fn write_stamp(mut file: &File, now_ms: u64) -> io::Result<()> {
    file.seek(io::SeekFrom::Start(0))?;
    file.set_len(0)?;
    file.write_all(now_ms.to_string().as_bytes())
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

    #[test]
    fn playback_error_keeps_every_candidates_message_not_just_the_last() {
        let err = playback_error(vec![
            "paplay: no pulseaudio socket".to_string(),
            "aplay: not found".to_string(),
        ]);
        let msg = err.to_string();
        assert!(
            msg.contains("paplay: no pulseaudio socket"),
            "the first, more informative candidate's error must survive: {msg}"
        );
        assert!(
            msg.contains("aplay: not found"),
            "the last candidate's error must survive too: {msg}"
        );
    }

    #[test]
    fn playback_error_is_generic_when_no_candidate_was_even_tried() {
        let err = playback_error(Vec::new());
        assert!(
            err.to_string().contains("no sound player available"),
            "an unsupported OS (empty candidate list) gets a plain explanation"
        );
    }

    #[test]
    fn the_reaper_thread_is_created_once_and_reused() {
        // One long-lived reaper, not a thread per clip (#74): `reaper()`
        // must hand back the very same sender on every call, never spin up
        // a fresh channel/thread pair.
        let first: *const Sender<Child> = reaper();
        let second: *const Sender<Child> = reaper();
        assert_eq!(
            first, second,
            "the reaper's sender is created once and reused, not per call"
        );
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

    /// A clock the test drives by hand. Shared (via `Rc`) between claimants so
    /// two simulated panes observe the same instant, the way two real panes
    /// racing on one transition do.
    #[derive(Clone)]
    struct FakeClock(std::rc::Rc<std::cell::Cell<u64>>);

    impl FakeClock {
        fn at(ms: u64) -> Self {
            FakeClock(std::rc::Rc::new(std::cell::Cell::new(ms)))
        }

        fn set(&self, ms: u64) {
            self.0.set(ms);
        }
    }

    impl EpochClock for FakeClock {
        fn now_ms(&self) -> u64 {
            self.0.get()
        }
    }

    /// A fresh, empty claim directory for one test. The name carries the test's
    /// own tag so tests running in parallel never share a directory.
    fn claim_dir_for(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "herdr-herd-claim-test-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp claim directory is creatable");
        dir
    }

    /// One simulated pane's claimant, over `dir` and driven by `clock`.
    fn pane(dir: &Path, clock: &FakeClock) -> FileTransitionClaim {
        FileTransitionClaim::new(dir.to_path_buf(), Box::new(clock.clone()), CLAIM_WINDOW_MS)
    }

    #[test]
    fn the_first_pane_to_claim_a_transition_wins_and_the_second_loses_it() {
        let dir = claim_dir_for("first-wins");
        let clock = FakeClock::at(1_000);
        let t = transition("a", AgentStatus::Working, AgentStatus::Blocked);

        let first = pane(&dir, &clock);
        let second = pane(&dir, &clock);

        assert!(first.claim(&t), "the first pane to see it owns the sound");
        assert!(
            !second.claim(&t),
            "a second pane recognises the sound as already played"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pane_that_saw_a_different_previous_status_still_loses_the_claim() {
        // Panes poll independently, so one can observe working->blocked where
        // another observes idle->blocked for the same agent. Same event, and
        // the sound is chosen by the target status, so it must be one claim.
        let dir = claim_dir_for("ignores-from");
        let clock = FakeClock::at(1_000);

        let first = pane(&dir, &clock);
        let second = pane(&dir, &clock);

        assert!(first.claim(&transition("a", AgentStatus::Working, AgentStatus::Blocked)));
        assert!(
            !second.claim(&transition("a", AgentStatus::Idle, AgentStatus::Blocked)),
            "the claim keys on the member and the target status, not on where it came from"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_members_reaching_the_same_status_are_claimed_separately() {
        let dir = claim_dir_for("per-member");
        let clock = FakeClock::at(1_000);
        let only = pane(&dir, &clock);

        assert!(only.claim(&transition("a", AgentStatus::Working, AgentStatus::Blocked)));
        assert!(
            only.claim(&transition("b", AgentStatus::Working, AgentStatus::Blocked)),
            "a different member's transition is a different claim"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_same_transition_is_claimable_again_once_the_window_has_passed() {
        let dir = claim_dir_for("window-expiry");
        let clock = FakeClock::at(1_000);
        let t = transition("a", AgentStatus::Working, AgentStatus::Blocked);
        let only = pane(&dir, &clock);

        assert!(only.claim(&t));
        clock.set(1_000 + CLAIM_WINDOW_MS);
        assert!(
            only.claim(&t),
            "a genuinely new occurrence past the window is audible again"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn losing_a_claim_does_not_push_the_window_forward() {
        // Otherwise a busy agent flipping status faster than the window could
        // starve the sound forever: every loss would re-arm the window.
        let dir = claim_dir_for("no-starvation");
        let clock = FakeClock::at(0);
        let t = transition("a", AgentStatus::Working, AgentStatus::Blocked);
        let first = pane(&dir, &clock);
        let second = pane(&dir, &clock);

        assert!(first.claim(&t));
        clock.set(CLAIM_WINDOW_MS - 1);
        assert!(!second.claim(&t), "still inside the window");
        clock.set(CLAIM_WINDOW_MS);
        assert!(
            first.claim(&t),
            "the window is measured from the last sound played, not the last attempt"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unusable_claim_directory_degrades_to_silence_not_to_an_error() {
        let dir = claim_dir_for("missing-dir").join("never-created");
        let clock = FakeClock::at(1_000);
        let only = pane(&dir, &clock);

        assert!(
            !only.claim(&transition("a", AgentStatus::Working, AgentStatus::Blocked)),
            "no claim directory means no sound, never a failure the caller must handle"
        );
        let _ = std::fs::remove_dir_all(dir.parent().expect("the temp parent exists"));
    }

    #[test]
    fn a_corrupt_claim_file_is_taken_over_instead_of_silencing_the_transition() {
        let dir = claim_dir_for("corrupt-file");
        let t = transition("a", AgentStatus::Working, AgentStatus::Blocked);
        std::fs::write(dir.join(claim_file_name(&t)), b"not a timestamp")
            .expect("the claim file is writable");
        let clock = FakeClock::at(1_000);

        assert!(
            pane(&dir, &clock).claim(&t),
            "an unreadable stamp is treated as no stamp at all"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stamp_from_the_future_does_not_silence_a_transition_forever() {
        // The stamp is wall-clock, so a clock that steps backwards (an NTP
        // correction between two panes) must not leave a claim nobody can take.
        let dir = claim_dir_for("clock-went-back");
        let t = transition("a", AgentStatus::Working, AgentStatus::Blocked);
        let clock = FakeClock::at(10 * CLAIM_WINDOW_MS);
        let only = pane(&dir, &clock);

        assert!(only.claim(&t));
        clock.set(0);
        assert!(
            only.claim(&t),
            "a stamp further away than the window in either direction is stale"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// How many claims `dir` holds, for asserting that a quiet config never
    /// touches the filesystem at all.
    fn claim_count(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .map(|entries| entries.count())
            .unwrap_or(0)
    }

    /// A config with `blocked` armed at `/tmp/blocked.wav` and the master
    /// switch on.
    fn blocked_armed() -> SoundConfig {
        SoundConfig {
            enabled: true,
            blocked: armed("/tmp/blocked.wav"),
            ..SoundConfig::default()
        }
    }

    #[test]
    fn exactly_one_of_two_panes_plays_the_sound_for_the_same_transition() {
        // The bug this fixes: every pane runs its own watcher, so all of them
        // reconcile the same transition and each spawned its own player.
        let dir = claim_dir_for("no-fanout");
        let clock = FakeClock::at(1_000);
        let cfg = blocked_armed();
        let transitions = [transition("a", AgentStatus::Working, AgentStatus::Blocked)];

        let first_speaker = Fake {
            calls: RefCell::new(Vec::new()),
            fail: false,
        };
        let second_speaker = Fake {
            calls: RefCell::new(Vec::new()),
            fail: false,
        };
        play_claimed(&first_speaker, &pane(&dir, &clock), &transitions, &cfg);
        play_claimed(&second_speaker, &pane(&dir, &clock), &transitions, &cfg);

        assert_eq!(
            first_speaker.calls.borrow().len() + second_speaker.calls.borrow().len(),
            1,
            "one transition is one sound, however many panes are watching"
        );
        assert_eq!(
            *first_speaker.calls.borrow(),
            vec![PathBuf::from("/tmp/blocked.wav")],
            "and it is the pane that claimed it that plays"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_burst_across_panes_is_still_a_single_sound() {
        let dir = claim_dir_for("burst-across-panes");
        let clock = FakeClock::at(1_000);
        let cfg = blocked_armed();
        let transitions = [
            transition("a", AgentStatus::Working, AgentStatus::Blocked),
            transition("b", AgentStatus::Idle, AgentStatus::Blocked),
        ];

        let first_speaker = Fake {
            calls: RefCell::new(Vec::new()),
            fail: false,
        };
        let second_speaker = Fake {
            calls: RefCell::new(Vec::new()),
            fail: false,
        };
        play_claimed(&first_speaker, &pane(&dir, &clock), &transitions, &cfg);
        play_claimed(&second_speaker, &pane(&dir, &clock), &transitions, &cfg);

        assert_eq!(
            first_speaker.calls.borrow().len(),
            1,
            "the winning pane still de-bounces the burst to one sound"
        );
        assert!(
            second_speaker.calls.borrow().is_empty(),
            "and the losing pane is silent for every member in the burst"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pane_that_missed_the_first_batch_still_plays_a_later_distinct_transition() {
        let dir = claim_dir_for("later-transition");
        let clock = FakeClock::at(1_000);
        let cfg = SoundConfig {
            enabled: true,
            blocked: armed("/tmp/blocked.wav"),
            done: armed("/tmp/done.wav"),
            ..SoundConfig::default()
        };
        let winner = pane(&dir, &clock);
        let speaker = Fake {
            calls: RefCell::new(Vec::new()),
            fail: false,
        };

        play_claimed(
            &speaker,
            &winner,
            &[transition("a", AgentStatus::Working, AgentStatus::Blocked)],
            &cfg,
        );
        play_claimed(
            &speaker,
            &winner,
            &[transition("a", AgentStatus::Blocked, AgentStatus::Done)],
            &cfg,
        );

        assert_eq!(
            *speaker.calls.borrow(),
            vec![
                PathBuf::from("/tmp/blocked.wav"),
                PathBuf::from("/tmp/done.wav"),
            ],
            "claiming one transition must not mute the next one"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_quiet_config_never_touches_the_claim_directory() {
        let dir = claim_dir_for("quiet-config");
        let clock = FakeClock::at(1_000);
        let cfg = SoundConfig {
            enabled: false,
            blocked: armed("/tmp/blocked.wav"),
            ..SoundConfig::default()
        };
        let speaker = Fake {
            calls: RefCell::new(Vec::new()),
            fail: false,
        };

        play_claimed(
            &speaker,
            &pane(&dir, &clock),
            &[transition("a", AgentStatus::Working, AgentStatus::Blocked)],
            &cfg,
        );

        assert!(speaker.calls.borrow().is_empty());
        assert_eq!(
            claim_count(&dir),
            0,
            "sounds are off by default, so the default install does no claim I/O at all"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_status_with_no_configured_sound_never_touches_the_claim_directory() {
        let dir = claim_dir_for("disarmed-status");
        let clock = FakeClock::at(1_000);
        let cfg = blocked_armed();
        let speaker = Fake {
            calls: RefCell::new(Vec::new()),
            fail: false,
        };

        play_claimed(
            &speaker,
            &pane(&dir, &clock),
            &[transition("a", AgentStatus::Working, AgentStatus::Done)],
            &cfg,
        );

        assert!(speaker.calls.borrow().is_empty());
        assert_eq!(claim_count(&dir), 0, "nothing to play, nothing to claim");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unclaimable_transition_plays_nothing_and_does_not_panic() {
        let dir = claim_dir_for("unclaimable").join("never-created");
        let clock = FakeClock::at(1_000);
        let cfg = blocked_armed();
        let speaker = Fake {
            calls: RefCell::new(Vec::new()),
            fail: false,
        };

        play_claimed(
            &speaker,
            &pane(&dir, &clock),
            &[transition("a", AgentStatus::Working, AgentStatus::Blocked)],
            &cfg,
        );

        assert!(
            speaker.calls.borrow().is_empty(),
            "an unusable claim store degrades to silence, never to a failure"
        );
        let _ = std::fs::remove_dir_all(dir.parent().expect("the temp parent exists"));
    }
}
