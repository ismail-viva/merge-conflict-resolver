//! Application state and behaviour, independent of the terminal.
//!
//! [`App`] consumes input events and mutates its state; drawing lives in
//! [`crate::ui`] and anything that needs the real terminal (running an
//! editor, quitting) is handed back to the caller as an [`Effect`]. That
//! keeps the whole interaction model testable without a TTY.

pub mod view;

use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};

use crate::action::Action;
use crate::config::{ColorDepth, Config, Key, LayoutMode, Lookup, Theme};
use crate::core::text::split_lines;
use crate::core::{LineRef, MergeSession, Op, OpError, Origin, Side};
use crate::git::{ConflictEntry, MergeContext, SharedBackend};
use crate::syntax::Language;

pub use view::{FileView, Pane, RowLayout, SaveInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Success,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub text: String,
    pub level: Level,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Help { scroll: usize },
    Files { selected: usize },
    ConfirmQuit,
}

/// A request to edit one hunk's text in the user's editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditRequest {
    pub file: usize,
    pub hunk: usize,
    pub text: String,
    /// File extension, so the editor can pick a syntax.
    pub extension: Option<String>,
}

/// Something only the terminal owner can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    None,
    Quit,
    Edit(EditRequest),
}

/// Screen areas of the last frame, for mouse hit-testing and paging.
#[derive(Debug, Clone, Copy, Default)]
pub struct Geometry {
    /// Text areas of the source, result and target panes.
    pub panes: [Rect; 3],
    /// Rows visible in a pane.
    pub height: usize,
}

pub struct FileSlot {
    pub entry: ConflictEntry,
    pub view: Option<FileView>,
}

const RATIO_SCALE: u16 = 10;
const RATIO_STEP: u16 = 2;

pub struct App {
    backend: SharedBackend,
    pub config: Config,
    pub context: MergeContext,
    pub files: Vec<FileSlot>,
    pub current: usize,
    pub focus: Pane,
    pub mode: Mode,
    pub message: Option<Message>,
    /// Keys typed so far of a multi-key binding.
    pub pending: Vec<Key>,
    pub geometry: Geometry,
    pub theme_index: usize,
    pub depth: ColorDepth,
    pub layout: LayoutMode,
    /// Relative pane widths (source, result, target).
    pub ratio: [u16; 3],
}

impl App {
    pub fn new(backend: SharedBackend, config: Config, theme_index: usize) -> anyhow::Result<App> {
        let entries = backend.conflicts()?;
        let first = entries
            .iter()
            .position(|e| e.unsupported.is_none())
            .ok_or_else(|| anyhow::anyhow!("no conflict that can be resolved line by line"))?;
        let mut app = App {
            context: backend.context(),
            backend,
            files: entries
                .into_iter()
                .map(|entry| FileSlot { entry, view: None })
                .collect(),
            current: first,
            focus: Pane::Result,
            mode: Mode::Normal,
            message: None,
            pending: Vec::new(),
            geometry: Geometry {
                panes: [Rect::default(); 3],
                height: 20,
            },
            theme_index,
            depth: ColorDepth::resolve(config.ui.color),
            layout: config.ui.layout,
            ratio: config.ui.panel_ratio.map(|r| r * RATIO_SCALE),
            config,
        };
        app.open_file(first);
        Ok(app)
    }

    // ---- accessors -------------------------------------------------------

    pub fn theme(&self) -> &Theme {
        &self.config.themes[self.theme_index]
    }

    pub fn view(&self) -> Option<&FileView> {
        self.files.get(self.current)?.view.as_ref()
    }

    fn view_mut(&mut self) -> Option<&mut FileView> {
        self.files.get_mut(self.current)?.view.as_mut()
    }

    pub fn current_path(&self) -> &str {
        &self.files[self.current].entry.path
    }

    pub fn has_unsaved_changes(&self) -> bool {
        self.files
            .iter()
            .filter_map(|f| f.view.as_ref())
            .any(FileView::is_dirty)
    }

    /// Whether every conflicted file has been written fully resolved.
    pub fn all_resolved(&self) -> bool {
        self.files.iter().all(|f| {
            f.view.as_ref().is_some_and(|v| {
                !v.is_dirty() && v.last_save.is_some_and(|save| save.unresolved == 0)
            })
        })
    }

    /// One line per file describing where it was left.
    pub fn summary(&self) -> Vec<String> {
        self.files
            .iter()
            .map(|file| {
                let state = match (&file.entry.unsupported, &file.view) {
                    (Some(reason), _) => format!("skipped ({reason})"),
                    (None, None) => "not opened".to_owned(),
                    (None, Some(view)) => match view.last_save {
                        Some(save) if view.is_dirty() => {
                            format!("{}, later changes not saved", describe_save(save))
                        }
                        Some(save) => describe_save(save),
                        None if view.is_dirty() => "changes not saved".to_owned(),
                        None => "untouched".to_owned(),
                    },
                };
                format!("{}: {state}", file.entry.path)
            })
            .collect()
    }

    fn notify(&mut self, level: Level, text: impl Into<String>) {
        self.message = Some(Message {
            text: text.into(),
            level,
        });
    }

    // ---- files -----------------------------------------------------------

    fn open_file(&mut self, index: usize) {
        let Some(slot) = self.files.get(index) else {
            return;
        };
        if let Some(reason) = &slot.entry.unsupported {
            let text = format!("{}: {reason}", slot.entry.path);
            self.notify(Level::Warn, text);
            return;
        }
        if slot.view.is_none() {
            let path = slot.entry.path.clone();
            match self.backend.load(&path) {
                Ok(input) => {
                    let language = self
                        .config
                        .ui
                        .syntax
                        .then(|| Language::from_path(&path))
                        .flatten();
                    self.files[index].view =
                        Some(FileView::with_language(MergeSession::new(input), language));
                }
                Err(err) => {
                    self.notify(Level::Error, format!("{path}: {err}"));
                    return;
                }
            }
        }
        self.current = index;
    }

    fn step_file(&mut self, forward: bool) {
        let count = self.files.len();
        let step = if forward { 1 } else { count - 1 };
        let mut index = self.current;
        for _ in 1..count {
            index = (index + step) % count;
            if self.files[index].entry.unsupported.is_none() {
                self.open_file(index);
                if self.current == index {
                    let text = format!("{} ({}/{count})", self.current_path(), index + 1);
                    self.notify(Level::Info, text);
                }
                return;
            }
        }
        self.notify(Level::Info, "No other conflicted file");
    }

    // ---- input -----------------------------------------------------------

    pub fn on_event(&mut self, event: Event) -> Effect {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key),
            Event::Mouse(mouse) => {
                self.on_mouse(mouse);
                Effect::None
            }
            _ => Effect::None,
        }
    }

    fn on_key(&mut self, event: KeyEvent) -> Effect {
        self.message = None;
        let key = Key::from_event(event);
        match self.mode {
            Mode::Normal => self.on_normal_key(key),
            Mode::Help { scroll } => {
                self.on_help_key(key, scroll);
                Effect::None
            }
            Mode::Files { selected } => {
                self.on_files_key(key, selected);
                Effect::None
            }
            Mode::ConfirmQuit => self.on_confirm_key(key),
        }
    }

    fn on_normal_key(&mut self, key: Key) -> Effect {
        self.pending.push(key);
        loop {
            match self.config.keymap.lookup(&self.pending) {
                Lookup::Match(action) => {
                    self.pending.clear();
                    return self.dispatch(action);
                }
                Lookup::Prefix => return Effect::None,
                Lookup::None if self.pending.len() > 1 => {
                    // The sequence broke off; the last key may start anew.
                    self.pending = vec![key];
                }
                Lookup::None => {
                    self.pending.clear();
                    return Effect::None;
                }
            }
        }
    }

    fn single_key_action(&self, key: Key) -> Option<Action> {
        match self.config.keymap.lookup(&[key]) {
            Lookup::Match(action) => Some(action),
            _ => None,
        }
    }

    fn on_help_key(&mut self, key: Key, scroll: usize) {
        let page = self.geometry.height.max(2) / 2;
        self.mode = match self.single_key_action(key) {
            Some(Action::CursorDown) => Mode::Help { scroll: scroll + 1 },
            Some(Action::CursorUp) => Mode::Help {
                scroll: scroll.saturating_sub(1),
            },
            Some(Action::HalfPageDown | Action::PageDown) => Mode::Help {
                scroll: scroll + page,
            },
            Some(Action::HalfPageUp | Action::PageUp) => Mode::Help {
                scroll: scroll.saturating_sub(page),
            },
            Some(Action::GotoTop) => Mode::Help { scroll: 0 },
            Some(Action::Cancel | Action::Quit | Action::Help) => Mode::Normal,
            _ if matches!(key.code, KeyCode::Esc | KeyCode::Enter) => Mode::Normal,
            _ => Mode::Help { scroll },
        };
    }

    fn on_files_key(&mut self, key: Key, selected: usize) {
        let last = self.files.len().saturating_sub(1);
        if key.code == KeyCode::Enter {
            self.mode = Mode::Normal;
            self.open_file(selected);
            return;
        }
        self.mode = match self.single_key_action(key) {
            Some(Action::CursorDown) => Mode::Files {
                selected: (selected + 1).min(last),
            },
            Some(Action::CursorUp) => Mode::Files {
                selected: selected.saturating_sub(1),
            },
            Some(Action::GotoBottom) => Mode::Files { selected: last },
            Some(Action::Cancel | Action::Quit | Action::FilePicker) => Mode::Normal,
            _ if key.code == KeyCode::Esc => Mode::Normal,
            _ => Mode::Files { selected },
        };
    }

    fn on_confirm_key(&mut self, key: Key) -> Effect {
        self.mode = Mode::Normal;
        match key.code {
            KeyCode::Char('y' | 'Y') => Effect::Quit,
            KeyCode::Char('w' | 'W') => {
                if self.save_all() {
                    Effect::Quit
                } else {
                    Effect::None
                }
            }
            _ => {
                self.notify(Level::Info, "Quit cancelled");
                Effect::None
            }
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent) {
        if self.mode != Mode::Normal {
            return;
        }
        let height = self.geometry.height;
        let panes = self.geometry.panes;
        let position = Position::new(mouse.column, mouse.row);
        let hit = Pane::ALL
            .into_iter()
            .find(|p| panes[p.index()].contains(position));
        let Some(view) = self.view_mut() else { return };
        // The pane and grid row under the pointer, if it is over a line.
        let pointed = hit.and_then(|pane| {
            let row = view.scroll + (mouse.row - panes[pane.index()].y) as usize;
            (row < view.rows.total()).then_some((pane, row))
        });
        match mouse.kind {
            MouseEventKind::ScrollDown => view.scroll_by(3, height),
            MouseEventKind::ScrollUp => view.scroll_by(-3, height),
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some((_, row)) = pointed {
                    view.move_to(row);
                }
                if let Some(pane) = hit {
                    self.focus = pane;
                }
                self.message = None;
            }
            _ => {}
        }
    }

    // ---- actions ---------------------------------------------------------

    pub fn dispatch(&mut self, action: Action) -> Effect {
        let page = self.geometry.height.max(1) as isize;
        match action {
            Action::CursorDown => self.with_view(|v| v.move_by(1)),
            Action::CursorUp => self.with_view(|v| v.move_by(-1)),
            Action::HalfPageDown => self.with_view(|v| v.move_by((page / 2).max(1))),
            Action::HalfPageUp => self.with_view(|v| v.move_by(-(page / 2).max(1))),
            Action::PageDown => self.with_view(|v| v.move_by(page)),
            Action::PageUp => self.with_view(|v| v.move_by(-page)),
            Action::GotoTop => self.with_view(|v| v.move_to(0)),
            Action::GotoBottom => self.with_view(|v| v.move_to(usize::MAX)),
            Action::NextHunk => self.jump(true, false),
            Action::PrevHunk => self.jump(false, false),
            Action::NextConflict => self.jump(true, true),
            Action::PrevConflict => self.jump(false, true),
            Action::FocusLeft => self.focus = self.focus.left(),
            Action::FocusRight => self.focus = self.focus.right(),
            Action::FocusNext => self.focus = self.focus.next(),
            Action::FocusPrev => self.focus = self.focus.prev(),
            Action::ScrollLeft => self.with_view(|v| v.hscroll = v.hscroll.saturating_sub(8)),
            Action::ScrollRight => self.with_view(|v| v.hscroll += 8),

            Action::ToggleAcceptLine => self.line_op(true),
            Action::ToggleRejectLine => self.line_op(false),
            Action::TakeHunk => match self.focus.side() {
                Some(side) => self.hunk_op(|hunk| Op::TakeSide { hunk, side }),
                None => self.finish_hunk(),
            },
            Action::TakeSource => self.hunk_op(|hunk| Op::TakeSide {
                hunk,
                side: Side::Source,
            }),
            Action::TakeTarget => self.hunk_op(|hunk| Op::TakeSide {
                hunk,
                side: Side::Target,
            }),
            Action::TakeBoth => self.hunk_op(|hunk| Op::TakeBoth {
                hunk,
                first: Side::Source,
            }),
            Action::TakeBothReversed => self.hunk_op(|hunk| Op::TakeBoth {
                hunk,
                first: Side::Target,
            }),
            Action::TakeNone => self.hunk_op(|hunk| Op::TakeNone { hunk }),
            Action::ResolveRemaining => self.hunk_op(|hunk| Op::ResolveRemaining { hunk }),
            Action::ResetHunk => self.apply(|hunk| Op::Reset { hunk }, false),
            Action::TakeAllSource => {
                self.apply(|_| Op::TakeSideEverywhere { side: Side::Source }, false)
            }
            Action::TakeAllTarget => {
                self.apply(|_| Op::TakeSideEverywhere { side: Side::Target }, false)
            }
            Action::EditHunk => return self.begin_edit(),

            Action::Undo => self.time_travel(true),
            Action::Redo => self.time_travel(false),
            Action::Save => {
                self.save(self.current);
            }
            Action::SaveAll => {
                self.save_all();
            }
            Action::NextFile => self.step_file(true),
            Action::PrevFile => self.step_file(false),
            Action::FilePicker => {
                self.mode = Mode::Files {
                    selected: self.current,
                }
            }

            Action::ToggleTheme => {
                self.theme_index = (self.theme_index + 1) % self.config.themes.len();
                let text = format!("Theme: {}", self.theme().name);
                self.notify(Level::Info, text);
            }
            Action::ToggleLayout => {
                self.layout = match self.layout {
                    LayoutMode::Columns => LayoutMode::Stacked,
                    LayoutMode::Stacked => LayoutMode::Columns,
                };
                self.notify(
                    Level::Info,
                    format!("Layout: {:?}", self.layout).to_lowercase(),
                );
            }
            Action::GrowCenter => self.ratio[1] = (self.ratio[1] + RATIO_STEP).min(80),
            Action::ShrinkCenter => self.ratio[1] = self.ratio[1].saturating_sub(RATIO_STEP).max(4),
            Action::ResetLayout => {
                self.ratio = self.config.ui.panel_ratio.map(|r| r * RATIO_SCALE);
                self.layout = self.config.ui.layout;
            }
            Action::Help => self.mode = Mode::Help { scroll: 0 },
            Action::Cancel => self.pending.clear(),
            Action::Quit => {
                if self.has_unsaved_changes() {
                    self.mode = Mode::ConfirmQuit;
                } else {
                    return Effect::Quit;
                }
            }
        }
        Effect::None
    }

    fn with_view(&mut self, f: impl FnOnce(&mut FileView)) {
        if let Some(view) = self.view_mut() {
            f(view);
        }
    }

    fn jump(&mut self, forward: bool, open_only: bool) {
        let Some(view) = self.view_mut() else { return };
        let target = if open_only {
            view.neighbor_open(forward)
        } else {
            view.neighbor_change(forward)
        };
        match target {
            Some(hunk) => view.goto_hunk(hunk),
            None => {
                let what = if open_only {
                    "unresolved hunk"
                } else {
                    "change"
                };
                let direction = if forward { "below" } else { "above" };
                self.notify(Level::Info, format!("No {what} {direction}"));
            }
        }
    }

    /// Accept (or reject) the single line under the cursor.
    fn line_op(&mut self, accept: bool) {
        let focus = self.focus;
        let Some(view) = self.view().filter(|v| v.current_hunk().is_some()) else {
            return;
        };
        let (hunk, offset) = view.position();
        let line = match focus.side() {
            Some(side) => Some(LineRef {
                side,
                index: offset,
            }),
            // In the result pane a line can only be removed again.
            None => match view.session.result_line(hunk, offset).map(|l| l.origin) {
                Some(Origin::Line(line)) if !accept => Some(line),
                _ => None,
            },
        };
        let Some(line) = line else {
            let hint = if accept {
                "Focus the source or target pane to accept one of its lines"
            } else {
                "Nothing to reject on this row"
            };
            self.notify(Level::Info, hint);
            return;
        };
        self.apply(
            |hunk| {
                if accept {
                    Op::ToggleAccept { hunk, line }
                } else {
                    Op::ToggleReject { hunk, line }
                }
            },
            false,
        );
    }

    /// `take_hunk` in the result pane: settle a hunk whose lines were picked
    /// one by one. Refuses to turn an untouched conflict into "take nothing".
    fn finish_hunk(&mut self) {
        let Some(view) = self.view().filter(|v| v.current_hunk().is_some()) else {
            return;
        };
        let (hunk, _) = view.position();
        if view.session.status(hunk) == crate::core::HunkStatus::Unresolved {
            let hint = |action: Action| self.config.keymap.hint(action).unwrap_or_default();
            let text = format!(
                "Nothing picked yet — take source ({}), target ({}), both ({}) or neither ({})",
                hint(Action::TakeSource),
                hint(Action::TakeTarget),
                hint(Action::TakeBoth),
                hint(Action::TakeNone),
            );
            self.notify(Level::Info, text);
            return;
        }
        self.hunk_op(|hunk| Op::ResolveRemaining { hunk });
    }

    fn hunk_op(&mut self, op: impl FnOnce(usize) -> Op) {
        self.apply(op, self.config.ui.auto_advance);
    }

    /// Run a merge operation on the hunk under the cursor and report it.
    fn apply(&mut self, op: impl FnOnce(usize) -> Op, advance: bool) {
        let Some(view) = self.view_mut().filter(|v| v.current_hunk().is_some()) else {
            return;
        };
        let (hunk, _) = view.position();
        match view.session.apply(op(hunk)) {
            Ok(applied) => {
                view.relayout();
                let open = view.session.stats().open;
                let pending = view.session.pending(hunk);
                let resolved_here = !view.session.status(hunk).is_open();
                if advance && resolved_here {
                    if let Some(next) = view.any_open() {
                        view.goto_hunk(next);
                    }
                }
                let progress = if applied.hunks.len() > 1 {
                    format!("{} hunks changed", applied.hunks.len())
                } else if pending > 0 {
                    format!(
                        "{pending} line{} still pending in this hunk",
                        plural(pending)
                    )
                } else {
                    "hunk resolved".to_owned()
                };
                let rest = if open == 0 {
                    "all hunks resolved — save to finish".to_owned()
                } else {
                    format!("{open} unresolved hunk{} left", plural(open))
                };
                let text = format!("{} · {progress} · {rest}", capitalize(applied.label));
                self.notify(Level::Success, text);
            }
            Err(OpError::NoChange) => self.notify(Level::Info, "Already in that state"),
            Err(err) => self.notify(Level::Warn, capitalize(&err.to_string())),
        }
    }

    fn time_travel(&mut self, undo: bool) {
        let Some(view) = self.view_mut() else { return };
        let applied = if undo {
            view.session.undo()
        } else {
            view.session.redo()
        };
        match applied {
            Some(applied) => {
                view.relayout();
                // Show the user what changed back.
                if let Some(first) = applied.hunks.first() {
                    if view.current_hunk() != Some(*first) {
                        view.goto_hunk(*first);
                    }
                }
                let open = view.session.stats().open;
                let verb = if undo { "Undid" } else { "Redid" };
                let text = format!(
                    "{verb}: {} · {open} unresolved hunk{}",
                    applied.label,
                    plural(open)
                );
                self.notify(Level::Success, text);
            }
            None => {
                let text = if undo {
                    "Nothing to undo"
                } else {
                    "Nothing to redo"
                };
                self.notify(Level::Info, text);
            }
        }
    }

    // ---- editing in $EDITOR ----------------------------------------------

    fn begin_edit(&mut self) -> Effect {
        let file = self.current;
        let extension = std::path::Path::new(self.current_path())
            .extension()
            .map(|e| e.to_string_lossy().into_owned());
        let Some(view) = self.view().filter(|v| v.current_hunk().is_some()) else {
            return Effect::None;
        };
        let (hunk, _) = view.position();
        // An open hunk is edited as a conflict block so both sides are at hand.
        let text = if view.session.status(hunk).is_open() {
            view.session.conflict_block(hunk)
        } else {
            view.session.result_text(hunk)
        };
        Effect::Edit(EditRequest {
            file,
            hunk,
            text,
            extension,
        })
    }

    /// Take over the outcome of an [`Effect::Edit`]; `None` means the editor
    /// could not be run or was aborted.
    pub fn finish_edit(&mut self, request: &EditRequest, edited: Result<String, String>) {
        let edited = match edited {
            Ok(text) => text,
            Err(reason) => {
                self.notify(Level::Error, format!("Edit failed: {reason}"));
                return;
            }
        };
        if edited == request.text {
            self.notify(Level::Info, "Edit left the hunk unchanged");
            return;
        }
        let has_markers = edited.lines().any(|l| {
            ["<<<<<<<", "|||||||", "=======", ">>>>>>>"]
                .iter()
                .any(|m| l.starts_with(m))
        });
        let Some(view) = self
            .files
            .get_mut(request.file)
            .and_then(|f| f.view.as_mut())
        else {
            return;
        };
        match view.session.apply(Op::SetCustom {
            hunk: request.hunk,
            lines: split_lines(&edited),
        }) {
            Ok(_) => {
                view.relayout();
                if has_markers {
                    self.notify(
                        Level::Warn,
                        "Hunk edited, but conflict markers are still in the text",
                    );
                } else {
                    self.notify(Level::Success, "Hunk replaced by your edit");
                }
            }
            Err(err) => self.notify(Level::Warn, capitalize(&err.to_string())),
        }
    }

    // ---- saving ----------------------------------------------------------

    /// Write one file. Hunks that are still open are written as conflict
    /// markers and the file is not staged, so nothing is lost or hidden.
    fn save(&mut self, index: usize) -> bool {
        let Some(slot) = self.files.get(index) else {
            return false;
        };
        let Some(view) = slot.view.as_ref() else {
            return false;
        };
        let path = slot.entry.path.clone();
        let output = view.session.output();
        let revision = view.session.revision();
        let resolved = output.unresolved == 0;
        match self.backend.save(&path, &output.text, resolved) {
            Ok(outcome) => {
                if let Some(view) = self.files[index].view.as_mut() {
                    view.saved_revision = revision;
                    view.last_save = Some(SaveInfo {
                        unresolved: output.unresolved,
                        staged: outcome.staged,
                    });
                }
                if !resolved {
                    let n = output.unresolved;
                    self.notify(
                        Level::Warn,
                        format!(
                            "Saved {path} with {n} unresolved hunk{} as conflict markers — not staged",
                            plural(n)
                        ),
                    );
                } else if self.all_resolved() {
                    let staged = if outcome.staged { " and staged" } else { "" };
                    self.notify(
                        Level::Success,
                        format!(
                            "Saved{staged} {path} · every conflict is resolved — quit and commit"
                        ),
                    );
                } else if outcome.staged {
                    self.notify(Level::Success, format!("Saved and staged {path}"));
                } else {
                    self.notify(Level::Success, format!("Saved {path} (not staged)"));
                }
                true
            }
            Err(err) => {
                self.notify(Level::Error, format!("Could not save {path}: {err}"));
                false
            }
        }
    }

    /// Write every file with unsaved changes; stops at the first failure.
    fn save_all(&mut self) -> bool {
        let dirty: Vec<usize> = (0..self.files.len())
            .filter(|i| self.files[*i].view.as_ref().is_some_and(FileView::is_dirty))
            .collect();
        if dirty.is_empty() {
            self.notify(Level::Info, "Nothing to save");
            return true;
        }
        dirty.into_iter().all(|index| self.save(index))
    }
}

fn describe_save(save: SaveInfo) -> String {
    match (save.unresolved, save.staged) {
        (0, true) => "resolved and staged".to_owned(),
        (0, false) => "resolved, not staged".to_owned(),
        (n, _) => format!("saved with {n} unresolved hunk{}", plural(n)),
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}
