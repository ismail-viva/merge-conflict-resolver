//! Drawing. Everything here is a pure function of [`App`] state; the only
//! thing written back is the frame's geometry (for paging and the mouse).

pub mod info;
pub mod panes;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};
use ratatui::Frame;

use crate::action::Action;
use crate::app::{App, FileView, Geometry, Level, Mode, Pane};
use crate::config::keymap::format_sequence;
use crate::config::{ColorDepth, IconSet, LayoutMode, Rgb, Theme};
use crate::core::{HunkKind, HunkStatus};

/// A theme bound to the terminal's color depth.
pub struct Palette<'a> {
    pub theme: &'a Theme,
    pub depth: ColorDepth,
}

impl Palette<'_> {
    pub fn color(&self, rgb: Rgb) -> Color {
        self.depth.color(rgb)
    }

    pub fn style(&self, fg: Rgb, bg: Rgb) -> Style {
        Style::new().fg(self.color(fg)).bg(self.color(bg))
    }

    fn base(&self) -> Style {
        self.style(self.theme.fg, self.theme.bg)
    }

    fn bar(&self) -> Style {
        self.style(self.theme.fg, self.theme.surface)
    }
}

/// Glyphs for line states and decorations, per [`IconSet`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbols {
    pub accepted: &'static str,
    pub rejected: &'static str,
    pub unresolved: &'static str,
    pub modified: &'static str,
    pub superseded: &'static str,
    pub from_source: &'static str,
    pub from_target: &'static str,
    pub bar: &'static str,
    pub no_eol: &'static str,
    pub arrow: &'static str,
    pub dot: &'static str,
    pub minimap: &'static str,
    /// Rounded ends of "pill" labels; empty when the font has none.
    pub cap_left: &'static str,
    pub cap_right: &'static str,
    // Decorative icons; empty when the icon set has none.
    pub app: &'static str,
    pub branch: &'static str,
    pub merge: &'static str,
    pub file: &'static str,
    pub commit: &'static str,
    pub author: &'static str,
    pub clock: &'static str,
    pub history: &'static str,
    pub keys: &'static str,
    pub info: &'static str,
    pub success: &'static str,
    pub warn: &'static str,
    pub error: &'static str,
    pub save: &'static str,
}

impl Symbols {
    /// Nerd Font icons (Font Awesome, Octicons and Powerline ranges).
    pub const NERD: Symbols = Symbols {
        accepted: "\u{f00c}",
        rejected: "\u{f00d}",
        unresolved: "\u{f128}",
        modified: "\u{f040}",
        superseded: "·",
        from_source: "\u{f0d9}",
        from_target: "\u{f0da}",
        bar: "▎",
        no_eol: "\u{f05e} no newline at end of file",
        arrow: "\u{f061}",
        dot: "\u{f111}",
        minimap: "▐",
        cap_left: "\u{e0b6}",
        cap_right: "\u{e0b4}",
        app: "\u{e727}",
        branch: "\u{e0a0}",
        merge: "\u{f419}",
        file: "\u{f15c}",
        commit: "\u{f417}",
        author: "\u{f007}",
        clock: "\u{f017}",
        history: "\u{f1da}",
        keys: "\u{f11c}",
        info: "\u{f05a}",
        success: "\u{f058}",
        warn: "\u{f071}",
        error: "\u{f057}",
        save: "\u{f0c7}",
    };

    pub const UNICODE: Symbols = Symbols {
        accepted: "✓",
        rejected: "✗",
        unresolved: "?",
        modified: "~",
        superseded: "·",
        from_source: "◀",
        from_target: "▶",
        bar: "▎",
        no_eol: "⊘ no newline at end of file",
        arrow: "→",
        dot: "●",
        minimap: "▐",
        cap_left: "",
        cap_right: "",
        app: "",
        branch: "",
        merge: "",
        file: "",
        commit: "",
        author: "",
        clock: "",
        history: "",
        keys: "",
        info: "",
        success: "✓",
        warn: "!",
        error: "✗",
        save: "",
    };

    pub const ASCII: Symbols = Symbols {
        accepted: "+",
        rejected: "x",
        unresolved: "?",
        modified: "~",
        superseded: ".",
        from_source: "<",
        from_target: ">",
        bar: "|",
        no_eol: "\\ no newline at end of file",
        arrow: "->",
        dot: "*",
        minimap: "|",
        success: "+",
        error: "x",
        ..Symbols::UNICODE
    };

    pub fn for_config(icons: IconSet) -> &'static Symbols {
        match icons {
            IconSet::Nerd => &Symbols::NERD,
            IconSet::Unicode => &Symbols::UNICODE,
            IconSet::Ascii => &Symbols::ASCII,
        }
    }
}

/// `icon` followed by a space, or nothing when the icon set has no glyph.
pub(crate) fn icon(glyph: &str) -> String {
    if glyph.is_empty() {
        String::new()
    } else {
        format!("{glyph} ")
    }
}

pub(crate) fn border_type(icons: IconSet) -> BorderType {
    match icons {
        IconSet::Ascii => BorderType::Plain,
        _ => BorderType::Rounded,
    }
}

/// A rounded label: `text` on a `color` background.
fn pill<'a>(text: String, color: Rgb, on: Rgb, p: &Palette, symbols: &Symbols) -> Vec<Span<'a>> {
    let cap = p.style(color, on);
    vec![
        Span::styled(symbols.cap_left, cap),
        Span::styled(
            text,
            p.style(p.theme.bg, color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(symbols.cap_right, cap),
    ]
}

/// Draw one frame.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let theme = app.theme().clone();
    let palette = Palette {
        theme: &theme,
        depth: app.depth,
    };
    let symbols = Symbols::for_config(app.config.ui.icons);
    frame.buffer_mut().set_style(area, palette.base());

    let info_height = if app.show_info {
        app.config.ui.info_height.max(3)
    } else {
        0
    };
    let [title, body, info, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(info_height),
        Constraint::Length(1),
    ])
    .areas(area);

    let minimap_width = u16::from(app.config.ui.minimap);
    let [body, minimap] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(minimap_width)]).areas(body);
    let outer = pane_areas(app, body);

    // Frame the panes first: their inner areas define this frame's geometry.
    let mut inner = [Rect::default(); 3];
    for pane in Pane::ALL {
        let block = pane_block(app, pane, &palette, symbols);
        inner[pane.index()] = block.inner(outer[pane.index()]);
        frame.render_widget(block, outer[pane.index()]);
    }
    let height = inner.iter().map(|r| r.height).min().unwrap_or(0) as usize;
    app.geometry = Geometry {
        panes: inner,
        height,
    };
    let scrolloff = app.config.ui.scrolloff as usize;
    if let Some(view) = app.files[app.current].view.as_mut() {
        view.ensure_visible(height, scrolloff);
    }

    if let Some(view) = app.view() {
        for pane in Pane::ALL {
            let widget = panes::PaneWidget {
                view,
                pane,
                focused: app.focus == pane,
                hover: app.hover.filter(|(p, _)| *p == pane).map(|(_, row)| row),
                palette: &palette,
                symbols,
                line_numbers: app.config.ui.line_numbers,
                tab_width: app.config.ui.tab_width as usize,
            };
            frame.render_widget(widget, inner[pane.index()]);
        }
        if minimap.width > 0 {
            draw_minimap(frame.buffer_mut(), minimap, view, &palette, symbols);
        }
    }

    draw_title(frame, title, app, &palette, symbols);
    if app.show_info {
        info::draw(frame, info, app, &palette, symbols);
    }
    draw_status(frame, status, app, &palette, symbols);

    match app.mode {
        Mode::Normal => {}
        Mode::Help { scroll } => draw_help(frame, area, app, &palette, symbols, scroll),
        Mode::Files { selected } => draw_files(frame, area, app, &palette, symbols, selected),
        Mode::ConfirmQuit => draw_confirm(frame, area, app, &palette, symbols),
    }
}

/// Outer rectangles of the source, result and target panes.
fn pane_areas(app: &App, body: Rect) -> [Rect; 3] {
    let [s, r, t] = app.ratio;
    match app.layout {
        LayoutMode::Columns => Layout::horizontal([
            Constraint::Fill(s),
            Constraint::Fill(r),
            Constraint::Fill(t),
        ])
        .areas(body),
        LayoutMode::Stacked => {
            let percent = app.config.ui.stacked_result_percent.clamp(10, 90);
            let [top, bottom] = Layout::vertical([
                Constraint::Percentage(100 - percent),
                Constraint::Percentage(percent),
            ])
            .areas(body);
            let [source, target] =
                Layout::horizontal([Constraint::Fill(s), Constraint::Fill(t)]).areas(top);
            [source, bottom, target]
        }
    }
}

/// Name, detail text and identity color of a pane.
fn pane_identity(app: &App, pane: Pane, theme: &Theme) -> (&'static str, String, Rgb) {
    match pane {
        Pane::Source => (
            "SOURCE",
            format!("{} (theirs)", app.context.source_label),
            theme.source,
        ),
        Pane::Result => ("RESULT", app.current_path().to_owned(), theme.accent),
        Pane::Target => (
            "TARGET",
            format!("{} (ours)", app.context.target_label),
            theme.target,
        ),
    }
}

fn pane_block<'a>(app: &App, pane: Pane, p: &Palette, symbols: &Symbols) -> Block<'a> {
    let theme = p.theme;
    let focused = app.focus == pane;
    let (name, detail, color) = pane_identity(app, pane, theme);
    let glyph = match pane {
        Pane::Result => symbols.merge,
        _ => symbols.branch,
    };
    let label = format!(" {}{name} ", icon(glyph));
    let mut title = vec![Span::raw(" ")];
    if focused {
        title.extend(pill(label, color, theme.bg, p, symbols));
    } else {
        title.push(Span::styled(
            label,
            p.style(color, theme.bg).add_modifier(Modifier::BOLD),
        ));
    }
    let detail_color = if focused { theme.fg } else { theme.muted };
    title.push(Span::styled(
        format!(" {detail} "),
        p.style(detail_color, theme.bg),
    ));
    Block::bordered()
        .border_type(border_type(app.config.ui.icons))
        .border_style(p.style(if focused { color } else { theme.border }, theme.bg))
        .style(p.base())
        .title(Line::from(title))
}

/// Overview of the whole file: where the changes and open conflicts are.
fn draw_minimap(buf: &mut Buffer, area: Rect, view: &FileView, p: &Palette, symbols: &Symbols) {
    let theme = p.theme;
    let total = view.rows.total().max(1);
    // Skip the rows taken by the pane borders.
    let track = Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(2),
        ..area
    };
    let cells = track.height as usize;
    if cells == 0 {
        return;
    }
    // Scale the file onto the track; a file shorter than the track maps 1:1.
    let cells = cells.min(total);
    let cell_of = |row: usize| (row * cells / total).min(cells - 1);
    let mut marks: Vec<Option<(u8, Rgb)>> = vec![None; cells];
    for (hunk, h) in view.session.hunks().iter().enumerate() {
        if h.kind == HunkKind::Unchanged {
            continue;
        }
        let mark = match view.session.status(hunk) {
            HunkStatus::Unresolved | HunkStatus::Partial => (3, theme.conflict),
            HunkStatus::Modified => (2, theme.modified),
            HunkStatus::Resolved => (1, theme.accepted),
            HunkStatus::Auto | HunkStatus::Unchanged => (0, theme.muted),
        };
        let start = view.rows.start(hunk);
        let (first, last) = (cell_of(start), cell_of(start + view.rows.height(hunk) - 1));
        for slot in &mut marks[first..=last] {
            if slot.is_none_or(|(priority, _)| priority < mark.0) {
                *slot = Some(mark);
            }
        }
    }
    let viewport =
        cell_of(view.scroll)..=cell_of((view.scroll + track.height as usize).saturating_sub(1));
    for (cell, mark) in marks.iter().enumerate() {
        let bg = if viewport.contains(&cell) {
            theme.cursor
        } else {
            theme.bg
        };
        let (symbol, fg) = match mark {
            Some((_, color)) => (symbols.minimap, *color),
            None => (" ", theme.muted),
        };
        buf.set_string(area.x, track.y + cell as u16, symbol, p.style(fg, bg));
    }
}

fn draw_title(frame: &mut Frame, area: Rect, app: &App, p: &Palette, symbols: &Symbols) {
    let theme = p.theme;
    let bar = p.bar();
    let on_bar = |color: Rgb| p.style(color, theme.surface);
    let bold = Modifier::BOLD;

    let mut left = pill(
        format!(" {}mcr ", icon(symbols.app)),
        theme.accent,
        theme.surface,
        p,
        symbols,
    );
    left.extend([
        Span::styled(format!("  {} ", app.context.operation), on_bar(theme.muted)),
        Span::styled(
            format!("{}{}", icon(symbols.branch), app.context.source_label),
            on_bar(theme.source).add_modifier(bold),
        ),
        Span::styled(format!(" {} ", symbols.arrow), on_bar(theme.muted)),
        Span::styled(
            format!("{}{}", icon(symbols.branch), app.context.target_label),
            on_bar(theme.target).add_modifier(bold),
        ),
        Span::styled(
            format!("   {}{}", icon(symbols.file), app.current_path()),
            on_bar(theme.fg),
        ),
    ]);
    if app.files.len() > 1 {
        left.push(Span::styled(
            format!("  file {}/{}", app.current + 1, app.files.len()),
            on_bar(theme.muted),
        ));
    }

    let mut right = Vec::new();
    if let Some(view) = app.view() {
        let (text, color) = match (view.is_dirty(), view.last_save) {
            (true, _) => (format!("{} unsaved", symbols.dot), theme.unresolved),
            (false, Some(save)) if save.staged => (
                format!("{}saved · staged", icon(symbols.save)),
                theme.accepted,
            ),
            (false, Some(_)) => (format!("{}saved", icon(symbols.save)), theme.muted),
            (false, None) => (String::new(), theme.muted),
        };
        if !text.is_empty() {
            right.push(Span::styled(format!("{text}  "), on_bar(color)));
        }
        let stats = view.session.stats();
        let (text, color) = if stats.open == 0 {
            (
                format!(
                    " {} all {} hunks resolved ",
                    symbols.accepted, stats.changes
                ),
                theme.accepted,
            )
        } else {
            (
                format!(
                    " {} {} of {} hunks unresolved ",
                    symbols.unresolved, stats.open, stats.changes
                ),
                theme.unresolved,
            )
        };
        right.extend(pill(text, color, theme.surface, p, symbols));
    }
    right.push(Span::styled(
        format!("  {} ", theme.name),
        on_bar(theme.muted),
    ));

    let right = Line::from(right);
    let right_width = (right.width() as u16).min(area.width);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width)]).areas(area);
    frame.buffer_mut().set_style(area, bar);
    frame.render_widget(Paragraph::new(Line::from(left)).style(bar), left_area);
    frame.render_widget(Paragraph::new(right).style(bar), right_area);
}

/// Bottom line: the focused pane, then the latest feedback or key hints.
fn draw_status(frame: &mut Frame, area: Rect, app: &App, p: &Palette, symbols: &Symbols) {
    let theme = p.theme;
    let bar = p.bar();
    frame.buffer_mut().set_style(area, bar);

    let mut right = Vec::new();
    if !app.pending.is_empty() {
        right.push(Span::styled(
            format!("{} …  ", format_sequence(&app.pending)),
            p.style(theme.accent, theme.surface)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(view) = app.view() {
        let total = view.rows.total().max(1);
        let percent = (view.cursor + 1) * 100 / total;
        right.push(Span::styled(
            format!("{percent:>3}%  "),
            p.style(theme.muted, theme.surface),
        ));
        right.extend(pill(
            format!(" {}:{} ", view.cursor + 1, total),
            theme.accent,
            theme.surface,
            p,
            symbols,
        ));
    }
    let right = Line::from(right);
    let right_width = (right.width() as u16).min(area.width);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width)]).areas(area);
    frame.render_widget(Paragraph::new(right).style(bar), right_area);

    let (name, _, color) = pane_identity(app, app.focus, theme);
    let mut left = pill(format!(" {name} "), color, theme.surface, p, symbols);
    left.push(Span::styled(" ", bar));
    match &app.message {
        Some(message) => {
            let (glyph, color) = match message.level {
                Level::Info => (symbols.info, theme.fg),
                Level::Success => (symbols.success, theme.accepted),
                Level::Warn => (symbols.warn, theme.unresolved),
                Level::Error => (symbols.error, theme.rejected),
            };
            let mut style = p.style(color, theme.surface);
            if message.level != Level::Info {
                style = style.add_modifier(Modifier::BOLD);
            }
            left.push(Span::styled(
                format!("{}{}", icon(glyph), message.text),
                style,
            ));
        }
        None => left.extend(hints(app, p)),
    }
    frame.render_widget(Paragraph::new(Line::from(left)).style(bar), left_area);
}

fn hints<'a>(app: &App, p: &Palette) -> Vec<Span<'a>> {
    let theme = p.theme;
    const HINTS: &[(Action, &str)] = &[
        (Action::ToggleAcceptLine, "accept"),
        (Action::ToggleRejectLine, "reject"),
        (Action::TakeHunk, "take hunk"),
        (Action::TakeBoth, "both"),
        (Action::NextConflict, "next"),
        (Action::Undo, "undo"),
        (Action::Save, "save"),
        (Action::Help, "help"),
        (Action::Quit, "quit"),
    ];
    let mut spans = Vec::new();
    for (action, label) in HINTS {
        let Some(key) = app.config.keymap.hint(*action) else {
            continue;
        };
        spans.push(Span::styled(
            key,
            p.style(theme.accent, theme.surface)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {label}   "),
            p.style(theme.muted, theme.surface),
        ));
    }
    spans
}

/// A centered popup of at most `width` × `height` cells.
fn popup(
    frame: &mut Frame,
    area: Rect,
    width: u16,
    height: u16,
    title: String,
    app: &App,
    p: &Palette,
) -> Rect {
    let width = width.min(area.width.saturating_sub(2)).max(1);
    let height = height.min(area.height.saturating_sub(2)).max(1);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    let theme = p.theme;
    let block = Block::bordered()
        .border_type(border_type(app.config.ui.icons))
        .border_style(p.style(theme.accent, theme.surface))
        .style(p.bar())
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            format!(" {title} "),
            p.style(theme.accent, theme.surface)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    inner
}

fn draw_help(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    p: &Palette,
    symbols: &Symbols,
    scroll: usize,
) {
    let theme = p.theme;
    let key_style = p
        .style(theme.accent, theme.surface)
        .add_modifier(Modifier::BOLD);
    let heading = p
        .style(theme.target, theme.surface)
        .add_modifier(Modifier::BOLD);
    let mut lines: Vec<Line> = Vec::new();
    let mut group = "";
    for action in Action::ALL {
        if action.group() != group {
            group = action.group();
            if !lines.is_empty() {
                lines.push(Line::default());
            }
            lines.push(Line::styled(group, heading));
        }
        let keys = app.config.keymap.keys_for(*action);
        let keys = if keys.is_empty() {
            "(unbound)".to_owned()
        } else {
            keys.join(", ")
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {keys:<18} "), key_style),
            Span::styled(action.description(), p.style(theme.fg, theme.surface)),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::styled("Markers", heading));
    for (symbol, color, meaning) in [
        (
            symbols.accepted,
            theme.accepted,
            "accepted — the line is part of the result",
        ),
        (
            symbols.rejected,
            theme.rejected,
            "rejected — the line is left out",
        ),
        (
            symbols.unresolved,
            theme.unresolved,
            "unresolved — still needs a decision",
        ),
        (
            symbols.modified,
            theme.modified,
            "modified — edited by hand",
        ),
        (
            symbols.from_source,
            theme.source,
            "result line taken from the source branch",
        ),
        (
            symbols.from_target,
            theme.target,
            "result line taken from the target branch",
        ),
    ] {
        lines.push(Line::from(vec![
            Span::styled(format!("  {symbol:<18} "), p.style(color, theme.surface)),
            Span::styled(meaning, p.style(theme.fg, theme.surface)),
        ]));
    }

    let title = format!("{}Keys", icon(symbols.keys));
    let inner = popup(frame, area, 88, lines.len() as u16 + 2, title, app, p);
    let max_scroll = lines.len().saturating_sub(inner.height as usize);
    let scroll = scroll.min(max_scroll) as u16;
    frame.render_widget(
        Paragraph::new(lines).style(p.bar()).scroll((scroll, 0)),
        inner,
    );
}

fn draw_files(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    p: &Palette,
    symbols: &Symbols,
    selected: usize,
) {
    let theme = p.theme;
    let lines: Vec<Line> = app
        .files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            let (glyph, color, note) = match (&file.entry.unsupported, &file.view) {
                (Some(reason), _) => (symbols.rejected, theme.muted, reason.clone()),
                (None, None) => (
                    symbols.unresolved,
                    theme.unresolved,
                    "not opened yet".to_owned(),
                ),
                (None, Some(view)) => {
                    let open = view.session.stats().open;
                    let saved = !view.is_dirty() && view.last_save.is_some();
                    match (open, saved) {
                        (0, true) => (symbols.accepted, theme.accepted, "resolved · saved".into()),
                        (0, false) => (
                            symbols.modified,
                            theme.modified,
                            "resolved · not saved".into(),
                        ),
                        (n, _) => (
                            symbols.unresolved,
                            theme.unresolved,
                            format!("{n} unresolved"),
                        ),
                    }
                }
            };
            let bg = if index == selected {
                theme.cursor
            } else {
                theme.surface
            };
            let marker = if index == app.current {
                symbols.bar
            } else {
                " "
            };
            Line::from(vec![
                Span::styled(format!("{marker} "), p.style(theme.accent, bg)),
                Span::styled(format!("{glyph} "), p.style(color, bg)),
                Span::styled(
                    format!("{}{}", icon(symbols.file), file.entry.path),
                    p.style(theme.fg, bg),
                ),
                Span::styled(format!("   {note} "), p.style(theme.muted, bg)),
            ])
            .style(Style::new().bg(p.color(bg)))
        })
        .collect();
    let title = format!("{}Conflicted files", icon(symbols.file));
    let inner = popup(frame, area, 84, lines.len() as u16 + 2, title, app, p);
    let scroll = (selected + 1).saturating_sub(inner.height as usize) as u16;
    frame.render_widget(
        Paragraph::new(lines).style(p.bar()).scroll((scroll, 0)),
        inner,
    );
}

fn draw_confirm(frame: &mut Frame, area: Rect, app: &App, p: &Palette, symbols: &Symbols) {
    let theme = p.theme;
    let dirty = app
        .files
        .iter()
        .filter(|f| f.view.as_ref().is_some_and(FileView::is_dirty))
        .count();
    let key = |k: &'static str| {
        Span::styled(
            k,
            p.style(theme.accent, theme.surface)
                .add_modifier(Modifier::BOLD),
        )
    };
    let text = |t: &'static str| Span::styled(t, p.style(theme.fg, theme.surface));
    let lines = vec![
        Line::styled(
            format!(
                "{dirty} file{} with unsaved changes.",
                if dirty == 1 { "" } else { "s" }
            ),
            p.style(theme.unresolved, theme.surface)
                .add_modifier(Modifier::BOLD),
        ),
        Line::default(),
        Line::from(vec![key("w"), text("  save all and quit")]),
        Line::from(vec![key("y"), text("  quit without saving")]),
        Line::from(vec![key("n"), text("  keep working")]),
    ];
    let title = format!("{}Quit?", icon(symbols.warn));
    let inner = popup(frame, area, 46, 7, title, app, p);
    frame.render_widget(Paragraph::new(lines).style(p.bar()), inner);
}
