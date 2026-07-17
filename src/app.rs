use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use nostr_sdk::prelude::*;
use serde::Deserialize;

use crate::{graphics::AvatarCache, network::UiEvent};

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
    FetchAvatar {
        pubkey: String,
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
    avatars: AvatarCache,
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
            avatars: AvatarCache::default(),
        }
    }

    pub fn set_avatar_cache(&mut self, avatars: AvatarCache) {
        self.avatars = avatars;
    }

    pub fn on_ui_event(&mut self, message: UiEvent) -> Option<Command> {
        match message {
            UiEvent::Event(event) => self.add_event(*event),
            UiEvent::FollowList(pubkeys) => {
                self.following_available = true;
                self.following_pubkeys = pubkeys.into_iter().collect();
                let events = self
                    .global_timeline
                    .events
                    .iter()
                    .filter(|event| self.following_pubkeys.contains(&event.pubkey))
                    .cloned()
                    .collect();
                replace_timeline_events(&mut self.following_timeline, events);
                None
            }
            UiEvent::Profile { pubkey, content } => {
                if self.profiles.contains_key(&pubkey) {
                    self.pending_profiles.remove(&pubkey);
                    None
                } else {
                    self.apply_profile(pubkey, &content)
                }
            }
            UiEvent::ReferencedEvent { event_id, event } => {
                self.pending_references.remove(&event_id);
                if let Some(event) = event.filter(|event| event.id == event_id) {
                    let pubkey = event.pubkey;
                    self.referenced_events.insert(event_id, *event);
                    let key = pubkey.to_hex();
                    if !self.profiles.contains_key(&key) && self.pending_profiles.insert(key) {
                        return Some(Command::FetchProfile(pubkey));
                    }
                }
                None
            }
            UiEvent::Identity(identity) => {
                self.identity = identity;
                None
            }
            UiEvent::Status(status) => {
                self.status = status;
                None
            }
            UiEvent::Nip05Verified {
                pubkey,
                address,
                verified,
            } => {
                let key = (pubkey, address);
                self.pending_nip05.remove(&key);
                if verified {
                    self.verified_nip05.insert(key);
                }
                None
            }
            UiEvent::Avatar { pubkey, url, image } => {
                let is_current = self
                    .profiles
                    .get(&pubkey)
                    .and_then(|profile| profile.picture.as_deref())
                    == Some(url.as_str());
                if is_current {
                    self.avatars.complete(pubkey, url, image);
                } else {
                    self.avatars.discard_completion(&pubkey, &url);
                }
                None
            }
        }
    }

    fn add_event(&mut self, event: Event) -> Option<Command> {
        let id = event.id.to_string();
        if !self.seen.insert(id) {
            return None;
        }

        match event.kind {
            Kind::Metadata => {
                let pubkey = event.pubkey.to_hex();
                if self
                    .profile_timestamps
                    .get(&pubkey)
                    .is_some_and(|timestamp| timestamp >= &event.created_at)
                {
                    return None;
                }
                self.profile_timestamps
                    .insert(pubkey.clone(), event.created_at);
                self.apply_profile(pubkey, &event.content)
            }
            Kind::TextNote | Kind::Repost => {
                let profile_key = if event.kind == Kind::Repost {
                    Event::from_json(&event.content)
                        .ok()
                        .map(|original| original.pubkey)
                        .unwrap_or(event.pubkey)
                } else {
                    event.pubkey
                };
                let is_following = self.following_pubkeys.contains(&event.pubkey);
                add_timeline_event(&mut self.global_timeline, event.clone());
                if is_following {
                    add_timeline_event(&mut self.following_timeline, event);
                }
                let key = profile_key.to_hex();
                if !self.profiles.contains_key(&key) && self.pending_profiles.insert(key) {
                    Some(Command::FetchProfile(profile_key))
                } else {
                    None
                }
            }
            Kind::Reaction => {
                if let Some(target) = event.tags.event_ids().next() {
                    self.reactions
                        .entry(target.to_string())
                        .or_default()
                        .add(&event.content);
                }
                None
            }
            _ => None,
        }
    }

    fn apply_profile(&mut self, pubkey: String, content: &str) -> Option<Command> {
        self.pending_profiles.remove(&pubkey);
        let Ok(profile) = serde_json::from_str::<Profile>(content) else {
            return None;
        };
        let verification = profile.nip05.as_ref().and_then(|address| {
            let key = (pubkey.clone(), address.clone());
            if !self.verified_nip05.contains(&key) && self.pending_nip05.insert(key) {
                Some(Command::VerifyNip05 {
                    pubkey: pubkey.clone(),
                    address: address.clone(),
                })
            } else {
                None
            }
        });
        self.avatars
            .profile_updated(&pubkey, profile.picture.as_deref());
        self.profiles.insert(pubkey, profile);
        verification
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Option<Command> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        if self.settings_open {
            return match key.code {
                KeyCode::Char('m') | KeyCode::Esc => {
                    self.settings_open = false;
                    None
                }
                KeyCode::Char('q') => Some(Command::Quit),
                _ => None,
            };
        }
        match self.mode.clone() {
            InputMode::Normal => self.on_normal_key(key),
            InputMode::Compose { reply_to } => self.on_input_key(key, reply_to, None),
            InputMode::Reaction { event } => self.on_input_key(key, None, Some(event)),
        }
    }

    fn on_normal_key(&mut self, key: KeyEvent) -> Option<Command> {
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), _) => Some(Command::Quit),
            (KeyCode::Tab, _) | (KeyCode::Char(']'), _) => {
                self.select_tab(match self.active_tab {
                    TimelineTab::Following => TimelineTab::Global,
                    TimelineTab::Global => TimelineTab::Following,
                });
                None
            }
            (KeyCode::BackTab, _) | (KeyCode::Char('['), _) => {
                self.select_tab(match self.active_tab {
                    TimelineTab::Following => TimelineTab::Global,
                    TimelineTab::Global => TimelineTab::Following,
                });
                None
            }
            (KeyCode::Char('1'), _) => {
                self.select_tab(TimelineTab::Following);
                None
            }
            (KeyCode::Char('2'), _) => {
                self.select_tab(TimelineTab::Global);
                None
            }
            (KeyCode::Char('m'), _) => {
                self.settings_open = true;
                None
            }
            (KeyCode::Char('j') | KeyCode::Down, _) => {
                let timeline = self.timeline_state_mut();
                timeline.selected =
                    (timeline.selected + 1).min(timeline.events.len().saturating_sub(1));
                None
            }
            (KeyCode::Char('k') | KeyCode::Up, _) => {
                let timeline = self.timeline_state_mut();
                timeline.selected = timeline.selected.saturating_sub(1);
                None
            }
            (KeyCode::Char('g'), _) => {
                let timeline = self.timeline_state_mut();
                timeline.selected = 0;
                resume_timeline(timeline);
                None
            }
            (KeyCode::Char('G'), _) => {
                let timeline = self.timeline_state_mut();
                if !timeline.events.is_empty() {
                    timeline.live = false;
                }
                timeline.selected = timeline.events.len().saturating_sub(1);
                None
            }
            (KeyCode::Char('l') | KeyCode::Enter, _) => {
                self.detail = true;
                None
            }
            (KeyCode::Char('h') | KeyCode::Esc, _) => {
                self.detail = false;
                None
            }
            (KeyCode::Char('i' | 'o'), _) => {
                self.begin_compose(None);
                None
            }
            (KeyCode::Char('r'), _) => {
                if let Some(event) = self.selected_event().cloned() {
                    self.begin_compose(Some(Box::new(event)));
                }
                None
            }
            (KeyCode::Char('+'), _) => {
                if self.read_only {
                    self.status = "read-only: set NOSTR_SECRET_KEY to react".to_owned();
                    return None;
                }
                self.selected_event().cloned().map(|event| Command::React {
                    event: Box::new(event),
                    reaction: "+".to_owned(),
                })
            }
            (KeyCode::Char('-'), _) => {
                if self.read_only {
                    self.status = "read-only: set NOSTR_SECRET_KEY to react".to_owned();
                    return None;
                }
                self.selected_event().cloned().map(|event| Command::React {
                    event: Box::new(event),
                    reaction: "-".to_owned(),
                })
            }
            (KeyCode::Char('e'), _) => {
                if self.read_only {
                    self.status = "read-only: set NOSTR_SECRET_KEY to react".to_owned();
                    return None;
                }
                if let Some(event) = self.selected_event().cloned() {
                    self.mode = InputMode::Reaction {
                        event: Box::new(event),
                    };
                    self.input.clear();
                }
                None
            }
            (KeyCode::Char('R'), _) => {
                if self.read_only {
                    self.status = "read-only: set NOSTR_SECRET_KEY to repost".to_owned();
                    None
                } else {
                    self.selected_event()
                        .cloned()
                        .map(|event| Command::Repost(Box::new(event)))
                }
            }
            _ => None,
        }
    }

    fn on_input_key(
        &mut self,
        key: KeyEvent,
        reply_to: Option<Box<Event>>,
        reaction_to: Option<Box<Event>>,
    ) -> Option<Command> {
        if key.code == KeyCode::Esc {
            self.mode = InputMode::Normal;
            self.input.clear();
            self.status = "cancelled".to_owned();
            return None;
        }
        if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
            let content = self.input.trim().to_owned();
            if content.is_empty() {
                self.status = "nothing to send".to_owned();
                return None;
            }
            self.mode = InputMode::Normal;
            self.input.clear();
            self.status = "sending…".to_owned();
            return if let Some(event) = reaction_to {
                Some(Command::React {
                    event,
                    reaction: content,
                })
            } else {
                Some(Command::Publish { content, reply_to })
            };
        }

        match key.code {
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.input.push(character)
            }
            KeyCode::Enter if reaction_to.is_none() => self.input.push('\n'),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Tab => self.input.push('\t'),
            _ => {}
        }
        None
    }

    fn begin_compose(&mut self, reply_to: Option<Box<Event>>) {
        if self.read_only {
            self.status = "read-only: set NOSTR_SECRET_KEY to publish".to_owned();
            return;
        }
        self.mode = InputMode::Compose { reply_to };
        self.input.clear();
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

    pub fn kitty_images_enabled(&self) -> bool {
        self.avatars.is_enabled()
    }

    /// Schedules only authors near the current viewport. Both the number of
    /// in-flight requests and the decoded image cache are bounded by
    /// `AvatarCache`.
    pub fn avatar_commands(&mut self) -> Vec<Command> {
        if !self.avatars.is_enabled() || self.timeline().is_empty() {
            return Vec::new();
        }

        const VIEWPORT_RADIUS: usize = 12;
        let selected = self.selected_index();
        let start = selected.saturating_sub(VIEWPORT_RADIUS);
        let end = (selected + VIEWPORT_RADIUS + 1).min(self.timeline().len());
        let mut candidates = Vec::new();
        let mut unique = HashSet::new();
        for event in &self.timeline()[start..end] {
            let pubkey = if event.kind == Kind::Repost {
                Event::from_json(&event.content)
                    .ok()
                    .map(|original| original.pubkey)
                    .unwrap_or(event.pubkey)
            } else {
                event.pubkey
            };
            let key = pubkey.to_hex();
            if !unique.insert(key.clone()) {
                continue;
            }
            if let Some(url) = self
                .profiles
                .get(&key)
                .and_then(|profile| profile.picture.clone())
            {
                candidates.push((key, url));
            }
        }

        candidates
            .into_iter()
            .filter_map(|(pubkey, url)| {
                self.avatars
                    .request(&pubkey, &url)
                    .then_some(Command::FetchAvatar { pubkey, url })
            })
            .collect()
    }

    /// Fetches NIP-21 profiles and event references near the viewport without
    /// adding referenced events to the main timeline.
    pub fn reference_commands(&mut self) -> Vec<Command> {
        const VIEWPORT_RADIUS: usize = 12;
        const MAX_PENDING_REFERENCES: usize = 4;

        let available = MAX_PENDING_REFERENCES.saturating_sub(self.pending_references.len());
        if self.timeline().is_empty() {
            return Vec::new();
        }

        let selected = self.selected_index();
        let start = selected.saturating_sub(VIEWPORT_RADIUS);
        let end = (selected + VIEWPORT_RADIUS + 1).min(self.timeline().len());
        let viewport_events = self.timeline()[start..end].to_vec();
        let mut ids = Vec::new();
        let mut pubkeys = Vec::new();
        let mut unique = HashSet::new();
        let mut unique_pubkeys = HashSet::new();
        for event in &viewport_events {
            let display = self.display_event(event);
            let expanded = expand_mentions(&display.event.content, &display.event.tags);
            for entity in content_entities(&expanded) {
                match entity.kind {
                    ContentEntityKind::Event(event_id) => {
                        if unique.insert(event_id)
                            && !self.referenced_events.contains_key(&event_id)
                            && !self
                                .global_timeline
                                .events
                                .iter()
                                .any(|event| event.id == event_id)
                            && !self.requested_references.contains(&event_id)
                        {
                            ids.push(event_id);
                        }
                    }
                    ContentEntityKind::Pubkey(pubkey) => {
                        let key = pubkey.to_hex();
                        if unique_pubkeys.insert(key.clone())
                            && !self.profiles.contains_key(&key)
                            && !self.pending_profiles.contains(&key)
                        {
                            pubkeys.push((key, pubkey));
                        }
                    }
                }
            }
        }

        let mut commands = Vec::new();
        for (key, pubkey) in pubkeys {
            self.pending_profiles.insert(key);
            commands.push(Command::FetchProfile(pubkey));
        }
        commands.extend(ids.into_iter().take(available).map(|event_id| {
            self.requested_references.insert(event_id);
            self.pending_references.insert(event_id);
            Command::FetchEvent(event_id)
        }));
        commands
    }

    pub fn avatar_protocol_mut(
        &mut self,
        pubkey: &PublicKey,
    ) -> Option<&mut ratatui_image::protocol::StatefulProtocol> {
        let key = pubkey.to_hex();
        let url = self.profiles.get(&key)?.picture.clone()?;
        self.avatars.protocol_mut(&key, &url)
    }

    pub fn take_deleted_avatar_ids(&mut self) -> Vec<u32> {
        self.avatars.take_deleted_ids()
    }

    pub fn clear_avatars(&mut self) {
        self.avatars.clear();
    }

    pub fn selected_event(&self) -> Option<&Event> {
        self.timeline().get(self.selected_index())
    }

    pub fn display_event(&self, event: &Event) -> DisplayEvent {
        if event.kind == Kind::Repost {
            if let Ok(original) = Event::from_json(&event.content) {
                return DisplayEvent {
                    event: original,
                    reposted_by: Some(event.pubkey),
                };
            }
        }
        DisplayEvent {
            event: event.clone(),
            reposted_by: None,
        }
    }

    pub fn rendered_content(&self, event: &Event) -> RenderedContent {
        let content = expand_mentions(&event.content, &event.tags);
        let mut parts = Vec::new();
        let mut cursor = 0;
        let mut quote = None;

        for entity in content_entities(&content) {
            push_content_part(&mut parts, &content[cursor..entity.start], false);
            match entity.kind {
                ContentEntityKind::Pubkey(pubkey) => {
                    push_content_part(&mut parts, &format!("@{}", self.author_name(&pubkey)), true);
                }
                ContentEntityKind::Event(event_id) if quote.is_none() => {
                    quote = Some(QuoteDisplay {
                        event_id,
                        event: self
                            .referenced_events
                            .get(&event_id)
                            .or_else(|| {
                                self.global_timeline
                                    .events
                                    .iter()
                                    .find(|event| event.id == event_id)
                            })
                            .cloned(),
                        loading: self.pending_references.contains(&event_id)
                            || !self.requested_references.contains(&event_id),
                    });
                }
                ContentEntityKind::Event(_) => {
                    push_content_part(&mut parts, &content[entity.start..entity.end], false);
                }
            }
            cursor = entity.end;
        }
        push_content_part(&mut parts, &content[cursor..], false);
        trim_content_parts(&mut parts);

        RenderedContent { parts, quote }
    }

    pub fn author_name(&self, pubkey: &PublicKey) -> String {
        let key = pubkey.to_hex();
        self.profiles
            .get(&key)
            .and_then(|profile| profile.display_name.as_ref().or(profile.name.as_ref()))
            .cloned()
            .unwrap_or_else(|| pubkey.to_bech32().unwrap_or(key).chars().take(16).collect())
    }

    pub fn nip05_label(&self, pubkey: &PublicKey) -> Option<String> {
        let key = pubkey.to_hex();
        let address = self.profiles.get(&key)?.nip05.clone()?;
        let marker = if self.verified_nip05.contains(&(key, address.clone())) {
            "✓"
        } else {
            "?"
        };
        Some(format!("{marker} {}", display_nip05_address(&address)))
    }

    pub fn reaction_summary(&self, event: &Event) -> String {
        self.reactions
            .get(&event.id.to_string())
            .map(Reactions::summary)
            .unwrap_or_default()
    }
}

pub struct DisplayEvent {
    pub event: Event,
    pub reposted_by: Option<PublicKey>,
}

pub struct RenderedContent {
    pub parts: Vec<RenderedPart>,
    pub quote: Option<QuoteDisplay>,
}

pub struct RenderedPart {
    pub text: String,
    pub mention: bool,
}

pub struct QuoteDisplay {
    pub event_id: EventId,
    pub event: Option<Event>,
    pub loading: bool,
}

fn push_content_part(parts: &mut Vec<RenderedPart>, text: &str, mention: bool) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = parts.last_mut().filter(|part| part.mention == mention) {
        last.text.push_str(text);
    } else {
        parts.push(RenderedPart {
            text: text.to_owned(),
            mention,
        });
    }
}

fn trim_content_parts(parts: &mut Vec<RenderedPart>) {
    while let Some(first) = parts.first_mut() {
        first.text = first.text.trim_start().to_owned();
        if first.text.is_empty() {
            parts.remove(0);
        } else {
            break;
        }
    }
    while let Some(last) = parts.last_mut() {
        last.text = last.text.trim_end().to_owned();
        if last.text.is_empty() {
            parts.pop();
        } else {
            break;
        }
    }
}

fn display_nip05_address(address: &str) -> &str {
    address
        .strip_prefix("_@")
        .filter(|domain| !domain.is_empty() && !domain.contains('@'))
        .unwrap_or(address)
}

pub fn expand_mentions(content: &str, tags: &Tags) -> String {
    let mut output = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(start) = rest.find("#[") {
        output.push_str(&rest[..start]);
        let candidate = &rest[start + 2..];
        let Some(end) = candidate.find(']') else {
            output.push_str(&rest[start..]);
            return output;
        };
        let replacement = candidate[..end]
            .parse::<usize>()
            .ok()
            .and_then(|index| tags.get(index))
            .and_then(|tag| {
                let value = tag.content()?;
                if tag.kind() == TagKind::p() {
                    PublicKey::parse(value)
                        .ok()
                        .and_then(|key| key.to_bech32().ok())
                        .map(|npub| format!("@{npub}"))
                } else if tag.kind() == TagKind::e() {
                    EventId::parse(value)
                        .ok()
                        .and_then(|id| id.to_bech32().ok())
                        .map(|note| format!("nostr:{note}"))
                } else {
                    None
                }
                .or_else(|| Some(format!("@{}", value.chars().take(16).collect::<String>())))
            });
        if let Some(replacement) = replacement {
            output.push_str(&replacement);
        } else {
            output.push_str(&rest[start..start + end + 3]);
        }
        rest = &candidate[end + 1..];
    }
    output.push_str(rest);
    output
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContentEntity {
    start: usize,
    end: usize,
    kind: ContentEntityKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentEntityKind {
    Pubkey(PublicKey),
    Event(EventId),
}

fn content_entities(content: &str) -> Vec<ContentEntity> {
    let mut entities = Vec::new();
    let mut cursor = 0;

    while cursor < content.len() {
        let nostr = content[cursor..].find("nostr:").map(|index| cursor + index);
        let npub = content[cursor..].find("@npub1").map(|index| cursor + index);
        let Some(start) = nostr.into_iter().chain(npub).min() else {
            break;
        };
        let is_at_npub = content[start..].starts_with("@npub1");
        let token_start = if is_at_npub { start + 1 } else { start };
        let end = content[token_start..]
            .char_indices()
            .take_while(|(_, character)| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || (!is_at_npub && *character == ':')
            })
            .last()
            .map(|(index, character)| token_start + index + character.len_utf8())
            .unwrap_or(token_start);

        let kind = if is_at_npub {
            PublicKey::parse(&content[token_start..end])
                .ok()
                .map(ContentEntityKind::Pubkey)
        } else {
            Nip21::parse(&content[start..end])
                .ok()
                .and_then(|entity| match entity {
                    Nip21::Pubkey(pubkey) => Some(ContentEntityKind::Pubkey(pubkey)),
                    Nip21::Profile(profile) => Some(ContentEntityKind::Pubkey(profile.public_key)),
                    Nip21::EventId(event_id) => Some(ContentEntityKind::Event(event_id)),
                    Nip21::Event(event) => Some(ContentEntityKind::Event(event.event_id)),
                    Nip21::Coordinate(_) => None,
                })
        };

        if let Some(kind) = kind {
            entities.push(ContentEntity { start, end, kind });
            cursor = end;
        } else {
            cursor = start + if is_at_npub { 1 } else { "nostr:".len() };
        }
    }

    entities
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn reaction_summary_is_stable() {
        let mut reactions = Reactions::default();
        reactions.add("🔥");
        reactions.add("+");
        reactions.add("🔥");
        assert_eq!(reactions.summary(), "+1 🔥2");
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
}
