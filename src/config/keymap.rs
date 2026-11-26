//! Key parsing and the (fully rebindable) keymap.
//!
//! A binding is a *sequence* of one or more keys, written the way they are
//! typed: `j`, `G`, `ctrl-d`, `shift-tab`, `g g`, `] c`. Vim notation
//! (`<c-d>`, `<space>`) is accepted too.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::action::Action;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KeymapError {
    #[error("invalid key `{0}`")]
    InvalidKey(String),
    #[error("empty key binding for `{0}`")]
    Empty(String),
    #[error("unknown action `{0}`")]
    UnknownAction(String),
    #[error("key `{key}` is bound to both `{first}` and `{second}`")]
    Duplicate {
        key: String,
        first: &'static str,
        second: &'static str,
    },
}

impl Key {
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        Self { code, mods }.normalized()
    }

    pub fn char(c: char) -> Self {
        Self::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    /// Fold the different ways terminals report shifted keys into one form:
    /// `shift-g` is `G`, `shift-tab` is `BackTab`.
    fn normalized(mut self) -> Self {
        self.mods &= KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT;
        match self.code {
            KeyCode::Char(c) => {
                if self.mods.contains(KeyModifiers::SHIFT) && c.is_ascii_lowercase() {
                    self.code = KeyCode::Char(c.to_ascii_uppercase());
                }
                self.mods.remove(KeyModifiers::SHIFT);
            }
            KeyCode::Tab if self.mods.contains(KeyModifiers::SHIFT) => {
                self.code = KeyCode::BackTab;
                self.mods.remove(KeyModifiers::SHIFT);
            }
            KeyCode::BackTab => self.mods.remove(KeyModifiers::SHIFT),
            _ => {}
        }
        self
    }

    pub fn from_event(event: KeyEvent) -> Self {
        Self::new(event.code, event.modifiers)
    }

    pub fn parse(text: &str) -> Result<Key, KeymapError> {
        let invalid = || KeymapError::InvalidKey(text.to_owned());
        let vim = text.len() > 2 && text.starts_with('<') && text.ends_with('>');
        let mut rest = if vim { &text[1..text.len() - 1] } else { text };
        let mut mods = KeyModifiers::NONE;

        while let Some((prefix, tail)) = rest.split_once('-') {
            if tail.is_empty() {
                break; // a trailing `-` is the key itself, as in `ctrl--`
            }
            let modifier = match prefix.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => KeyModifiers::CONTROL,
                "alt" | "meta" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                "c" if vim => KeyModifiers::CONTROL,
                "a" | "m" if vim => KeyModifiers::ALT,
                "s" if vim => KeyModifiers::SHIFT,
                _ => break,
            };
            mods |= modifier;
            rest = tail;
        }

        let mut chars = rest.chars();
        let code = match (chars.next(), chars.next()) {
            (Some(c), None) => KeyCode::Char(c),
            (None, _) => return Err(invalid()),
            _ => match rest.to_ascii_lowercase().as_str() {
                "space" => KeyCode::Char(' '),
                "enter" | "return" | "cr" => KeyCode::Enter,
                "esc" | "escape" => KeyCode::Esc,
                "tab" => KeyCode::Tab,
                "backtab" => KeyCode::BackTab,
                "backspace" | "bs" => KeyCode::Backspace,
                "delete" | "del" => KeyCode::Delete,
                "insert" | "ins" => KeyCode::Insert,
                "up" => KeyCode::Up,
                "down" => KeyCode::Down,
                "left" => KeyCode::Left,
                "right" => KeyCode::Right,
                "home" => KeyCode::Home,
                "end" => KeyCode::End,
                "pageup" | "pgup" => KeyCode::PageUp,
                "pagedown" | "pgdn" => KeyCode::PageDown,
                "lt" => KeyCode::Char('<'),
                "gt" => KeyCode::Char('>'),
                name => match name.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                    Some(n @ 1..=12) => KeyCode::F(n),
                    _ => return Err(invalid()),
                },
            },
        };
        Ok(Key::new(code, mods))
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (flag, name) in [
            (KeyModifiers::CONTROL, "ctrl-"),
            (KeyModifiers::ALT, "alt-"),
            (KeyModifiers::SHIFT, "shift-"),
        ] {
            if self.mods.contains(flag) {
                f.write_str(name)?;
            }
        }
        match self.code {
            KeyCode::Char(' ') => f.write_str("space"),
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::Enter => f.write_str("enter"),
            KeyCode::Esc => f.write_str("esc"),
            KeyCode::Tab => f.write_str("tab"),
            KeyCode::BackTab => f.write_str("shift-tab"),
            KeyCode::Backspace => f.write_str("backspace"),
            KeyCode::Delete => f.write_str("delete"),
            KeyCode::Insert => f.write_str("insert"),
            KeyCode::Up => f.write_str("up"),
            KeyCode::Down => f.write_str("down"),
            KeyCode::Left => f.write_str("left"),
            KeyCode::Right => f.write_str("right"),
            KeyCode::Home => f.write_str("home"),
            KeyCode::End => f.write_str("end"),
            KeyCode::PageUp => f.write_str("pageup"),
            KeyCode::PageDown => f.write_str("pagedown"),
            KeyCode::F(n) => write!(f, "f{n}"),
            other => write!(f, "{other:?}"),
        }
    }
}

/// Parse a whitespace-separated key sequence such as `g g`.
pub fn parse_sequence(text: &str) -> Result<Vec<Key>, KeymapError> {
    let keys = text
        .split_whitespace()
        .map(Key::parse)
        .collect::<Result<Vec<_>, _>>()?;
    if keys.is_empty() {
        return Err(KeymapError::InvalidKey(text.to_owned()));
    }
    Ok(keys)
}

pub fn format_sequence(keys: &[Key]) -> String {
    keys.iter()
        .map(Key::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Outcome of looking up the keys typed so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookup {
    Match(Action),
    /// The keys start a longer binding; wait for more.
    Prefix,
    None,
}

const DEFAULTS: &[(Action, &[&str])] = &[
    (Action::CursorDown, &["j", "down"]),
    (Action::CursorUp, &["k", "up"]),
    (Action::HalfPageDown, &["ctrl-d"]),
    (Action::HalfPageUp, &["ctrl-u"]),
    (Action::PageDown, &["ctrl-f", "pagedown"]),
    (Action::PageUp, &["ctrl-b", "pageup"]),
    (Action::GotoTop, &["g g", "home"]),
    (Action::GotoBottom, &["G", "end"]),
    (Action::NextHunk, &["J"]),
    (Action::PrevHunk, &["K"]),
    (Action::NextConflict, &["n", "] c"]),
    (Action::PrevConflict, &["N", "[ c"]),
    (Action::FocusLeft, &["h", "left"]),
    (Action::FocusRight, &["l", "right"]),
    (Action::FocusNext, &["tab"]),
    (Action::FocusPrev, &["shift-tab"]),
    (Action::ScrollLeft, &["H"]),
    (Action::ScrollRight, &["L"]),
    (Action::ToggleAcceptLine, &["space"]),
    (Action::ToggleRejectLine, &["x"]),
    (Action::TakeHunk, &["enter"]),
    (Action::TakeSource, &["s"]),
    (Action::TakeTarget, &["t"]),
    (Action::TakeBoth, &["b"]),
    (Action::TakeBothReversed, &["B"]),
    (Action::TakeNone, &["d"]),
    (Action::ResolveRemaining, &["c"]),
    (Action::ResetHunk, &["r"]),
    (Action::EditHunk, &["e"]),
    (Action::TakeAllSource, &["S"]),
    (Action::TakeAllTarget, &["T"]),
    (Action::Undo, &["u", "z"]),
    (Action::Redo, &["ctrl-r", "Z"]),
    (Action::Save, &["w", "ctrl-s"]),
    (Action::SaveAll, &["W"]),
    (Action::NextFile, &["] f"]),
    (Action::PrevFile, &["[ f"]),
    (Action::FilePicker, &["f"]),
    (Action::ToggleTheme, &["ctrl-t"]),
    (Action::ToggleLayout, &["v"]),
    (Action::GrowCenter, &["+"]),
    (Action::ShrinkCenter, &["-"]),
    (Action::ResetLayout, &["="]),
    (Action::Help, &["?"]),
    (Action::Cancel, &["esc"]),
    (Action::Quit, &["q", "ctrl-c"]),
];

#[derive(Debug, Clone)]
pub struct Keymap {
    /// Per action, its bindings in display order.
    by_action: BTreeMap<Action, Vec<Vec<Key>>>,
    lookup: HashMap<Vec<Key>, Action>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::with_overrides(&BTreeMap::new()).expect("default keymap is valid")
    }
}

impl Keymap {
    /// The default keymap with `overrides` applied (`action` → sequences).
    ///
    /// Listing an action replaces all of its default keys; an empty list
    /// unbinds it. A key given to an action is taken away from whichever
    /// default binding used it, so a rebind never needs a matching unbind.
    pub fn with_overrides(overrides: &BTreeMap<String, Vec<String>>) -> Result<Self, KeymapError> {
        let mut by_action: BTreeMap<Action, Vec<Vec<Key>>> = BTreeMap::new();
        for (action, keys) in DEFAULTS {
            let sequences = keys
                .iter()
                .map(|k| parse_sequence(k))
                .collect::<Result<_, _>>()?;
            by_action.insert(*action, sequences);
        }

        let mut custom: HashMap<Vec<Key>, Action> = HashMap::new();
        for (name, keys) in overrides {
            let action =
                Action::from_name(name).ok_or_else(|| KeymapError::UnknownAction(name.clone()))?;
            let mut sequences = Vec::new();
            for key in keys {
                if key.trim().is_empty() {
                    return Err(KeymapError::Empty(name.clone()));
                }
                let sequence = parse_sequence(key)?;
                if let Some(first) = custom.insert(sequence.clone(), action) {
                    if first != action {
                        return Err(KeymapError::Duplicate {
                            key: format_sequence(&sequence),
                            first: first.name(),
                            second: action.name(),
                        });
                    }
                }
                sequences.push(sequence);
            }
            by_action.insert(action, sequences);
        }
        for (action, sequences) in by_action.iter_mut() {
            sequences.retain(|seq| custom.get(seq).is_none_or(|owner| owner == action));
        }

        let mut lookup = HashMap::new();
        for (action, sequences) in &by_action {
            for sequence in sequences {
                if let Some(first) = lookup.insert(sequence.clone(), *action) {
                    return Err(KeymapError::Duplicate {
                        key: format_sequence(sequence),
                        first: first.name(),
                        second: action.name(),
                    });
                }
            }
        }
        Ok(Self { by_action, lookup })
    }

    pub fn lookup(&self, keys: &[Key]) -> Lookup {
        if let Some(action) = self.lookup.get(keys) {
            return Lookup::Match(*action);
        }
        let is_prefix = self
            .lookup
            .keys()
            .any(|seq| seq.len() > keys.len() && seq.starts_with(keys));
        if is_prefix {
            Lookup::Prefix
        } else {
            Lookup::None
        }
    }

    /// All bindings of `action`, formatted for display.
    pub fn keys_for(&self, action: Action) -> Vec<String> {
        self.by_action
            .get(&action)
            .map(|seqs| seqs.iter().map(|s| format_sequence(s)).collect())
            .unwrap_or_default()
    }

    /// The primary binding of `action`, if it has one.
    pub fn hint(&self, action: Action) -> Option<String> {
        self.keys_for(action).into_iter().next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> Key {
        Key::parse(text).unwrap()
    }

    fn overrides(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
            .collect()
    }

    #[test]
    fn parses_plain_modified_and_named_keys() {
        assert_eq!(key("j"), Key::char('j'));
        assert_eq!(key("G"), Key::char('G'));
        assert_eq!(
            key("ctrl-d"),
            Key::new(KeyCode::Char('d'), KeyModifiers::CONTROL)
        );
        assert_eq!(
            key("Ctrl-Alt-x").mods,
            KeyModifiers::CONTROL | KeyModifiers::ALT
        );
        assert_eq!(key("space"), Key::char(' '));
        assert_eq!(key("shift-tab").code, KeyCode::BackTab);
        assert_eq!(key("f5").code, KeyCode::F(5));
        assert_eq!(key("-"), Key::char('-'));
        assert_eq!(
            key("ctrl--"),
            Key::new(KeyCode::Char('-'), KeyModifiers::CONTROL)
        );
        assert_eq!(key("<c-d>"), key("ctrl-d"));
        assert_eq!(key("<space>"), key("space"));
        assert_eq!(key("<"), Key::char('<'));
        assert!(Key::parse("ctrl-").is_err() || key("ctrl-").code == KeyCode::Char('-'));
        assert!(Key::parse("bogus").is_err());
        assert!(Key::parse("f99").is_err());
    }

    #[test]
    fn terminal_events_normalize_to_parsed_keys() {
        let shifted = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert_eq!(Key::from_event(shifted), key("G"));
        let lower = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::SHIFT);
        assert_eq!(Key::from_event(lower), key("G"));
        let backtab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(Key::from_event(backtab), key("shift-tab"));
    }

    #[test]
    fn display_round_trips() {
        for text in [
            "j",
            "G",
            "ctrl-d",
            "space",
            "shift-tab",
            "alt-enter",
            "f2",
            "pageup",
        ] {
            assert_eq!(key(text).to_string(), text);
        }
        assert_eq!(format_sequence(&parse_sequence("g g").unwrap()), "g g");
    }

    #[test]
    fn default_keymap_resolves_sequences() {
        let map = Keymap::default();
        assert_eq!(map.lookup(&[key("j")]), Lookup::Match(Action::CursorDown));
        assert_eq!(map.lookup(&[key("g")]), Lookup::Prefix);
        assert_eq!(
            map.lookup(&[key("g"), key("g")]),
            Lookup::Match(Action::GotoTop)
        );
        assert_eq!(
            map.lookup(&[key("]"), key("c")]),
            Lookup::Match(Action::NextConflict)
        );
        assert_eq!(map.lookup(&[key("f9")]), Lookup::None);
    }

    #[test]
    fn every_action_has_a_default_binding() {
        let map = Keymap::default();
        for action in Action::ALL {
            assert!(map.hint(*action).is_some(), "{} is unbound", action.name());
        }
    }

    #[test]
    fn overrides_replace_and_steal_keys() {
        let map =
            Keymap::with_overrides(&overrides(&[("cursor_down", &["s"]), ("quit", &[])])).unwrap();
        assert_eq!(map.lookup(&[key("s")]), Lookup::Match(Action::CursorDown));
        assert_eq!(map.lookup(&[key("j")]), Lookup::None);
        assert!(
            map.keys_for(Action::TakeSource).is_empty(),
            "s was taken from take_source"
        );
        assert_eq!(map.lookup(&[key("q")]), Lookup::None);
    }

    #[test]
    fn invalid_overrides_are_rejected() {
        assert_eq!(
            Keymap::with_overrides(&overrides(&[("fly", &["x"])])).unwrap_err(),
            KeymapError::UnknownAction("fly".into())
        );
        assert!(matches!(
            Keymap::with_overrides(&overrides(&[("undo", &["a"]), ("redo", &["a"])])),
            Err(KeymapError::Duplicate { .. })
        ));
        assert!(matches!(
            Keymap::with_overrides(&overrides(&[("undo", &["notakey"])])),
            Err(KeymapError::InvalidKey(_))
        ));
    }
}
