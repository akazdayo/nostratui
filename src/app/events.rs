use super::*;

impl App {
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
            UiEvent::Image { key, url, image } => {
                self.images.complete(key, url, image);
                None
            }
        }
    }

    pub(super) fn add_event(&mut self, event: Event) -> Option<Command> {
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
            Kind::TextNote | Kind::Repost | Kind::GenericRepost => {
                let profile_key = if is_repost(&event) {
                    embedded_repost(&event)
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
                    let custom_emoji = used_custom_emojis(&event)
                        .into_iter()
                        .find(|emoji| event.content == format!(":{}:", emoji.shortcode));
                    let reactions = self.reactions.entry(target.to_string()).or_default();
                    reactions.add(&event.content);
                    if let Some(emoji) = custom_emoji {
                        reactions
                            .custom_emojis
                            .entry(emoji.shortcode)
                            .or_insert(emoji.url);
                    }
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
        self.images
            .source_updated(&avatar_image_key(&pubkey), profile.picture.as_deref());
        self.profiles.insert(pubkey, profile);
        verification
    }
}
