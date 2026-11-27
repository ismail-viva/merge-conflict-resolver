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

/// Which glyphs the interface may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IconSet {
    /// Nerd Font icons (needs a patched font, e.g. JetBrainsMono Nerd Font).
    #[default]
    Nerd,
    /// Plain Unicode symbols that every modern font has.
    Unicode,
    /// ASCII only, including the borders.
    Ascii,
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
    pub line_numbers: bool,
    /// Color source code by its syntax.
    pub syntax: bool,
    /// Set comments and control-flow keywords in italics.
    pub italic: bool,
    pub minimap: bool,
    pub mouse: bool,
    /// Glyphs for markers and decorations.
    pub icons: IconSet,
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
            line_numbers: true,
            syntax: true,
            italic: true,
            minimap: false,
            mouse: true,
            icons: IconSet::Nerd,
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
        Self::from_toml_with_themes(text, BTreeMap::new())
    }

    /// Like [`Config::from_toml`], with themes defined outside the file.
    /// A `[themes.<name>]` table in the file wins over a theme file of the
    /// same name.
    pub fn from_toml_with_themes(
        text: &str,
        mut theme_files: BTreeMap<String, ThemeOverride>,
    ) -> Result<Config> {
        let raw: RawConfig = toml::from_str(text)?;
        theme_files.extend(raw.themes);
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
        let themes = theme::build_themes(&theme_files).context("in themes")?;

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

    /// Load `explicit`, or the default configuration file if it exists,
    /// together with the theme files in the `themes/` directory next to it.
    pub fn load(explicit: Option<&Path>) -> Result<Config> {
        let path = explicit.map(Path::to_owned).or_else(default_path);
        let themes = match path.as_deref().and_then(Path::parent) {
            Some(dir) => load_theme_files(&dir.join("themes"))?,
            None => BTreeMap::new(),
        };
        let text = match &path {
            Some(path) if explicit.is_some() || path.is_file() => std::fs::read_to_string(path)
                .with_context(|| format!("cannot read config file {}", path.display()))?,
            _ => String::new(),
        };
        let shown = path.map(|p| p.display().to_string()).unwrap_or_default();
        Config::from_toml_with_themes(&text, themes)
            .with_context(|| format!("invalid configuration ({shown})"))
    }

    /// Index into [`Config::themes`] of the theme to start with.
    pub fn initial_theme(&self, requested: Option<&str>) -> Result<usize> {
        let name = requested.unwrap_or(&self.ui.theme);
        let name = if name == "auto" {
            let dark = theme::terminal_is_dark(std::env::var("COLORFGBG").ok().as_deref());
            if dark {
                "tokyonight"
            } else {
                "tokyonight-day"
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

/// Read every `<name>.toml` in `dir` as the theme `<name>`. Each file holds
/// what a `[themes.<name>]` table would. A missing directory is fine.
pub fn load_theme_files(dir: &Path) -> Result<BTreeMap<String, ThemeOverride>> {
    let mut themes = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(themes);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_stem().and_then(|stem| stem.to_str());
        let is_toml = path.extension().is_some_and(|ext| ext == "toml");
        let (Some(name), true) = (name, is_toml && path.is_file()) else {
            continue;
        };
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read theme file {}", path.display()))?;
        let theme: ThemeOverride = toml::from_str(&text)
            .with_context(|| format!("invalid theme file {}", path.display()))?;
        themes.insert(name.to_owned(), theme);
    }
    Ok(themes)
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
# "auto" (tokyonight, or tokyonight-day on a light terminal), "tokyonight",
# "tokyonight-storm", "tokyonight-day", "dark", "light" (Material) or a
# [themes.<name>] defined below.
theme = "{theme}"
# "auto" (from $COLORTERM), "truecolor" or "256".
color = "auto"
# "columns": source | result | target.  "stacked": source | target over result.
layout = "columns"
# Relative widths of the source, result and target panes.
panel_ratio = [{r0}, {r1}, {r2}]
# Height share of the result pane in the stacked layout.
stacked_result_percent = {stacked}
line_numbers = {line_numbers}
# Syntax highlighting, and italics for comments and control-flow keywords.
syntax = {syntax}
italic = {italic}
# Overview column marking where changes and open conflicts are.
minimap = {minimap}
mouse = {mouse}
# "nerd" (Nerd Font icons), "unicode" (plain symbols) or "ascii".
icons = "nerd"
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
        line_numbers = ui.line_numbers,
        syntax = ui.syntax,
        italic = ui.italic,
        minimap = ui.minimap,
        mouse = ui.mouse,
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
# `base` is any built-in theme (default "tokyonight"); `tint` is the opacity (0–1) of
# diff and conflict backgrounds. Colors are "#rrggbb". The `syntax` table
# colors source code.
#
# [themes.midnight]
# base = "tokyonight"
# tint = 0.20
"##,
    );
    let dark = Theme::tokyonight();
    for role in Theme::ROLES {
        if let Some(Rgb(r, g, b)) = dark.role(role) {
            out.push_str(&format!("# {role:<10} = \"#{r:02x}{g:02x}{b:02x}\"\n"));
        }
    }
    out.push_str("#\n# [themes.midnight.syntax]\n");
    for token in crate::syntax::Token::ALL {
        let Rgb(r, g, b) = dark.syntax.color(*token);
        out.push_str(&format!(
            "# {:<16} = \"#{r:02x}{g:02x}{b:02x}\"\n",
            token.name()
        ));
    }
    out.push_str(
        "#\n# A theme can also live in its own file: themes/<name>.toml next to this\n\
         # file, holding the same keys without the [themes.<name>] header.\n",
    );
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
        assert_eq!(config.themes.len(), Theme::BUILTIN.len());
        assert_eq!(config.ui.icons, IconSet::Nerd);
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
            icons = "ascii"

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
        assert_eq!(
            config.themes[config.initial_theme(Some("light")).unwrap()].name,
            "light"
        );
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

    #[test]
    fn theme_files_are_loaded_next_to_the_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::create_dir(dir.path().join("themes")).unwrap();
        std::fs::write(
            dir.path().join("themes/ocean.toml"),
            "base = \"tokyonight-day\"\naccent = \"#006064\"\n[syntax]\nkeyword = \"#aa00ff\"\ntype = \"#004d40\"\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("themes/notes.txt"), "not a theme").unwrap();

        // Theme files work without any config file…
        let config = Config::load(Some(&config_path)).err();
        assert!(
            config.is_some(),
            "an explicitly named config file must exist"
        );
        std::fs::write(&config_path, "[ui]\ntheme = \"ocean\"\n").unwrap();
        let config = Config::load(Some(&config_path)).unwrap();
        let ocean = &config.themes[config.initial_theme(None).unwrap()];
        assert_eq!(ocean.name, "ocean");
        assert_eq!(ocean.accent, Rgb(0, 0x60, 0x64));
        assert_eq!(ocean.bg, Theme::tokyonight_day().bg);
        assert_eq!(ocean.syntax.keyword, Rgb(0xaa, 0, 0xff));
        assert_eq!(ocean.syntax.type_, Rgb(0, 0x4d, 0x40));
        assert_eq!(ocean.syntax.string, Theme::tokyonight_day().syntax.string);

        // …and a table in the config file wins over a file of the same name.
        std::fs::write(&config_path, "[themes.ocean]\naccent = \"#ff0000\"\n").unwrap();
        let config = Config::load(Some(&config_path)).unwrap();
        let ocean = config.themes.iter().find(|t| t.name == "ocean").unwrap();
        assert_eq!(ocean.accent, Rgb(0xff, 0, 0));

        std::fs::write(dir.path().join("themes/bad.toml"), "accent = \"red\"").unwrap();
        let err = format!("{:#}", Config::load(Some(&config_path)).unwrap_err());
        assert!(err.contains("invalid color"), "{err}");
    }

    #[test]
    fn the_example_theme_file_sets_every_color() {
        let text = include_str!("../../themes/catppuccin-mocha.toml");
        let over: ThemeOverride = toml::from_str(text).unwrap();
        let value: toml::Table = toml::from_str(text).unwrap();
        for role in Theme::ROLES {
            assert!(
                value.contains_key(*role),
                "the template is missing `{role}`"
            );
        }
        let syntax = value["syntax"].as_table().unwrap();
        for role in theme::SyntaxTheme::ROLES {
            assert!(
                syntax.contains_key(*role),
                "the template is missing syntax `{role}`"
            );
        }
        let themes = theme::build_themes(&BTreeMap::from([("mocha".to_owned(), over)])).unwrap();
        let mocha = themes.last().unwrap();
        assert_eq!(mocha.bg, Rgb(0x1e, 0x1e, 0x2e));
        assert_eq!(mocha.syntax.string, Rgb(0xa6, 0xe3, 0xa1));
    }
}
