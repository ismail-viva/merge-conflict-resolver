//! Themes: a small set of semantic colors from the Material palette.
//!
//! Terminals have no alpha channel, so "translucent" highlights are produced
//! by blending an accent color into the background ([`Rgb::blend`]). On
//! terminals without 24-bit color every color is mapped to the nearest entry
//! of the 256-color palette.

use ratatui::style::Color;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ThemeError {
    #[error("invalid color `{0}` (expected `#rrggbb`)")]
    InvalidColor(String),
    #[error("theme `{theme}` is based on unknown theme `{base}` (use `dark` or `light`)")]
    UnknownBase { theme: String, base: String },
    #[error("unknown theme `{0}`")]
    UnknownTheme(String),
}

impl Rgb {
    pub fn parse(text: &str) -> Result<Rgb, ThemeError> {
        let invalid = || ThemeError::InvalidColor(text.to_owned());
        let hex = text.strip_prefix('#').ok_or_else(invalid)?;
        if hex.len() != 6 || !hex.is_ascii() {
            return Err(invalid());
        }
        let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| invalid());
        Ok(Rgb(channel(0)?, channel(2)?, channel(4)?))
    }

    /// `self` covered by `over` at opacity `alpha` (0 = self, 1 = over).
    pub fn blend(self, over: Rgb, alpha: f32) -> Rgb {
        let alpha = alpha.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * alpha).round() as u8;
        Rgb(
            mix(self.0, over.0),
            mix(self.1, over.1),
            mix(self.2, over.2),
        )
    }

    /// Nearest color of the xterm 256-color palette.
    pub fn to_ansi256(self) -> u8 {
        const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        let nearest_step = |v: u8| {
            (0..6)
                .min_by_key(|&i| (STEPS[i] as i32 - v as i32).abs())
                .unwrap_or(0)
        };
        let (r, g, b) = (
            nearest_step(self.0),
            nearest_step(self.1),
            nearest_step(self.2),
        );
        let cube = Rgb(STEPS[r], STEPS[g], STEPS[b]);
        let cube_index = 16 + 36 * r as u8 + 6 * g as u8 + b as u8;

        let average = (self.0 as u32 + self.1 as u32 + self.2 as u32) / 3;
        let gray_step = ((average.saturating_sub(8) + 5) / 10).min(23) as u8;
        let gray_level = 8 + 10 * gray_step;
        let gray = Rgb(gray_level, gray_level, gray_level);

        if self.distance(gray) < self.distance(cube) {
            232 + gray_step
        } else {
            cube_index
        }
    }

    fn distance(self, other: Rgb) -> i32 {
        let d = |a: u8, b: u8| (a as i32 - b as i32).pow(2);
        d(self.0, other.0) + d(self.1, other.1) + d(self.2, other.2)
    }
}

/// How many colors the terminal can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorMode {
    /// Decide from `$COLORTERM`.
    #[default]
    Auto,
    Truecolor,
    #[serde(rename = "256")]
    Ansi256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    TrueColor,
    Ansi256,
}

impl ColorDepth {
    pub fn resolve(mode: ColorMode) -> Self {
        match mode {
            ColorMode::Truecolor => ColorDepth::TrueColor,
            ColorMode::Ansi256 => ColorDepth::Ansi256,
            ColorMode::Auto => Self::from_colorterm(std::env::var("COLORTERM").ok().as_deref()),
        }
    }

    pub fn from_colorterm(value: Option<&str>) -> Self {
        match value {
            Some(v) if v.contains("truecolor") || v.contains("24bit") => ColorDepth::TrueColor,
            _ => ColorDepth::Ansi256,
        }
    }

    pub fn color(self, rgb: Rgb) -> Color {
        match self {
            ColorDepth::TrueColor => Color::Rgb(rgb.0, rgb.1, rgb.2),
            ColorDepth::Ansi256 => Color::Indexed(rgb.to_ansi256()),
        }
    }
}

macro_rules! theme {
    ($( $(#[$doc:meta])* $field:ident ),* $(,)?) => {
        #[derive(Debug, Clone, PartialEq)]
        pub struct Theme {
            pub name: String,
            /// Opacity of diff / conflict backgrounds over `bg`.
            pub tint: f32,
            $( $(#[$doc])* pub $field: Rgb, )*
        }

        /// A `[themes.<name>]` table: any subset of a theme's colors.
        #[derive(Debug, Clone, Default, Deserialize, PartialEq)]
        #[serde(default, deny_unknown_fields)]
        pub struct ThemeOverride {
            /// Built-in theme to start from: `dark` (default) or `light`.
            pub base: Option<String>,
            pub tint: Option<f32>,
            $( pub $field: Option<String>, )*
        }

        impl Theme {
            /// Color role names, as used in the configuration file.
            pub const ROLES: &'static [&'static str] = &[ $( stringify!($field), )* ];

            fn apply(&mut self, over: &ThemeOverride) -> Result<(), ThemeError> {
                if let Some(tint) = over.tint {
                    self.tint = tint.clamp(0.0, 1.0);
                }
                $( if let Some(color) = &over.$field { self.$field = Rgb::parse(color)?; } )*
                Ok(())
            }

            pub fn role(&self, name: &str) -> Option<Rgb> {
                match name { $( stringify!($field) => Some(self.$field), )* _ => None }
            }
        }
    };
}

theme! {
    /// Window background.
    bg,
    /// Bars, popups and filler rows.
    surface,
    /// Primary text.
    fg,
    /// Line numbers, hints, rejected text.
    muted,
    /// Unfocused pane borders.
    border,
    /// Focused border, cursor bar, highlights.
    accent,
    /// Identity color of the source (incoming) branch.
    source,
    /// Identity color of the target (current) branch.
    target,
    /// Lines added relative to the base.
    added,
    /// Lines removed relative to the base.
    removed,
    /// Lines changed relative to the base; manual edits.
    modified,
    /// Conflicting hunks.
    conflict,
    /// Accepted lines.
    accepted,
    /// Rejected lines.
    rejected,
    /// Lines and hunks awaiting a decision.
    unresolved,
}

impl Theme {
    /// Material dark: grey 900 surface with 300-weight accents.
    pub fn dark() -> Theme {
        Theme {
            name: "dark".into(),
            tint: 0.16,
            bg: Rgb(0x1e, 0x1e, 0x1e),
            surface: Rgb(0x2a, 0x2a, 0x2a),
            fg: Rgb(0xe0, 0xe0, 0xe0),
            muted: Rgb(0x8a, 0x8a, 0x8a),
            border: Rgb(0x42, 0x42, 0x42),
            accent: Rgb(0x82, 0xb1, 0xff),
            source: Rgb(0x64, 0xb5, 0xf6),
            target: Rgb(0xba, 0x68, 0xc8),
            added: Rgb(0x81, 0xc7, 0x84),
            removed: Rgb(0xe5, 0x73, 0x73),
            modified: Rgb(0xff, 0xd5, 0x4f),
            conflict: Rgb(0xff, 0x8a, 0x65),
            accepted: Rgb(0x81, 0xc7, 0x84),
            rejected: Rgb(0xe5, 0x73, 0x73),
            unresolved: Rgb(0xff, 0xb7, 0x4d),
        }
    }

    /// Material light: grey 50 surface with 700-weight accents.
    pub fn light() -> Theme {
        Theme {
            name: "light".into(),
            tint: 0.14,
            bg: Rgb(0xfa, 0xfa, 0xfa),
            surface: Rgb(0xec, 0xef, 0xf1),
            fg: Rgb(0x21, 0x21, 0x21),
            muted: Rgb(0x75, 0x75, 0x75),
            border: Rgb(0xbd, 0xbd, 0xbd),
            accent: Rgb(0x29, 0x62, 0xff),
            source: Rgb(0x19, 0x76, 0xd2),
            target: Rgb(0x7b, 0x1f, 0xa2),
            added: Rgb(0x38, 0x8e, 0x3c),
            removed: Rgb(0xd3, 0x2f, 0x2f),
            modified: Rgb(0xf5, 0x7f, 0x17),
            conflict: Rgb(0xe6, 0x4a, 0x19),
            accepted: Rgb(0x2e, 0x7d, 0x32),
            rejected: Rgb(0xc6, 0x28, 0x28),
            unresolved: Rgb(0xef, 0x6c, 0x00),
        }
    }

    fn builtin(name: &str) -> Option<Theme> {
        match name {
            "dark" => Some(Theme::dark()),
            "light" => Some(Theme::light()),
            _ => None,
        }
    }

    /// `accent` as a translucent background.
    pub fn tinted(&self, accent: Rgb) -> Rgb {
        self.bg.blend(accent, self.tint)
    }

    /// Whether the background is darker than the text.
    pub fn is_dark(&self) -> bool {
        let luma = |c: Rgb| 299 * c.0 as u32 + 587 * c.1 as u32 + 114 * c.2 as u32;
        luma(self.bg) < luma(self.fg)
    }
}

/// The built-in themes plus the user's, in switching order.
pub fn build_themes(
    overrides: &std::collections::BTreeMap<String, ThemeOverride>,
) -> Result<Vec<Theme>, ThemeError> {
    let mut themes = vec![Theme::dark(), Theme::light()];
    for (name, over) in overrides {
        let base_name = over.base.as_deref().unwrap_or(match name.as_str() {
            "light" => "light",
            _ => "dark",
        });
        let mut theme = Theme::builtin(base_name).ok_or_else(|| ThemeError::UnknownBase {
            theme: name.clone(),
            base: base_name.to_owned(),
        })?;
        theme.name = name.clone();
        theme.apply(over)?;
        match themes.iter_mut().find(|t| t.name == *name) {
            Some(existing) => *existing = theme,
            None => themes.push(theme),
        }
    }
    Ok(themes)
}

/// Guess whether the terminal is dark from `$COLORFGBG` (`fg;bg`).
pub fn terminal_is_dark(colorfgbg: Option<&str>) -> bool {
    colorfgbg
        .and_then(|v| v.rsplit(';').next())
        .and_then(|bg| bg.parse::<u8>().ok())
        .is_none_or(|bg| bg <= 6 || bg == 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_colors() {
        assert_eq!(Rgb::parse("#ff8a65"), Ok(Rgb(255, 138, 101)));
        assert_eq!(Rgb::parse("#FFFFFF"), Ok(Rgb(255, 255, 255)));
        for bad in ["ff8a65", "#fff", "#gggggg", "#ff8a6é", ""] {
            assert!(Rgb::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn blend_interpolates() {
        let (black, white) = (Rgb(0, 0, 0), Rgb(255, 255, 255));
        assert_eq!(black.blend(white, 0.0), black);
        assert_eq!(black.blend(white, 1.0), white);
        assert_eq!(black.blend(white, 0.5), Rgb(128, 128, 128));
        assert_eq!(black.blend(white, 7.0), white);
    }

    #[test]
    fn tints_stay_close_to_the_background() {
        for theme in [Theme::dark(), Theme::light()] {
            let tint = theme.tinted(theme.conflict);
            assert_ne!(tint, theme.bg);
            assert!(tint.distance(theme.bg) < tint.distance(theme.conflict));
        }
        assert!(Theme::dark().is_dark());
        assert!(!Theme::light().is_dark());
    }

    #[test]
    fn ansi256_mapping_hits_known_entries() {
        assert_eq!(Rgb(0, 0, 0).to_ansi256(), 16);
        assert_eq!(Rgb(255, 255, 255).to_ansi256(), 231);
        assert_eq!(Rgb(255, 0, 0).to_ansi256(), 196);
        assert_eq!(Rgb(0x1e, 0x1e, 0x1e).to_ansi256(), 234);
        assert_eq!(Rgb(128, 128, 128).to_ansi256(), 244);
    }

    #[test]
    fn color_depth_follows_colorterm() {
        assert_eq!(
            ColorDepth::from_colorterm(Some("truecolor")),
            ColorDepth::TrueColor
        );
        assert_eq!(
            ColorDepth::from_colorterm(Some("24bit")),
            ColorDepth::TrueColor
        );
        assert_eq!(ColorDepth::from_colorterm(None), ColorDepth::Ansi256);
        assert_eq!(
            ColorDepth::Ansi256.color(Rgb(255, 0, 0)),
            Color::Indexed(196)
        );
        assert_eq!(
            ColorDepth::TrueColor.color(Rgb(1, 2, 3)),
            Color::Rgb(1, 2, 3)
        );
    }

    #[test]
    fn custom_themes_extend_and_override_builtins() {
        let mut overrides = std::collections::BTreeMap::new();
        overrides.insert(
            "ocean".to_owned(),
            ThemeOverride {
                base: Some("light".into()),
                accent: Some("#006064".into()),
                tint: Some(0.3),
                ..ThemeOverride::default()
            },
        );
        overrides.insert(
            "dark".to_owned(),
            ThemeOverride {
                bg: Some("#000000".into()),
                ..ThemeOverride::default()
            },
        );
        let themes = build_themes(&overrides).unwrap();
        assert_eq!(
            themes.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            ["dark", "light", "ocean"]
        );
        assert_eq!(themes[0].bg, Rgb(0, 0, 0));
        assert_eq!(themes[2].accent, Rgb(0, 0x60, 0x64));
        assert_eq!(themes[2].fg, Theme::light().fg);
        assert_eq!(themes[2].tint, 0.3);
    }

    #[test]
    fn bad_theme_definitions_are_reported() {
        let mut overrides = std::collections::BTreeMap::new();
        overrides.insert(
            "x".to_owned(),
            ThemeOverride {
                base: Some("neon".into()),
                ..ThemeOverride::default()
            },
        );
        assert!(matches!(
            build_themes(&overrides),
            Err(ThemeError::UnknownBase { .. })
        ));
    }

    #[test]
    fn colorfgbg_detection() {
        assert!(terminal_is_dark(Some("15;0")));
        assert!(!terminal_is_dark(Some("0;15")));
        assert!(terminal_is_dark(Some("12;default;8")));
        assert!(terminal_is_dark(None));
    }
}
