use chrono::{DateTime, Local};
use nostr_sdk::prelude::*;
use ratatui::style::Stylize;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use ratatui_image::{FilterType, Resize, StatefulImage};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, InputMode, QuoteDisplay, RenderedPart, TimelineTab};

const ACCENT: Color = Color::Rgb(180, 140, 255);
const DIM: Color = Color::Rgb(130, 135, 150);
const AVATAR_WIDTH: u16 = 4;
const AVATAR_HEIGHT: u16 = 2;
const DETAIL_HEADER_HEIGHT: u16 = 4;
const AVATAR_INDENT: &str = "      ";

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
    if app.settings_open() {
        draw_settings(frame, app);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let identity = if app.identity == "read-only" {
        "read-only".to_owned()
    } else {
        compact(&app.identity, 18)
    };
    let tab = |value: TimelineTab| {
        let count = app.timeline_count(value);
        let label = format!(
            " {} {} ({count}) ",
            match value {
                TimelineTab::Following => "1",
                TimelineTab::Global => "2",
            },
            value.label()
        );
        if app.active_tab() == value {
            Span::styled(
                label,
                Style::default()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD),
            )
        } else if value == TimelineTab::Following && !app.following_available() {
            Span::styled(label, Style::default().fg(Color::DarkGray))
        } else {
            Span::styled(label, Style::default().fg(DIM))
        }
    };
    let header = Paragraph::new(vec![
        Line::from(vec![
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
        ]),
        Line::from(vec![
            Span::raw("  "),
            tab(TimelineTab::Following),
            Span::raw(" "),
            tab(TimelineTab::Global),
        ]),
    ])
    .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(header, area);
}

fn draw_timeline(frame: &mut Frame, app: &mut App, area: Rect) {
    let mut items = Vec::with_capacity(app.timeline().len());
    let mut item_heights = Vec::with_capacity(app.timeline().len());
    let mut authors = Vec::with_capacity(app.timeline().len());
    for event in app.timeline() {
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
        let rendered = app.rendered_content(&display.event);
        let mut body = vec![Span::raw(AVATAR_INDENT)];
        body.extend(compact_content_spans(&rendered.parts, 180));
        let mut lines = vec![
            Line::from(vec![
                Span::raw(AVATAR_INDENT),
                Span::styled(repost, Style::default().fg(Color::Yellow)),
                Span::styled(author, Style::default().fg(ACCENT).bold()),
                Span::styled(nip05, Style::default().fg(Color::Green)),
                Span::raw("  "),
                Span::styled(
                    format_time(display.event.created_at),
                    Style::default().fg(DIM),
                ),
            ]),
            Line::from(body),
        ];
        if let Some(quote) = rendered.quote.as_ref() {
            lines.extend(quote_lines(app, quote, AVATAR_INDENT));
        }
        lines.extend([
            Line::from(vec![
                Span::raw(AVATAR_INDENT),
                Span::styled(reactions, Style::default().fg(Color::Magenta)),
            ]),
            Line::raw(AVATAR_INDENT),
        ]);
        item_heights.push(lines.len() as u16);
        authors.push(display.event.pubkey);
        items.push(ListItem::new(Text::from(lines)));
    }

    let title = format!(" {} timeline ", app.active_tab().label());
    let block = Block::default().title(title).borders(Borders::ALL);
    if app.timeline().is_empty() {
        let message = if app.active_tab() == TimelineTab::Following && !app.following_available() {
            "Following timeline requires NOSTR_SECRET_KEY"
        } else if app.active_tab() == TimelineTab::Following {
            "No notes from followed accounts yet"
        } else {
            "No notes received yet"
        };
        frame.render_widget(
            Paragraph::new(message)
                .style(Style::default().fg(DIM))
                .block(block),
            area,
        );
        app.sync_timeline_viewport(0);
        return;
    }
    let inner = block.inner(area);
    let list = List::new(items)
        .block(block)
        .highlight_symbol("▌ ")
        .highlight_style(Style::default().bg(Color::Rgb(35, 30, 48)));
    let mut state = ListState::default()
        .with_offset(app.timeline_offset())
        .with_selected(Some(app.selected_index()));
    frame.render_stateful_widget(list, area, &mut state);
    app.sync_timeline_viewport(state.offset());

    if inner.width < AVATAR_WIDTH + 2 {
        return;
    }
    let first = state.offset();
    let mut y = inner.y;
    for (pubkey, height) in authors.iter().zip(item_heights.iter()).skip(first) {
        if y.saturating_add(*height) > inner.bottom() {
            break;
        }
        let avatar_area = Rect::new(inner.x + 2, y, AVATAR_WIDTH, AVATAR_HEIGHT);
        render_avatar(frame, app, pubkey, avatar_area);
        y = y.saturating_add(*height);
    }
}

fn quote_lines(app: &App, quote: &QuoteDisplay, indent: &str) -> Vec<Line<'static>> {
    match quote.event.as_ref() {
        Some(event) => {
            let author = app.author_name(&event.pubkey);
            let rendered = app.rendered_content(event);
            let mut body = vec![
                Span::raw(indent.to_owned()),
                Span::styled("│ ", Style::default().fg(Color::Cyan)),
            ];
            body.extend(compact_content_spans(&rendered.parts, 160));
            vec![
                Line::from(vec![
                    Span::raw(indent.to_owned()),
                    Span::styled("┌ ↳ ", Style::default().fg(Color::Cyan)),
                    Span::styled(author, Style::default().fg(ACCENT).bold()),
                    Span::raw("  "),
                    Span::styled(format_time(event.created_at), Style::default().fg(DIM)),
                ]),
                Line::from(body),
            ]
        }
        None => {
            let id = quote
                .event_id
                .to_bech32()
                .unwrap_or_else(|_| quote.event_id.to_string());
            vec![
                Line::from(vec![
                    Span::raw(indent.to_owned()),
                    Span::styled("┌ ↳ quoted note", Style::default().fg(Color::Cyan)),
                ]),
                Line::from(vec![
                    Span::raw(indent.to_owned()),
                    Span::styled(
                        format!(
                            "│ {} · {}",
                            compact(&id, 22),
                            if quote.loading {
                                "loading…"
                            } else {
                                "unavailable"
                            }
                        ),
                        Style::default().fg(DIM),
                    ),
                ]),
            ]
        }
    }
}

fn compact_content_spans(parts: &[RenderedPart], max_chars: usize) -> Vec<Span<'static>> {
    let transformed: Vec<_> = parts
        .iter()
        .map(|part| (part.text.replace('\n', " ↵ "), part.mention))
        .collect();
    let total_chars = transformed
        .iter()
        .map(|(text, _)| text.chars().count())
        .sum::<usize>();
    let mut remaining = max_chars;
    let mut spans = Vec::new();

    for (text, mention) in transformed {
        if remaining == 0 {
            break;
        }
        let compact: String = text.chars().take(remaining).collect();
        remaining = remaining.saturating_sub(compact.chars().count());
        if !compact.is_empty() {
            spans.push(content_span(compact, mention));
        }
    }
    if total_chars > max_chars {
        spans.push(Span::raw("…"));
    }
    spans
}

fn detailed_content_lines(parts: &[RenderedPart]) -> Vec<Line<'static>> {
    let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    for part in parts {
        for (index, text) in part.text.split('\n').enumerate() {
            if index > 0 {
                lines.push(Vec::new());
            }
            if !text.is_empty() {
                lines
                    .last_mut()
                    .expect("content always has a line")
                    .push(content_span(text.to_owned(), part.mention));
            }
        }
    }
    lines.into_iter().map(Line::from).collect()
}

fn content_span(text: String, mention: bool) -> Span<'static> {
    if mention {
        Span::styled(text, Style::default().fg(Color::Cyan).bold())
    } else {
        Span::raw(text)
    }
}

fn draw_detail(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(event) = app.selected_event() else {
        frame.render_widget(
            Paragraph::new("No event selected")
                .block(Block::default().title(" Detail ").borders(Borders::ALL)),
            area,
        );
        return;
    };
    let display = app.display_event(event);
    let author_key = display.event.pubkey;
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
    let about = profile.and_then(|value| value.about.clone());
    let mut header_lines = vec![
        Line::styled(
            app.author_name(&display.event.pubkey),
            Style::default().fg(ACCENT).bold(),
        ),
        Line::styled(pubkey, Style::default().fg(Color::Cyan)),
    ];
    if let Some(nip05) = app.nip05_label(&display.event.pubkey) {
        header_lines.push(Line::styled(nip05, Style::default().fg(Color::Green)));
    }
    if let Some(about) = about {
        header_lines.push(Line::styled(compact(&about, 100), Style::default().fg(DIM)));
    }
    let rendered = app.rendered_content(&display.event);
    let mut body_lines = detailed_content_lines(&rendered.parts);
    if let Some(quote) = rendered.quote.as_ref() {
        body_lines.push(Line::raw(""));
        body_lines.extend(quote_lines(app, quote, ""));
    }
    body_lines.extend([
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
    let block = Block::default()
        .title(" Detail · h to close ")
        .borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.kitty_images_enabled()
        && inner.width >= AVATAR_WIDTH + 10
        && inner.height > DETAIL_HEADER_HEIGHT
    {
        let header_area = Rect::new(
            inner.x + AVATAR_WIDTH + 2,
            inner.y,
            inner.width - AVATAR_WIDTH - 2,
            DETAIL_HEADER_HEIGHT,
        );
        frame.render_widget(
            Paragraph::new(header_lines).wrap(Wrap { trim: false }),
            header_area,
        );
        render_avatar(
            frame,
            app,
            &author_key,
            Rect::new(inner.x, inner.y, AVATAR_WIDTH, AVATAR_HEIGHT),
        );
        let body_area = Rect::new(
            inner.x,
            inner.y + DETAIL_HEADER_HEIGHT,
            inner.width,
            inner.height - DETAIL_HEADER_HEIGHT,
        );
        frame.render_widget(
            Paragraph::new(body_lines).wrap(Wrap { trim: false }),
            body_area,
        );
    } else {
        header_lines.push(Line::raw(""));
        header_lines.extend(body_lines);
        frame.render_widget(
            Paragraph::new(header_lines).wrap(Wrap { trim: false }),
            inner,
        );
    }
}

fn render_avatar(frame: &mut Frame, app: &mut App, pubkey: &PublicKey, area: Rect) {
    let Some(protocol) = app.avatar_protocol_mut(pubkey) else {
        return;
    };
    frame.render_stateful_widget(
        StatefulImage::default().resize(Resize::Fit(Some(FilterType::Triangle))),
        area,
        protocol,
    );
    // Consume encoding errors so stale results are not retained indefinitely.
    let _ = protocol.last_encoding_result();
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
    let inner_width = area.width.saturating_sub(2).max(1);
    let inner_height = area.height.saturating_sub(2).max(1);
    let layout = editor_layout(&app.input, inner_width);
    let scroll = layout
        .cursor_row
        .saturating_sub(usize::from(inner_height.saturating_sub(1)));
    let text = Text::from(
        layout
            .lines
            .iter()
            .map(|line| Line::raw(line.as_str()))
            .collect::<Vec<_>>(),
    );
    frame.render_widget(
        Paragraph::new(text)
            .block(Block::default().title(title).borders(Borders::ALL))
            .scroll((scroll.min(u16::MAX as usize) as u16, 0)),
        area,
    );
    let x = area.x + 1 + layout.cursor_column.min(usize::from(inner_width - 1)) as u16;
    let visible_row = layout.cursor_row.saturating_sub(scroll);
    let y = area.y + 1 + visible_row.min(usize::from(inner_height - 1)) as u16;
    frame.set_cursor_position((x, y));
}

#[derive(Debug, PartialEq, Eq)]
struct EditorLayout {
    lines: Vec<String>,
    cursor_column: usize,
    cursor_row: usize,
}

/// Hard-wraps editor input using the same terminal-cell widths used by ratatui.
/// Grapheme clusters keep combining characters and emoji sequences together.
fn editor_layout(input: &str, width: u16) -> EditorLayout {
    let width = usize::from(width.max(1));
    let mut lines = vec![String::new()];
    let mut column: usize = 0;

    for grapheme in input.graphemes(true) {
        if grapheme == "\n" {
            lines.push(String::new());
            column = 0;
            continue;
        }

        let grapheme_width = grapheme.width();
        if grapheme_width > 0 && column > 0 && column.saturating_add(grapheme_width) > width {
            lines.push(String::new());
            column = 0;
        }
        lines
            .last_mut()
            .expect("editor always has a line")
            .push_str(grapheme);
        column = column.saturating_add(grapheme_width);
    }

    // Once the final cell is occupied, the insertion cursor belongs at the
    // beginning of the next visual row.
    if column >= width {
        lines.push(String::new());
        column = 0;
    }

    EditorLayout {
        cursor_column: column,
        cursor_row: lines.len() - 1,
        lines,
    }
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let timeline_mode = if app.is_live() {
        "LIVE".to_owned()
    } else if app.unseen_count() > 0 {
        format!("PAUSED · {} new", app.unseen_count())
    } else {
        "PAUSED".to_owned()
    };
    let help = if app.settings_open() {
        " SETTINGS  m/Esc close  q quit ".to_owned()
    } else if matches!(app.mode, InputMode::Normal) {
        format!(
            " {timeline_mode}  Tab/1/2 timeline  j/k move  g LIVE/top  G last  l/Enter detail  m settings  i/o post  r reply  +/-/e react  R repost  q quit "
        )
    } else {
        format!(" {timeline_mode}  INSERT ")
    };
    frame.render_widget(
        Paragraph::new(help).style(Style::default().fg(Color::Black).bg(DIM)),
        area,
    );
}

fn draw_settings(frame: &mut Frame, app: &App) {
    let screen = frame.area();
    let width = screen.width.saturating_sub(4).clamp(1, 72);
    let desired_height = app.relays().len() as u16 + 10;
    let height = screen.height.saturating_sub(2).clamp(1, desired_height);
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );

    let account_mode = if app.read_only {
        "read-only"
    } else {
        "write enabled"
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Account  ", Style::default().fg(DIM)),
            Span::styled(account_mode, Style::default().fg(ACCENT).bold()),
        ]),
        Line::styled(app.identity.as_str(), Style::default().fg(Color::Cyan)),
        Line::from(vec![
            Span::styled("Images   ", Style::default().fg(DIM)),
            Span::styled(
                if app.kitty_images_enabled() {
                    "Kitty graphics"
                } else {
                    "disabled (terminal unsupported)"
                },
                Style::default().fg(ACCENT),
            ),
        ]),
        Line::raw(""),
        Line::styled("Relays", Style::default().fg(DIM).bold()),
    ];
    if app.relays().is_empty() {
        lines.push(Line::styled("  (none)", Style::default().fg(DIM)));
    } else {
        lines.extend(
            app.relays()
                .iter()
                .map(|relay| Line::raw(format!("  • {relay}"))),
        );
    }
    lines.extend([
        Line::raw(""),
        Line::styled("m / Esc  close", Style::default().fg(DIM)),
    ]);

    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" Settings ")
                    .title_style(Style::default().fg(ACCENT).bold())
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
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
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use nostr_sdk::prelude::*;
    use ratatui::{
        backend::TestBackend,
        style::{Color, Modifier},
        Terminal,
    };

    use crate::{app::App, network::UiEvent};

    use super::{compact, content_span, draw, editor_layout, EditorLayout};

    #[test]
    fn compact_handles_unicode() {
        assert_eq!(compact("こんにちは", 3), "こんに…");
        assert_eq!(compact("abc", 3), "abc");
    }

    #[test]
    fn mention_span_has_distinct_style() {
        let mention = content_span("@Alice".to_owned(), true);
        let body = content_span("hello".to_owned(), false);

        assert_eq!(mention.style.fg, Some(Color::Cyan));
        assert!(mention.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(body.style.fg, None);
    }

    #[test]
    fn editor_cursor_uses_terminal_width_for_japanese() {
        assert_eq!(
            editor_layout("abc日本語", 20),
            EditorLayout {
                lines: vec!["abc日本語".to_owned()],
                cursor_column: 9,
                cursor_row: 0,
            }
        );
    }

    #[test]
    fn editor_cursor_tracks_unicode_wrapping_and_newlines() {
        assert_eq!(
            editor_layout("日本語\nか\u{3099}", 4),
            EditorLayout {
                lines: vec!["日本".to_owned(), "語".to_owned(), "か\u{3099}".to_owned()],
                cursor_column: 2,
                cursor_row: 2,
            }
        );
    }

    #[test]
    fn timeline_is_live_until_the_viewport_scrolls_from_the_top() {
        let keys = Keys::generate();
        let mut app = App::new(true, Vec::new());
        for (content, timestamp) in [("newest", 300), ("middle", 200), ("oldest", 100)] {
            let event = EventBuilder::text_note(content)
                .custom_created_at(Timestamp::from_secs(timestamp))
                .sign_with_keys(&keys)
                .unwrap();
            app.on_ui_event(UiEvent::Event(Box::new(event)));
        }
        // At this height exactly two timeline items fit in the viewport.
        let mut terminal = Terminal::new(TestBackend::new(80, 15)).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert_eq!(app.selected_index(), 1);
        assert_eq!(app.timeline_offset(), 0);
        assert!(app.is_live());

        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert_eq!(app.selected_index(), 2);
        assert_eq!(app.timeline_offset(), 1);
        assert!(!app.is_live());

        app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert_eq!(app.selected_index(), 1);
        assert_eq!(app.timeline_offset(), 1);
        assert!(!app.is_live());

        app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert_eq!(app.selected_index(), 0);
        assert_eq!(app.timeline_offset(), 0);
        assert!(app.is_live());
    }
}
