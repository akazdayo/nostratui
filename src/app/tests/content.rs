use super::super::*;

#[test]
fn expands_nip08_indexed_mentions() {
    let tags = Tags::parse([["p", "abc123"], ["e", "def456"]]).unwrap();
    assert_eq!(
        expand_mentions("hello #[0], see #[1]", &tags),
        "hello @abc123, see @def456"
    );
}

#[test]
fn leaves_invalid_mentions_untouched() {
    let tags = Tags::new();
    assert_eq!(expand_mentions("hello #[9]", &tags), "hello #[9]");
    assert_eq!(expand_mentions("hello #[", &tags), "hello #[");
}

#[test]
fn renders_nip21_profile_as_named_mention() {
    let mentioned = Keys::generate().public_key();
    let uri = Nip19Profile::new(mentioned, Vec::<RelayUrl>::new())
        .to_nostr_uri()
        .unwrap();
    let event = EventBuilder::text_note(format!("hello {uri}!"))
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let mut app = App::new(true, Vec::new());
    app.profiles.insert(
        mentioned.to_hex(),
        Profile {
            display_name: Some("Alice".to_owned()),
            ..Profile::default()
        },
    );

    let rendered = app.rendered_content(&event);

    assert_eq!(
        rendered
            .parts
            .iter()
            .map(|part| part.text.as_str())
            .collect::<String>(),
        "hello @Alice!"
    );
    assert!(rendered
        .parts
        .iter()
        .any(|part| part.mention && part.text == "@Alice"));
    assert!(rendered.quote.is_none());
}

#[test]
fn recognizes_tagged_nip30_custom_emoji() {
    let event = EventBuilder::text_note("hello :blob-cat: :missing:")
        .tags([Tag::parse(["emoji", "blob-cat", "https://example.com/blob-cat.png"]).unwrap()])
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let app = App::new(true, Vec::new());

    let rendered = app.rendered_content(&event);
    let emoji = rendered
        .parts
        .iter()
        .find_map(|part| part.emoji.as_ref())
        .unwrap();

    assert_eq!(emoji.shortcode, "blob-cat");
    assert_eq!(emoji.url, "https://example.com/blob-cat.png");
    assert_eq!(
        rendered
            .parts
            .iter()
            .map(|part| part.text.as_str())
            .collect::<String>(),
        "hello :blob-cat: :missing:"
    );
}

#[test]
fn custom_emoji_download_completion_becomes_renderable() {
    let url = "https://emoji.invalid/party.png";
    let event = EventBuilder::text_note(":party:")
        .tags([Tag::parse(["emoji", "party", url]).unwrap()])
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let mut app = App::new(true, Vec::new());
    app.set_image_cache(ImageCache::kitty_for_test());
    app.on_ui_event(UiEvent::Event(Box::new(event.clone())));

    let command = app
        .image_commands()
        .into_iter()
        .find(|command| matches!(command, Command::FetchImage { .. }))
        .unwrap();
    let Command::FetchImage { key, url } = command else {
        unreachable!();
    };
    assert_eq!(url, "https://emoji.invalid/party.png");
    app.on_ui_event(UiEvent::Image {
        key,
        url,
        image: Some(::image::DynamicImage::new_rgba8(16, 16)),
    });

    let rendered = app.rendered_content(&event);
    let emoji = rendered
        .parts
        .iter()
        .find_map(|part| part.emoji.as_ref())
        .unwrap();
    assert!(app.custom_emoji_ready(emoji));
}

#[test]
fn extracts_supported_image_links_from_note_content() {
    let event = EventBuilder::text_note(
        "photo (https://cdn.example.com/Cat.JPEG?size=large). page https://example.com/about",
    )
    .sign_with_keys(&Keys::generate())
    .unwrap();
    let app = App::new(true, Vec::new());

    assert_eq!(
        app.rendered_content(&event).image_urls,
        vec!["https://cdn.example.com/Cat.JPEG?size=large"]
    );
}

#[test]
fn extracts_extensionless_nip92_image_links() {
    let image_url = "https://cdn.example.com/blob/abc123";
    let event = EventBuilder::text_note(format!("photo {image_url}"))
        .tags([
            Tag::parse(["imeta", &format!("url {image_url}"), "m image/png"]).unwrap(),
            Tag::parse(["imeta", "url http://example.com/unsafe", "m image/png"]).unwrap(),
        ])
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let app = App::new(true, Vec::new());

    assert_eq!(app.rendered_content(&event).image_urls, vec![image_url]);
}

#[test]
fn post_image_download_completion_becomes_renderable() {
    let image_url = "https://cdn.example.com/photo.webp";
    let event = EventBuilder::text_note(format!("look {image_url}"))
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let mut app = App::new(true, Vec::new());
    app.set_image_cache(ImageCache::kitty_for_test());
    app.on_ui_event(UiEvent::Event(Box::new(event)));

    let command = app
        .image_commands()
        .into_iter()
        .find(|command| matches!(command, Command::FetchImage { url, .. } if url == image_url))
        .unwrap();
    let Command::FetchImage { key, url } = command else {
        unreachable!();
    };
    app.on_ui_event(UiEvent::Image {
        key,
        url,
        image: Some(::image::DynamicImage::new_rgba8(32, 32)),
    });

    assert_eq!(app.post_image_preview_size(image_url, 24, 8), Some((16, 8)));
}

#[test]
fn custom_emoji_parser_skips_url_colons_and_finds_later_shortcode() {
    let tags = HashMap::from([(
        "party".to_owned(),
        "https://example.com/party.png".to_owned(),
    )]);

    assert_eq!(
        custom_emoji_references("https://example.com :party:", &tags),
        vec![(20, 27, "party".to_owned())]
    );
}

#[test]
fn parses_nevent_and_renders_fetched_quote() {
    let quoted = EventBuilder::text_note("quoted body")
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let uri = Nip19Event::from(&quoted).to_nostr_uri().unwrap();
    let outer = EventBuilder::text_note(format!("my comment\n{uri}"))
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let mut app = App::new(true, Vec::new());

    let pending = app.rendered_content(&outer);
    assert_eq!(
        pending
            .parts
            .iter()
            .map(|part| part.text.as_str())
            .collect::<String>(),
        "my comment"
    );
    assert_eq!(
        pending.quote.as_ref().map(|quote| quote.event_id),
        Some(quoted.id)
    );
    assert!(pending.quote.and_then(|quote| quote.event).is_none());

    app.on_ui_event(UiEvent::ReferencedEvent {
        event_id: quoted.id,
        event: Some(Box::new(quoted.clone())),
    });
    let rendered = app.rendered_content(&outer);

    assert_eq!(
        rendered
            .quote
            .and_then(|quote| quote.event)
            .map(|event| event.content),
        Some("quoted body".to_owned())
    );
}

#[test]
fn schedules_each_quoted_event_fetch_once() {
    let quoted = EventBuilder::text_note("quoted body")
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let uri = Nip19Event::from(&quoted).to_nostr_uri().unwrap();
    let outer = EventBuilder::text_note(uri)
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let mut app = App::new(true, Vec::new());
    app.on_ui_event(UiEvent::Event(Box::new(outer)));

    let first = app.reference_commands();
    let second = app.reference_commands();

    assert!(first
        .iter()
        .any(|command| matches!(command, Command::FetchEvent(id) if *id == quoted.id)));
    assert!(!second
        .iter()
        .any(|command| matches!(command, Command::FetchEvent(id) if *id == quoted.id)));
}

#[test]
fn fetches_and_exposes_the_parent_of_a_reply() {
    let parent = EventBuilder::text_note("parent body")
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let reply = EventBuilder::text_note_reply("reply body", &parent, None, None)
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let mut app = App::new(true, Vec::new());
    app.on_ui_event(UiEvent::Event(Box::new(reply.clone())));

    let pending = app.reply_display(&reply).unwrap();
    assert_eq!(pending.event_id, parent.id);
    assert!(pending.event.is_none());
    assert!(app
        .reference_commands()
        .iter()
        .any(|command| matches!(command, Command::FetchEvent(id) if *id == parent.id)));
    assert!(!app
        .reference_commands()
        .iter()
        .any(|command| matches!(command, Command::FetchEvent(id) if *id == parent.id)));

    app.on_ui_event(UiEvent::ReferencedEvent {
        event_id: parent.id,
        event: Some(Box::new(parent.clone())),
    });

    assert_eq!(
        app.reply_display(&reply)
            .and_then(|reply| reply.event)
            .map(|event| event.content),
        Some("parent body".to_owned())
    );
}

#[test]
fn reply_chains_target_the_immediate_parent() {
    let root = EventBuilder::text_note("root")
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let parent = EventBuilder::text_note_reply("parent", &root, None, None)
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let reply = EventBuilder::text_note_reply("reply", &parent, Some(&root), None)
        .sign_with_keys(&Keys::generate())
        .unwrap();

    assert_eq!(reply_target_id(&reply), Some(parent.id));
}

#[test]
fn recognizes_legacy_positional_reply_tags() {
    let root = EventBuilder::text_note("root")
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let parent = EventBuilder::text_note("parent")
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let reply = EventBuilder::text_note("legacy reply")
        .tags([Tag::event(root.id), Tag::event(parent.id)])
        .sign_with_keys(&Keys::generate())
        .unwrap();

    assert_eq!(reply_target_id(&reply), Some(parent.id));
}

#[test]
fn indexed_event_mentions_are_not_mistaken_for_replies() {
    let mentioned = EventBuilder::text_note("mentioned")
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let note = EventBuilder::text_note("see #[0]")
        .tags([Tag::event(mentioned.id)])
        .sign_with_keys(&Keys::generate())
        .unwrap();

    assert_eq!(reply_target_id(&note), None);
}

#[test]
fn renders_embedded_repost_as_the_original_note() {
    let original = EventBuilder::text_note("original body")
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let reposter = Keys::generate();
    let repost = EventBuilder::repost(&original, None)
        .sign_with_keys(&reposter)
        .unwrap();
    let mut app = App::new(true, Vec::new());

    app.on_ui_event(UiEvent::Event(Box::new(repost.clone())));
    let display = app.display_event(&repost);

    assert_eq!(display.event.id, original.id);
    assert_eq!(display.event.content, "original body");
    assert_eq!(display.reposted_by, Some(reposter.public_key()));
}

#[test]
fn fetches_and_renders_repost_with_missing_embedded_event() {
    let original = EventBuilder::text_note("fetched body")
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let reposter = Keys::generate();
    let repost = EventBuilder::new(Kind::Repost, "")
        .tags([Tag::event(original.id), Tag::public_key(original.pubkey)])
        .sign_with_keys(&reposter)
        .unwrap();
    let mut app = App::new(true, Vec::new());
    app.on_ui_event(UiEvent::Event(Box::new(repost.clone())));

    assert!(app
        .reference_commands()
        .iter()
        .any(|command| matches!(command, Command::FetchEvent(id) if *id == original.id)));

    app.on_ui_event(UiEvent::ReferencedEvent {
        event_id: original.id,
        event: Some(Box::new(original.clone())),
    });
    let display = app.display_event(&repost);

    assert_eq!(display.event.id, original.id);
    assert_eq!(display.event.content, "fetched body");
    assert_eq!(display.reposted_by, Some(reposter.public_key()));
}

#[test]
fn renders_generic_repost_and_targets_original_for_actions() {
    let original = EventBuilder::new(Kind::LongFormTextNote, "long-form body")
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let reposter = Keys::generate();
    let repost = EventBuilder::repost(&original, None)
        .sign_with_keys(&reposter)
        .unwrap();
    assert_eq!(repost.kind, Kind::GenericRepost);

    let mut app = App::new(false, Vec::new());
    app.select_tab(TimelineTab::Global);
    app.on_ui_event(UiEvent::Event(Box::new(repost.clone())));

    let display = app.display_event(&repost);
    assert_eq!(display.event.id, original.id);
    assert_eq!(display.reposted_by, Some(reposter.public_key()));

    let command = app
        .on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
        .unwrap();
    assert!(matches!(
        command,
        Command::React { event, reaction } if event.id == original.id && reaction == "+"
    ));
}

#[test]
fn reaction_summary_is_stable() {
    let mut reactions = Reactions::default();
    reactions.add("🔥");
    reactions.add("+");
    reactions.add("🔥");
    assert_eq!(reactions.summary(), "+1 🔥2");
}

#[test]
fn locally_forwarded_reaction_is_rendered_once_after_a_relay_echo() {
    let target = EventBuilder::text_note("target")
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let reaction = EventBuilder::reaction(&target, "+")
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let mut app = App::new(true, Vec::new());

    app.on_ui_event(UiEvent::Event(Box::new(target.clone())));
    app.on_ui_event(UiEvent::Event(Box::new(reaction.clone())));
    app.on_ui_event(UiEvent::Event(Box::new(reaction)));

    let rendered = app
        .rendered_reactions(&target)
        .into_iter()
        .map(|part| part.text)
        .collect::<String>();
    assert_eq!(rendered, "+1");
}

#[test]
fn custom_reaction_is_exposed_as_a_renderable_emoji() {
    let event = EventBuilder::text_note("target")
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let mut reactions = Reactions::default();
    reactions.add(":party:");
    reactions.add(":party:");
    reactions.custom_emojis.insert(
        "party".to_owned(),
        "https://example.com/party.png".to_owned(),
    );
    let mut app = App::new(true, Vec::new());
    app.reactions.insert(event.id.to_string(), reactions);

    let rendered = app.rendered_reactions(&event);

    assert_eq!(
        rendered
            .iter()
            .find_map(|part| part.emoji.as_ref())
            .map(|emoji| emoji.shortcode.as_str()),
        Some("party")
    );
    assert_eq!(rendered.len(), 2);
    assert_eq!(rendered[1].text, "2");
    assert!(rendered[1].emoji.is_none());
}

#[test]
fn nip05_root_identifier_is_displayed_as_domain_only() {
    let pubkey = Keys::generate().public_key();
    let key = pubkey.to_hex();
    let address = "_@example.com".to_owned();
    let mut app = App::new(true, Vec::new());
    app.profiles.insert(
        key.clone(),
        Profile {
            nip05: Some(address.clone()),
            ..Profile::default()
        },
    );

    assert_eq!(app.nip05_label(&pubkey).as_deref(), Some("? example.com"));

    app.verified_nip05.insert((key, address));
    assert_eq!(app.nip05_label(&pubkey).as_deref(), Some("✓ example.com"));
}
