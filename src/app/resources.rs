use super::*;

impl App {
    pub fn kitty_images_enabled(&self) -> bool {
        self.images.is_enabled()
    }

    /// Schedules only images near the current viewport. Both the number of
    /// in-flight requests and the decoded image cache are bounded.
    pub fn image_commands(&mut self) -> Vec<Command> {
        if !self.images.is_enabled() || self.timeline().is_empty() {
            return Vec::new();
        }

        const VIEWPORT_RADIUS: usize = 12;
        let selected = self.selected_index();
        let start = selected.saturating_sub(VIEWPORT_RADIUS);
        let end = (selected + VIEWPORT_RADIUS + 1).min(self.timeline().len());
        let mut candidates = Vec::new();
        let mut unique_authors = HashSet::new();
        let mut unique_images = HashSet::new();
        for (i, event) in self.timeline()[start..end].iter().enumerate() {
            let display = self.display_event(event);
            let pubkey = display.event.pubkey;
            let key = pubkey.to_hex();
            if unique_authors.insert(key.clone()) {
                if let Some(url) = self
                    .profiles
                    .get(&key)
                    .and_then(|profile| profile.picture.clone())
                {
                    candidates.push((avatar_image_key(&key), url));
                }
            }

            for emoji in used_custom_emojis(&display.event) {
                let image_key = emoji_image_key(&emoji.url);
                if unique_images.insert(image_key.clone()) {
                    candidates.push((image_key, emoji.url));
                }
            }
            for url in post_image_urls(&display.event) {
                let image_key = post_image_key(&url);
                if unique_images.insert(image_key.clone()) {
                    candidates.push((image_key, url.clone()));
                }
                // Schedule high-resolution detail images for the selected event
                if start + i == selected {
                    let detail_key = detail_image_key(&url);
                    candidates.push((detail_key, url));
                }
            }
            if let Some(reactions) = self.reactions.get(&display.event.id.to_string()) {
                for url in reactions.custom_emojis.values() {
                    let image_key = emoji_image_key(url);
                    if unique_images.insert(image_key.clone()) {
                        candidates.push((image_key, url.clone()));
                    }
                }
            }
        }

        candidates
            .into_iter()
            .filter_map(|(key, url)| {
                self.images
                    .request(&key, &url)
                    .then_some(Command::FetchImage { key, url })
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
            for pubkey in [Some(display.event.pubkey), display.reposted_by]
                .into_iter()
                .flatten()
            {
                let key = pubkey.to_hex();
                if unique_pubkeys.insert(key.clone())
                    && !self.profiles.contains_key(&key)
                    && !self.pending_profiles.contains(&key)
                {
                    pubkeys.push((key, pubkey));
                }
            }
            if is_repost(event) && display.event.id == event.id {
                if let Some(event_id) = repost_target_id(event) {
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
            }
            if let Some(event_id) = reply_target_id(&display.event) {
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
        self.images.protocol_mut(&avatar_image_key(&key), &url)
    }

    pub fn custom_emoji_ready(&self, emoji: &CustomEmoji) -> bool {
        self.images
            .has_protocol(&emoji_image_key(&emoji.url), &emoji.url)
    }

    pub fn custom_emoji_protocol_mut(
        &mut self,
        emoji: &CustomEmoji,
    ) -> Option<&mut ratatui_image::protocol::StatefulProtocol> {
        self.images
            .protocol_mut(&emoji_image_key(&emoji.url), &emoji.url)
    }

    pub fn post_image_preview_size(
        &self,
        url: &str,
        max_width: u16,
        max_height: u16,
    ) -> Option<(u16, u16)> {
        self.images
            .preview_size(&post_image_key(url), url, max_width, max_height)
    }

    pub fn post_image_protocol_mut(
        &mut self,
        url: &str,
    ) -> Option<&mut ratatui_image::protocol::StatefulProtocol> {
        self.images.protocol_mut(&post_image_key(url), url)
    }

    pub fn detail_post_image_preview_size(
        &self,
        url: &str,
        max_width: u16,
        max_height: u16,
    ) -> Option<(u16, u16)> {
        self.images
            .preview_size(&detail_image_key(url), url, max_width, max_height)
    }

    pub fn detail_post_image_protocol_mut(
        &mut self,
        url: &str,
    ) -> Option<&mut ratatui_image::protocol::StatefulProtocol> {
        self.images.protocol_mut(&detail_image_key(url), url)
    }

    pub fn take_deleted_image_ids(&mut self) -> Vec<u32> {
        self.images.take_deleted_ids()
    }

    pub fn clear_images(&mut self) {
        self.images.clear();
    }
}
