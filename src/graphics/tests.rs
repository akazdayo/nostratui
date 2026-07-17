use ratatui::{buffer::Buffer, layout::Rect, widgets::StatefulWidget};
use ratatui_image::StatefulImage;

use super::*;

fn enabled_cache() -> ImageCache {
    ImageCache::kitty_for_test()
}

#[test]
fn disabled_cache_never_schedules_downloads() {
    let mut cache = ImageCache::default();
    assert!(!cache.request("pubkey", "https://example.com/avatar.png"));
}

#[test]
fn enabled_cache_rejects_non_https_custom_emoji() {
    let mut cache = enabled_cache();
    assert!(!cache.request("emoji:party", "http://example.com/party.png"));
}

#[test]
fn kitty_environment_is_a_capability_detection_fallback() {
    assert!(kitty_terminal_from_env_values(
        Some("xterm-kitty"),
        None,
        None
    ));
    assert!(kitty_terminal_from_env_values(
        Some("tmux-256color"),
        None,
        Some("1")
    ));
    assert!(kitty_terminal_from_env_values(
        Some("dumb"),
        Some("ghostty"),
        None
    ));
    assert!(!kitty_terminal_from_env_values(
        Some("xterm-256color"),
        None,
        None
    ));
}

#[test]
fn delete_sequence_targets_each_image_id() {
    let mut output = Vec::new();
    delete_kitty_images(&mut output, &[12, 34]).unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("a=d,d=I,i=12"));
    assert!(output.contains("a=d,d=I,i=34"));
}

#[test]
fn pending_downloads_are_bounded() {
    let mut cache = enabled_cache();
    for index in 0..MAX_PENDING_IMAGES {
        assert!(cache.request(
            &format!("pubkey-{index}"),
            &format!("https://example.com/{index}.png")
        ));
    }
    assert!(!cache.request("one-too-many", "https://example.com/full.png"));
    assert_eq!(cache.pending.len(), MAX_PENDING_IMAGES);
}

#[test]
fn completed_custom_emoji_is_ready_for_inline_rendering() {
    let mut cache = enabled_cache();
    let key = "emoji:https://example.com/party.png";
    let url = "https://example.com/party.png";
    assert!(cache.request(key, url));

    cache.complete(
        key.to_owned(),
        url.to_owned(),
        Some(DynamicImage::new_rgba8(16, 16)),
    );

    assert!(cache.has_protocol(key, url));
    let mut buffer = Buffer::empty(Rect::new(0, 0, 2, 1));
    let protocol = cache.protocol_mut(key, url).unwrap();
    StatefulImage::default().render(Rect::new(0, 0, 2, 1), &mut buffer, protocol);
    assert!(buffer[(0, 0)].symbol().contains("\x1b_G"));
}

#[test]
fn preview_size_uses_image_and_terminal_cell_aspect_ratios() {
    let mut cache = enabled_cache();
    let cases = [
        ("wide", (128, 72), (24, 7)),
        ("square", (128, 128), (16, 8)),
        ("portrait", (64, 128), (8, 8)),
    ];

    for (name, (width, height), expected) in cases {
        let key = format!("post:{name}");
        let url = format!("https://example.com/{name}.png");
        assert!(cache.request(&key, &url));
        cache.complete(
            key.clone(),
            url.clone(),
            Some(DynamicImage::new_rgba8(width, height)),
        );

        assert_eq!(cache.preview_size(&key, &url, 24, 8), Some(expected));
    }
}

#[test]
fn decoded_cache_evicts_and_releases_kitty_ids() {
    let mut cache = enabled_cache();
    for index in 0..MAX_CACHED_IMAGES + 3 {
        let pubkey = format!("pubkey-{index}");
        let url = format!("https://example.com/{index}.png");
        assert!(cache.request(&pubkey, &url));
        cache.complete(pubkey, url, Some(DynamicImage::new_rgba8(16, 16)));
    }

    assert_eq!(cache.entries.len(), MAX_CACHED_IMAGES);
    assert_eq!(cache.deleted_ids.len(), 3);
}
