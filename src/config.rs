//! Plugin configuration: a tiny, tolerant reader for the four opinionated knobs.
//! Parsed by hand (no new crate dependency) from `config.toml` in the plugin
//! config dir; any missing or malformed key degrades to its default.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The four opinionated knobs. Sensible defaults; a config file overrides them.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Whether the `control` watchdog runs at all.
    pub enabled: bool,
    /// Strip height in rows.
    pub strip_rows: u16,
    /// Controller poll cadence in milliseconds.
    pub sweep_interval_ms: u64,
    /// Calm pets — no wandering or bounce.
    pub reduced_motion: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            enabled: true,
            strip_rows: 5,
            sweep_interval_ms: 3000,
            reduced_motion: false,
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
            let line = raw.split('#').next().unwrap_or("").trim();
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
                _ => {}
            }
        }
        cfg
    }
}

/// Read `dir/config.toml` if present; otherwise return defaults.
pub fn load_from_dir(dir: &Path) -> Config {
    match std::fs::read_to_string(dir.join("config.toml")) {
        Ok(s) => Config::from_toml_str(&s),
        Err(_) => Config::default(),
    }
}

/// Resolve the plugin config dir by asking herdr (`herdr plugin config-dir
/// herdr-pets`, plain-path stdout). Thin glue; `None` on any failure.
pub fn resolve_config_dir() -> Option<PathBuf> {
    let bin = std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_string());
    let out = Command::new(bin)
        .args(["plugin", "config-dir", "herdr-pets"])
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
    fn default_config_has_the_opinionated_values() {
        assert_eq!(
            Config::default(),
            Config {
                enabled: true,
                strip_rows: 5,
                sweep_interval_ms: 3000,
                reduced_motion: false
            }
        );
    }

    #[test]
    fn from_toml_str_parses_all_four_keys() {
        let c = Config::from_toml_str(
            "enabled = false\nstrip_rows = 5\nsweep_interval_ms = 1500\nreduced_motion = true\n",
        );
        assert_eq!(
            c,
            Config {
                enabled: false,
                strip_rows: 5,
                sweep_interval_ms: 1500,
                reduced_motion: true
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
        let dir = std::env::temp_dir().join(format!("herdr-pets-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), "strip_rows = 9\n").unwrap();
        assert_eq!(load_from_dir(&dir).strip_rows, 9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_from_dir_defaults_when_the_file_is_absent() {
        let dir =
            std::env::temp_dir().join(format!("herdr-pets-cfg-absent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load_from_dir(&dir), Config::default());
    }
}
