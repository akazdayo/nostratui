use super::*;

pub(super) fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
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

pub(super) fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
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
            " {timeline_mode}  Tab/1/2 timeline  j/k move  g LIVE/top  G last  l/Enter detail  m settings  i/o post  r reply  f/e react  R repost  q quit "
        )
    } else {
        format!(" {timeline_mode}  INSERT ")
    };
    frame.render_widget(
        Paragraph::new(help).style(Style::default().fg(Color::Black).bg(DIM)),
        area,
    );
}

pub(super) fn draw_settings(frame: &mut Frame, app: &App) {
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

pub(super) fn format_time(timestamp: Timestamp) -> String {
    DateTime::from_timestamp(timestamp.as_secs() as i64, 0)
        .map(|time| time.with_timezone(&Local).format("%m-%d %H:%M").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

pub(super) fn compact(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let compact: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{compact}…")
    } else {
        compact
    }
}
