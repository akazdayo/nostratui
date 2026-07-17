use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nostr_sdk::prelude::*;
use ratatui::{
    backend::TestBackend,
    style::{Color, Modifier},
    Terminal,
};

use crate::{
    app::{App, Profile},
    network::UiEvent,
};

use super::{
    compact, content_span, draw, editor_layout, post_image_resize, timeline_render_window,
    EditorLayout,
};

#[test]
fn compact_handles_unicode() {
    assert_eq!(compact("こんにちは", 3), "こんに…");
    assert_eq!(compact("abc", 3), "abc");
}

#[test]
fn timeline_render_window_is_bounded_around_the_selection() {
    assert_eq!(timeline_render_window(5_000, 0, 33), (0, 34));
    assert_eq!(timeline_render_window(5_000, 2_500, 33), (2_467, 2_534));
    assert_eq!(timeline_render_window(5_000, 4_999, 33), (4_966, 5_000));
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
fn post_images_are_upscaled_to_the_reserved_preview_area() {
    assert!(matches!(
        post_image_resize(),
        ratatui_image::Resize::Scale(_)
    ));
}

#[test]
fn repost_header_separates_reposter_from_original_author() {
    let original_author = Keys::generate();
    let reposter = Keys::generate();
    let original = EventBuilder::text_note("original body")
        .sign_with_keys(&original_author)
        .unwrap();
    let repost = EventBuilder::repost(&original, None)
        .sign_with_keys(&reposter)
        .unwrap();
    let mut app = App::new(true, Vec::new());
    app.profiles.insert(
        original_author.public_key().to_hex(),
        Profile {
            display_name: Some("Original Author".to_owned()),
            ..Profile::default()
        },
    );
    app.profiles.insert(
        reposter.public_key().to_hex(),
        Profile {
            display_name: Some("Alice".to_owned()),
            ..Profile::default()
        },
    );
    app.on_ui_event(UiEvent::Event(Box::new(repost)));
    let mut terminal = Terminal::new(TestBackend::new(80, 15)).unwrap();

    terminal.draw(|frame| draw(frame, &mut app)).unwrap();

    let buffer = terminal.backend().buffer();
    let rows = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    let repost_row = rows
        .iter()
        .position(|row| row.contains("Reposted by Alice"))
        .unwrap();
    let original_row = rows
        .iter()
        .position(|row| row.contains("Original Author"))
        .unwrap();

    assert_eq!(original_row, repost_row + 1);
}

#[test]
fn reply_context_is_rendered_above_the_reply() {
    let parent_author = Keys::generate();
    let reply_author = Keys::generate();
    let parent = EventBuilder::text_note("parent body")
        .custom_created_at(Timestamp::from_secs(100))
        .sign_with_keys(&parent_author)
        .unwrap();
    let reply = EventBuilder::text_note_reply("reply body", &parent, None, None)
        .custom_created_at(Timestamp::from_secs(200))
        .sign_with_keys(&reply_author)
        .unwrap();
    let mut app = App::new(true, Vec::new());
    app.profiles.insert(
        parent_author.public_key().to_hex(),
        Profile {
            display_name: Some("Alice".to_owned()),
            ..Profile::default()
        },
    );
    app.on_ui_event(UiEvent::Event(Box::new(parent)));
    app.on_ui_event(UiEvent::Event(Box::new(reply)));
    let mut terminal = Terminal::new(TestBackend::new(80, 18)).unwrap();

    terminal.draw(|frame| draw(frame, &mut app)).unwrap();

    let rows = (0..terminal.backend().buffer().area.height)
        .map(|y| {
            (0..terminal.backend().buffer().area.width)
                .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    let context_row = rows
        .iter()
        .position(|row| row.contains("Reply to Alice · parent body"))
        .unwrap();
    let reply_row = rows
        .iter()
        .position(|row| row.contains("reply body"))
        .unwrap();

    assert!(context_row < reply_row);
}

#[test]
fn editor_cursor_uses_terminal_width_for_japanese() {
    assert_eq!(
        editor_layout("abc日本語", "abc日本語".len(), 20),
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
        editor_layout("日本語\nか\u{3099}", "日本語\nか\u{3099}".len(), 4),
        EditorLayout {
            lines: vec!["日本".to_owned(), "語".to_owned(), "か\u{3099}".to_owned()],
            cursor_column: 2,
            cursor_row: 2,
        }
    );
}

#[test]
fn editor_cursor_can_be_rendered_inside_unicode_input() {
    assert_eq!(
        editor_layout("日本語", "日本".len(), 4),
        EditorLayout {
            lines: vec!["日本".to_owned(), "語".to_owned()],
            cursor_column: 0,
            cursor_row: 1,
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

#[test]
fn virtualized_timeline_keeps_global_scroll_state() {
    let keys = Keys::generate();
    let mut app = App::new(true, Vec::new());
    for timestamp in 1..=100 {
        let event = EventBuilder::text_note(format!("note {timestamp}"))
            .custom_created_at(Timestamp::from_secs(timestamp))
            .sign_with_keys(&keys)
            .unwrap();
        app.on_ui_event(UiEvent::Event(Box::new(event)));
    }
    let mut terminal = Terminal::new(TestBackend::new(80, 15)).unwrap();

    app.on_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE));
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert_eq!(app.selected_index(), 99);
    assert!(app.timeline_offset() > 0);
    assert!(app.timeline_offset() <= app.selected_index());

    for _ in 0..15 {
        app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    }
    assert_eq!(app.selected_index(), 84);
    assert!(app.timeline_offset() <= app.selected_index());

    app.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    assert_eq!(app.selected_index(), 0);
    assert_eq!(app.timeline_offset(), 0);
    assert!(app.is_live());
}
