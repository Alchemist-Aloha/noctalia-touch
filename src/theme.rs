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

pub fn xdg(var: &str, fallback: &str) -> Option<PathBuf> {
    env::var_os(var)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(fallback)))
}

pub fn palette() -> Palette {
    let css = xdg("XDG_CACHE_HOME", ".cache")
        .and_then(|p| fs::read_to_string(p.join("noctalia-touch/colors.css")).ok())
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
}
