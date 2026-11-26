//! Everything the user can ask the application to do.
//!
//! Actions are the vocabulary shared by the keymap (which produces them),
//! the configuration file (which names them) and the app (which runs them).

macro_rules! actions {
    ($( $group:literal { $( $variant:ident => $name:literal, $desc:literal; )* } )*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Action { $( $( $variant, )* )* }

        impl Action {
            /// Every action, in help-screen order.
            pub const ALL: &'static [Action] = &[ $( $( Action::$variant, )* )* ];

            /// The name used in the configuration file.
            pub fn name(self) -> &'static str {
                match self { $( $( Action::$variant => $name, )* )* }
            }

            pub fn description(self) -> &'static str {
                match self { $( $( Action::$variant => $desc, )* )* }
            }

            /// Help-screen section.
            pub fn group(self) -> &'static str {
                match self { $( $( Action::$variant => $group, )* )* }
            }

            pub fn from_name(name: &str) -> Option<Action> {
                match name { $( $( $name => Some(Action::$variant), )* )* _ => None }
            }
        }
    };
}

actions! {
    "Navigation" {
        CursorDown => "cursor_down", "Move down one line";
        CursorUp => "cursor_up", "Move up one line";
        HalfPageDown => "half_page_down", "Move down half a page";
        HalfPageUp => "half_page_up", "Move up half a page";
        PageDown => "page_down", "Move down one page";
        PageUp => "page_up", "Move up one page";
        GotoTop => "goto_top", "Jump to the first line";
        GotoBottom => "goto_bottom", "Jump to the last line";
        NextHunk => "next_hunk", "Jump to the next change";
        PrevHunk => "prev_hunk", "Jump to the previous change";
        NextConflict => "next_conflict", "Jump to the next unresolved hunk";
        PrevConflict => "prev_conflict", "Jump to the previous unresolved hunk";
        FocusLeft => "focus_left", "Focus the pane to the left";
        FocusRight => "focus_right", "Focus the pane to the right";
        FocusNext => "focus_next", "Cycle pane focus forward";
        FocusPrev => "focus_prev", "Cycle pane focus backward";
        ScrollLeft => "scroll_left", "Scroll text left";
        ScrollRight => "scroll_right", "Scroll text right";
    }
    "Resolve lines" {
        ToggleAcceptLine => "toggle_accept_line", "Accept the line under the cursor (again: undecide)";
        ToggleRejectLine => "toggle_reject_line", "Reject the line under the cursor (again: undecide)";
    }
    "Resolve hunks" {
        TakeHunk => "take_hunk", "Take the focused side's hunk (result pane: finish a line-by-line pick)";
        TakeSource => "take_source", "Take the source hunk";
        TakeTarget => "take_target", "Take the target hunk";
        TakeBoth => "take_both", "Take both: source, then target";
        TakeBothReversed => "take_both_reversed", "Take both: target, then source";
        TakeNone => "take_none", "Take neither side";
        ResolveRemaining => "resolve_remaining", "Reject the hunk's pending lines, keep accepted ones";
        ResetHunk => "reset_hunk", "Reset the hunk to its initial state";
        EditHunk => "edit_hunk", "Edit the hunk's result in $EDITOR";
        TakeAllSource => "take_all_source", "Take source for every unresolved hunk";
        TakeAllTarget => "take_all_target", "Take target for every unresolved hunk";
    }
    "History & files" {
        Undo => "undo", "Undo the last change";
        Redo => "redo", "Redo the last undone change";
        Save => "save", "Write the file (and stage it once fully resolved)";
        SaveAll => "save_all", "Write every modified file";
        NextFile => "next_file", "Open the next conflicted file";
        PrevFile => "prev_file", "Open the previous conflicted file";
        FilePicker => "file_picker", "Choose a conflicted file";
    }
    "View" {
        ToggleTheme => "toggle_theme", "Switch to the next theme";
        ToggleLayout => "toggle_layout", "Switch between column and stacked layout";
        ToggleInfo => "toggle_info", "Show or hide the git history panel";
        GrowCenter => "grow_center", "Widen the result pane";
        ShrinkCenter => "shrink_center", "Narrow the result pane";
        ResetLayout => "reset_layout", "Restore the configured pane sizes";
        Help => "help", "Show this help";
        Cancel => "cancel", "Close a dialog / clear a pending key";
        Quit => "quit", "Quit";
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_round_trip() {
        let mut seen = std::collections::HashSet::new();
        for action in Action::ALL {
            assert!(
                seen.insert(action.name()),
                "duplicate name {}",
                action.name()
            );
            assert_eq!(Action::from_name(action.name()), Some(*action));
        }
        assert_eq!(Action::from_name("nope"), None);
    }
}
