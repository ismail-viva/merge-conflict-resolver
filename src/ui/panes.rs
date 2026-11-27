//! The three synchronized panes.
//!
//! [`describe`] turns one grid row of one pane into a plain description
//! (text, marker, colors); [`PaneWidget`] paints those descriptions. Keeping
//! the two apart makes the visual language unit-testable.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthChar;

use super::{Palette, Symbols};
use crate::app::{FileView, Pane};
use crate::config::{Rgb, Theme};
use crate::core::text::{has_eol, strip_eol};
use crate::core::{Hunk, HunkKind, HunkStatus, LineRef, LineState, Origin, Side};
use crate::syntax::{Span, Token};

/// How one row of one pane looks.
#[derive(Debug, Clone, PartialEq)]
pub struct RowView<'a> {
    pub marker: Option<(&'static str, Rgb)>,
    /// 1-based line number in the pane's file.
    pub number: Option<usize>,
    pub text: Option<&'a str>,
    /// Shown instead of text on placeholder rows.
    pub label: Option<String>,
    pub bg: Rgb,
    pub fg: Rgb,
    pub strike: bool,
}

/// The diff color of a side's lines in `hunk`, if that side changed.
fn side_accent(hunk: &Hunk, side: Side, theme: &Theme) -> Option<Rgb> {
    let changed = |own_len: usize| {
        if hunk.base.is_empty() {
            theme.added
        } else if own_len == 0 {
            theme.removed
        } else {
            theme.modified
        }
    };
    match (hunk.kind, side) {
        (HunkKind::Unchanged, _) => None,
        (HunkKind::Conflict, _) => Some(theme.conflict),
        (HunkKind::BothSame, Side::Source) | (HunkKind::SourceOnly, Side::Source) => {
            Some(changed(hunk.source.len()))
        }
        (HunkKind::BothSame, Side::Target) | (HunkKind::TargetOnly, Side::Target) => {
            Some(changed(hunk.target.len()))
        }
        (HunkKind::SourceOnly, Side::Target) | (HunkKind::TargetOnly, Side::Source) => None,
    }
}

pub fn describe<'a>(
    view: &'a FileView,
    theme: &Theme,
    symbols: &Symbols,
    pane: Pane,
    hunk: usize,
    offset: usize,
) -> RowView<'a> {
    let session = &view.session;
    let h = session.hunk(hunk);
    let status = session.status(hunk);
    let plain = RowView {
        marker: None,
        number: None,
        text: None,
        label: None,
        bg: theme.bg,
        fg: theme.fg,
        strike: false,
    };
    let filler = |accent: Option<Rgb>| RowView {
        bg: match accent {
            Some(accent) => theme.bg.blend(accent, theme.tint * 0.5),
            None => theme.bg,
        },
        ..plain.clone()
    };

    let Some(side) = pane.side() else {
        // Result pane.
        return match session.result_line(hunk, offset) {
            Some(line) => {
                let number = Some(view.rows.result_start(hunk) + offset + 1);
                let row = RowView {
                    number,
                    text: Some(line.text),
                    ..plain
                };
                match line.origin {
                    Origin::Unchanged => row,
                    Origin::Line(LineRef { side, .. }) => {
                        let (symbol, color) = match side {
                            Side::Source => (symbols.from_source, theme.source),
                            Side::Target => (symbols.from_target, theme.target),
                        };
                        RowView {
                            marker: Some((symbol, color)),
                            bg: theme.tinted(color),
                            ..row
                        }
                    }
                    Origin::Custom => RowView {
                        marker: Some((symbols.modified, theme.modified)),
                        bg: theme.tinted(theme.modified),
                        ..row
                    },
                }
            }
            None if status.is_open() => {
                let pending = session.pending(hunk);
                let label = (offset == session.result_len(hunk)).then(|| {
                    format!(
                        "{} unresolved · {pending} line{} pending",
                        symbols.unresolved,
                        if pending == 1 { "" } else { "s" }
                    )
                });
                RowView {
                    label,
                    bg: theme.tinted(theme.conflict),
                    fg: theme.unresolved,
                    ..plain
                }
            }
            None => filler(None),
        };
    };

    let accent = side_accent(h, side, theme);
    let range = match side {
        Side::Source => &h.source,
        Side::Target => &h.target,
    };
    let Some(text) = session.side_lines(hunk, side).get(offset) else {
        return filler(accent);
    };
    let row = RowView {
        number: Some(range.start + offset + 1),
        text: Some(text),
        ..plain
    };
    if h.kind == HunkKind::Unchanged {
        return row;
    }
    let row = RowView {
        bg: accent.map_or(theme.bg, |a| theme.tinted(a)),
        ..row
    };
    if status == HunkStatus::Modified {
        // Superseded by a manual edit: the per-line decisions no longer apply.
        return RowView {
            marker: Some((symbols.superseded, theme.muted)),
            fg: theme.muted,
            ..row
        };
    }
    match session.line_state(
        hunk,
        LineRef {
            side,
            index: offset,
        },
    ) {
        Some(LineState::Accepted) => RowView {
            marker: Some((symbols.accepted, theme.accepted)),
            ..row
        },
        Some(LineState::Rejected) => RowView {
            marker: Some((symbols.rejected, theme.rejected)),
            fg: theme.muted,
            strike: true,
            ..row
        },
        Some(LineState::Unresolved) => RowView {
            marker: Some((symbols.unresolved, theme.unresolved)),
            ..row
        },
        None => row,
    }
}

/// Make `text` displayable: expand tabs, neutralize control characters,
/// then cut the window `[skip, skip + width)` out of it by display column.
pub fn fit(text: &str, skip: usize, width: usize, tab_width: usize) -> String {
    fit_tokens(text, &[], skip, width, tab_width)
        .into_iter()
        .map(|(piece, _)| piece)
        .collect()
}

/// [`fit`], keeping track of which syntax token every piece belongs to.
/// `spans` are byte ranges of `text`, in order.
pub fn fit_tokens(
    text: &str,
    spans: &[Span],
    skip: usize,
    width: usize,
    tab_width: usize,
) -> Vec<(String, Option<Token>)> {
    let mut out: Vec<(String, Option<Token>)> = Vec::new();
    let mut column = 0usize;
    let end = skip + width;
    let mut next_span = 0usize;
    for (byte, c) in text.char_indices() {
        if column >= end {
            break;
        }
        while spans.get(next_span).is_some_and(|s| s.end as usize <= byte) {
            next_span += 1;
        }
        let token = spans
            .get(next_span)
            .filter(|s| s.start as usize <= byte)
            .map(|s| s.token);
        let mut push = |c: char, column: &mut usize| {
            let w = c.width().unwrap_or(0);
            let shown = if *column >= skip && *column + w <= end {
                Some(c)
            } else if *column < end && *column + w > skip && w > 1 {
                // A wide character cut by the window edge.
                Some(' ')
            } else {
                None
            };
            if let Some(shown) = shown {
                match out.last_mut() {
                    Some((piece, last)) if *last == token => piece.push(shown),
                    _ => out.push((shown.to_string(), token)),
                }
            }
            *column += w;
        };
        match c {
            '\t' => {
                let stop = (column / tab_width + 1) * tab_width;
                while column < stop {
                    push(' ', &mut column);
                }
            }
            c if c.is_control() => push('\u{fffd}', &mut column),
            c => push(c, &mut column),
        }
    }
    out
}

pub struct PaneWidget<'a> {
    pub view: &'a FileView,
    pub pane: Pane,
    pub focused: bool,
    pub palette: &'a Palette<'a>,
    pub symbols: &'a Symbols,
    pub line_numbers: bool,
    pub tab_width: usize,
    /// Set comments and control-flow keywords in italics.
    pub italic: bool,
}

impl PaneWidget<'_> {
    fn number_width(&self) -> usize {
        let lines = match self.pane.side() {
            Some(side) => self.view.session.side_len(side),
            None => self.view.rows.total(),
        };
        lines.max(1).to_string().len().max(3)
    }
}

impl Widget for PaneWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let theme = self.palette.theme;
        let p = self.palette;
        let view = self.view;
        let current_hunk = view.current_hunk();
        let number_width = self.number_width();

        for dy in 0..area.height {
            let y = area.y + dy;
            let row = view.scroll + dy as usize;
            let line_area = Rect {
                y,
                height: 1,
                ..area
            };
            let Some((hunk, offset)) = view.rows.locate(row) else {
                buf.set_style(line_area, p.style(theme.muted, theme.bg));
                continue;
            };
            let cell = describe(view, theme, self.symbols, self.pane, hunk, offset);

            // Only the current line is set apart: a distinct background on
            // plain rows, a lift of the tint on diff rows.
            let tinted = cell.bg != theme.bg;
            let mut bg = cell.bg;
            if row == view.cursor {
                bg = match (tinted, self.focused) {
                    (false, true) => theme.cursor,
                    (false, false) => theme.bg.blend(theme.cursor, 0.6),
                    (true, focused) => cell.bg.blend(theme.fg, if focused { 0.14 } else { 0.07 }),
                };
            }
            buf.set_style(line_area, p.style(cell.fg, bg));

            let mut x = area.x;
            let right = area.right();
            let put = |buf: &mut Buffer, x: &mut u16, text: &str, style: Style| {
                if *x < right {
                    let (next, _) = buf.set_stringn(*x, y, text, (right - *x) as usize, style);
                    *x = next;
                }
            };

            // Bar marking the hunk under the cursor.
            let in_current =
                current_hunk == Some(hunk) && view.session.hunk(hunk).kind != HunkKind::Unchanged;
            let bar_color = if self.focused {
                theme.accent
            } else {
                theme.muted
            };
            let bar = if in_current { self.symbols.bar } else { " " };
            put(buf, &mut x, bar, p.style(bar_color, bg));

            let (marker, marker_color) = cell.marker.unwrap_or((" ", theme.muted));
            put(
                buf,
                &mut x,
                marker,
                p.style(marker_color, bg).add_modifier(Modifier::BOLD),
            );
            put(buf, &mut x, " ", p.style(cell.fg, bg));

            if self.line_numbers {
                let number = match cell.number {
                    Some(n) => format!("{n:>number_width$} "),
                    None => " ".repeat(number_width + 1),
                };
                let style = if row == view.cursor && self.focused {
                    p.style(theme.accent, bg).add_modifier(Modifier::BOLD)
                } else {
                    // The dim gutter color would vanish on a tinted row.
                    let color = match (row == view.cursor, tinted) {
                        (true, _) => theme.fg,
                        (false, true) => theme.muted,
                        (false, false) => theme.gutter,
                    };
                    p.style(color, bg)
                };
                put(buf, &mut x, &number, style);
            }

            let width = right.saturating_sub(x) as usize;
            if let Some(label) = &cell.label {
                put(
                    buf,
                    &mut x,
                    label,
                    p.style(cell.fg, bg).add_modifier(Modifier::ITALIC),
                );
            } else if let Some(text) = cell.text {
                let mut style = p.style(cell.fg, bg);
                if cell.strike {
                    style = style.add_modifier(Modifier::CROSSED_OUT);
                }
                // Syntax colors apply to live text only: rejected and
                // superseded lines stay uniformly dimmed.
                let spans = if cell.strike || cell.fg != theme.fg {
                    &[]
                } else {
                    view.syntax_at(self.pane, hunk, offset)
                };
                let line = strip_eol(text);
                for (piece, token) in fit_tokens(line, spans, view.hscroll, width, self.tab_width) {
                    let style = match token {
                        Some(token) => {
                            let colored = p.style(theme.syntax_color(token, tinted), bg);
                            if self.italic && token.is_italic() {
                                colored.add_modifier(Modifier::ITALIC)
                            } else {
                                colored
                            }
                        }
                        None => style,
                    };
                    put(buf, &mut x, &piece, style);
                }
                if !has_eol(text) {
                    put(buf, &mut x, " ", p.style(theme.muted, bg));
                    put(buf, &mut x, self.symbols.no_eol, p.style(theme.muted, bg));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{MergeInput, MergeSession, Op};

    fn view() -> FileView {
        FileView::new(MergeSession::new(MergeInput {
            base: "a\nb\nc\n".into(),
            source: "a\nsrc-1\nsrc-2\nc\nnew\n".into(),
            target: "a\ntgt\nc\n".into(),
            ..MergeInput::default()
        }))
    }

    fn row(view: &FileView, pane: Pane, hunk: usize, offset: usize) -> RowView<'_> {
        describe(view, &Theme::dark(), &Symbols::UNICODE, pane, hunk, offset)
    }

    #[test]
    fn fit_expands_tabs_and_windows_by_column() {
        assert_eq!(fit("\tx", 0, 10, 4), "    x");
        assert_eq!(fit("ab\tc", 0, 10, 4), "ab  c");
        assert_eq!(fit("abcdef", 2, 3, 4), "cde");
        assert_eq!(fit("a\u{1b}b", 0, 10, 4), "a\u{fffd}b");
        assert_eq!(fit("日本語", 0, 4, 4), "日本");
        assert_eq!(fit("日本語", 1, 4, 4), " 本 ");
        assert_eq!(fit("abc", 10, 5, 4), "");
    }

    #[test]
    fn unresolved_conflict_rows_are_marked_on_all_panes() {
        let v = view();
        let theme = Theme::dark();
        let source = row(&v, Pane::Source, 1, 0);
        assert_eq!(
            source.marker,
            Some((Symbols::UNICODE.unresolved, theme.unresolved))
        );
        assert_eq!(source.bg, theme.tinted(theme.conflict));
        assert_eq!(source.text, Some("src-1\n"));
        assert_eq!(source.number, Some(2));

        let result = row(&v, Pane::Result, 1, 0);
        assert!(result.label.as_deref().unwrap().contains("3 lines pending"));
        assert_eq!(result.bg, theme.tinted(theme.conflict));
        assert_eq!(row(&v, Pane::Result, 1, 1).label, None);

        // The target side is one line shorter: padded with a tinted filler.
        let pad = row(&v, Pane::Target, 1, 1);
        assert_eq!(pad.text, None);
        assert_ne!(pad.bg, theme.bg);
    }

    #[test]
    fn accepted_rejected_and_result_rows_differ() {
        let mut v = view();
        v.session
            .apply(Op::TakeSide {
                hunk: 1,
                side: Side::Source,
            })
            .unwrap();
        v.relayout();
        let theme = Theme::dark();

        let accepted = row(&v, Pane::Source, 1, 0);
        assert_eq!(
            accepted.marker,
            Some((Symbols::UNICODE.accepted, theme.accepted))
        );
        assert!(!accepted.strike);

        let rejected = row(&v, Pane::Target, 1, 0);
        assert_eq!(
            rejected.marker,
            Some((Symbols::UNICODE.rejected, theme.rejected))
        );
        assert!(rejected.strike);
        assert_eq!(rejected.fg, theme.muted);

        let result = row(&v, Pane::Result, 1, 1);
        assert_eq!(
            result.marker,
            Some((Symbols::UNICODE.from_source, theme.source))
        );
        assert_eq!(result.text, Some("src-2\n"));
        assert_eq!(result.number, Some(3));
    }

    #[test]
    fn manual_edits_are_shown_as_modified() {
        let mut v = view();
        v.session
            .apply(Op::SetCustom {
                hunk: 1,
                lines: vec!["mine\n".into()],
            })
            .unwrap();
        v.relayout();
        let theme = Theme::dark();
        let result = row(&v, Pane::Result, 1, 0);
        assert_eq!(
            result.marker,
            Some((Symbols::UNICODE.modified, theme.modified))
        );
        assert_eq!(result.bg, theme.tinted(theme.modified));
        assert_eq!(row(&v, Pane::Source, 1, 0).fg, theme.muted);
    }

    #[test]
    fn one_sided_changes_use_diff_colors_on_the_changed_side_only() {
        let v = view();
        let theme = Theme::dark();
        let hunk = v
            .session
            .hunks()
            .iter()
            .position(|h| h.kind == HunkKind::SourceOnly)
            .unwrap();
        let added = row(&v, Pane::Source, hunk, 0);
        assert_eq!(added.bg, theme.tinted(theme.added));
        assert_eq!(
            added.marker,
            Some((Symbols::UNICODE.accepted, theme.accepted))
        );
        let result = row(&v, Pane::Result, hunk, 0);
        assert_eq!(result.text, Some("new\n"));
        assert_eq!(row(&v, Pane::Target, hunk, 0).text, None);
    }

    #[test]
    fn unchanged_rows_are_plain() {
        let v = view();
        let theme = Theme::dark();
        for pane in Pane::ALL {
            let r = row(&v, pane, 0, 0);
            assert_eq!((r.marker, r.bg, r.fg), (None, theme.bg, theme.fg));
            assert_eq!(r.text, Some("a\n"));
        }
    }
}
