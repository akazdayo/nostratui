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
    timeline_viewport_offset, EditorLayout,
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
fn tall_selected_item_does_not_evict_multiple_visible_posts() {
    let heights = [4, 4, 4, 12, 4];

    assert_eq!(timeline_viewport_offset(&heights, 0, 3, 15), 0);
    assert_eq!(timeline_viewport_offset(&heights, 0, 3, 13), 1);
}

#[test]
fn timeline_does_not_render_an_author_only_bottom_row() {
    let mut app = App::new(true, Vec::new());
    for (name, timestamp) in [
        ("First Author", 400),
        ("Second Author", 300),
        ("Third Author", 200),
        ("Bottom Author", 100),
    ] {
        let keys = Keys::generate();
        app.profiles.insert(
            keys.public_key().to_hex(),
            Profile {
                display_name: Some(name.to_owned()),
                ..Profile::default()
            },
        );
        let event = EventBuilder::text_note(format!("body by {name}"))
            .custom_created_at(Timestamp::from_secs(timestamp))
            .sign_with_keys(&keys)
            .unwrap();
        app.on_ui_event(UiEvent::Event(Box::new(event)));
    }
    // The timeline has thirteen inner rows. Three regular posts consume
    // twelve, so the fourth must not render as a one-line author fragment.
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();

    terminal.draw(|frame| draw(frame, &mut app)).unwrap();

    let buffer = terminal.backend().buffer();
    let rendered = (0..buffer.area.height).any(|y| {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect::<String>()
            .contains("Bottom Author")
    });
    assert!(!rendered, "an author-only bottom row should be omitted");
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
fn loaded_image_does_not_advance_timeline_past_selected_item() {
    let keys = Keys::generate();
    let image_url = "https://cdn.example.com/photo.webp";
    let newest = EventBuilder::text_note(format!("newest {image_url}"))
        .custom_created_at(Timestamp::from_secs(200))
        .sign_with_keys(&keys)
        .unwrap();
    let older = EventBuilder::text_note("older")
        .custom_created_at(Timestamp::from_secs(100))
        .sign_with_keys(&keys)
        .unwrap();
    let mut app = App::new(true, Vec::new());
    app.set_image_cache(crate::graphics::ImageCache::kitty_for_test());
    app.on_ui_event(UiEvent::Event(Box::new(older)));
    app.on_ui_event(UiEvent::Event(Box::new(newest)));
    let (key, url) = app
        .image_commands()
        .into_iter()
        .find_map(|command| match command {
            crate::app::Command::FetchImage { key, url } if url == image_url => Some((key, url)),
            _ => None,
        })
        .expect("post image should be requested");
    app.on_ui_event(UiEvent::Image {
        key,
        url,
        image: Some(::image::DynamicImage::new_rgba8(100, 100)),
    });
    // The timeline has eight inner rows. Without constraining the image to
    // the four rows left by the note chrome, the first item grows to twelve
    // rows and Ratatui advances the viewport to the older item.
    let mut terminal = Terminal::new(TestBackend::new(80, 15)).unwrap();

    terminal.draw(|frame| draw(frame, &mut app)).unwrap();

    assert_eq!(app.selected_index(), 0);
    assert_eq!(app.timeline_offset(), 0);
    assert!(app.is_live());
    let buffer = terminal.backend().buffer();
    let rendered = (0..buffer.area.height).any(|y| {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect::<String>()
            .contains("newest")
    });
    assert!(rendered, "the selected image note should remain visible");
}

#[test]
fn tall_image_post_is_partially_rendered_without_a_multi_post_jump() {
    let keys = Keys::generate();
    let image_url = "https://cdn.example.com/tall.webp";
    let mut app = App::new(true, Vec::new());
    app.set_image_cache(crate::graphics::ImageCache::kitty_for_test());
    for (content, timestamp) in [
        ("first", 500),
        ("second", 400),
        ("third", 300),
        (image_url, 200),
        ("last", 100),
    ] {
        let event = EventBuilder::text_note(content)
            .custom_created_at(Timestamp::from_secs(timestamp))
            .sign_with_keys(&keys)
            .unwrap();
        app.on_ui_event(UiEvent::Event(Box::new(event)));
    }
    let (key, url) = app
        .image_commands()
        .into_iter()
        .find_map(|command| match command {
            crate::app::Command::FetchImage { key, url } if url == image_url => Some((key, url)),
            _ => None,
        })
        .expect("post image should be requested");
    app.on_ui_event(UiEvent::Image {
        key,
        url,
        image: Some(::image::DynamicImage::new_rgba8(100, 100)),
    });
    for _ in 0..3 {
        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    }
    // The timeline has fifteen inner rows: three regular posts use twelve,
    // leaving enough room for the selected image post's header and content.
    let mut terminal = Terminal::new(TestBackend::new(80, 22)).unwrap();

    terminal.draw(|frame| draw(frame, &mut app)).unwrap();

    assert_eq!(app.selected_index(), 3);
    assert_eq!(app.timeline_offset(), 0);
    let buffer = terminal.backend().buffer();
    let rendered = (0..buffer.area.height).any(|y| {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect::<String>()
            .contains(image_url)
    });
    assert!(
        rendered,
        "the partially visible selected post should render"
    );
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
