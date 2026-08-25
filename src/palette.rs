//! Role -> color: coat roles tint to the agent's hue; skin/eye/outline are
//! fixed (outline + neutrals are theme-aware). `dim` and `ghost` are engine
//! state overrides applied here so no sprite bakes them in.

use crate::anim::Rgb;
use crate::sprite::Role;

/// Which theme is active; only affects the outline color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Theme {
    Dark,
    Light,
}

/// Engine-driven overrides applied on top of a role's base color.
#[derive(Debug, Clone, Copy)]
pub struct StateStyle {
    pub dim: bool,
    pub ghost: bool,
}

impl StateStyle {
    /// No overrides: render the role's base color unchanged.
    pub fn none() -> Self {
        Self {
            dim: false,
            ghost: false,
        }
    }
}

/// HSL (degrees, 0..1, 0..1) -> RGB.
pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Rgb {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let to = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Rgb(to(r1), to(g1), to(b1))
}

/// Resolve a sprite `Role` to a concrete color for this agent's hue, theme,
/// and state overrides. `None` means transparent (paint nothing).
pub fn role_color(role: Role, hue: u16, theme: Theme, style: StateStyle) -> Option<Rgb> {
    let h = hue as f32;
    // coat saturation/lightness steps; ghost flattens sat, dim lowers both.
    let (mut sat, light): (f32, f32) = match role {
        Role::CoatLight => (0.52, 0.86),
        Role::CoatMid => (0.48, 0.66),
        Role::CoatShadow => (0.46, 0.52),
        Role::Accent => (0.72, 0.55),
        Role::Outline => {
            return Some(match theme {
                Theme::Dark => Rgb(18, 18, 18),
                Theme::Light => Rgb(28, 28, 28),
            });
        }
        Role::Eye => return Some(Rgb(20, 20, 20)),
        Role::Skin => return Some(Rgb(0xe7, 0xad, 0x86)),
        Role::Horn => return Some(Rgb(0xdc, 0xcb, 0xa6)),
        Role::Transparent => return None,
    };
    let mut light = light;
    if style.ghost {
        sat = 0.06;
    }
    if style.dim {
        sat *= 0.5;
        light = (light * 0.82).min(0.7);
    }
    Some(hsl_to_rgb(h, sat, light))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sprite::Role;

    #[test]
    fn transparent_role_has_no_color() {
        assert_eq!(
            role_color(Role::Transparent, 120, Theme::Dark, StateStyle::none()),
            None
        );
    }

    #[test]
    fn coat_roles_track_the_hue() {
        // Different hues produce different coat colors.
        let a = role_color(Role::CoatMid, 20, Theme::Dark, StateStyle::none());
        let b = role_color(Role::CoatMid, 200, Theme::Dark, StateStyle::none());
        assert!(a.is_some() && a != b);
    }

    #[test]
    fn coat_light_is_lighter_than_shadow() {
        let l = role_color(Role::CoatLight, 200, Theme::Dark, StateStyle::none()).unwrap();
        let s = role_color(Role::CoatShadow, 200, Theme::Dark, StateStyle::none()).unwrap();
        let sum = |c: Rgb| c.0 as u32 + c.1 as u32 + c.2 as u32;
        assert!(sum(l) > sum(s));
    }

    #[test]
    fn skin_and_eye_ignore_hue() {
        assert_eq!(
            role_color(Role::Skin, 10, Theme::Dark, StateStyle::none()),
            role_color(Role::Skin, 300, Theme::Dark, StateStyle::none())
        );
    }

    #[test]
    fn ghost_desaturates_the_coat_toward_grey() {
        let normal = role_color(Role::CoatMid, 200, Theme::Dark, StateStyle::none()).unwrap();
        let ghost = role_color(
            Role::CoatMid,
            200,
            Theme::Dark,
            StateStyle {
                dim: false,
                ghost: true,
            },
        )
        .unwrap();
        let spread = |c: Rgb| c.0.max(c.1).max(c.2) as i32 - c.0.min(c.1).min(c.2) as i32;
        assert!(spread(ghost) < spread(normal), "ghost should be greyer");
    }
}
