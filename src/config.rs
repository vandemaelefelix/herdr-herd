//! Plugin configuration: a tiny, tolerant reader for the six opinionated knobs.
//! Parsed by hand (no new crate dependency) from `config.toml` in the plugin
//! config dir; any missing or malformed key degrades to its default.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::agent::AgentStatus;

/// Which rendering backend to use. `Auto` probes for kitty support and falls
/// back to half-blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererKind {
    Auto,
    Kitty,
    HalfBlock,
}

/// One status's sound: whether it's armed, and which file to play. `enabled`
/// with no `path` is inert — there's nothing to play — which lets a status
/// ship "pre-armed" without a bundled audio asset.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SoundSetting {
    pub enabled: bool,
    pub path: Option<PathBuf>,
}

/// Per-status notification sounds, gated by a master switch. Quiet by
/// default: the master switch is off, so no sound plays out of the box even
/// though `blocked` is pre-armed — flip `sounds_enabled = true` and point
/// `sound_blocked_path` at a file to hear it.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundConfig {
    pub enabled: bool,
    pub idle: SoundSetting,
    pub working: SoundSetting,
    pub blocked: SoundSetting,
    pub done: SoundSetting,
}

impl Default for SoundConfig {
    fn default() -> Self {
        SoundConfig {
            enabled: false,
            idle: SoundSetting::default(),
            working: SoundSetting::default(),
            blocked: SoundSetting {
                enabled: true,
                path: None,
            },
            done: SoundSetting::default(),
        }
    }
}

impl SoundConfig {
    /// The configured sound for `status`. `Unknown` never has one — there's
    /// no meaningful "you should come look" transition for an undetected pane.
    pub fn for_status(&self, status: AgentStatus) -> Option<&SoundSetting> {
        match status {
            AgentStatus::Idle => Some(&self.idle),
            AgentStatus::Working => Some(&self.working),
            AgentStatus::Blocked => Some(&self.blocked),
            AgentStatus::Done => Some(&self.done),
            AgentStatus::Unknown => None,
        }
    }

    fn setting_mut(&mut self, status: AgentStatus) -> Option<&mut SoundSetting> {
        match status {
            AgentStatus::Idle => Some(&mut self.idle),
            AgentStatus::Working => Some(&mut self.working),
            AgentStatus::Blocked => Some(&mut self.blocked),
            AgentStatus::Done => Some(&mut self.done),
            AgentStatus::Unknown => None,
        }
    }
}

/// The six opinionated knobs. Sensible defaults; a config file overrides them.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Whether the `control` watchdog runs at all.
    pub enabled: bool,
    /// Strip height in rows. Defaults to a slim 5, deliberately shorter than
    /// [`crate::render::STRIP_ROWS`] (the half-block band's full height): the
    /// kitty backend adapts its own band to whatever pane it's given, so a
    /// kitty user pays nothing for the slim default, while a half-block user
    /// sees the member cropped down to fit, losing headroom rather than feet
    /// (#37). A half-block user who wants the full band, uncropped, should
    /// set this to `render::STRIP_ROWS` explicitly.
    pub strip_rows: u16,
    /// Controller poll cadence in milliseconds.
    pub sweep_interval_ms: u64,
    /// Calm members — no wandering or bounce.
    pub reduced_motion: bool,
    /// Which renderer backend to use.
    pub renderer: RendererKind,
    /// Member sprite scale factor (clamped to 1..=24). See the default for why
    /// it sits near 1 transmitted pixel per screen pixel.
    pub member_scale: usize,
    /// Notification-sound settings.
    pub sounds: SoundConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            enabled: true,
            strip_rows: 5,
            sweep_interval_ms: 3000,
            reduced_motion: false,
            renderer: RendererKind::Auto,
            // The kitty backend displays a member in ~7x4 cells, i.e. roughly
            // 56x68 screen px, from a 16x19 sprite-pixel crop window. Scale 4
            // transmits ~64x76 px for that: about 1:1, with a little headroom.
            // The old default of 7 transmitted 112x133 px, 2-3x oversampled for
            // a nearest-neighbour upscale the terminal does anyway, and paid
            // for it in transmission bursts on every cold frame and resize.
            member_scale: 4,
            sounds: SoundConfig::default(),
        }
    }
}

impl Config {
    /// Parse a `config.toml` body: start from defaults and override recognized
    /// keys. Tolerant — comments (`#`), blank lines, unknown keys, and
    /// unparsable values are ignored, so a malformed config degrades to defaults
    /// rather than crashing.
    pub fn from_toml_str(s: &str) -> Config {
        let mut cfg = Config::default();
        for raw in s.lines() {
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, val)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let val = val.trim().trim_matches(['"', '\'']).trim();
            match key {
                "enabled" => {
                    if let Ok(v) = val.parse() {
                        cfg.enabled = v;
                    }
                }
                "strip_rows" => {
                    if let Ok(v) = val.parse() {
                        cfg.strip_rows = v;
                    }
                }
                "sweep_interval_ms" => {
                    if let Ok(v) = val.parse() {
                        cfg.sweep_interval_ms = v;
                    }
                }
                "reduced_motion" => {
                    if let Ok(v) = val.parse() {
                        cfg.reduced_motion = v;
                    }
                }
                "renderer" => {
                    cfg.renderer = match val {
                        "kitty" => RendererKind::Kitty,
                        "half-block" | "half_block" | "halfblock" => RendererKind::HalfBlock,
                        _ => RendererKind::Auto, // "auto" or anything unrecognized
                    };
                }
                "member_scale" => {
                    if let Ok(v) = val.parse::<usize>() {
                        cfg.member_scale = v.clamp(1, 24);
                    }
                }
                "sounds_enabled" => {
                    if let Ok(v) = val.parse() {
                        cfg.sounds.enabled = v;
                    }
                }
                "sound_idle_enabled" => set_sound_enabled(&mut cfg.sounds, AgentStatus::Idle, val),
                "sound_working_enabled" => {
                    set_sound_enabled(&mut cfg.sounds, AgentStatus::Working, val)
                }
                "sound_blocked_enabled" => {
                    set_sound_enabled(&mut cfg.sounds, AgentStatus::Blocked, val)
                }
                "sound_done_enabled" => set_sound_enabled(&mut cfg.sounds, AgentStatus::Done, val),
                "sound_idle_path" => set_sound_path(&mut cfg.sounds, AgentStatus::Idle, val),
                "sound_working_path" => set_sound_path(&mut cfg.sounds, AgentStatus::Working, val),
                "sound_blocked_path" => set_sound_path(&mut cfg.sounds, AgentStatus::Blocked, val),
                "sound_done_path" => set_sound_path(&mut cfg.sounds, AgentStatus::Done, val),
                _ => {}
            }
        }
        cfg
    }
}

/// `line` up to its first `#` that is outside a quoted string, so a value
/// like `sound_blocked_path = "/Users/felix/Music/#1 alert.wav"` keeps its
/// `#` instead of being truncated as if the rest were a comment.
fn strip_comment(line: &str) -> &str {
    let mut in_quote: Option<char> = None;
    for (i, c) in line.char_indices() {
        match in_quote {
            Some(q) => {
                if c == q {
                    in_quote = None;
                }
            }
            None => match c {
                '"' | '\'' => in_quote = Some(c),
                '#' => return &line[..i],
                _ => {}
            },
        }
    }
    line
}

/// Set `status`'s enabled flag from a raw config value; an unparsable value
/// (or an unrecognized status) is ignored, keeping the prior setting.
fn set_sound_enabled(sounds: &mut SoundConfig, status: AgentStatus, val: &str) {
    if let (Ok(v), Some(setting)) = (val.parse(), sounds.setting_mut(status)) {
        setting.enabled = v;
    }
}

/// Set `status`'s sound file path from a raw config value: empty clears it
/// back to "no sound configured", anything else becomes the path verbatim.
fn set_sound_path(sounds: &mut SoundConfig, status: AgentStatus, val: &str) {
    if let Some(setting) = sounds.setting_mut(status) {
        setting.path = if val.is_empty() {
            None
        } else {
            Some(PathBuf::from(val))
        };
    }
}

/// Read `dir/config.toml` if present; otherwise return defaults.
pub fn load_from_dir(dir: &Path) -> Config {
    match std::fs::read_to_string(dir.join("config.toml")) {
        Ok(s) => Config::from_toml_str(&s),
        Err(_) => Config::default(),
    }
}

/// `Some(path)` when `HERDR_HERD_CONFIG_DIR` names one, else `None`. Split out
/// as a pure function so the precedence is testable without touching process
/// env (same seam style as [`crate::herdr::resolve_program`]).
///
/// The override exists for the dev test harness: the installed plugin's config
/// dir is global, so without it a config change made while testing would leak
/// into the session the user actually works in.
pub fn config_dir_override(var: Option<String>) -> Option<PathBuf> {
    match var {
        Some(v) if !v.is_empty() => Some(PathBuf::from(v)),
        _ => None,
    }
}

/// Resolve the plugin config dir: `HERDR_HERD_CONFIG_DIR` if set, else ask
/// herdr (`herdr plugin config-dir herdr-herd`, plain-path stdout). Thin glue;
/// `None` on any failure.
pub fn resolve_config_dir() -> Option<PathBuf> {
    if let Some(dir) = config_dir_override(std::env::var("HERDR_HERD_CONFIG_DIR").ok()) {
        return Some(dir);
    }
    let bin = std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_string());
    let out = Command::new(bin)
        .args(["plugin", "config-dir", "herdr-herd"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8(out.stdout).ok()?;
    let path = path.trim();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

/// The effective config: from the resolved config dir, or defaults.
pub fn load() -> Config {
    resolve_config_dir()
        .map(|d| load_from_dir(&d))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_config_dir_override_wins_over_asking_herdr() {
        assert_eq!(
            config_dir_override(Some("/tmp/herd-test/config".to_string())),
            Some(PathBuf::from("/tmp/herd-test/config"))
        );
    }

    #[test]
    fn an_unset_or_empty_override_falls_through_to_asking_herdr() {
        assert_eq!(config_dir_override(None), None);
        assert_eq!(config_dir_override(Some(String::new())), None);
    }

    #[test]
    fn default_config_has_the_opinionated_values() {
        assert_eq!(
            Config::default(),
            Config {
                enabled: true,
                strip_rows: 5,
                sweep_interval_ms: 3000,
                reduced_motion: false,
                renderer: RendererKind::Auto,
                member_scale: 4,
                sounds: SoundConfig::default(),
            }
        );
    }

    #[test]
    fn from_toml_str_parses_the_core_keys() {
        let c = Config::from_toml_str(
            "enabled = false\nstrip_rows = 5\nsweep_interval_ms = 1500\nreduced_motion = true\n",
        );
        assert_eq!(
            c,
            Config {
                enabled: false,
                strip_rows: 5,
                sweep_interval_ms: 1500,
                reduced_motion: true,
                renderer: RendererKind::Auto,
                member_scale: 4,
                sounds: SoundConfig::default(),
            }
        );
    }

    #[test]
    fn from_toml_str_defaults_missing_keys_and_ignores_comments() {
        let c = Config::from_toml_str("# a comment\nreduced_motion = true  # calm\n");
        assert!(c.reduced_motion);
        assert!(c.enabled, "an unspecified key keeps its default");
        assert_eq!(c.strip_rows, 5);
    }

    #[test]
    fn from_toml_str_ignores_malformed_lines_and_unknown_keys() {
        let c = Config::from_toml_str(
            "garbage line\nunknown_key = 9\nstrip_rows = notanumber\nenabled = true\n",
        );
        assert_eq!(
            c,
            Config::default(),
            "malformed/unknown ignored; enabled=true matches default"
        );
    }

    #[test]
    fn load_from_dir_reads_config_toml_when_present() {
        let dir = std::env::temp_dir().join(format!("herdr-herd-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), "strip_rows = 9\n").unwrap();
        assert_eq!(load_from_dir(&dir).strip_rows, 9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_from_dir_defaults_when_the_file_is_absent() {
        let dir =
            std::env::temp_dir().join(format!("herdr-herd-cfg-absent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load_from_dir(&dir), Config::default());
    }

    #[test]
    fn parses_renderer_and_scale() {
        let c = Config::from_toml_str("renderer = kitty\nmember_scale = 5\n");
        assert_eq!(c.renderer, RendererKind::Kitty);
        assert_eq!(c.member_scale, 5);
    }

    #[test]
    fn renderer_defaults_to_auto_and_scale_to_four() {
        let c = Config::default();
        assert_eq!(c.renderer, RendererKind::Auto);
        assert_eq!(c.member_scale, 4);
    }

    #[test]
    fn unknown_renderer_value_falls_back_to_auto() {
        let c = Config::from_toml_str("renderer = hologram\n");
        assert_eq!(c.renderer, RendererKind::Auto);
    }

    #[test]
    fn sounds_default_to_quiet_with_blocked_pre_armed() {
        let c = Config::default();
        assert!(!c.sounds.enabled, "master switch is off out of the box");
        assert!(c.sounds.blocked.enabled, "blocked is pre-armed");
        assert_eq!(c.sounds.blocked.path, None, "but no bundled sound file");
        assert!(!c.sounds.done.enabled);
        assert!(!c.sounds.idle.enabled);
        assert!(!c.sounds.working.enabled);
    }

    #[test]
    fn from_toml_str_parses_the_sound_keys() {
        let c = Config::from_toml_str(
            "sounds_enabled = true\n\
             sound_blocked_path = /home/me/blocked.wav\n\
             sound_done_enabled = true\n\
             sound_done_path = \"/home/me/done.wav\"\n",
        );
        assert!(c.sounds.enabled);
        assert!(
            c.sounds.blocked.enabled,
            "blocked keeps its pre-armed default"
        );
        assert_eq!(
            c.sounds.blocked.path,
            Some(PathBuf::from("/home/me/blocked.wav"))
        );
        assert!(c.sounds.done.enabled);
        assert_eq!(c.sounds.done.path, Some(PathBuf::from("/home/me/done.wav")));
    }

    /// A `#` inside a quoted path is not a comment: naive `split('#')`
    /// truncation would silence this path to a directory, and `afplay`
    /// spawning successfully against a directory would report "played" with
    /// no sound and no diagnostic.
    #[test]
    fn a_hash_inside_a_quoted_sound_path_is_not_stripped_as_a_comment() {
        let c = Config::from_toml_str(
            "sound_blocked_path = \"/Users/felix/Music/#1 alert.wav\" # trailing comment\n",
        );
        assert_eq!(
            c.sounds.blocked.path,
            Some(PathBuf::from("/Users/felix/Music/#1 alert.wav"))
        );
    }

    #[test]
    fn sound_path_can_be_cleared_back_to_none() {
        let c = Config::from_toml_str("sound_blocked_path = /tmp/a.wav\nsound_blocked_path = \n");
        assert_eq!(c.sounds.blocked.path, None);
    }

    #[test]
    fn sound_enabled_can_be_turned_off_per_status() {
        let c = Config::from_toml_str("sound_blocked_enabled = false\n");
        assert!(!c.sounds.blocked.enabled);
    }

    #[test]
    fn malformed_sound_values_are_ignored() {
        let c = Config::from_toml_str("sounds_enabled = notabool\nsound_blocked_enabled = maybe\n");
        assert_eq!(c, Config::default());
    }

    #[test]
    fn for_status_has_no_sound_for_unknown() {
        let c = Config::default();
        assert!(c.sounds.for_status(AgentStatus::Unknown).is_none());
        assert!(c.sounds.for_status(AgentStatus::Blocked).is_some());
    }
}
