use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use nostr_sdk::prelude::*;
use serde::Deserialize;

use crate::network::UiEvent;

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
    Quit,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Profile {
    pub name: Option<String>,
    pub display_name: Option<String>,
    pub nip05: Option<String>,
    pub about: Option<String>,
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

pub struct App {
    pub timeline: Vec<Event>,
    pub selected: usize,
    pub profiles: HashMap<String, Profile>,
    pub verified_nip05: HashSet<(String, String)>,
    pub reactions: HashMap<String, Reactions>,
    pub mode: InputMode,
    pub input: String,
    pub detail: bool,
    pub status: String,
    pub identity: String,
    pub read_only: bool,
    seen: HashSet<String>,
    pending_nip05: HashSet<(String, String)>,
    pending_profiles: HashSet<String>,
    profile_timestamps: HashMap<String, Timestamp>,
}

impl App {
    pub fn new(read_only: bool) -> Self {
        Self {
            timeline: Vec::new(),
            selected: 0,
            profiles: HashMap::new(),
            verified_nip05: HashSet::new(),
            reactions: HashMap::new(),
            mode: InputMode::Normal,
            input: String::new(),
            detail: false,
            status: "starting…".to_owned(),
            identity: "loading…".to_owned(),
            read_only,
            seen: HashSet::new(),
            pending_nip05: HashSet::new(),
            pending_profiles: HashSet::new(),
            profile_timestamps: HashMap::new(),
        }
    }

    pub fn on_ui_event(&mut self, message: UiEvent) -> Option<Command> {
        match message {
            UiEvent::Event(event) => self.add_event(*event),
            UiEvent::Profile { pubkey, content } => {
                if self.profiles.contains_key(&pubkey) {
                    self.pending_profiles.remove(&pubkey);
                    None
                } else {
                    self.apply_profile(pubkey, &content)
                }
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
                self.timeline.push(event);
                self.timeline.sort_by_key(|event| Reverse(event.created_at));
                if self.selected >= self.timeline.len() {
                    self.selected = self.timeline.len().saturating_sub(1);
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
        self.profiles.insert(pubkey, profile);
        verification
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Option<Command> {
        if key.kind == KeyEventKind::Release {
            return None;
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
            (KeyCode::Char('j') | KeyCode::Down, _) => {
                self.selected = (self.selected + 1).min(self.timeline.len().saturating_sub(1));
                None
            }
            (KeyCode::Char('k') | KeyCode::Up, _) => {
                self.selected = self.selected.saturating_sub(1);
                None
            }
            (KeyCode::Char('g'), _) => {
                self.selected = 0;
                None
            }
            (KeyCode::Char('G'), _) => {
                self.selected = self.timeline.len().saturating_sub(1);
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

    pub fn selected_event(&self) -> Option<&Event> {
        self.timeline.get(self.selected)
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
        Some(format!("{marker} {address}"))
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

impl DisplayEvent {
    /// Expands the legacy NIP-08 `#[index]` notation using the indexed tag.
    pub fn content_with_mentions(&self) -> String {
        expand_mentions(&self.event.content, &self.event.tags)
    }
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
    fn reaction_summary_is_stable() {
        let mut reactions = Reactions::default();
        reactions.add("🔥");
        reactions.add("+");
        reactions.add("🔥");
        assert_eq!(reactions.summary(), "+1 🔥2");
    }
}
