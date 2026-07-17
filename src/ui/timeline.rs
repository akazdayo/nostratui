use super::*;

const POST_IMAGE_MAX_WIDTH: u16 = 24;
const POST_IMAGE_MAX_HEIGHT: u16 = 8;

pub(super) fn draw_timeline(frame: &mut Frame, app: &mut App, area: Rect) {
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
    let timeline_len = app.timeline().len();
    let selected = app.selected_index().min(timeline_len - 1);
    let (window_start, window_end) = timeline_render_window(timeline_len, selected, inner.height);
    let window_len = window_end - window_start;
    let mut items = Vec::with_capacity(window_len);
    let mut item_heights = Vec::with_capacity(window_len);
    let mut authors = Vec::with_capacity(window_len);
    let mut item_emojis = Vec::with_capacity(window_len);
    let mut item_post_images = Vec::with_capacity(window_len);
    let content_width = area
        .width
        .saturating_sub(2 + 2 + AVATAR_INDENT.width() as u16)
        .min(180);
    for event in &app.timeline()[window_start..window_end] {
        let display = app.display_event(event);
        let author = app.author_name(&display.event.pubkey);
        let nip05 = app
            .nip05_label(&display.event.pubkey)
            .map(|value| format!("  {value}"))
            .unwrap_or_default();
        let reposted_by = display.reposted_by;
        let reactions =
            compact_content_line(app, &app.rendered_reactions(&display.event), content_width);
        let rendered = app.rendered_content(&display.event);
        let content = compact_content_line(app, &rendered.parts, content_width);
        let mut body = vec![Span::raw(AVATAR_INDENT)];
        body.extend(content.spans);
        let mut lines = Vec::new();
        if let Some(reposter) = reposted_by.as_ref() {
            lines.push(repost_line(app, reposter, AVATAR_INDENT));
        }
        if let Some(reply) = app.reply_display(&display.event).as_ref() {
            lines.push(reply_line(app, reply, AVATAR_INDENT));
        }
        let avatar_row = lines.len() as u16;
        lines.push(Line::from(vec![
            Span::raw(AVATAR_INDENT),
            Span::styled(author, Style::default().fg(ACCENT).bold()),
            Span::styled(nip05, Style::default().fg(Color::Green)),
            Span::raw("  "),
            Span::styled(
                format_time(display.event.created_at),
                Style::default().fg(DIM),
            ),
        ]));
        let content_row = lines.len() as u16;
        lines.push(Line::from(body));
        if let Some(quote) = rendered.quote.as_ref() {
            lines.extend(quote_lines(app, quote, AVATAR_INDENT));
        }
        let mut post_images = Vec::new();
        // Ratatui's List does not render an item that is taller than its
        // viewport. Keep room for the reactions and separator so loading an
        // image cannot make the selected timeline item disappear and advance
        // the list offset unexpectedly.
        let image_max_height = inner
            .height
            .saturating_sub(lines.len().min(u16::MAX as usize) as u16)
            .saturating_sub(2)
            .min(POST_IMAGE_MAX_HEIGHT);
        if let Some((url, (width, height))) = rendered.image_urls.iter().find_map(|url| {
            app.post_image_preview_size(
                url,
                content_width.min(POST_IMAGE_MAX_WIDTH),
                image_max_height,
            )
            .map(|size| (url, size))
        }) {
            post_images.push(PostImage {
                row: lines.len() as u16,
                column: AVATAR_INDENT.width() as u16,
                width,
                height,
                url: url.clone(),
            });
            lines.extend(std::iter::repeat_with(|| Line::raw(AVATAR_INDENT)).take(height as usize));
        }
        let reaction_row = lines.len() as u16;
        let mut reaction_spans = vec![Span::raw(AVATAR_INDENT)];
        reaction_spans.extend(reactions.spans.into_iter().map(|mut span| {
            span.style = span.style.fg(Color::Magenta);
            span
        }));
        lines.extend([Line::from(reaction_spans), Line::raw(AVATAR_INDENT)]);
        item_heights.push(lines.len() as u16);
        authors.push((display.event.pubkey, avatar_row));
        let mut emojis = content
            .images
            .into_iter()
            .map(|mut image| {
                image.row = content_row;
                image.column += AVATAR_INDENT.width() as u16;
                image
            })
            .collect::<Vec<_>>();
        emojis.extend(reactions.images.into_iter().map(|mut image| {
            image.row = reaction_row;
            image.column += AVATAR_INDENT.width() as u16;
            image
        }));
        item_emojis.push(emojis);
        item_post_images.push(post_images);
        items.push(ListItem::new(Text::from(lines)));
    }

    let list = List::new(items)
        .block(block)
        .highlight_symbol("▌ ")
        .highlight_style(Style::default().bg(Color::Rgb(35, 30, 48)));
    let relative_offset = app
        .timeline_offset()
        .saturating_sub(window_start)
        .min(window_len - 1);
    let relative_selected = selected - window_start;
    let mut state = ListState::default()
        .with_offset(relative_offset)
        .with_selected(Some(relative_selected));
    frame.render_stateful_widget(list, area, &mut state);
    app.sync_timeline_viewport(window_start + state.offset());

    if inner.width < AVATAR_WIDTH + 2 {
        return;
    }
    let first = state.offset();
    let mut y = inner.y;
    for ((((pubkey, avatar_row), height), emojis), post_images) in authors
        .iter()
        .zip(item_heights.iter())
        .zip(item_emojis.iter())
        .zip(item_post_images.iter())
        .skip(first)
    {
        if y.saturating_add(*height) > inner.bottom() {
            break;
        }
        let avatar_area = Rect::new(
            inner.x + 2,
            y.saturating_add(*avatar_row),
            AVATAR_WIDTH,
            AVATAR_HEIGHT,
        );
        render_avatar(frame, app, pubkey, avatar_area);
        render_custom_emojis(frame, app, emojis, (inner.x + 2, y), inner);
        render_post_images(frame, app, post_images, (inner.x + 2, y), inner);
        y = y.saturating_add(*height);
    }
}

pub(super) fn timeline_render_window(
    timeline_len: usize,
    selected: usize,
    viewport_height: u16,
) -> (usize, usize) {
    // One item per terminal row on each side is a conservative overscan;
    // timeline items currently occupy several rows each.
    let overscan = usize::from(viewport_height).max(1);
    let start = selected.saturating_sub(overscan);
    let end = selected
        .saturating_add(overscan)
        .saturating_add(1)
        .min(timeline_len);
    (start, end)
}

pub(super) fn draw_detail(frame: &mut Frame, app: &mut App, area: Rect) {
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
    let mut header_lines = Vec::new();
    if let Some(reposter) = display.reposted_by.as_ref() {
        header_lines.push(repost_line(app, reposter, ""));
    }
    header_lines.extend([
        Line::styled(
            app.author_name(&display.event.pubkey),
            Style::default().fg(ACCENT).bold(),
        ),
        Line::styled(pubkey, Style::default().fg(Color::Cyan)),
    ]);
    if let Some(nip05) = app.nip05_label(&display.event.pubkey) {
        header_lines.push(Line::styled(nip05, Style::default().fg(Color::Green)));
    }
    if let Some(about) = about {
        header_lines.push(Line::styled(compact(&about, 100), Style::default().fg(DIM)));
    }
    let rendered = app.rendered_content(&display.event);
    let content_layout =
        detailed_content_layout(app, &rendered.parts, area.width.saturating_sub(2));
    let mut body_lines = Vec::new();
    if let Some(reply) = app.reply_display(&display.event).as_ref() {
        body_lines.push(reply_line(app, reply, ""));
        body_lines.push(Line::raw(""));
    }
    body_lines.extend(content_layout.lines);
    let mut content_images = content_layout.images;
    if let Some(quote) = rendered.quote.as_ref() {
        body_lines.push(Line::raw(""));
        body_lines.extend(quote_lines(app, quote, ""));
    }
    let mut post_images = Vec::new();
    if let Some((url, (width, height))) = rendered.image_urls.iter().find_map(|url| {
        app.post_image_preview_size(
            url,
            area.width.saturating_sub(2).min(POST_IMAGE_MAX_WIDTH),
            POST_IMAGE_MAX_HEIGHT,
        )
        .map(|size| (url, size))
    }) {
        body_lines.push(Line::raw(""));
        post_images.push(PostImage {
            row: body_lines.len() as u16,
            column: 0,
            width,
            height,
            url: url.clone(),
        });
        body_lines.extend(std::iter::repeat_with(|| Line::raw("")).take(height as usize));
    }
    body_lines.extend([
        Line::raw(""),
        Line::styled(format!("note  {note_id}"), Style::default().fg(Color::Cyan)),
        Line::styled(
            format!("time  {}", format_time(display.event.created_at)),
            Style::default().fg(DIM),
        ),
    ]);
    let reaction_prefix = "reactions  ";
    let reaction_row = body_lines.len() as u16;
    let reactions = compact_content_line(
        app,
        &app.rendered_reactions(&display.event),
        area.width
            .saturating_sub(2 + reaction_prefix.width() as u16),
    );
    let mut reaction_spans = vec![Span::styled(
        reaction_prefix,
        Style::default().fg(Color::Magenta),
    )];
    reaction_spans.extend(reactions.spans.into_iter().map(|mut span| {
        span.style = span.style.fg(Color::Magenta);
        span
    }));
    body_lines.push(Line::from(reaction_spans));
    content_images.extend(reactions.images.into_iter().map(|mut image| {
        image.row = reaction_row;
        image.column += reaction_prefix.width() as u16;
        image
    }));
    let block = Block::default()
        .title(" Detail · h to close ")
        .borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let detail_header_height = (header_lines.len() as u16).max(AVATAR_HEIGHT);

    if app.kitty_images_enabled()
        && inner.width >= AVATAR_WIDTH + 10
        && inner.height > detail_header_height
    {
        let header_area = Rect::new(
            inner.x + AVATAR_WIDTH + 2,
            inner.y,
            inner.width - AVATAR_WIDTH - 2,
            detail_header_height,
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
            inner.y + detail_header_height,
            inner.width,
            inner.height - detail_header_height,
        );
        frame.render_widget(
            Paragraph::new(body_lines).wrap(Wrap { trim: false }),
            body_area,
        );
        render_custom_emojis(
            frame,
            app,
            &content_images,
            (body_area.x, body_area.y),
            body_area,
        );
        render_post_images(
            frame,
            app,
            &post_images,
            (body_area.x, body_area.y),
            body_area,
        );
    } else {
        let content_y = inner.y + header_lines.len() as u16 + 1;
        header_lines.push(Line::raw(""));
        header_lines.extend(body_lines);
        frame.render_widget(
            Paragraph::new(header_lines).wrap(Wrap { trim: false }),
            inner,
        );
        render_custom_emojis(frame, app, &content_images, (inner.x, content_y), inner);
        render_post_images(frame, app, &post_images, (inner.x, content_y), inner);
    }
}
