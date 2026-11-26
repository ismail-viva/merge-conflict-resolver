//! User configuration (`config.toml`): layout, behaviour, keys and themes.

pub mod keymap;
pub mod theme;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

pub use keymap::{Key, Keymap, Lookup};
pub use theme::{ColorDepth, ColorMode, Rgb, Theme, ThemeOverride};

use crate::action::Action;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayoutMode {
    /// Source │ result │ target, side by side.
    #[default]
    Columns,
    /// Source │ target on top, the result below.
    Stacked,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    /// `dark`, `light`, `auto` or the name of a `[themes.<name>]` table.
    pub theme: String,
    pub color: ColorMode,
    pub layout: LayoutMode,
    /// Relative widths of the source, result and target panes.
    pub panel_ratio: [u16; 3],
    /// Share of the height given to the result pane in the stacked layout.
    pub stacked_result_percent: u16,
    pub show_info: bool,
    /// Height of the git history panel, borders included.
    pub info_height: u16,
    pub line_numbers: bool,
    pub minimap: bool,
    pub mouse: bool,
    /// Use ASCII instead of Unicode for markers.
    pub ascii: bool,
    pub tab_width: u16,
    /// Lines kept visible above and below the cursor.
    pub scrolloff: u16,
    /// Jump to the next unresolved hunk after resolving one.
    pub auto_advance: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "auto".to_owned(),
            color: ColorMode::Auto,
            layout: LayoutMode::Columns,
            panel_ratio: [1, 1, 1],
            stacked_result_percent: 45,
            show_info: true,
            info_height: 7,
            line_numbers: true,
            minimap: true,
            mouse: true,
            ascii: false,
            tab_width: 4,
            scrolloff: 3,
            auto_advance: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GitConfig {
    /// `git add` a file once it is saved with every hunk resolved.
    pub auto_stage: bool,
    /// Keep the previous working-tree file under `.git/mcr-backup/`.
    pub backup: bool,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            auto_stage: true,
            backup: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
enum KeyList {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawConfig {
    ui: UiConfig,
    git: GitConfig,
    keys: BTreeMap<String, KeyList>,
    themes: BTreeMap<String, ThemeOverride>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub ui: UiConfig,
    pub git: GitConfig,
    pub keymap: Keymap,
    /// Built-in and user themes, in switching order.
    pub themes: Vec<Theme>,
}

impl Default for Config {
    fn default() -> Self {
        Self::from_toml("").expect("default configuration is valid")
    }
}

impl Config {
    pub fn from_toml(text: &str) -> Result<Config> {
        let raw: RawConfig = toml::from_str(text)?;
        let keys: BTreeMap<String, Vec<String>> = raw
            .keys
            .into_iter()
            .map(|(action, keys)| {
                let keys = match keys {
                    KeyList::One(key) => vec![key],
                    KeyList::Many(keys) => keys,
                };
                (action, keys)
            })
            .collect();
        let keymap = Keymap::with_overrides(&keys).context("in [keys]")?;
        let themes = theme::build_themes(&raw.themes).context("in [themes]")?;

        let ui = raw.ui;
        if ui.panel_ratio.contains(&0) {
            bail!("ui.panel_ratio entries must be at least 1");
        }
        if ui.theme != "auto" && !themes.iter().any(|t| t.name == ui.theme) {
            bail!("ui.theme: unknown theme `{}`", ui.theme);
        }
        if ui.tab_width == 0 {
            bail!("ui.tab_width must be at least 1");
        }
        Ok(Config {
            ui,
            git: raw.git,
            keymap,
            themes,
        })
    }

    /// Load `explicit`, or the default configuration file if it exists.
    pub fn load(explicit: Option<&Path>) -> Result<Config> {
        let path = match explicit {
            Some(path) => path.to_owned(),
            None => match default_path() {
                Some(path) if path.is_file() => path,
                _ => return Ok(Config::default()),
            },
        };
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read config file {}", path.display()))?;
        Config::from_toml(&text).with_context(|| format!("invalid config file {}", path.display()))
    }

    /// Index into [`Config::themes`] of the theme to start with.
    pub fn initial_theme(&self, requested: Option<&str>) -> Result<usize> {
        let name = requested.unwrap_or(&self.ui.theme);
        let name = if name == "auto" {
            let dark = theme::terminal_is_dark(std::env::var("COLORFGBG").ok().as_deref());
            if dark {
                "dark"
            } else {
                "light"
            }
        } else {
            name
        };
        match self.themes.iter().position(|t| t.name == name) {
            Some(index) => Ok(index),
            None => bail!(
                "unknown theme `{name}` (available: {})",
                self.themes
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

/// `$MCR_CONFIG`, else `$XDG_CONFIG_HOME/mcr/config.toml`, else
/// `~/.config/mcr/config.toml`.
pub fn default_path() -> Option<PathBuf> {
    let env = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(path) = env("MCR_CONFIG") {
        return Some(path);
    }
    let dir = env("XDG_CONFIG_HOME")
        .or_else(|| env("HOME").map(|home| home.join(".config")))
        .or_else(|| env("APPDATA"))?;
    Some(dir.join("mcr").join("config.toml"))
}

/// A complete, commented configuration file showing every default.
pub fn default_config_toml() -> String {
    let ui = UiConfig::default();
    let git = GitConfig::default();
    let mut out = String::new();
    out.push_str(&format!(
        r##"# mcr configuration — every value below is the default.
# Location: $MCR_CONFIG, or $XDG_CONFIG_HOME/mcr/config.toml

[ui]
# "auto" (follow the terminal), "dark", "light" or a [themes.<name>] below.
theme = "{theme}"
# "auto" (from $COLORTERM), "truecolor" or "256".
color = "auto"
# "columns": source | result | target.  "stacked": source | target over result.
layout = "columns"
# Relative widths of the source, result and target panes.
panel_ratio = [{r0}, {r1}, {r2}]
# Height share of the result pane in the stacked layout.
stacked_result_percent = {stacked}
# Git history panel (author, date, commit) for the line under the cursor.
show_info = {show_info}
info_height = {info_height}
line_numbers = {line_numbers}
# Overview column marking where changes and open conflicts are.
minimap = {minimap}
mouse = {mouse}
# ASCII markers for terminals / fonts without Unicode symbols.
ascii = {ascii}
tab_width = {tab_width}
scrolloff = {scrolloff}
# Jump to the next unresolved hunk after resolving one.
auto_advance = {auto_advance}

[git]
# Run `git add` on a file once it is saved with every hunk resolved.
auto_stage = {auto_stage}
# Keep the previous working-tree file under .git/mcr-backup/ on first save.
backup = {backup}

# Keys: `action = "key"` or `action = ["key", "other"]`; `[]` unbinds.
# Keys look like: j  G  space  enter  ctrl-d  alt-x  shift-tab  f5  <c-d>
# A binding may be a sequence: "g g", "] c".
[keys]
"##,
        theme = ui.theme,
        r0 = ui.panel_ratio[0],
        r1 = ui.panel_ratio[1],
        r2 = ui.panel_ratio[2],
        stacked = ui.stacked_result_percent,
        show_info = ui.show_info,
        info_height = ui.info_height,
        line_numbers = ui.line_numbers,
        minimap = ui.minimap,
        mouse = ui.mouse,
        ascii = ui.ascii,
        tab_width = ui.tab_width,
        scrolloff = ui.scrolloff,
        auto_advance = ui.auto_advance,
        auto_stage = git.auto_stage,
        backup = git.backup,
    ));

    let keymap = Keymap::default();
    let mut group = "";
    for action in Action::ALL {
        if action.group() != group {
            group = action.group();
            out.push_str(&format!("\n# {group}\n"));
        }
        let keys: Vec<String> = keymap
            .keys_for(*action)
            .iter()
            .map(|k| format!("{k:?}"))
            .collect();
        out.push_str(&format!(
            "{:<22} = [{}]  # {}\n",
            action.name(),
            keys.join(", "),
            action.description()
        ));
    }

    out.push_str(
        r##"
# Themes: override colors of a built-in theme, or define your own.
# `base` is "dark" (default) or "light"; `tint` is the opacity (0–1) of
# diff and conflict backgrounds. Colors are "#rrggbb".
#
# [themes.midnight]
# base = "dark"
# tint = 0.20
"##,
    );
    let dark = Theme::dark();
    for role in Theme::ROLES {
        if let Some(Rgb(r, g, b)) = dark.role(role) {
            out.push_str(&format!("# {role:<10} = \"#{r:02x}{g:02x}{b:02x}\"\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_is_the_default() {
        let config = Config::from_toml("").unwrap();
        assert_eq!(config.ui, UiConfig::default());
        assert_eq!(config.git, GitConfig::default());
        assert_eq!(config.themes.len(), 2);
    }

    #[test]
    fn generated_default_config_parses_to_the_default() {
        let text = default_config_toml();
        let config = Config::from_toml(&text).unwrap();
        assert_eq!(config.ui, UiConfig::default());
        let defaults = Keymap::default();
        for action in Action::ALL {
            assert_eq!(config.keymap.keys_for(*action), defaults.keys_for(*action));
        }
    }

    #[test]
    fn full_config_is_applied() {
        let config = Config::from_toml(
            r##"
            [ui]
            theme = "midnight"
            layout = "stacked"
            panel_ratio = [2, 3, 2]
            color = "256"
            ascii = true

            [git]
            auto_stage = false

            [keys]
            save = "ctrl-w"
            undo = ["u", "<c-z>"]

            [themes.midnight]
            accent = "#00e5ff"
            tint = 0.25
            "##,
        )
        .unwrap();
        assert_eq!(config.ui.layout, LayoutMode::Stacked);
        assert_eq!(config.ui.panel_ratio, [2, 3, 2]);
        assert_eq!(config.ui.color, ColorMode::Ansi256);
        assert!(!config.git.auto_stage);
        assert_eq!(config.keymap.keys_for(Action::Save), ["ctrl-w"]);
        assert_eq!(config.keymap.keys_for(Action::Undo), ["u", "ctrl-z"]);
        let index = config.initial_theme(None).unwrap();
        assert_eq!(config.themes[index].name, "midnight");
        assert_eq!(config.themes[index].accent, Rgb(0, 0xe5, 0xff));
        assert_eq!(config.initial_theme(Some("light")).unwrap(), 1);
        assert!(config.initial_theme(Some("nope")).is_err());
    }

    #[test]
    fn mistakes_are_rejected_with_context() {
        for (toml, needle) in [
            ("[ui]\nthem = \"dark\"", "unknown field"),
            ("[ui]\ntheme = \"nope\"", "unknown theme"),
            ("[ui]\npanel_ratio = [1, 0, 1]", "panel_ratio"),
            ("[keys]\nfly = \"x\"", "unknown action"),
            ("[keys]\nsave = \"notakey\"", "invalid key"),
            ("[themes.x]\naccent = \"blue\"", "invalid color"),
        ] {
            let err = format!("{:#}", Config::from_toml(toml).unwrap_err());
            assert!(err.contains(needle), "`{toml}` gave: {err}");
        }
    }
}
