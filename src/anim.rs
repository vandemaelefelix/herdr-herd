//! Named motion primitives and overlay specs — the "animation config" library
//! that `.sprite` files reference. All motion is a pure function of a phase in
//! 0.0..1.0, so it is deterministic and testable. Horizontal roaming (`Wander`)
//! is owned by the herd simulation, not by per-pet offsets.

use std::f32::consts::TAU;

/// An 8-bit-per-channel color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// A named motion primitive a sprite can combine into a `MotionSpec`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    None,
    Breathe,
    Hop,
    Shake,
    Sway,
    Wander,
}

/// One or more `Motion`s composed together (e.g. `"hop+wander"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MotionSpec {
    pub motions: Vec<Motion>,
}

/// Parse `"hop"`, `"hop+wander"`, `"none"`. Unknown token -> Err.
pub fn parse_motion(s: &str) -> Result<MotionSpec, String> {
    let mut motions = Vec::new();
    for tok in s.split('+').map(str::trim).filter(|t| !t.is_empty()) {
        let m = match tok {
            "none" => Motion::None,
            "breathe" => Motion::Breathe,
            "hop" => Motion::Hop,
            "shake" => Motion::Shake,
            "sway" => Motion::Sway,
            "wander" => Motion::Wander,
            other => return Err(format!("unknown motion '{other}'")),
        };
        motions.push(m);
    }
    if motions.is_empty() {
        return Err("empty motion".into());
    }
    Ok(MotionSpec { motions })
}

/// True if `spec` includes `Motion::Wander` (the herd owns its horizontal roam).
pub fn has_wander(spec: &MotionSpec) -> bool {
    spec.motions.contains(&Motion::Wander)
}

/// A pixel offset applied to a sprite before blitting. Negative `dy` = up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Offset {
    pub dx: f32,
    pub dy: f32,
}

/// Sum the local (non-wander) motions at `phase` (0.0..1.0).
pub fn motion_offset(spec: &MotionSpec, phase: f32) -> Offset {
    let t = phase * TAU;
    let mut o = Offset { dx: 0.0, dy: 0.0 };
    for m in &spec.motions {
        match m {
            Motion::None | Motion::Wander => {}
            Motion::Breathe => o.dy += -0.5 * (1.0 - t.cos()) / 2.0, // gentle rise/settle, <=0.5
            // Vertical lift is capped at 1 px so it fits the sprite's fixed
            // 1px headroom (sprites are <= 14 px; the render band reserves
            // that 1px above them, plus more again for the focus hat) — a
            // bigger jump would clip the top.
            Motion::Hop => o.dy -= (t.sin()).max(0.0), // lifts on the upbeat
            Motion::Shake => {
                o.dx += 1.2 * (t * 2.0).sin();
                o.dy -= (t * 2.0).sin().abs();
            }
            Motion::Sway => o.dx += 1.0 * t.sin(),
        }
    }
    o
}

/// What an overlay renders: a speech/thought bubble or a status badge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    None,
    Bubble(String),
    Badge(String),
}

/// The color an overlay renders in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayColor {
    Default,
    Accent,
    Literal(Rgb),
}

/// An overlay's kind plus its color.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlaySpec {
    pub kind: Overlay,
    pub color: OverlayColor,
}

/// Parse `"bubble:Zz"`, `"badge:! color=accent"`, `"badge:! color=#e62d23"`, `"none"`.
pub fn parse_overlay(s: &str) -> Result<OverlaySpec, String> {
    let mut kind = Overlay::None;
    let mut color = OverlayColor::Default;
    for (i, part) in s.split_whitespace().enumerate() {
        if i == 0 {
            kind = match part.split_once(':') {
                Some(("bubble", g)) => Overlay::Bubble(g.to_string()),
                Some(("badge", g)) => Overlay::Badge(g.to_string()),
                None if part == "none" => Overlay::None,
                _ => return Err(format!("bad overlay '{part}'")),
            };
        } else if let Some(c) = part.strip_prefix("color=") {
            color = parse_color(c)?;
        }
    }
    Ok(OverlaySpec { kind, color })
}

fn parse_color(c: &str) -> Result<OverlayColor, String> {
    if c == "accent" {
        return Ok(OverlayColor::Accent);
    }
    let hex = c
        .strip_prefix('#')
        .ok_or_else(|| format!("bad color '{c}'"))?;
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("bad color '{c}'"));
    }
    let byte =
        |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| format!("bad color '{c}'"));
    Ok(OverlayColor::Literal(Rgb(byte(0)?, byte(2)?, byte(4)?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_motion_reads_single_and_composed() {
        assert_eq!(parse_motion("hop").unwrap().motions, vec![Motion::Hop]);
        let m = parse_motion("hop+wander").unwrap().motions;
        assert!(m.contains(&Motion::Hop) && m.contains(&Motion::Wander));
        assert_eq!(parse_motion("none").unwrap().motions, vec![Motion::None]);
    }

    #[test]
    fn parse_motion_rejects_unknown() {
        assert!(parse_motion("moonwalk").is_err());
    }

    #[test]
    fn breathe_offset_is_vertical_and_bounded() {
        let spec = parse_motion("breathe").unwrap();
        for p in [0.0, 0.25, 0.5, 0.75] {
            let o = motion_offset(&spec, p);
            assert_eq!(o.dx, 0.0);
            assert!(o.dy.abs() <= 1.5, "breathe stays gentle");
        }
    }

    #[test]
    fn hop_offset_never_goes_below_ground() {
        let spec = parse_motion("hop").unwrap();
        for p in [0.0, 0.5, 1.0] {
            assert!(
                motion_offset(&spec, p).dy <= 0.0,
                "hop only lifts (negative = up)"
            );
        }
    }

    #[test]
    fn wander_is_detected_and_adds_no_local_offset() {
        let spec = parse_motion("wander").unwrap();
        assert!(has_wander(&spec));
        let o = motion_offset(&spec, 0.5);
        assert_eq!((o.dx, o.dy), (0.0, 0.0));
    }

    #[test]
    fn parse_overlay_reads_kind_and_color() {
        let b = parse_overlay("bubble:Zz").unwrap();
        assert_eq!(b.kind, Overlay::Bubble("Zz".into()));
        assert_eq!(b.color, OverlayColor::Default);

        let badge = parse_overlay("badge:!").unwrap();
        assert_eq!(badge.kind, Overlay::Badge("!".into()));

        assert_eq!(
            parse_overlay("badge:! color=accent").unwrap().color,
            OverlayColor::Accent
        );
        assert_eq!(
            parse_overlay("badge:! color=#e62d23").unwrap().color,
            OverlayColor::Literal(Rgb(0xe6, 0x2d, 0x23))
        );
        assert_eq!(parse_overlay("none").unwrap().kind, Overlay::None);
    }

    #[test]
    fn parse_overlay_rejects_malformed_color_without_panicking() {
        assert!(parse_overlay("badge:! color=#12zz34").is_err());
        assert!(parse_overlay("badge:! color=#1é234").is_err()); // 6 bytes, non-ASCII -> Err, no panic
        assert!(parse_overlay("badge:! color=#fff").is_err()); // wrong length
    }
}
