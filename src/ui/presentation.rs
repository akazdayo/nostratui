use super::*;

pub(super) fn repost_line(app: &App, reposter: &PublicKey, indent: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw(indent.to_owned()),
        Span::styled("↻ Reposted by ", Style::default().fg(Color::Yellow)),
        Span::styled(
            app.author_name(reposter),
            Style::default().fg(Color::Yellow).bold(),
        ),
    ])
}

pub(super) fn reply_line(app: &App, reply: &ReplyDisplay, indent: &str) -> Line<'static> {
    let mut spans = vec![
        Span::raw(indent.to_owned()),
        Span::styled("↳ Reply to ", Style::default().fg(Color::Cyan)),
    ];
    if let Some(event) = reply.event.as_ref() {
        spans.push(Span::styled(
            app.author_name(&event.pubkey),
            Style::default().fg(Color::Cyan).bold(),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(DIM)));
        spans.extend(compact_content_spans(
            &app.rendered_content(event).parts,
            100,
        ));
    } else {
        let id = reply
            .event_id
            .to_bech32()
            .unwrap_or_else(|_| reply.event_id.to_string());
        spans.push(Span::styled(
            format!(
                "{} · {}",
                compact(&id, 22),
                if reply.loading {
                    "loading…"
                } else {
                    "unavailable"
                }
            ),
            Style::default().fg(DIM),
        ));
    }
    Line::from(spans)
}

pub(super) fn quote_lines(app: &App, quote: &QuoteDisplay, indent: &str) -> Vec<Line<'static>> {
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

#[derive(Debug, Clone)]
pub(super) struct InlineImage {
    pub(super) row: u16,
    pub(super) column: u16,
    pub(super) emoji: CustomEmoji,
}

pub(super) struct InlineLine {
    pub(super) spans: Vec<Span<'static>>,
    pub(super) images: Vec<InlineImage>,
}

pub(super) struct InlineLayout {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) images: Vec<InlineImage>,
}

#[derive(Debug, Clone)]
pub(super) struct PostImage {
    pub(super) row: u16,
    pub(super) column: u16,
    pub(super) width: u16,
    pub(super) height: u16,
    pub(super) url: String,
}

pub(super) fn compact_content_line(app: &App, parts: &[RenderedPart], width: u16) -> InlineLine {
    let mut spans = Vec::new();
    let mut images = Vec::new();
    let mut column = 0_u16;
    let mut truncated = false;

    'parts: for part in parts {
        if let Some(emoji) = part
            .emoji
            .as_ref()
            .filter(|emoji| app.custom_emoji_ready(emoji))
        {
            if column.saturating_add(2) > width {
                truncated = true;
                break;
            }
            spans.push(Span::raw("  "));
            images.push(InlineImage {
                row: 0,
                column,
                emoji: emoji.clone(),
            });
            column += 2;
            continue;
        }

        let text = part.text.replace('\n', " ↵ ");
        for grapheme in text.graphemes(true) {
            let grapheme_width = grapheme.width().min(u16::MAX as usize) as u16;
            if column.saturating_add(grapheme_width) > width {
                truncated = true;
                break 'parts;
            }
            spans.push(content_span(grapheme.to_owned(), part.mention));
            column = column.saturating_add(grapheme_width);
        }
    }
    if truncated && column < width {
        spans.push(Span::raw("…"));
    }
    InlineLine { spans, images }
}

// Quotes keep their shortcode fallback because their compact border layout is
// not currently an inline-image surface.
pub(super) fn compact_content_spans(
    parts: &[RenderedPart],
    max_chars: usize,
) -> Vec<Span<'static>> {
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

pub(super) fn detailed_content_layout(
    app: &App,
    parts: &[RenderedPart],
    width: u16,
) -> InlineLayout {
    let width = width.max(1);
    let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut images = Vec::new();
    let mut row = 0_u16;
    let mut column = 0_u16;

    for part in parts {
        if let Some(emoji) = part
            .emoji
            .as_ref()
            .filter(|emoji| app.custom_emoji_ready(emoji))
        {
            if column > 0 && column.saturating_add(2) > width {
                lines.push(Vec::new());
                row = row.saturating_add(1);
                column = 0;
            }
            lines
                .last_mut()
                .expect("content always has a line")
                .push(Span::raw("  "));
            images.push(InlineImage {
                row,
                column,
                emoji: emoji.clone(),
            });
            column = column.saturating_add(2);
            continue;
        }

        for grapheme in part.text.graphemes(true) {
            if grapheme == "\n" {
                lines.push(Vec::new());
                row = row.saturating_add(1);
                column = 0;
                continue;
            }
            let grapheme_width = grapheme.width().min(u16::MAX as usize) as u16;
            if grapheme_width > 0 && column > 0 && column.saturating_add(grapheme_width) > width {
                lines.push(Vec::new());
                row = row.saturating_add(1);
                column = 0;
            }
            lines
                .last_mut()
                .expect("content always has a line")
                .push(content_span(grapheme.to_owned(), part.mention));
            column = column.saturating_add(grapheme_width);
        }
    }

    InlineLayout {
        lines: lines.into_iter().map(Line::from).collect(),
        images,
    }
}

pub(super) fn content_span(text: String, mention: bool) -> Span<'static> {
    if mention {
        Span::styled(text, Style::default().fg(Color::Cyan).bold())
    } else {
        Span::raw(text)
    }
}

pub(super) fn render_avatar(frame: &mut Frame, app: &mut App, pubkey: &PublicKey, area: Rect) {
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

pub(super) fn render_custom_emojis(
    frame: &mut Frame,
    app: &mut App,
    images: &[InlineImage],
    origin: (u16, u16),
    clip: Rect,
) {
    for image in images {
        let x = origin.0.saturating_add(image.column);
        let y = origin.1.saturating_add(image.row);
        if x < clip.x || y < clip.y || x.saturating_add(2) > clip.right() || y >= clip.bottom() {
            continue;
        }
        let Some(protocol) = app.custom_emoji_protocol_mut(&image.emoji) else {
            continue;
        };
        frame.render_stateful_widget(
            StatefulImage::default().resize(Resize::Fit(Some(FilterType::Triangle))),
            Rect::new(x, y, 2, 1),
            protocol,
        );
        let _ = protocol.last_encoding_result();
    }
}

pub(super) fn render_post_images(
    frame: &mut Frame,
    app: &mut App,
    images: &[PostImage],
    origin: (u16, u16),
    clip: Rect,
) {
    for image in images {
        let x = origin.0.saturating_add(image.column);
        let y = origin.1.saturating_add(image.row);
        if image.width == 0
            || image.height == 0
            || x < clip.x
            || y < clip.y
            || x.saturating_add(image.width) > clip.right()
            || y.saturating_add(image.height) > clip.bottom()
        {
            continue;
        }
        let Some(protocol) = app.post_image_protocol_mut(&image.url) else {
            continue;
        };
        frame.render_stateful_widget(
            // Post images are normalized to a bounded pixel size while decoding.
            // `Fit` does not upscale sources that are already smaller than the
            // target cell area, leaving most of the reserved preview blank.
            StatefulImage::default().resize(post_image_resize()),
            Rect::new(x, y, image.width, image.height),
            protocol,
        );
        let _ = protocol.last_encoding_result();
    }
}

pub(super) fn post_image_resize() -> Resize {
    Resize::Scale(Some(FilterType::Triangle))
}
