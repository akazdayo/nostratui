use super::*;

mod content;

#[test]
fn compose_cursor_moves_and_edits_at_grapheme_boundaries() {
    let mut app = App::new(false, Vec::new());
    let press = |app: &mut App, code| {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    };

    press(&mut app, KeyCode::Char('i'));
    for character in ['a', '日', 'b'] {
        press(&mut app, KeyCode::Char(character));
    }
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Char('X'));

    assert_eq!(app.input, "aX日b");
    assert_eq!(app.input_cursor(), "aX".len());

    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Backspace);
    assert_eq!(app.input, "aXb");
    assert_eq!(app.input_cursor(), "aX".len());

    press(&mut app, KeyCode::Delete);
    assert_eq!(app.input, "aX");
}

#[test]
fn compose_cursor_moves_between_lines_and_to_line_edges() {
    let mut app = App::new(false, Vec::new());
    app.mode = InputMode::Compose { reply_to: None };
    app.input = "abc\n日本語".to_owned();
    app.input_cursor = app.input.len();

    app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(app.input_cursor(), 3);
    app.on_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
    assert_eq!(app.input_cursor(), 0);
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.input_cursor(), 4);
    app.on_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    assert_eq!(app.input_cursor(), app.input.len());
}
#[test]
fn incoming_notes_do_not_change_the_selected_event() {
    let keys = Keys::generate();
    let note = |content, timestamp| {
        EventBuilder::text_note(content)
            .custom_created_at(Timestamp::from_secs(timestamp))
            .sign_with_keys(&keys)
            .unwrap()
    };
    let mut app = App::new(true, Vec::new());
    let selected = note("selected", 100);
    let selected_id = selected.id;

    app.add_event(selected);
    app.add_event(note("older", 50));
    app.global_timeline.selected = 0;
    app.global_timeline.live = false;
    app.add_event(note("incoming", 200));

    assert_eq!(app.selected_index(), 1);
    assert_eq!(app.unseen_count(), 1);
    assert_eq!(
        app.selected_event().map(|event| event.id),
        Some(selected_id)
    );
}

#[test]
fn live_timeline_follows_newest_and_g_resumes_it() {
    let keys = Keys::generate();
    let note = |content, timestamp| {
        EventBuilder::text_note(content)
            .custom_created_at(Timestamp::from_secs(timestamp))
            .sign_with_keys(&keys)
            .unwrap()
    };
    let mut app = App::new(true, Vec::new());
    app.add_event(note("first", 100));
    app.sync_timeline_viewport(1);
    app.add_event(note("new while paused", 200));

    assert!(!app.is_live());
    assert_eq!(app.unseen_count(), 1);
    assert_eq!(
        app.selected_event().map(|event| event.content.as_str()),
        Some("first")
    );

    app.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    app.add_event(note("new while live", 300));

    assert!(app.is_live());
    assert_eq!(app.unseen_count(), 0);
    assert_eq!(
        app.selected_event().map(|event| event.content.as_str()),
        Some("new while live")
    );
}

#[test]
fn timeline_mode_tracks_the_rendered_viewport() {
    let mut app = App::new(true, Vec::new());

    app.sync_timeline_viewport(1);
    assert!(!app.is_live());

    app.sync_timeline_viewport(0);
    assert!(app.is_live());
    assert_eq!(app.unseen_count(), 0);
}

#[test]
fn detail_view_uses_the_same_timeline_mode_rules() {
    let event = EventBuilder::text_note("selected")
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let mut app = App::new(true, Vec::new());
    app.add_event(event);

    app.on_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
    app.sync_timeline_viewport(0);
    assert!(app.detail);
    assert!(app.is_live());

    app.sync_timeline_viewport(1);
    assert!(!app.is_live());
}

#[test]
fn locally_published_note_and_relay_echo_are_shown_once() {
    let event = EventBuilder::text_note("my post")
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let event_id = event.id;
    let mut app = App::new(false, Vec::new());
    app.select_tab(TimelineTab::Global);

    app.on_ui_event(UiEvent::Event(Box::new(event.clone())));
    app.on_ui_event(UiEvent::Event(Box::new(event)));

    assert_eq!(app.timeline().len(), 1);
    assert_eq!(app.selected_event().map(|event| event.id), Some(event_id));
    assert_eq!(
        app.selected_event().map(|event| event.content.as_str()),
        Some("my post")
    );
}

#[test]
fn notes_are_routed_to_global_and_following_timelines() {
    let followed = Keys::generate();
    let stranger = Keys::generate();
    let followed_note = EventBuilder::text_note("followed")
        .sign_with_keys(&followed)
        .unwrap();
    let stranger_note = EventBuilder::text_note("global only")
        .sign_with_keys(&stranger)
        .unwrap();
    let mut app = App::new(false, Vec::new());

    app.on_ui_event(UiEvent::FollowList(vec![followed.public_key()]));
    app.on_ui_event(UiEvent::Event(Box::new(followed_note)));
    app.on_ui_event(UiEvent::Event(Box::new(stranger_note)));

    assert_eq!(app.timeline_count(TimelineTab::Following), 1);
    assert_eq!(app.timeline_count(TimelineTab::Global), 2);
    assert_eq!(app.timeline()[0].content, "followed");

    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.active_tab(), TimelineTab::Global);
    assert_eq!(app.timeline().len(), 2);
}

#[test]
fn changing_follow_list_rebuilds_the_following_timeline() {
    let first = Keys::generate();
    let second = Keys::generate();
    let mut app = App::new(false, Vec::new());
    app.on_ui_event(UiEvent::Event(Box::new(
        EventBuilder::text_note("first")
            .sign_with_keys(&first)
            .unwrap(),
    )));
    app.on_ui_event(UiEvent::Event(Box::new(
        EventBuilder::text_note("second")
            .sign_with_keys(&second)
            .unwrap(),
    )));

    app.on_ui_event(UiEvent::FollowList(vec![first.public_key()]));
    assert_eq!(app.timeline().len(), 1);
    assert_eq!(app.timeline()[0].content, "first");

    app.on_ui_event(UiEvent::FollowList(vec![second.public_key()]));
    assert_eq!(app.timeline().len(), 1);
    assert_eq!(app.timeline()[0].content, "second");
}

#[test]
fn m_toggles_the_settings_modal() {
    let mut app = App::new(true, vec!["wss://relay.example.com".to_owned()]);

    app.on_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
    assert!(app.settings_open());
    assert_eq!(app.relays(), ["wss://relay.example.com"]);

    app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.settings_open());
}
