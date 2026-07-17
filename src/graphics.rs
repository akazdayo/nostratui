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

const MAX_CACHED_AVATARS: usize = 32;
const MAX_PENDING_AVATARS: usize = 4;

struct CacheEntry {
    url: String,
    protocol: Option<StatefulProtocol>,
}

/// A bounded avatar cache. `None` protocols are negative cache entries.
///
/// Kitty keeps transmitted image data in the terminal, so evicted IDs are
/// returned to the caller and explicitly deleted between terminal frames.
#[derive(Default)]
pub struct AvatarCache {
    picker: Option<Picker>,
    entries: HashMap<String, CacheEntry>,
    least_recently_used: VecDeque<String>,
    pending: HashMap<String, String>,
    deleted_ids: Vec<u32>,
}

impl AvatarCache {
    /// Detects terminal capabilities. Avatars are enabled only when Kitty is
    /// selected; other terminals retain the text-only UI.
    pub fn detect() -> Result<Self, ratatui_image::errors::Errors> {
        let picker = Picker::from_query_stdio()?;
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

    pub fn request(&mut self, pubkey: &str, url: &str) -> bool {
        if !self.is_enabled()
            || url.len() > 2_048
            || !url
                .get(..8)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
        {
            return false;
        }
        if self
            .entries
            .get(pubkey)
            .is_some_and(|entry| entry.url == url)
        {
            self.touch(pubkey);
            return false;
        }
        if self
            .pending
            .get(pubkey)
            .is_some_and(|pending| pending == url)
            || self.pending.len() >= MAX_PENDING_AVATARS
        {
            return false;
        }

        // At most one request per author. If a profile changed while its old
        // image was loading, the new URL will be picked up after it completes.
        if self.pending.contains_key(pubkey) {
            return false;
        }
        self.remove_entry(pubkey);
        self.pending.insert(pubkey.to_owned(), url.to_owned());
        true
    }

    pub fn complete(&mut self, pubkey: String, url: String, image: Option<DynamicImage>) {
        if self
            .pending
            .get(&pubkey)
            .is_none_or(|pending| pending != &url)
        {
            return;
        }
        self.pending.remove(&pubkey);

        let protocol = image.and_then(|image| {
            self.picker
                .as_ref()
                .map(|picker| picker.new_resize_protocol(image))
        });
        self.remove_entry(&pubkey);
        self.entries
            .insert(pubkey.clone(), CacheEntry { url, protocol });
        self.touch(&pubkey);
        self.evict_to_limit();
    }

    pub fn discard_completion(&mut self, pubkey: &str, url: &str) {
        if self
            .pending
            .get(pubkey)
            .is_some_and(|pending| pending == url)
        {
            self.pending.remove(pubkey);
        }
    }

    pub fn profile_updated(&mut self, pubkey: &str, current_url: Option<&str>) {
        if self
            .entries
            .get(pubkey)
            .is_some_and(|entry| Some(entry.url.as_str()) != current_url)
        {
            self.remove_entry(pubkey);
        }
    }

    pub fn protocol_mut(&mut self, pubkey: &str, url: &str) -> Option<&mut StatefulProtocol> {
        if !self
            .entries
            .get(pubkey)
            .is_some_and(|entry| entry.url == url && entry.protocol.is_some())
        {
            return None;
        }
        self.touch(pubkey);
        self.entries
            .get_mut(pubkey)
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
        while self.entries.len() > MAX_CACHED_AVATARS {
            let Some(pubkey) = self.least_recently_used.pop_front() else {
                break;
            };
            self.remove_entry(&pubkey);
        }
    }

    fn remove_entry(&mut self, pubkey: &str) {
        self.least_recently_used.retain(|key| key != pubkey);
        let Some(entry) = self.entries.remove(pubkey) else {
            return;
        };
        if let Some(id) = entry.protocol.as_ref().and_then(kitty_image_id) {
            self.deleted_ids.push(id);
        }
    }
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
    use super::*;

    fn enabled_cache() -> AvatarCache {
        let mut picker = Picker::from_fontsize((10, 20));
        picker.set_protocol_type(ProtocolType::Kitty);
        AvatarCache {
            picker: Some(picker),
            ..AvatarCache::default()
        }
    }

    #[test]
    fn disabled_cache_never_schedules_downloads() {
        let mut cache = AvatarCache::default();
        assert!(!cache.request("pubkey", "https://example.com/avatar.png"));
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
        for index in 0..MAX_PENDING_AVATARS {
            assert!(cache.request(
                &format!("pubkey-{index}"),
                &format!("https://example.com/{index}.png")
            ));
        }
        assert!(!cache.request("one-too-many", "https://example.com/full.png"));
        assert_eq!(cache.pending.len(), MAX_PENDING_AVATARS);
    }

    #[test]
    fn decoded_cache_evicts_and_releases_kitty_ids() {
        let mut cache = enabled_cache();
        for index in 0..MAX_CACHED_AVATARS + 3 {
            let pubkey = format!("pubkey-{index}");
            let url = format!("https://example.com/{index}.png");
            assert!(cache.request(&pubkey, &url));
            cache.complete(pubkey, url, Some(DynamicImage::new_rgba8(16, 16)));
        }

        assert_eq!(cache.entries.len(), MAX_CACHED_AVATARS);
        assert_eq!(cache.deleted_ids.len(), 3);
    }
}
