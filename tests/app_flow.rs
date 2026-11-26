//! End-to-end: key events in, rendered screen and repository state out.

mod common;

use std::sync::Arc;

use common::TestRepo;
use mcr::app::{App, Effect, Level, Mode, Pane};
use mcr::config::keymap::parse_sequence;
use mcr::config::{ColorDepth, Config, Rgb, Theme};
use mcr::core::{HunkStatus, LineState, Side};
use mcr::git::repo::GitOptions;
use mcr::git::GitBackend;
use mcr::ui;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    Event, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::style::Color;
use ratatui::Terminal;

fn app_with(repo: &TestRepo, config: Config) -> App {
    let options = GitOptions {
        auto_stage: config.git.auto_stage,
        backup: config.git.backup,
    };
    let backend = Arc::new(GitBackend::discover(repo.path(), options).unwrap());
    let theme = config.initial_theme(None).unwrap();
    let mut app = App::new(backend, config, theme).unwrap();
    app.depth = ColorDepth::TrueColor;
    app
}

/// The Material dark theme with plain Unicode markers: stable to assert on.
fn app(repo: &TestRepo) -> App {
    app_with(
        repo,
        Config::from_toml("[ui]\ntheme = \"dark\"\nicons = \"unicode\"").unwrap(),
    )
}

/// Type whitespace-separated keys, e.g. `"h space g g ctrl-r"`.
fn press(app: &mut App, keys: &str) -> Effect {
    let mut effect = Effect::None;
    for key in parse_sequence(keys).unwrap() {
        effect = app.on_event(Event::Key(KeyEvent::new(key.code, key.mods)));
    }
    effect
}

fn render(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| ui::draw(frame, app)).unwrap();
    terminal
}

fn screen(app: &mut App) -> String {
    let terminal = render(app, 140, 40);
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn message(app: &App) -> (&str, Level) {
    let message = app.message.as_ref().expect("a feedback message");
    (message.text.as_str(), message.level)
}

fn status(app: &App) -> HunkStatus {
    let view = app.view().unwrap();
    view.session.status(view.position().0)
}

fn result(app: &App) -> String {
    app.view().unwrap().session.output().text
}

#[test]
fn resolve_every_file_with_the_keyboard_and_commit() {
    let repo = TestRepo::conflicted();
    let mut app = app(&repo);
    assert_eq!(app.current_path(), "file.txt");
    assert_eq!(
        status(&app),
        HunkStatus::Unresolved,
        "cursor starts on the conflict"
    );

    press(&mut app, "s");
    assert_eq!(status(&app), HunkStatus::Resolved);
    assert_eq!(message(&app).1, Level::Success);
    press(&mut app, "w");
    assert!(
        message(&app).0.contains("Saved and staged file.txt"),
        "{:?}",
        app.message
    );
    assert_eq!(repo.unmerged(), ["other.txt"]);

    press(&mut app, "] f");
    assert_eq!(app.current_path(), "other.txt");
    press(&mut app, "b w");
    assert!(
        message(&app).0.contains("every conflict is resolved"),
        "{:?}",
        app.message
    );
    assert!(app.all_resolved());
    assert!(repo.unmerged().is_empty());
    assert_eq!(repo.read("other.txt"), "x source\nx target\n");
    assert_eq!(press(&mut app, "q"), Effect::Quit);

    repo.git(&["commit", "-q", "--no-edit"]);
    assert_eq!(repo.git(&["status", "--porcelain"]).trim(), "");
}

#[test]
fn lines_from_both_sides_can_be_combined() {
    let repo = TestRepo::conflicted();
    let mut app = app(&repo);
    // Source pane: accept the second source line only.
    press(&mut app, "h j space");
    assert_eq!(app.focus, Pane::Source);
    assert_eq!(status(&app), HunkStatus::Partial);
    // Target pane: accept its (only) line, on the hunk's first row.
    press(&mut app, "l l k space");
    assert_eq!(app.focus, Pane::Target);
    // Back in the result pane, settle the hunk: the first source line is dropped.
    press(&mut app, "h enter");
    assert_eq!(status(&app), HunkStatus::Resolved);
    assert!(result(&app).contains("one\nextra from feature\ntwo from main\nthree\n"));

    let view = app.view().unwrap();
    let hunk = view.position().0;
    let state = |side, index| {
        view.session
            .line_state(hunk, mcr::core::LineRef { side, index })
    };
    assert_eq!(state(Side::Source, 0), Some(LineState::Rejected));
    assert_eq!(state(Side::Source, 1), Some(LineState::Accepted));
    assert_eq!(state(Side::Target, 0), Some(LineState::Accepted));
}

#[test]
fn a_result_line_can_be_removed_from_the_result_pane() {
    let repo = TestRepo::conflicted();
    let mut app = app(&repo);
    press(&mut app, "b");
    assert!(result(&app).contains("two from feature\nextra from feature\ntwo from main\n"));
    press(&mut app, "j x");
    assert!(result(&app).contains("two from feature\ntwo from main\n"));
    press(&mut app, "space");
    assert!(message(&app).0.contains("Focus the source or target pane"));
}

#[test]
fn undo_and_redo_walk_the_history() {
    let repo = TestRepo::conflicted();
    let mut app = app(&repo);
    let untouched = result(&app);
    press(&mut app, "t");
    let took_target = result(&app);
    press(&mut app, "r");
    assert_eq!(result(&app), untouched);

    press(&mut app, "u");
    assert_eq!(result(&app), took_target);
    assert!(message(&app).0.starts_with("Undid: reset hunk"));
    press(&mut app, "u");
    assert_eq!(result(&app), untouched);
    press(&mut app, "u");
    assert_eq!(message(&app), ("Nothing to undo", Level::Info));

    press(&mut app, "ctrl-r");
    assert_eq!(result(&app), took_target);
    press(&mut app, "ctrl-r ctrl-r");
    assert_eq!(result(&app), untouched);
    assert_eq!(message(&app), ("Nothing to redo", Level::Info));
}

#[test]
fn undo_returns_the_cursor_to_the_changed_hunk() {
    let repo = TestRepo::conflicted();
    let mut app = app(&repo);
    let conflict = app.view().unwrap().position().0;
    press(&mut app, "s G");
    assert_ne!(app.view().unwrap().position().0, conflict);
    press(&mut app, "u");
    assert_eq!(app.view().unwrap().position().0, conflict);
    assert_eq!(status(&app), HunkStatus::Unresolved);
}

#[test]
fn saving_with_open_hunks_writes_markers_and_does_not_stage() {
    let repo = TestRepo::conflicted();
    let mut app = app(&repo);
    press(&mut app, "w");
    let (text, level) = message(&app);
    assert_eq!(level, Level::Warn);
    assert!(
        text.contains("1 unresolved hunk as conflict markers — not staged"),
        "{text}"
    );
    assert!(repo.read("file.txt").contains("<<<<<<< main"));
    assert_eq!(repo.unmerged(), ["file.txt", "other.txt"]);
    assert!(!app.all_resolved());
}

#[test]
fn quitting_with_unsaved_changes_asks_first() {
    let repo = TestRepo::conflicted();
    let mut app = app(&repo);
    let on_disk = repo.read("file.txt");
    press(&mut app, "s");
    assert_eq!(press(&mut app, "q"), Effect::None);
    assert_eq!(app.mode, Mode::ConfirmQuit);
    assert!(screen(&mut app).contains("quit without saving"));
    assert_eq!(press(&mut app, "n"), Effect::None);
    assert_eq!(app.mode, Mode::Normal);

    press(&mut app, "q");
    assert_eq!(press(&mut app, "y"), Effect::Quit);
    assert_eq!(
        repo.read("file.txt"),
        on_disk,
        "nothing is written without an explicit save"
    );

    press(&mut app, "q");
    assert_eq!(press(&mut app, "w"), Effect::Quit);
    assert!(repo.read("file.txt").contains("two from feature\n"));
    assert_eq!(repo.unmerged(), ["other.txt"]);
}

#[test]
fn take_hunk_in_the_result_pane_never_empties_an_untouched_conflict() {
    let repo = TestRepo::conflicted();
    let mut app = app(&repo);
    assert_eq!(app.focus, Pane::Result);
    press(&mut app, "enter");
    assert_eq!(status(&app), HunkStatus::Unresolved);
    assert!(message(&app).0.starts_with("Nothing picked yet"));
    // In a side pane it takes that side.
    press(&mut app, "l enter");
    assert!(result(&app).contains("one\ntwo from main\nthree\n"));
}

#[test]
fn bulk_take_and_navigation_between_conflicts() {
    let repo = TestRepo::conflicted();
    let mut app = app(&repo);
    press(&mut app, "g g");
    assert_eq!(app.view().unwrap().cursor, 0);
    assert_eq!(
        status(&app),
        HunkStatus::Auto,
        "the file starts with the target-only `zero`"
    );
    press(&mut app, "] c");
    assert_eq!(status(&app), HunkStatus::Unresolved);
    press(&mut app, "J");
    assert_eq!(
        status(&app),
        HunkStatus::Auto,
        "next change: the source-only `six`"
    );
    press(&mut app, "K");
    assert_eq!(status(&app), HunkStatus::Unresolved);
    press(&mut app, "n");
    assert_eq!(message(&app), ("No unresolved hunk below", Level::Info));
    press(&mut app, "S");
    assert_eq!(app.view().unwrap().session.stats().open, 0);
    assert!(message(&app).0.contains("all hunks resolved"));
}

#[test]
fn manual_edits_replace_the_hunk_and_can_be_undone() {
    let repo = TestRepo::conflicted();
    let mut app = app(&repo);
    let Effect::Edit(request) = press(&mut app, "e") else {
        panic!("expected an edit request")
    };
    assert!(
        request.text.starts_with("<<<<<<< main\n"),
        "open hunks are edited as conflict blocks"
    );
    assert_eq!(request.extension.as_deref(), Some("txt"));

    app.finish_edit(&request, Ok(request.text.clone()));
    assert_eq!(
        status(&app),
        HunkStatus::Unresolved,
        "an unchanged edit is a no-op"
    );

    app.finish_edit(&request, Ok("two, merged by hand\n".to_owned()));
    assert_eq!(status(&app), HunkStatus::Modified);
    assert!(result(&app).contains("one\ntwo, merged by hand\nthree\n"));
    press(&mut app, "h space");
    assert_eq!(
        message(&app).1,
        Level::Warn,
        "picking lines must not discard the edit"
    );
    assert_eq!(status(&app), HunkStatus::Modified);

    let Effect::Edit(again) = press(&mut app, "e") else {
        panic!("expected an edit request")
    };
    assert_eq!(again.text, "two, merged by hand\n");
    app.finish_edit(&again, Err("editor crashed".to_owned()));
    assert_eq!(message(&app).1, Level::Error);
    assert_eq!(status(&app), HunkStatus::Modified);

    press(&mut app, "u");
    assert_eq!(status(&app), HunkStatus::Unresolved);
}

#[test]
fn the_screen_shows_all_three_panes_and_the_resolution_state() {
    let repo = TestRepo::conflicted();
    let mut app = app(&repo);
    let text = screen(&mut app);
    for needle in [
        "SOURCE",
        "feature (theirs)",
        "RESULT",
        "file.txt",
        "TARGET",
        "main (ours)",
        "two from feature",
        "two from main",
        "? unresolved · 3 lines pending",
        "1 of 3 hunks unresolved",
        "file 1/2",
        "Hunk 2/3 · conflict · 3 lines pending",
    ] {
        assert!(text.contains(needle), "missing `{needle}` in:\n{text}");
    }
    press(&mut app, "s");
    let text = screen(&mut app);
    assert!(
        text.contains("all 3 hunks resolved") && text.contains("unsaved"),
        "{text}"
    );
    assert!(text.contains("◀"), "result lines show their origin");
    press(&mut app, "w");
    assert!(screen(&mut app).contains("saved · staged"));
}

#[test]
fn conflict_rows_get_a_translucent_tint_and_themes_switch_at_runtime() {
    let repo = TestRepo::conflicted();
    let mut app = app(&repo);
    let rgb = |c: Rgb| Color::Rgb(c.0, c.1, c.2);
    let conflict_row = |app: &App| {
        let view = app.view().unwrap();
        let area = app.geometry.panes[Pane::Source.index()];
        (area.x + 10, area.y + (view.cursor - view.scroll) as u16)
    };

    let dark = Theme::dark();
    let terminal = render(&mut app, 140, 40);
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(0, 5)].bg, rgb(dark.bg));
    let cell = &buffer[conflict_row(&app)];
    // Cursor row: the conflict tint, slightly lifted — never the raw accent.
    assert_ne!(cell.bg, rgb(dark.bg));
    assert_ne!(cell.bg, rgb(dark.conflict));

    press(&mut app, "ctrl-t");
    assert_eq!(app.theme().name, "light");
    assert_eq!(message(&app).0, "Theme: light");
    let light = Theme::light();
    let terminal = render(&mut app, 140, 40);
    assert_eq!(terminal.backend().buffer()[(0, 5)].bg, rgb(light.bg));
    // The cycle wraps around to the default theme.
    press(&mut app, "ctrl-t");
    assert_eq!(app.theme().name, "tokyonight");

    // 256-color terminals get indexed colors only.
    app.depth = ColorDepth::Ansi256;
    let terminal = render(&mut app, 140, 40);
    let buffer = terminal.backend().buffer();
    let all_indexed = buffer
        .content()
        .iter()
        .all(|c| !matches!(c.bg, Color::Rgb(..)) && !matches!(c.fg, Color::Rgb(..)));
    assert!(all_indexed);
}

#[test]
fn mouse_click_selects_a_line_and_wheel_scrolls() {
    let repo = TestRepo::conflicted();
    let mut app = app(&repo);
    render(&mut app, 140, 12);
    let area = app.geometry.panes[Pane::Target.index()];
    let mouse = |kind, row| {
        Event::Mouse(MouseEvent {
            kind,
            column: area.x + 5,
            row,
            modifiers: KeyModifiers::NONE,
        })
    };
    app.on_event(mouse(MouseEventKind::Down(MouseButton::Left), area.y + 2));
    assert_eq!(app.focus, Pane::Target);
    assert_eq!(app.view().unwrap().cursor, 2);

    app.on_event(mouse(MouseEventKind::ScrollDown, area.y));
    assert!(app.view().unwrap().scroll > 0);
}

#[test]
fn help_and_file_picker_popups() {
    let repo = TestRepo::conflicted();
    let mut app = app(&repo);
    press(&mut app, "?");
    assert!(matches!(app.mode, Mode::Help { .. }));
    let text = screen(&mut app);
    assert!(
        text.contains("Keys") && text.contains("Move down one line"),
        "{text}"
    );
    press(&mut app, "j j esc");
    assert_eq!(app.mode, Mode::Normal);

    press(&mut app, "f");
    let text = screen(&mut app);
    assert!(
        text.contains("Conflicted files") && text.contains("other.txt"),
        "{text}"
    );
    press(&mut app, "j enter");
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.current_path(), "other.txt");
    press(&mut app, "[ f");
    assert_eq!(app.current_path(), "file.txt");
}

#[test]
fn keys_layout_and_behaviour_follow_the_configuration() {
    let repo = TestRepo::conflicted();
    let config = Config::from_toml(
        r##"
        [ui]
        layout = "stacked"
        theme = "light"
        icons = "ascii"
        auto_advance = true
        [git]
        auto_stage = false
        [keys]
        take_source = "ctrl-y"
        save = ["F2"]
        "##,
    )
    .unwrap();
    let mut app = app_with(&repo, config);
    press(&mut app, "s");
    assert_eq!(status(&app), HunkStatus::Unresolved, "`s` was rebound");
    press(&mut app, "ctrl-y");
    assert_eq!(app.view().unwrap().session.stats().open, 0);
    press(&mut app, "w");
    assert!(app.view().unwrap().is_dirty(), "`w` no longer saves");
    press(&mut app, "f2");
    assert!(
        message(&app).0.contains("(not staged)"),
        "{:?}",
        app.message
    );
    assert_eq!(repo.unmerged(), ["file.txt", "other.txt"]);

    // Stacked: the result pane sits below the two branches.
    render(&mut app, 120, 40);
    let [source, result, target] = app.geometry.panes;
    assert_eq!(source.y, target.y);
    assert!(result.y > source.y && result.width > source.width);
    press(&mut app, "v");
    render(&mut app, 120, 40);
    let [source, result, _] = app.geometry.panes;
    assert_eq!(source.y, result.y);
    let before = result.width;
    press(&mut app, "+ +");
    render(&mut app, 120, 40);
    assert!(app.geometry.panes[1].width > before);
    assert!(
        !screen(&mut app).contains('✓'),
        "ascii mode avoids unicode markers"
    );
}

#[test]
fn rendering_never_panics_on_tiny_or_huge_terminals() {
    let repo = TestRepo::conflicted();
    let mut app = app(&repo);
    for (width, height) in [(1, 1), (5, 3), (20, 6), (40, 10), (300, 100)] {
        for keys in ["?", "esc f", "esc s q", "n G g g"] {
            press(&mut app, keys);
            render(&mut app, width, height);
        }
    }
}

#[test]
fn empty_inputs_are_harmless() {
    let dir = tempfile::tempdir().unwrap();
    let path = |name: &str| dir.path().join(name);
    for name in ["base", "local", "remote"] {
        std::fs::write(path(name), "").unwrap();
    }
    let backend = Arc::new(mcr::git::FileBackend::new(
        Some(path("base")),
        path("local"),
        path("remote"),
        path("merged"),
    ));
    let mut app = App::new(backend, Config::default(), 0).unwrap();
    for keys in [
        "j k G g g n N J K",
        "space x enter s t b B d c r S T u ctrl-r h space l l x",
    ] {
        press(&mut app, keys);
        render(&mut app, 80, 24);
    }
    assert_eq!(press(&mut app, "e"), Effect::None);
    press(&mut app, "w");
    assert_eq!(std::fs::read_to_string(path("merged")).unwrap(), "");
    assert!(app.all_resolved());
}

#[test]
fn defaults_are_tokyo_night_with_icons_and_only_the_current_line_stands_out() {
    let repo = TestRepo::conflicted();
    let mut app = app_with(&repo, Config::default());
    let theme = Theme::tokyonight();
    assert_eq!(app.theme(), &theme);
    let rgb = |c: Rgb| Color::Rgb(c.0, c.1, c.2);

    press(&mut app, "g g j");
    let terminal = render(&mut app, 140, 40);
    let buffer = terminal.backend().buffer();
    let area = app.geometry.panes[Pane::Result.index()];
    let bg_at = |row: u16| buffer[(area.x + 12, area.y + row)].bg;
    // Row 1 holds the cursor; rows 4.. are ordinary unchanged lines.
    assert_eq!(
        bg_at(1),
        rgb(theme.cursor),
        "current line has its own background"
    );
    for row in [4, 5, 6] {
        assert_eq!(
            bg_at(row),
            rgb(theme.bg),
            "other plain lines carry no decoration"
        );
    }
    let text = screen(&mut app);
    assert!(
        text.contains('\u{e0a0}') && text.contains('\u{f128}'),
        "nerd font icons are used"
    );
}
