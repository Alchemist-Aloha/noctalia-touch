use gpui_kit::{Rgba, rgb};
use std::{env, fs, path::PathBuf};

#[derive(Clone, Copy)]
pub struct Palette {
    pub surface: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub tonal: Rgba,
    pub on_tonal: Rgba,
    pub primary: Rgba,
    pub on_primary: Rgba,
    pub shadow: Rgba,
}

/// Accent roles for interactive surfaces, derived from the theme primary or a
/// user-chosen color so custom accents keep readable foregrounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Accent {
    pub fill: Rgba,
    pub on_fill: Rgba,
    pub container: Rgba,
    pub on_container: Rgba,
    /// Panel background: the theme surface tinted toward the accent.
    pub surface: Rgba,
    /// Resting key and toolbar button fill.
    pub key: Rgba,
    /// Outlines, the drag grip and secondary text.
    pub outline: Rgba,
}

/// Accent choices offered in the keyboard toolbar as (dark theme, light theme) tones.
pub const ACCENT_PRESETS: [(u32, u32); 7] = [
    (0xa8c7fa, 0x0b57d0), // blue
    (0x80d5d0, 0x006a66), // teal
    (0x6dd58c, 0x146c2e), // green
    (0xffb951, 0x845400), // amber
    (0xffb4ab, 0xb3261e), // red
    (0xffb0cd, 0x984061), // pink
    (0xd0bcff, 0x6750a4), // purple
];

fn channel(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG relative luminance.
pub fn luminance(c: Rgba) -> f32 {
    0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
}

/// Linear blend from `a` (t = 0) to `b` (t = 1), keeping `a`'s alpha.
pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a,
    }
}

/// Parse `#rrggbb` or `rrggbb`.
pub fn parse_hex(text: &str) -> Option<u32> {
    let hex = text.trim();
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    (hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(hex, 16).ok())
        .flatten()
}

impl Palette {
    pub fn is_dark(&self) -> bool {
        luminance(self.surface) < 0.2
    }

    /// The preset tone that suits this palette's light or dark surface.
    pub fn preset(&self, index: usize) -> u32 {
        let (dark, light) = ACCENT_PRESETS[index % ACCENT_PRESETS.len()];
        if self.is_dark() { dark } else { light }
    }

    pub fn accent(&self, custom: Option<u32>) -> Accent {
        let (fill, on_fill) = match custom {
            None => (self.primary, self.on_primary),
            Some(value) => {
                let fill = rgb(value);
                // Pick the foreground with the better contrast, tinted toward the fill.
                let on_fill = if luminance(fill) > 0.179 {
                    mix(fill, rgb(0x000000), 0.78)
                } else {
                    mix(fill, rgb(0xffffff), 0.88)
                };
                (fill, on_fill)
            }
        };
        // Every role derives from the accent so the whole keyboard follows the
        // theme primary (or the custom color), even when the scheme's surface is
        // neutral gray or its secondary container is an unrelated hue.
        let dark = self.is_dark();
        Accent {
            fill,
            on_fill,
            container: mix(self.surface, fill, 0.32),
            on_container: mix(fill, self.text, if dark { 0.35 } else { 0.25 }),
            surface: mix(self.surface, fill, if dark { 0.10 } else { 0.08 }),
            key: mix(self.surface, fill, if dark { 0.22 } else { 0.16 }),
            outline: mix(self.muted, fill, 0.5),
        }
    }
}

pub fn xdg(var: &str, fallback: &str) -> Option<PathBuf> {
    env::var_os(var)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(fallback)))
}

pub fn palette_path() -> Option<PathBuf> {
    Some(xdg("XDG_CACHE_HOME", ".cache")?.join("noctalia-touch/colors.css"))
}

pub fn palette() -> Palette {
    let css = palette_path()
        .and_then(|p| fs::read_to_string(p).ok())
        .unwrap_or_default();
    parse(&css)
}

fn parse(css: &str) -> Palette {
    let mut palette = Palette {
        surface: rgb(0x1f1d2e),
        text: rgb(0xe0def4),
        muted: rgb(0xb8b3ce),
        tonal: rgb(0x362e42),
        on_tonal: rgb(0xf3deec),
        primary: rgb(0xc4a7e7),
        on_primary: rgb(0x2d1d40),
        shadow: rgb(0x100e18),
    };
    for rule in css.split(';') {
        let mut words = rule.split_whitespace();
        if let (Some("@define-color"), Some(name), Some(hex), None) =
            (words.next(), words.next(), words.next(), words.next())
            && let Some(hex) = hex.strip_prefix('#').filter(|c| c.len() == 6)
            && let Ok(value) = u32::from_str_radix(hex, 16)
        {
            *match name {
                "float_surface" => &mut palette.surface,
                "float_text" => &mut palette.text,
                "float_muted" => &mut palette.muted,
                "float_tonal" => &mut palette.tonal,
                "float_on_tonal" => &mut palette.on_tonal,
                "float_primary" => &mut palette.primary,
                "float_on_primary" => &mut palette.on_primary,
                "float_shadow" => &mut palette.shadow,
                _ => continue,
            } = rgb(value);
        }
    }
    palette
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_roles_override_fallback_selectively() {
        let p =
            parse("@define-color float_primary #81ffa9;\n@define-color float_on_primary invalid;");
        assert_eq!(p.primary, rgb(0x81ffa9));
        assert_eq!(p.on_primary, parse("").on_primary);
    }

    #[test]
    fn accents_follow_theme_or_custom_color_with_contrast() {
        let p = parse("");
        assert_eq!(p.accent(None).fill, p.primary);
        assert_eq!(p.accent(None).on_fill, p.on_primary);
        let light = p.accent(Some(0xffeeaa));
        assert!(luminance(light.on_fill) < 0.1);
        let dark = p.accent(Some(0x102040));
        assert!(luminance(dark.on_fill) > 0.5);
        assert_eq!(parse_hex("#A1b2C3"), Some(0xa1b2c3));
        assert_eq!(parse_hex("a1b2c3"), Some(0xa1b2c3));
        assert_eq!(parse_hex("#a1b2c"), None);
        assert_eq!(parse_hex("#a1b2cg"), None);
        assert!(p.is_dark());
        // Background and keys move toward the accent, keys more than the panel.
        let a = p.accent(Some(0xff8000));
        let red = |c: Rgba| c.r - p.surface.r;
        assert!(red(a.key) > red(a.surface) && red(a.surface) > 0.);
        assert_eq!(p.preset(0), ACCENT_PRESETS[0].0);
    }
}
