//! Sprite data format: role-painted text, one file per animal, all five states
//! and their frames inside. Roles (not colors) so tinting + theming are free.
//! Loaded embedded by default; `$HERDR_PETS_SPRITES` overrides by name.

use std::collections::BTreeMap;

use crate::agent::AgentStatus;
use crate::anim::{MotionSpec, OverlaySpec, parse_motion, parse_overlay};

/// A legend symbol's meaning: what part of the pet a pixel paints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Transparent,
    Outline,
    Eye,
    Skin,
    Horn,
    CoatLight,
    CoatMid,
    CoatShadow,
    Accent,
}

/// Map one legend character to its `Role`. `None` for anything not in the legend.
pub fn role_from_char(c: char) -> Option<Role> {
    Some(match c {
        '.' | ' ' => Role::Transparent,
        '#' => Role::Outline,
        'e' => Role::Eye,
        'p' => Role::Skin,
        'h' => Role::Horn,
        'L' => Role::CoatLight,
        'M' => Role::CoatMid,
        'S' => Role::CoatShadow,
        'a' => Role::Accent,
        _ => return None,
    })
}

/// One role-painted pixel grid, row-major (`cells.len() == w * h`).
#[derive(Debug, Clone)]
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub cells: Vec<Role>,
}

/// One agent-status state's animation: its frames plus timing/motion/overlay config.
#[derive(Debug, Clone)]
pub struct StateSpec {
    pub frames: Vec<Frame>,
    pub frame_ms: u32,
    pub motion: MotionSpec,
    pub overlay: OverlaySpec,
    pub dim: bool,
    pub ghost: bool,
}

/// A parsed `.sprite` file: a name plus one `StateSpec` per `AgentStatus`.
#[derive(Debug, Clone)]
pub struct Species {
    pub name: String,
    pub states: BTreeMap<AgentStatus, StateSpec>,
}

impl Species {
    /// The (w, h) of this species' frames — taken from whichever state/frame
    /// comes first; all frames in a species share one size.
    pub fn size(&self) -> (usize, usize) {
        self.states
            .values()
            .next()
            .and_then(|s| s.frames.first())
            .map(|f| (f.w, f.h))
            .unwrap_or((0, 0))
    }
}

fn status_from_key(k: &str) -> Option<AgentStatus> {
    Some(match k {
        "idle" => AgentStatus::Idle,
        "working" => AgentStatus::Working,
        "done" => AgentStatus::Done,
        "blocked" => AgentStatus::Blocked,
        "unknown" => AgentStatus::Unknown,
        _ => return None,
    })
}

/// Find `key=value` in a state header's trailing tokens and return `value`.
fn kv<'a>(header: &'a str, key: &str) -> Option<&'a str> {
    header
        .split_whitespace()
        .find_map(|tok| tok.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

fn parse_frame(lines: &[&str]) -> Result<Frame, String> {
    let w = lines[0].chars().count();
    let mut cells = Vec::with_capacity(w * lines.len());
    for line in lines {
        if line.chars().count() != w {
            return Err("ragged frame (rows differ in width)".into());
        }
        for c in line.chars() {
            cells.push(role_from_char(c).ok_or_else(|| format!("illegal symbol '{c}'"))?);
        }
    }
    Ok(Frame {
        w,
        h: lines.len(),
        cells,
    })
}

/// Parse one `.sprite` file into a `Species`.
pub fn parse_species(src: &str) -> Result<Species, String> {
    let mut name = String::new();
    let mut states = BTreeMap::new();
    let mut cur_status: Option<AgentStatus> = None;
    let mut cur_header = String::new();
    let mut frames: Vec<Frame> = Vec::new();
    let mut buf: Vec<&str> = Vec::new();

    // Helper to flush the accumulated frame block into the current state.
    fn flush(buf: &mut Vec<&str>, frames: &mut Vec<Frame>) -> Result<(), String> {
        if !buf.is_empty() {
            frames.push(parse_frame(buf)?);
            buf.clear();
        }
        Ok(())
    }
    fn commit(
        states: &mut BTreeMap<AgentStatus, StateSpec>,
        status: Option<AgentStatus>,
        header: &str,
        frames: &mut Vec<Frame>,
    ) -> Result<(), String> {
        if let Some(st) = status {
            if frames.is_empty() {
                return Err(format!("state {st:?} has no frames"));
            }
            let frame_ms = kv(header, "frame_ms")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let motion = parse_motion(kv(header, "motion").unwrap_or("none"))?;
            let overlay_str = kv(header, "overlay").unwrap_or("none");
            let overlay = match kv(header, "color") {
                Some(c) => parse_overlay(&format!("{overlay_str} color={c}"))?,
                None => parse_overlay(overlay_str)?,
            };
            let dim = kv(header, "dim") == Some("true");
            let ghost = kv(header, "ghost") == Some("true");
            states.insert(
                st,
                StateSpec {
                    frames: std::mem::take(frames),
                    frame_ms,
                    motion,
                    overlay,
                    dim,
                    ghost,
                },
            );
        }
        Ok(())
    }

    for raw in src.lines() {
        let line = raw.trim_end();
        if let Some(rest) = line.strip_prefix("name") {
            if let Some(v) = rest.trim_start().strip_prefix('=') {
                name = v.trim().to_string();
            }
            continue;
        }
        if line.trim_start().starts_with('[') {
            flush(&mut buf, &mut frames)?;
            commit(&mut states, cur_status, &cur_header, &mut frames)?;
            let open = line.find('[').ok_or("missing [ in state header")?;
            let close = line.find(']').ok_or("missing ] in state header")?;
            let key = &line[open + 1..close];
            cur_status =
                Some(status_from_key(key).ok_or_else(|| format!("unknown state '{key}'"))?);
            cur_header = line[close + 1..].trim().to_string();
            continue;
        }
        if line.trim().is_empty() {
            flush(&mut buf, &mut frames)?;
            continue;
        }
        if cur_status.is_some() {
            buf.push(line);
        }
    }
    flush(&mut buf, &mut frames)?;
    commit(&mut states, cur_status, &cur_header, &mut frames)?;

    if name.is_empty() {
        return Err("missing name".into());
    }
    for st in [
        AgentStatus::Idle,
        AgentStatus::Working,
        AgentStatus::Done,
        AgentStatus::Blocked,
        AgentStatus::Unknown,
    ] {
        if !states.contains_key(&st) {
            return Err(format!("species '{name}' missing state {st:?}"));
        }
    }
    Ok(Species { name, states })
}

/// Embedded sprite sources. Add one line per new animal.
///
/// `test-blob.sprite` is intentionally absent here: it is a unit-test fixture
/// only (included directly by the sprite/render tests), not a shipped species.
const EMBEDDED: &[&str] = &[
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/sprites/sheep.sprite")),
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/sprites/goat.sprite")),
];

/// Parse the embedded sprites. Guarded by `every_embedded_species_is_valid`.
pub fn embedded_species() -> Vec<Species> {
    EMBEDDED
        .iter()
        .filter_map(|src| parse_species(src).ok())
        .collect()
}

/// Embedded species, with any `$HERDR_PETS_SPRITES/*.sprite` overriding by name.
pub fn load_species() -> Vec<Species> {
    let mut out = embedded_species();
    if let Some(dir) = std::env::var_os("HERDR_PETS_SPRITES")
        && let Ok(entries) = std::fs::read_dir(&dir)
    {
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("sprite") {
                continue;
            }
            match std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|s| parse_species(&s))
            {
                Ok(sp) => {
                    out.retain(|x| x.name != sp.name);
                    out.push(sp);
                }
                Err(err) => eprintln!("herdr-pets: skipping sprite {path:?}: {err}"),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentStatus;
    use crate::anim::{OverlayColor, Rgb};

    const BLOB: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/sprites/test-blob.sprite"
    ));

    #[test]
    fn parses_name_states_and_frame_grid() {
        let sp = parse_species(BLOB).expect("valid fixture");
        assert_eq!(sp.name, "TestBlob");
        assert_eq!(sp.states.len(), 5);
        let idle = &sp.states[&AgentStatus::Idle];
        assert_eq!(idle.frames.len(), 2);
        assert_eq!((idle.frames[0].w, idle.frames[0].h), (4, 4));
        assert_eq!(idle.frame_ms, 500);
        assert!(idle.frames[0].cells.contains(&Role::CoatMid));
        assert!(idle.frames[0].cells.contains(&Role::Outline));
    }

    #[test]
    fn unknown_state_carries_ghost_and_overlay_config() {
        let sp = parse_species(BLOB).unwrap();
        let u = &sp.states[&AgentStatus::Unknown];
        assert!(u.ghost);
    }

    #[test]
    fn illegal_symbol_is_a_parse_error() {
        let bad = "name = X\n[idle] frame_ms=1 motion=none overlay=none\n.Z.\n";
        assert!(parse_species(bad).is_err());
    }

    #[test]
    fn ragged_frame_is_a_parse_error() {
        let bad = "name=X\n[idle] frame_ms=1 motion=none overlay=none\nMM\nMMM\n";
        assert!(parse_species(bad).is_err());
    }

    #[test]
    fn overlay_color_config_reaches_the_spec() {
        let sp = parse_species(BLOB).unwrap();
        let done = &sp.states[&AgentStatus::Done];
        assert_eq!(done.overlay.color, OverlayColor::Accent);
        let blocked = &sp.states[&AgentStatus::Blocked];
        assert_eq!(
            blocked.overlay.color,
            OverlayColor::Literal(Rgb(0xe6, 0x2d, 0x23))
        );
    }

    #[test]
    fn species_missing_a_state_is_an_error() {
        let bad = "name = X\n[idle] frame_ms=1 motion=none overlay=none\n.M.\nMMM\n.M.\n";
        assert!(parse_species(bad).is_err());
    }

    #[test]
    fn role_from_char_covers_the_legend() {
        assert_eq!(role_from_char('#'), Some(Role::Outline));
        assert_eq!(role_from_char('M'), Some(Role::CoatMid));
        assert_eq!(role_from_char('.'), Some(Role::Transparent));
        assert_eq!(role_from_char(' '), Some(Role::Transparent));
        assert_eq!(role_from_char('Z'), None);
    }

    // The guard: every shipped species is well-formed. Fails CI on a bad sprite.
    #[test]
    fn every_embedded_species_is_valid() {
        // `embedded_species()` silently drops sprites that fail to parse
        // (`filter_map(...ok())`), so a broken embedded sprite would vanish
        // from `all` below rather than failing the guard. Parse the raw
        // sources directly first so a broken sprite fails loudly.
        for src in EMBEDDED {
            parse_species(src).expect("embedded sprite must parse");
        }

        let all = embedded_species();
        assert!(!all.is_empty());
        for sp in &all {
            let species_size = sp.size();
            for st in [
                AgentStatus::Idle,
                AgentStatus::Working,
                AgentStatus::Done,
                AgentStatus::Blocked,
                AgentStatus::Unknown,
            ] {
                let spec = sp
                    .states
                    .get(&st)
                    .unwrap_or_else(|| panic!("{} missing state {st:?}", sp.name));
                assert!(!spec.frames.is_empty(), "{} {st:?} has no frames", sp.name);
                let (w, h) = (spec.frames[0].w, spec.frames[0].h);
                assert!(h <= 14, "{} taller than the 7-row budget", sp.name);
                assert_eq!(
                    (w, h),
                    species_size,
                    "{} {st:?} size differs from species size",
                    sp.name
                );
                for f in &spec.frames {
                    assert_eq!((f.w, f.h), (w, h), "{} {st:?} frame size drift", sp.name);
                }
            }
        }
    }
}
