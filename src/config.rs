//! Plugin configuration: a tiny, tolerant reader for the six opinionated knobs.
//! Parsed by hand (no new crate dependency) from `config.toml` in the plugin
//! config dir; any missing or malformed key degrades to its default.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Which rendering backend to use. `Auto` probes for kitty support and falls
/// back to half-blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererKind {
    Auto,
    Kitty,
    HalfBlock,
}

/// The six opinionated knobs. Sensible defaults; a config file overrides them.
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
    /// Which renderer backend to use.
    pub renderer: RendererKind,
    /// Pet sprite scale factor (clamped to 1..=24).
    pub pet_scale: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            enabled: true,
            strip_rows: 5,
            sweep_interval_ms: 3000,
            reduced_motion: false,
            renderer: RendererKind::Auto,
            pet_scale: 7,
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
                "renderer" => {
                    cfg.renderer = match val {
                        "kitty" => RendererKind::Kitty,
                        "half-block" | "half_block" | "halfblock" => RendererKind::HalfBlock,
                        _ => RendererKind::Auto, // "auto" or anything unrecognized
                    };
                }
                "pet_scale" => {
                    if let Ok(v) = val.parse::<usize>() {
                        cfg.pet_scale = v.clamp(1, 24);
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
                reduced_motion: false,
                renderer: RendererKind::Auto,
                pet_scale: 7,
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
                pet_scale: 7,
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

    #[test]
    fn parses_renderer_and_scale() {
        let c = Config::from_toml_str("renderer = kitty\npet_scale = 5\n");
        assert_eq!(c.renderer, RendererKind::Kitty);
        assert_eq!(c.pet_scale, 5);
    }

    #[test]
    fn renderer_defaults_to_auto_and_scale_to_seven() {
        let c = Config::default();
        assert_eq!(c.renderer, RendererKind::Auto);
        assert_eq!(c.pet_scale, 7);
    }

    #[test]
    fn unknown_renderer_value_falls_back_to_auto() {
        let c = Config::from_toml_str("renderer = hologram\n");
        assert_eq!(c.renderer, RendererKind::Auto);
    }
}
