//! The history panel: resolution status of the hunk under the cursor and the
//! git history (author, date, commit) of the selected or hovered line.

use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use super::{Palette, Symbols};
use crate::app::{App, FileView, LineHistory, LineInfo};
use crate::config::Rgb;
use crate::core::{HunkKind, HunkStatus, LineState};
use crate::git::blame::relative_time;

/// "Hunk 2/5 · conflict · 3 lines pending" for the hunk under the cursor.
pub fn hunk_summary(view: &FileView) -> (String, HunkStatus) {
    let (hunk, _) = view.position();
    let session = &view.session;
    if session.hunks().is_empty() {
        return ("Empty file".to_owned(), HunkStatus::Unchanged);
    }
    let status = session.status(hunk);
    let kind = session.hunk(hunk).kind;
    if kind == HunkKind::Unchanged && status == HunkStatus::Unchanged {
        return ("Unchanged in both branches".to_owned(), status);
    }
    let changes = session
        .hunks()
        .iter()
        .filter(|h| h.kind != HunkKind::Unchanged)
        .count();
    let ordinal = session.hunks()[..=hunk]
        .iter()
        .filter(|h| h.kind != HunkKind::Unchanged)
        .count();
    let kind_text = match kind {
        HunkKind::Unchanged => "unchanged",
        HunkKind::SourceOnly => "changed in source only",
        HunkKind::TargetOnly => "changed in target only",
        HunkKind::BothSame => "same change in both",
        HunkKind::Conflict => "conflict",
    };
    let pending = session.pending(hunk);
    let status_text = match status {
        HunkStatus::Unchanged => "unchanged".to_owned(),
        HunkStatus::Auto => "resolved automatically".to_owned(),
        HunkStatus::Unresolved => format!("unresolved, {pending} lines pending"),
        HunkStatus::Partial => format!("partially resolved, {pending} lines pending"),
        HunkStatus::Resolved => "resolved".to_owned(),
        HunkStatus::Modified => "resolved by manual edit".to_owned(),
    };
    let position = if kind == HunkKind::Unchanged {
        String::new()
    } else {
        format!("Hunk {ordinal}/{changes} · ")
    };
    (format!("{position}{kind_text} · {status_text}"), status)
}

pub fn draw(frame: &mut Frame, area: Rect, app: &App, p: &Palette, symbols: &Symbols) {
    let theme = p.theme;
    let on = |color: Rgb| p.style(color, theme.bg);
    let block = Block::bordered()
        .border_type(super::border_type(app.config.ui.icons))
        .border_style(on(theme.border))
        .style(on(theme.fg))
        .title(Span::styled(
            format!(" {}HISTORY ", super::icon(symbols.history)),
            on(theme.muted).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(view) = app.view() else { return };

    let mut lines: Vec<Line> = Vec::new();
    let (summary, status) = hunk_summary(view);
    let status_color = match status {
        HunkStatus::Unresolved | HunkStatus::Partial => theme.unresolved,
        HunkStatus::Resolved => theme.accepted,
        HunkStatus::Modified => theme.modified,
        HunkStatus::Auto | HunkStatus::Unchanged => theme.muted,
    };
    let mut head = vec![Span::styled(
        format!(" {summary}"),
        on(status_color).add_modifier(Modifier::BOLD),
    )];

    let info = app.line_info();
    if let Some(LineInfo {
        line: Some((side, number)),
        state,
        ..
    }) = &info
    {
        let side_color = match side {
            crate::core::Side::Source => theme.source,
            crate::core::Side::Target => theme.target,
        };
        head.push(Span::styled("   │   ", on(theme.border)));
        head.push(Span::styled(
            format!("{} line {number}", side.name()),
            on(side_color),
        ));
        let state = match state {
            Some(LineState::Accepted) => Some((symbols.accepted, "accepted", theme.accepted)),
            Some(LineState::Rejected) => Some((symbols.rejected, "rejected", theme.rejected)),
            Some(LineState::Unresolved) => {
                Some((symbols.unresolved, "unresolved", theme.unresolved))
            }
            None => None,
        };
        if let Some((symbol, text, color)) = state {
            head.push(Span::styled(format!(" · {symbol} {text}"), on(color)));
        }
    }
    lines.push(Line::from(head));

    match info.map(|i| i.history) {
        Some(LineHistory::Commit(commit)) => {
            let now = chrono::Utc::now().timestamp();
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {}{}", super::icon(symbols.commit), commit.short_sha()),
                    on(theme.unresolved).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("   {}{}", super::icon(symbols.author), commit.author),
                    on(theme.accent),
                ),
                Span::styled(format!(" <{}>", commit.author_mail), on(theme.muted)),
                Span::styled(
                    format!("   {}{}", super::icon(symbols.clock), commit.author_date()),
                    on(theme.fg),
                ),
                Span::styled(
                    format!(" ({})", relative_time(commit.author_time, now)),
                    on(theme.muted),
                ),
            ]));
            lines.push(Line::styled(
                format!(" {}", commit.summary),
                on(theme.fg).add_modifier(Modifier::BOLD),
            ));
            if !commit.committer.is_empty() && commit.committer != commit.author {
                lines.push(Line::styled(
                    format!(" committed by {}", commit.committer),
                    on(theme.muted),
                ));
            }
            for body_line in commit.body.lines().filter(|l| !l.trim().is_empty()) {
                lines.push(Line::styled(format!(" {body_line}"), on(theme.muted)));
            }
        }
        Some(LineHistory::Loading) => {
            lines.push(Line::styled(" Loading git history…", on(theme.muted)));
        }
        Some(LineHistory::Note(note)) => {
            lines.push(Line::styled(format!(" {note}"), on(theme.muted)));
        }
        None => {}
    }
    frame.render_widget(Paragraph::new(lines).style(on(theme.fg)), inner);
}
