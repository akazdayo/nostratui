use chrono::{DateTime, Local};
use nostr_sdk::prelude::*;
use ratatui::style::Stylize;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, InputMode};

const ACCENT: Color = Color::Rgb(180, 140, 255);
const DIM: Color = Color::Rgb(130, 135, 150);

pub fn draw(frame: &mut Frame, app: &mut App) {
    let input_height = if matches!(app.mode, InputMode::Normal) {
        1
    } else {
        6
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(input_height),
            Constraint::Length(1),
        ])
        .split(frame.area());

    draw_header(frame, app, rows[0]);
    if app.detail {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(rows[1]);
        draw_timeline(frame, app, columns[0]);
        draw_detail(frame, app, columns[1]);
    } else {
        draw_timeline(frame, app, rows[1]);
    }
    draw_input(frame, app, rows[2]);
    draw_footer(frame, app, rows[3]);
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let identity = if app.identity == "read-only" {
        "read-only".to_owned()
    } else {
        compact(&app.identity, 18)
    };
    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            "  nostr-ratatui ",
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(identity, Style::default().fg(Color::Cyan)),
        Span::raw("  ·  "),
        Span::styled(&app.status, Style::default().fg(DIM)),
    ]))
    .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(header, area);
}

fn draw_timeline(frame: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<ListItem> = app
        .timeline
        .iter()
        .map(|event| {
            let display = app.display_event(event);
            let author = app.author_name(&display.event.pubkey);
            let nip05 = app
                .nip05_label(&display.event.pubkey)
                .map(|value| format!("  {value}"))
                .unwrap_or_default();
            let repost = display
                .reposted_by
                .as_ref()
                .map(|key| format!("↻ {}  ", app.author_name(key)))
                .unwrap_or_default();
            let reactions = app.reaction_summary(&display.event);
            let body = display.content_with_mentions().replace('\n', " ↵ ");
            ListItem::new(Text::from(vec![
                Line::from(vec![
                    Span::styled(repost, Style::default().fg(Color::Yellow)),
                    Span::styled(author, Style::default().fg(ACCENT).bold()),
                    Span::styled(nip05, Style::default().fg(Color::Green)),
                    Span::raw("  "),
                    Span::styled(
                        format_time(display.event.created_at),
                        Style::default().fg(DIM),
                    ),
                ]),
                Line::from(Span::raw(compact(&body, 180))),
                Line::from(Span::styled(reactions, Style::default().fg(Color::Magenta))),
                Line::raw(""),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().title(" Timeline ").borders(Borders::ALL))
        .highlight_symbol("▌ ")
        .highlight_style(Style::default().bg(Color::Rgb(35, 30, 48)));
    let mut state =
        ListState::default().with_selected((!app.timeline.is_empty()).then_some(app.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_detail(frame: &mut Frame, app: &App, area: Rect) {
    let Some(event) = app.selected_event() else {
        frame.render_widget(
            Paragraph::new("No event selected")
                .block(Block::default().title(" Detail ").borders(Borders::ALL)),
            area,
        );
        return;
    };
    let display = app.display_event(event);
    let pubkey = display
        .event
        .pubkey
        .to_bech32()
        .unwrap_or_else(|_| display.event.pubkey.to_hex());
    let note_id = display
        .event
        .id
        .to_bech32()
        .unwrap_or_else(|_| display.event.id.to_string());
    let profile = app.profiles.get(&display.event.pubkey.to_hex());
    let mut lines = vec![
        Line::styled(
            app.author_name(&display.event.pubkey),
            Style::default().fg(ACCENT).bold(),
        ),
        Line::styled(pubkey, Style::default().fg(Color::Cyan)),
    ];
    if let Some(nip05) = app.nip05_label(&display.event.pubkey) {
        lines.push(Line::styled(nip05, Style::default().fg(Color::Green)));
    }
    if let Some(about) = profile.and_then(|value| value.about.as_ref()) {
        lines.push(Line::raw(""));
        lines.push(Line::styled(about, Style::default().fg(DIM)));
    }
    lines.extend([
        Line::raw(""),
        Line::raw(display.content_with_mentions()),
        Line::raw(""),
        Line::styled(format!("note  {note_id}"), Style::default().fg(Color::Cyan)),
        Line::styled(
            format!("time  {}", format_time(display.event.created_at)),
            Style::default().fg(DIM),
        ),
        Line::styled(
            format!("reactions  {}", app.reaction_summary(&display.event)),
            Style::default().fg(Color::Magenta),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" Detail · h to close ")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    match &app.mode {
        InputMode::Normal => {}
        InputMode::Compose { reply_to } => {
            let title = if reply_to.is_some() {
                " Reply · Ctrl-S send · Esc cancel "
            } else {
                " New note · Ctrl-S send · Esc cancel "
            };
            draw_editor(frame, app, area, title);
        }
        InputMode::Reaction { .. } => {
            draw_editor(
                frame,
                app,
                area,
                " Emoji reaction · Ctrl-S send · Esc cancel ",
            );
        }
    }
}

fn draw_editor(frame: &mut Frame, app: &App, area: Rect, title: &str) {
    frame.render_widget(
        Paragraph::new(app.input.as_str())
            .block(Block::default().title(title).borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        area,
    );
    let last_line = app.input.rsplit('\n').next().unwrap_or("");
    let x = area.x
        + 1
        + last_line
            .chars()
            .count()
            .min(area.width.saturating_sub(3) as usize) as u16;
    let line_count = app
        .input
        .chars()
        .filter(|character| *character == '\n')
        .count();
    let y = area.y + 1 + (line_count as u16).min(area.height.saturating_sub(3));
    frame.set_cursor_position((x, y));
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let help = if matches!(app.mode, InputMode::Normal) {
        " j/k move  g/G first/last  l/Enter detail  i/o post  r reply  +/-/e react  R repost  q quit "
    } else {
        " INSERT "
    };
    frame.render_widget(
        Paragraph::new(help).style(Style::default().fg(Color::Black).bg(DIM)),
        area,
    );
}

fn format_time(timestamp: Timestamp) -> String {
    DateTime::from_timestamp(timestamp.as_secs() as i64, 0)
        .map(|time| time.with_timezone(&Local).format("%m-%d %H:%M").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

fn compact(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let compact: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{compact}…")
    } else {
        compact
    }
}

#[cfg(test)]
mod tests {
    use super::compact;

    #[test]
    fn compact_handles_unicode() {
        assert_eq!(compact("こんにちは", 3), "こんに…");
        assert_eq!(compact("abc", 3), "abc");
    }
}
