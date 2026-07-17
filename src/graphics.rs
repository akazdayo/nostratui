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
    pixel_size: Option<(u32, u32)>,
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

        let pixel_size = image.as_ref().map(|image| (image.width(), image.height()));
        let protocol = image.and_then(|image| {
            self.picker
                .as_ref()
                .map(|picker| picker.new_resize_protocol(image))
        });
        self.remove_entry(&key);
        self.entries.insert(
            key.clone(),
            CacheEntry {
                url,
                protocol,
                pixel_size,
            },
        );
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

    /// Returns a tight cell rectangle preserving the image's pixel aspect
    /// ratio and accounting for non-square terminal cells.
    pub fn preview_size(
        &self,
        key: &str,
        url: &str,
        max_width: u16,
        max_height: u16,
    ) -> Option<(u16, u16)> {
        if max_width == 0 || max_height == 0 {
            return None;
        }
        let entry = self.entries.get(key)?;
        if entry.url != url || entry.protocol.is_none() {
            return None;
        }
        let (image_width, image_height) = entry.pixel_size?;
        let (cell_width, cell_height) = self.picker.as_ref()?.font_size();
        if image_width == 0 || image_height == 0 || cell_width == 0 || cell_height == 0 {
            return None;
        }

        let height = div_ceil(
            u64::from(image_height) * u64::from(max_width) * u64::from(cell_width),
            u64::from(image_width) * u64::from(cell_height),
        )
        .max(1);
        if height <= u64::from(max_height) {
            return Some((max_width, height as u16));
        }

        let width = div_ceil(
            u64::from(image_width) * u64::from(max_height) * u64::from(cell_height),
            u64::from(image_height) * u64::from(cell_width),
        )
        .clamp(1, u64::from(max_width));
        Some((width as u16, max_height))
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

fn div_ceil(numerator: u64, denominator: u64) -> u64 {
    numerator / denominator + u64::from(!numerator.is_multiple_of(denominator))
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
mod tests;
