use super::*;

impl App {
    pub fn selected_event(&self) -> Option<&Event> {
        self.timeline().get(self.selected_index())
    }

    pub(super) fn selected_content_event(&self) -> Option<Event> {
        let selected = self.selected_event()?;
        let display = self.display_event(selected);
        (!is_repost(selected) || display.event.id != selected.id).then_some(display.event)
    }

    pub fn display_event(&self, event: &Event) -> DisplayEvent {
        if is_repost(event) {
            let original = embedded_repost(event).or_else(|| {
                let target = repost_target_id(event)?;
                self.referenced_events
                    .get(&target)
                    .or_else(|| {
                        self.global_timeline
                            .events
                            .iter()
                            .find(|candidate| candidate.id == target)
                    })
                    .cloned()
            });
            if let Some(original) = original {
                return DisplayEvent {
                    event: original,
                    reposted_by: Some(event.pubkey),
                };
            }
            return DisplayEvent {
                event: event.clone(),
                reposted_by: Some(event.pubkey),
            };
        }
        DisplayEvent {
            event: event.clone(),
            reposted_by: None,
        }
    }

    pub fn reply_display(&self, event: &Event) -> Option<ReplyDisplay> {
        let event_id = reply_target_id(event)?;
        Some(ReplyDisplay {
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
        })
    }

    pub fn rendered_content(&self, event: &Event) -> RenderedContent {
        let content = expand_mentions(&event.content, &event.tags);
        let image_urls = post_image_urls(event);
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
        parts = emojify_parts(parts, &custom_emoji_tags(&event.tags));

        RenderedContent {
            parts,
            quote,
            image_urls,
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
        Some(format!("{marker} {}", display_nip05_address(&address)))
    }

    pub fn rendered_reactions(&self, event: &Event) -> Vec<RenderedPart> {
        let Some(reactions) = self.reactions.get(&event.id.to_string()) else {
            return Vec::new();
        };
        emojify_parts(
            vec![RenderedPart {
                text: reactions.summary(),
                mention: false,
                emoji: None,
            }],
            &reactions.custom_emojis,
        )
    }
}

pub struct DisplayEvent {
    pub event: Event,
    pub reposted_by: Option<PublicKey>,
}

pub struct RenderedContent {
    pub parts: Vec<RenderedPart>,
    pub quote: Option<QuoteDisplay>,
    pub image_urls: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPart {
    pub text: String,
    pub mention: bool,
    pub emoji: Option<CustomEmoji>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomEmoji {
    pub shortcode: String,
    pub url: String,
}

pub struct QuoteDisplay {
    pub event_id: EventId,
    pub event: Option<Event>,
    pub loading: bool,
}

pub struct ReplyDisplay {
    pub event_id: EventId,
    pub event: Option<Event>,
    pub loading: bool,
}

pub(super) fn is_repost(event: &Event) -> bool {
    matches!(event.kind, Kind::Repost | Kind::GenericRepost)
}

pub(super) fn repost_target_id(event: &Event) -> Option<EventId> {
    is_repost(event)
        .then(|| event.tags.event_ids().next().copied())
        .flatten()
}

pub(super) fn embedded_repost(event: &Event) -> Option<Event> {
    if !is_repost(event) {
        return None;
    }
    let original = Event::from_json(&event.content)
        .ok()
        .filter(|original| original.verify().is_ok())?;
    repost_target_id(event)
        .is_none_or(|target| target == original.id)
        .then_some(original)
}

/// Finds the immediate parent of a NIP-10 reply. Marked tags take precedence;
/// unmarked tags use the deprecated positional convention where the last `e`
/// tag is the event being replied to.
pub(super) fn reply_target_id(event: &Event) -> Option<EventId> {
    if event.kind != Kind::TextNote {
        return None;
    }

    let mut reply = None;
    let mut root = None;
    let mut legacy = None;
    let indexed_mentions = indexed_tag_references(&event.content);
    for (index, tag) in event.tags.iter().enumerate() {
        let Some(TagStandard::Event {
            event_id, marker, ..
        }) = tag.as_standardized()
        else {
            continue;
        };
        match marker {
            Some(Marker::Reply) => reply.get_or_insert(*event_id),
            Some(Marker::Root) => root.get_or_insert(*event_id),
            None => {
                if !indexed_mentions.contains(&index) {
                    legacy = Some(*event_id);
                }
                continue;
            }
        };
    }
    reply.or(root).or(legacy)
}

fn indexed_tag_references(content: &str) -> HashSet<usize> {
    let mut indices = HashSet::new();
    let mut rest = content;
    while let Some(start) = rest.find("#[") {
        let candidate = &rest[start + 2..];
        let Some(end) = candidate.find(']') else {
            break;
        };
        if let Ok(index) = candidate[..end].parse() {
            indices.insert(index);
        }
        rest = &candidate[end + 1..];
    }
    indices
}

fn push_content_part(parts: &mut Vec<RenderedPart>, text: &str, mention: bool) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = parts
        .last_mut()
        .filter(|part| part.emoji.is_none() && part.mention == mention)
    {
        last.text.push_str(text);
    } else {
        parts.push(RenderedPart {
            text: text.to_owned(),
            mention,
            emoji: None,
        });
    }
}

fn push_emoji_part(parts: &mut Vec<RenderedPart>, emoji: CustomEmoji) {
    parts.push(RenderedPart {
        text: format!(":{}:", emoji.shortcode),
        mention: false,
        emoji: Some(emoji),
    });
}

fn custom_emoji_tags(tags: &Tags) -> HashMap<String, String> {
    let mut emojis = HashMap::new();
    for tag in tags.iter().filter(|tag| tag.kind() == TagKind::Emoji) {
        let values = tag.as_slice();
        let (Some(shortcode), Some(url)) = (values.get(1), values.get(2)) else {
            continue;
        };
        if shortcode.is_empty()
            || shortcode.len() > 64
            || !shortcode.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
        {
            continue;
        }
        emojis
            .entry(shortcode.clone())
            .or_insert_with(|| url.clone());
    }
    emojis
}

pub(super) fn used_custom_emojis(event: &Event) -> Vec<CustomEmoji> {
    let tags = custom_emoji_tags(&event.tags);
    let mut used = Vec::new();
    let mut seen = HashSet::new();
    for (_, _, shortcode) in custom_emoji_references(&event.content, &tags) {
        if seen.insert(shortcode.clone()) {
            if let Some(url) = tags.get(&shortcode) {
                used.push(CustomEmoji {
                    shortcode,
                    url: url.clone(),
                });
            }
        }
    }
    used
}

fn emojify_parts(
    parts: Vec<RenderedPart>,
    emoji_tags: &HashMap<String, String>,
) -> Vec<RenderedPart> {
    if emoji_tags.is_empty() {
        return parts;
    }
    let mut output = Vec::new();
    for part in parts {
        if part.mention || part.emoji.is_some() {
            output.push(part);
            continue;
        }
        let mut cursor = 0;
        for (start, end, shortcode) in custom_emoji_references(&part.text, emoji_tags) {
            push_content_part(&mut output, &part.text[cursor..start], false);
            let url = emoji_tags
                .get(&shortcode)
                .expect("custom emoji reference came from the tag map")
                .clone();
            push_emoji_part(&mut output, CustomEmoji { shortcode, url });
            cursor = end;
        }
        push_content_part(&mut output, &part.text[cursor..], false);
    }
    output
}

pub(super) fn custom_emoji_references(
    content: &str,
    emoji_tags: &HashMap<String, String>,
) -> Vec<(usize, usize, String)> {
    let mut references = Vec::new();
    let mut cursor = 0;
    while cursor < content.len() {
        let Some(relative_start) = content[cursor..].find(':') else {
            break;
        };
        let start = cursor + relative_start;
        let candidate_start = start + 1;
        let Some(relative_end) = content[candidate_start..].find(':') else {
            break;
        };
        let end = candidate_start + relative_end;
        let shortcode = &content[candidate_start..end];
        if emoji_tags.contains_key(shortcode) {
            references.push((start, end + 1, shortcode.to_owned()));
            cursor = end + 1;
        } else {
            cursor = candidate_start;
        }
    }
    references
}

pub(super) fn avatar_image_key(pubkey: &str) -> String {
    format!("avatar:{pubkey}")
}

pub(super) fn emoji_image_key(url: &str) -> String {
    format!("emoji:{url}")
}

pub(super) fn post_image_key(url: &str) -> String {
    format!("post:{url}")
}

/// Extracts a small, ordered set of image links from note content and NIP-92
/// `imeta` tags. Plain links need a supported file extension; metadata may
/// identify extensionless image URLs by MIME type.
pub(super) fn post_image_urls(event: &Event) -> Vec<String> {
    const MAX_IMAGES_PER_POST: usize = 4;

    let mut urls = Vec::new();
    let mut seen = HashSet::new();
    for token in event.content.split_whitespace() {
        let lowercase = token.to_ascii_lowercase();
        let Some(start) = lowercase.find("https://") else {
            continue;
        };
        let candidate = trim_url_punctuation(&token[start..]);
        if is_supported_image_url(candidate) && seen.insert(candidate.to_owned()) {
            urls.push(candidate.to_owned());
            if urls.len() == MAX_IMAGES_PER_POST {
                return urls;
            }
        }
    }

    for tag in event
        .tags
        .iter()
        .filter(|tag| tag.as_slice().first().is_some_and(|kind| kind == "imeta"))
    {
        let mut url = None;
        let mut image_mime = false;
        for value in tag.as_slice().iter().skip(1) {
            if let Some(value) = value.strip_prefix("url ") {
                url = Some(trim_url_punctuation(value));
            } else if value
                .strip_prefix("m ")
                .is_some_and(|mime| mime.to_ascii_lowercase().starts_with("image/"))
            {
                image_mime = true;
            }
        }
        let Some(url) = url.filter(|url| image_mime || is_supported_image_url(url)) else {
            continue;
        };
        if is_safe_https_url(url) && seen.insert(url.to_owned()) {
            urls.push(url.to_owned());
            if urls.len() == MAX_IMAGES_PER_POST {
                break;
            }
        }
    }
    urls
}

fn trim_url_punctuation(url: &str) -> &str {
    url.trim_matches(|character: char| {
        matches!(
            character,
            '(' | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '<'
                | '>'
                | '"'
                | '\''
                | ','
                | '.'
                | ':'
                | ';'
                | '!'
                | '?'
                | '。'
                | '、'
        )
    })
}

fn is_supported_image_url(url: &str) -> bool {
    if !is_safe_https_url(url) {
        return false;
    }
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg"]
        .iter()
        .any(|extension| path.ends_with(extension))
}

fn is_safe_https_url(url: &str) -> bool {
    url.len() <= 2_048
        && url
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
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
pub(super) struct ContentEntity {
    start: usize,
    end: usize,
    pub(super) kind: ContentEntityKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContentEntityKind {
    Pubkey(PublicKey),
    Event(EventId),
}

pub(super) fn content_entities(content: &str) -> Vec<ContentEntity> {
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
