use std::{
    collections::{HashMap, VecDeque},
    env,
    fmt::Write as _,
    io::{self, Write},
};

use image::DynamicImage;
use ratatui_image::{
    picker::{cap_parser::Parser, Picker, ProtocolType},
    protocol::{StatefulProtocol, StatefulProtocolType},
};

const MAX_CACHED_IMAGES: usize = 64;
const MAX_PENDING_IMAGES: usize = 8;

struct CacheEntry {
    url: String,
    protocol: Option<StatefulProtocol>,
}

/// A bounded terminal-image cache. `None` protocols are negative cache entries.
///
/// Kitty keeps transmitted image data in the terminal, so evicted IDs are
/// returned to the caller and explicitly deleted between terminal frames.
#[derive(Default)]
pub struct ImageCache {
    picker: Option<Picker>,
    entries: HashMap<String, CacheEntry>,
    least_recently_used: VecDeque<String>,
    pending: HashMap<String, String>,
    deleted_ids: Vec<u32>,
}

impl ImageCache {
    /// Detects terminal capabilities. Images are enabled only when Kitty is
    /// selected; other terminals retain the text-only UI.
    pub fn detect() -> Result<Self, ratatui_image::errors::Errors> {
        let kitty_from_env = kitty_terminal_from_env_values(
            env::var("TERM").ok().as_deref(),
            env::var("TERM_PROGRAM").ok().as_deref(),
            env::var("KITTY_WINDOW_ID").ok().as_deref(),
        );
        let mut picker = match Picker::from_query_stdio() {
            Ok(picker) => picker,
            Err(_) if kitty_from_env => Picker::from_fontsize((10, 20)),
            Err(error) => return Err(error),
        };
        // Some Kitty-compatible terminals (notably Ghostty in mediated PTYs)
        // answer the graphics query but not
        // the cell-size query. ratatui-image then falls back to Halfblocks and
        // loses the successful graphics result, so use Kitty's own environment
        // markers as a conservative fallback.
        if picker.protocol_type() != ProtocolType::Kitty && kitty_from_env {
            picker.set_protocol_type(ProtocolType::Kitty);
        }
        if picker.protocol_type() != ProtocolType::Kitty {
            return Ok(Self::default());
        }
        Ok(Self {
            picker: Some(picker),
            ..Self::default()
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.picker.is_some()
    }

    #[cfg(test)]
    pub fn kitty_for_test() -> Self {
        let mut picker = Picker::from_fontsize((10, 20));
        picker.set_protocol_type(ProtocolType::Kitty);
        Self {
            picker: Some(picker),
            ..Self::default()
        }
    }

    pub fn request(&mut self, key: &str, url: &str) -> bool {
        if !self.is_enabled()
            || url.len() > 2_048
            || !url
                .get(..8)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
        {
            return false;
        }
        if self.entries.get(key).is_some_and(|entry| entry.url == url) {
            self.touch(key);
            return false;
        }
        if self.pending.get(key).is_some_and(|pending| pending == url)
            || self.pending.len() >= MAX_PENDING_IMAGES
        {
            return false;
        }

        // At most one request per logical image. If its URL changed while the
        // old image was loading, the new URL is picked up after it completes.
        if self.pending.contains_key(key) {
            return false;
        }
        self.remove_entry(key);
        self.pending.insert(key.to_owned(), url.to_owned());
        true
    }

    pub fn complete(&mut self, key: String, url: String, image: Option<DynamicImage>) {
        if self.pending.get(&key).is_none_or(|pending| pending != &url) {
            return;
        }
        self.pending.remove(&key);

        let protocol = image.and_then(|image| {
            self.picker
                .as_ref()
                .map(|picker| picker.new_resize_protocol(image))
        });
        self.remove_entry(&key);
        self.entries
            .insert(key.clone(), CacheEntry { url, protocol });
        self.touch(&key);
        self.evict_to_limit();
    }

    pub fn source_updated(&mut self, key: &str, current_url: Option<&str>) {
        if self
            .entries
            .get(key)
            .is_some_and(|entry| Some(entry.url.as_str()) != current_url)
        {
            self.remove_entry(key);
        }
    }

    pub fn has_protocol(&self, key: &str, url: &str) -> bool {
        self.entries
            .get(key)
            .is_some_and(|entry| entry.url == url && entry.protocol.is_some())
    }

    pub fn protocol_mut(&mut self, key: &str, url: &str) -> Option<&mut StatefulProtocol> {
        if !self
            .entries
            .get(key)
            .is_some_and(|entry| entry.url == url && entry.protocol.is_some())
        {
            return None;
        }
        self.touch(key);
        self.entries
            .get_mut(key)
            .and_then(|entry| entry.protocol.as_mut())
    }

    pub fn take_deleted_ids(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.deleted_ids)
    }

    pub fn clear(&mut self) {
        let keys: Vec<_> = self.entries.keys().cloned().collect();
        for key in keys {
            self.remove_entry(&key);
        }
        self.pending.clear();
        self.least_recently_used.clear();
    }

    fn touch(&mut self, pubkey: &str) {
        self.least_recently_used.retain(|key| key != pubkey);
        self.least_recently_used.push_back(pubkey.to_owned());
    }

    fn evict_to_limit(&mut self) {
        while self.entries.len() > MAX_CACHED_IMAGES {
            let Some(key) = self.least_recently_used.pop_front() else {
                break;
            };
            self.remove_entry(&key);
        }
    }

    fn remove_entry(&mut self, key: &str) {
        self.least_recently_used.retain(|cached| cached != key);
        let Some(entry) = self.entries.remove(key) else {
            return;
        };
        if let Some(id) = entry.protocol.as_ref().and_then(kitty_image_id) {
            self.deleted_ids.push(id);
        }
    }
}

fn kitty_terminal_from_env_values(
    term: Option<&str>,
    term_program: Option<&str>,
    kitty_window_id: Option<&str>,
) -> bool {
    kitty_window_id.is_some_and(|value| !value.is_empty())
        || term.is_some_and(|value| matches!(value, "xterm-kitty" | "xterm-ghostty"))
        || term_program.is_some_and(|value| {
            value.eq_ignore_ascii_case("kitty") || value.eq_ignore_ascii_case("ghostty")
        })
}

fn kitty_image_id(protocol: &StatefulProtocol) -> Option<u32> {
    match protocol.protocol_type() {
        StatefulProtocolType::Kitty(kitty) => Some(kitty.unique_id),
        _ => None,
    }
}

pub fn delete_kitty_images(writer: &mut impl Write, ids: &[u32]) -> io::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let is_tmux = env::var("TERM").is_ok_and(|term| term.starts_with("tmux"))
        || env::var("TERM_PROGRAM").is_ok_and(|program| program == "tmux");
    let (start, escape, end) = Parser::escape_tmux(is_tmux);
    let mut sequence = String::new();
    for id in ids {
        write!(sequence, "{start}{escape}_Ga=d,d=I,i={id}{escape}\\{end}")
            .expect("writing to a String cannot fail");
    }
    writer.write_all(sequence.as_bytes())?;
    writer.flush()
}

pub fn delete_all_kitty_images(writer: &mut impl Write) -> io::Result<()> {
    writer.write_all(b"\x1b_Ga=d,d=A\x1b\\")?;
    writer.flush()
}

#[cfg(test)]
mod tests {
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
}
