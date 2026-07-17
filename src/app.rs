use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use nostr_sdk::prelude::*;
use serde::Deserialize;

use crate::{graphics::ImageCache, network::UiEvent};

#[derive(Debug)]
pub enum Command {
    Publish {
        content: String,
        reply_to: Option<Box<Event>>,
    },
    React {
        event: Box<Event>,
        reaction: String,
    },
    Repost(Box<Event>),
    VerifyNip05 {
        pubkey: String,
        address: String,
    },
    FetchProfile(PublicKey),
    FetchEvent(EventId),
    FetchImage {
        key: String,
        url: String,
    },
    Quit,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Profile {
    pub name: Option<String>,
    pub display_name: Option<String>,
    pub nip05: Option<String>,
    pub about: Option<String>,
    pub picture: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Reactions {
    pub likes: usize,
    pub dislikes: usize,
    pub custom: HashMap<String, usize>,
    pub custom_emojis: HashMap<String, String>,
}

impl Reactions {
    fn add(&mut self, value: &str) {
        match value {
            "+" | "" => self.likes += 1,
            "-" => self.dislikes += 1,
            other => *self.custom.entry(other.to_owned()).or_default() += 1,
        }
    }

    pub fn summary(&self) -> String {
        let mut values = Vec::new();
        if self.likes > 0 {
            values.push(format!("+{}", self.likes));
        }
        if self.dislikes > 0 {
            values.push(format!("-{}", self.dislikes));
        }
        let mut custom: Vec<_> = self.custom.iter().collect();
        custom.sort_by(|a, b| a.0.cmp(b.0));
        values.extend(
            custom
                .into_iter()
                .map(|(emoji, count)| format!("{emoji}{count}")),
        );
        values.join(" ")
    }
}

#[derive(Debug, Clone)]
pub enum InputMode {
    Normal,
    Compose { reply_to: Option<Box<Event>> },
    Reaction { event: Box<Event> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineTab {
    Following,
    Global,
}

impl TimelineTab {
    pub fn label(self) -> &'static str {
        match self {
            Self::Following => "Following",
            Self::Global => "Global",
        }
    }
}

#[derive(Default)]
struct TimelineState {
    events: Vec<Event>,
    selected: usize,
    live: bool,
    unseen: usize,
    offset: usize,
}

impl TimelineState {
    fn new() -> Self {
        Self {
            live: true,
            ..Self::default()
        }
    }
}

fn add_timeline_event(timeline: &mut TimelineState, event: Event) {
    let selected_id = timeline.events.get(timeline.selected).map(|event| event.id);
    let selected_viewport_row = timeline.selected.saturating_sub(timeline.offset);
    let incoming_id = event.id;
    timeline.events.push(event);
    timeline
        .events
        .sort_by_key(|event| Reverse(event.created_at));
    if timeline.live {
        timeline.selected = 0;
        resume_timeline(timeline);
    } else {
        timeline.selected = selected_id
            .and_then(|id| timeline.events.iter().position(|event| event.id == id))
            .unwrap_or_else(|| {
                timeline
                    .selected
                    .min(timeline.events.len().saturating_sub(1))
            });
        timeline.offset = timeline.selected.saturating_sub(selected_viewport_row);
        if timeline
            .events
            .iter()
            .position(|event| event.id == incoming_id)
            .is_some_and(|position| position < timeline.selected)
        {
            timeline.unseen += 1;
        }
    }
}

fn replace_timeline_events(timeline: &mut TimelineState, mut events: Vec<Event>) {
    let selected_id = timeline.events.get(timeline.selected).map(|event| event.id);
    events.sort_by_key(|event| Reverse(event.created_at));
    timeline.events = events;
    if timeline.live {
        timeline.selected = 0;
        timeline.offset = 0;
    } else {
        timeline.selected = selected_id
            .and_then(|id| timeline.events.iter().position(|event| event.id == id))
            .unwrap_or_else(|| {
                timeline
                    .selected
                    .min(timeline.events.len().saturating_sub(1))
            });
        timeline.offset = timeline.offset.min(timeline.selected);
    }
}

fn resume_timeline(timeline: &mut TimelineState) {
    timeline.live = true;
    timeline.unseen = 0;
    timeline.offset = 0;
}

pub struct App {
    pub profiles: HashMap<String, Profile>,
    pub verified_nip05: HashSet<(String, String)>,
    pub reactions: HashMap<String, Reactions>,
    pub mode: InputMode,
    pub input: String,
    pub detail: bool,
    pub status: String,
    pub identity: String,
    pub read_only: bool,
    settings_open: bool,
    relays: Vec<String>,
    active_tab: TimelineTab,
    following_available: bool,
    following_pubkeys: HashSet<PublicKey>,
    following_timeline: TimelineState,
    global_timeline: TimelineState,
    seen: HashSet<String>,
    pending_nip05: HashSet<(String, String)>,
    pending_profiles: HashSet<String>,
    referenced_events: HashMap<EventId, Event>,
    requested_references: HashSet<EventId>,
    pending_references: HashSet<EventId>,
    profile_timestamps: HashMap<String, Timestamp>,
    images: ImageCache,
}

impl App {
    pub fn new(read_only: bool, relays: Vec<String>) -> Self {
        Self {
            profiles: HashMap::new(),
            verified_nip05: HashSet::new(),
            reactions: HashMap::new(),
            mode: InputMode::Normal,
            input: String::new(),
            detail: false,
            status: "starting…".to_owned(),
            identity: "loading…".to_owned(),
            read_only,
            settings_open: false,
            relays,
            active_tab: if read_only {
                TimelineTab::Global
            } else {
                TimelineTab::Following
            },
            following_available: !read_only,
            following_pubkeys: HashSet::new(),
            following_timeline: TimelineState::new(),
            global_timeline: TimelineState::new(),
            seen: HashSet::new(),
            pending_nip05: HashSet::new(),
            pending_profiles: HashSet::new(),
            referenced_events: HashMap::new(),
            requested_references: HashSet::new(),
            pending_references: HashSet::new(),
            profile_timestamps: HashMap::new(),
            images: ImageCache::default(),
        }
    }

    pub fn set_image_cache(&mut self, images: ImageCache) {
        self.images = images;
    }

    fn timeline_state(&self) -> &TimelineState {
        match self.active_tab {
            TimelineTab::Following => &self.following_timeline,
            TimelineTab::Global => &self.global_timeline,
        }
    }

    fn timeline_state_mut(&mut self) -> &mut TimelineState {
        match self.active_tab {
            TimelineTab::Following => &mut self.following_timeline,
            TimelineTab::Global => &mut self.global_timeline,
        }
    }

    pub fn select_tab(&mut self, tab: TimelineTab) {
        self.active_tab = tab;
    }

    pub fn active_tab(&self) -> TimelineTab {
        self.active_tab
    }

    pub fn following_available(&self) -> bool {
        self.following_available
    }

    pub fn timeline(&self) -> &[Event] {
        &self.timeline_state().events
    }

    pub fn selected_index(&self) -> usize {
        self.timeline_state().selected
    }

    pub fn timeline_count(&self, tab: TimelineTab) -> usize {
        match tab {
            TimelineTab::Following => self.following_timeline.events.len(),
            TimelineTab::Global => self.global_timeline.events.len(),
        }
    }

    pub fn sync_timeline_viewport(&mut self, offset: usize) {
        let timeline = self.timeline_state_mut();
        timeline.offset = offset;
        if offset == 0 {
            resume_timeline(timeline);
        } else {
            timeline.live = false;
        }
    }

    pub fn timeline_offset(&self) -> usize {
        self.timeline_state().offset
    }

    pub fn is_live(&self) -> bool {
        self.timeline_state().live
    }

    pub fn unseen_count(&self) -> usize {
        self.timeline_state().unseen
    }

    pub fn settings_open(&self) -> bool {
        self.settings_open
    }

    pub fn relays(&self) -> &[String] {
        &self.relays
    }
}

mod content;
mod events;
mod input;
mod resources;

#[cfg(test)]
use content::custom_emoji_references;
use content::{
    avatar_image_key, content_entities, embedded_repost, emoji_image_key, is_repost,
    reply_target_id, repost_target_id, used_custom_emojis, ContentEntityKind,
};
pub use content::{expand_mentions, CustomEmoji, QuoteDisplay, RenderedPart, ReplyDisplay};
#[cfg(test)]
mod tests;
