use std::{collections::HashMap, io::Cursor, time::Duration};

use ::image::{imageops::FilterType, DynamicImage, ImageReader, Limits, RgbaImage};
use nostr_sdk::prelude::*;
use reqwest::Client as HttpClient;
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::app::Command;

#[derive(Debug)]
pub struct NetworkConfig {
    pub relays: Vec<String>,
    pub secret_key: Option<String>,
    pub limit: usize,
}

#[derive(Debug)]
pub enum UiEvent {
    Event(Box<Event>),
    FollowList(Vec<PublicKey>),
    Profile {
        pubkey: String,
        content: String,
    },
    Identity(String),
    Nip05Verified {
        pubkey: String,
        address: String,
        verified: bool,
    },
    Image {
        key: String,
        url: String,
        image: Option<DynamicImage>,
    },
    ReferencedEvent {
        event_id: EventId,
        event: Option<Box<Event>>,
    },
    Status(String),
}

pub async fn run(
    config: NetworkConfig,
    mut commands: mpsc::Receiver<Command>,
    ui_tx: mpsc::Sender<UiEvent>,
) {
    if let Err(error) = run_inner(config, &mut commands, &ui_tx).await {
        let _ = ui_tx
            .send(UiEvent::Status(format!("network error: {error:#}")))
            .await;
    }
}

async fn run_inner(
    config: NetworkConfig,
    commands: &mut mpsc::Receiver<Command>,
    ui_tx: &mpsc::Sender<UiEvent>,
) -> anyhow::Result<()> {
    let (client, own_pubkey) = match config.secret_key {
        Some(secret) => {
            let keys = Keys::parse(&secret)?;
            let public_key = keys.public_key();
            ui_tx
                .send(UiEvent::Identity(public_key.to_bech32()?))
                .await?;
            (Client::new(keys), Some(public_key))
        }
        None => {
            ui_tx
                .send(UiEvent::Identity("read-only".to_owned()))
                .await?;
            (Client::default(), None)
        }
    };

    let mut notifications = client.notifications();
    for relay in &config.relays {
        match client.add_relay(relay).await {
            Ok(_) => {}
            Err(error) => {
                let _ = ui_tx
                    .send(UiEvent::Status(format!("invalid relay {relay}: {error}")))
                    .await;
            }
        }
    }
    client.connect().await;
    let global_filter = Filter::new()
        .kinds([
            Kind::Metadata,
            Kind::TextNote,
            Kind::Repost,
            Kind::GenericRepost,
            Kind::Reaction,
        ])
        .limit(config.limit);
    client
        .subscribe_with_id(SubscriptionId::new("global-timeline"), global_filter, None)
        .await?;

    let following_subscription = SubscriptionId::new("following-timeline");
    let mut contact_list_timestamp = None;
    if let Some(pubkey) = own_pubkey {
        ui_tx.send(UiEvent::FollowList(vec![pubkey])).await?;
        client
            .subscribe_with_id(
                SubscriptionId::new("contact-list"),
                Filter::new()
                    .author(pubkey)
                    .kind(Kind::ContactList)
                    .limit(1),
                None,
            )
            .await?;
        client
            .subscribe_with_id(
                following_subscription.clone(),
                Filter::new()
                    .author(pubkey)
                    .kinds([Kind::TextNote, Kind::Repost, Kind::GenericRepost])
                    .limit(config.limit),
                None,
            )
            .await?;
    }
    ui_tx
        .send(UiEvent::Status(format!(
            "connected · {} relay(s)",
            config.relays.len()
        )))
        .await?;

    let http = HttpClient::builder()
        .user_agent(concat!("nostr-ratatui/", env!("CARGO_PKG_VERSION")))
        .https_only(true)
        .timeout(Duration::from_secs(8))
        .build()?;

    loop {
        tokio::select! {
            notification = notifications.recv() => {
                match notification {
                    Ok(RelayPoolNotification::Event { event, .. }) => {
                        if event.kind == Kind::ContactList && own_pubkey == Some(event.pubkey) {
                            if contact_list_timestamp.is_none_or(|timestamp| event.created_at > timestamp) {
                                contact_list_timestamp = Some(event.created_at);
                                let mut pubkeys: Vec<_> = event.tags.public_keys().copied().collect();
                                if let Some(pubkey) = own_pubkey {
                                    pubkeys.push(pubkey);
                                }
                                pubkeys.sort_unstable();
                                pubkeys.dedup();
                                ui_tx.send(UiEvent::FollowList(pubkeys.clone())).await?;

                                client.unsubscribe(&following_subscription).await;
                                client
                                    .subscribe_with_id(
                                        following_subscription.clone(),
                                        Filter::new()
                                            .authors(pubkeys)
                                            .kinds([
                                                Kind::TextNote,
                                                Kind::Repost,
                                                Kind::GenericRepost,
                                            ])
                                            .limit(config.limit),
                                        None,
                                    )
                                    .await?;
                            }
                        } else {
                            ui_tx.send(UiEvent::Event(event)).await?;
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        ui_tx.send(UiEvent::Status(format!("relay channel: {error}"))).await?;
                    }
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { break };
                handle_command(&client, &http, command, ui_tx).await;
            }
        }
    }

    client.shutdown().await;
    Ok(())
}

async fn handle_command(
    client: &Client,
    http: &HttpClient,
    command: Command,
    ui_tx: &mpsc::Sender<UiEvent>,
) {
    match &command {
        Command::VerifyNip05 { pubkey, address } => {
            let pubkey = pubkey.clone();
            let address = address.clone();
            let http = http.clone();
            let ui_tx = ui_tx.clone();
            tokio::spawn(async move {
                let verified = verify_nip05(&http, &pubkey, &address)
                    .await
                    .unwrap_or(false);
                let _ = ui_tx
                    .send(UiEvent::Nip05Verified {
                        pubkey,
                        address,
                        verified,
                    })
                    .await;
            });
            return;
        }
        Command::FetchProfile(pubkey) => {
            let pubkey = *pubkey;
            let client = client.clone();
            let ui_tx = ui_tx.clone();
            tokio::spawn(async move {
                match client.fetch_metadata(pubkey, Duration::from_secs(5)).await {
                    Ok(Some(metadata)) => {
                        let _ = ui_tx
                            .send(UiEvent::Profile {
                                pubkey: pubkey.to_hex(),
                                content: metadata.as_json(),
                            })
                            .await;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let _ = ui_tx
                            .send(UiEvent::Status(format!("profile lookup: {error}")))
                            .await;
                    }
                }
            });
            return;
        }
        Command::FetchEvent(event_id) => {
            let event_id = *event_id;
            let client = client.clone();
            let ui_tx = ui_tx.clone();
            tokio::spawn(async move {
                let event = client
                    .fetch_events(Filter::new().id(event_id), Duration::from_secs(5))
                    .await
                    .ok()
                    .and_then(|events| events.into_iter().next())
                    .map(Box::new);
                let _ = ui_tx
                    .send(UiEvent::ReferencedEvent { event_id, event })
                    .await;
            });
            return;
        }
        Command::FetchImage { key, url } => {
            let key = key.clone();
            let url = url.clone();
            let http = http.clone();
            let ui_tx = ui_tx.clone();
            tokio::spawn(async move {
                let image = match fetch_image(&http, &url).await {
                    Ok(image) => Some(image),
                    Err(error) => {
                        let _ = ui_tx
                            .send(UiEvent::Status(format!("image load failed: {error:#}")))
                            .await;
                        None
                    }
                };
                let _ = ui_tx.send(UiEvent::Image { key, url, image }).await;
            });
            return;
        }
        Command::Quit => return,
        _ => {}
    }

    let result = match command {
        Command::Publish { content, reply_to } => {
            let (content, mention_tags) = nip08_mentions(&content);
            let builder = match reply_to {
                Some(event) => EventBuilder::text_note_reply(content, &event, None, None),
                None => EventBuilder::text_note(content),
            }
            .tags(mention_tags);
            match client.sign_event_builder(builder).await {
                Ok(event) => match client.send_event(&event).await {
                    Ok(output) => {
                        // A relay is not required to echo a newly accepted event back to
                        // our subscription. Forward the exact signed event to the UI so
                        // the post appears immediately; a later relay echo is harmless
                        // because App deduplicates events by ID.
                        let _ = ui_tx.send(UiEvent::Event(Box::new(event))).await;
                        Ok(format!("published {}", short_id(&output.id().to_string())))
                    }
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            }
        }
        Command::React { event, reaction } => client
            .send_event_builder(EventBuilder::reaction(&*event, reaction))
            .await
            .map(|_| "reaction sent".to_owned()),
        Command::Repost(event) => client
            .send_event_builder(EventBuilder::repost(&event, None))
            .await
            .map(|_| "reposted".to_owned()),
        Command::VerifyNip05 { .. }
        | Command::FetchProfile(_)
        | Command::FetchEvent(_)
        | Command::FetchImage { .. }
        | Command::Quit => unreachable!(),
    };

    let status = match result {
        Ok(message) => message,
        Err(error) => format!("send failed: {error}"),
    };
    let _ = ui_tx.send(UiEvent::Status(status)).await;
}

const MAX_IMAGE_DOWNLOAD_BYTES: usize = 2 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 2_048;
const MAX_IMAGE_DECODE_ALLOC: u64 = 32 * 1024 * 1024;
const CACHED_IMAGE_SIZE: u32 = 128;

async fn fetch_image(http: &HttpClient, url: &str) -> anyhow::Result<DynamicImage> {
    let mut response = http.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_IMAGE_DOWNLOAD_BYTES as u64)
    {
        anyhow::bail!("image response exceeds download limit");
    }

    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(0)
        .min(MAX_IMAGE_DOWNLOAD_BYTES);
    let mut bytes = Vec::with_capacity(capacity);
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_IMAGE_DOWNLOAD_BYTES {
            anyhow::bail!("image response exceeds download limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        anyhow::bail!("empty image response");
    }

    tokio::task::spawn_blocking(move || decode_image(bytes))
        .await
        .map_err(|error| anyhow::anyhow!("image decoder task failed: {error}"))?
}

fn decode_image(bytes: Vec<u8>) -> anyhow::Result<DynamicImage> {
    decode_raster_image(&bytes).or_else(|raster_error| {
        decode_svg_image(&bytes).map_err(|svg_error| {
            anyhow::anyhow!("unsupported raster image ({raster_error}); invalid SVG ({svg_error})")
        })
    })
}

fn decode_raster_image(bytes: &[u8]) -> anyhow::Result<DynamicImage> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_IMAGE_DECODE_ALLOC);

    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(limits);
    let image = reader.decode()?;
    // Retain only a small, normalized RGBA image. The full decoded source is
    // dropped before the event crosses into the UI task.
    Ok(DynamicImage::ImageRgba8(
        image
            .resize(CACHED_IMAGE_SIZE, CACHED_IMAGE_SIZE, FilterType::Triangle)
            .to_rgba8(),
    ))
}

fn decode_svg_image(bytes: &[u8]) -> anyhow::Result<DynamicImage> {
    // `from_data_nested` deliberately ignores external file references. The
    // SVG itself is untrusted relay content and must not read local resources.
    let tree = resvg::usvg::Tree::from_data_nested(bytes, &resvg::usvg::Options::default())?;
    let source_size = tree.size();
    let source_width = source_size.width();
    let source_height = source_size.height();
    if !source_width.is_finite()
        || !source_height.is_finite()
        || source_width <= 0.0
        || source_height <= 0.0
        || source_width > MAX_IMAGE_DIMENSION as f32
        || source_height > MAX_IMAGE_DIMENSION as f32
    {
        anyhow::bail!("SVG dimensions are invalid or exceed the limit");
    }

    let scale =
        (CACHED_IMAGE_SIZE as f32 / source_width).min(CACHED_IMAGE_SIZE as f32 / source_height);
    let width = (source_width * scale).round().max(1.0) as u32;
    let height = (source_height * scale).round().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| anyhow::anyhow!("could not allocate SVG pixmap"))?;
    let transform = resvg::tiny_skia::Transform::from_scale(
        width as f32 / source_width,
        height as f32 / source_height,
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let pixels = pixmap.take_demultiplied();
    let image = RgbaImage::from_raw(width, height, pixels)
        .ok_or_else(|| anyhow::anyhow!("SVG renderer returned an invalid pixel buffer"))?;
    Ok(DynamicImage::ImageRgba8(image))
}

#[derive(Debug, Deserialize)]
struct Nip05Document {
    #[serde(default)]
    names: HashMap<String, String>,
}

async fn verify_nip05(http: &HttpClient, pubkey: &str, address: &str) -> anyhow::Result<bool> {
    let (name, domain) = address
        .split_once('@')
        .ok_or_else(|| anyhow::anyhow!("invalid NIP-05 address"))?;
    if name.is_empty()
        || domain.is_empty()
        || domain.contains('@')
        || !domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
        || domain.starts_with('.')
        || domain.ends_with('.')
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        anyhow::bail!("invalid NIP-05 address");
    }
    let url = format!("https://{domain}/.well-known/nostr.json?name={name}");
    let document: Nip05Document = http
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(document
        .names
        .get(name)
        .is_some_and(|value| value.eq_ignore_ascii_case(pubkey)))
}

fn short_id(value: &str) -> &str {
    value.get(..8).unwrap_or(value)
}

/// Converts human-friendly NIP-19 mentions to the legacy NIP-08 indexed form.
fn nip08_mentions(content: &str) -> (String, Vec<Tag>) {
    const PREFIXES: [&str; 3] = ["@npub1", "nostr:npub1", "nostr:note1"];
    let mut output = String::with_capacity(content.len());
    let mut tags = Vec::new();
    let mut indexes = HashMap::<String, usize>::new();
    let mut rest = content;

    while let Some((start, prefix)) = PREFIXES
        .iter()
        .filter_map(|prefix| rest.find(prefix).map(|start| (start, *prefix)))
        .min_by_key(|(start, _)| *start)
    {
        output.push_str(&rest[..start]);
        let candidate = &rest[start..];
        let token_len = candidate
            .chars()
            .take_while(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '@' | ':')
            })
            .map(char::len_utf8)
            .sum::<usize>();
        let token = &candidate[..token_len];
        let nip19 = token
            .strip_prefix('@')
            .or_else(|| token.strip_prefix("nostr:"))
            .unwrap_or(token);

        let tag = if prefix.contains("npub") {
            PublicKey::parse(nip19).ok().map(Tag::public_key)
        } else {
            EventId::parse(nip19).ok().map(Tag::event)
        };

        if let Some(tag) = tag {
            let index = *indexes.entry(nip19.to_owned()).or_insert_with(|| {
                let index = tags.len();
                tags.push(tag);
                index
            });
            output.push_str(&format!("#[{index}]"));
        } else {
            output.push_str(token);
        }
        rest = &candidate[token_len..];
    }
    output.push_str(rest);
    (output, tags)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use ::image::{DynamicImage, ImageFormat};
    use nostr_sdk::prelude::*;

    use super::{decode_image, nip08_mentions, short_id, CACHED_IMAGE_SIZE};

    #[test]
    fn short_ids_are_safe() {
        assert_eq!(short_id("123456789"), "12345678");
        assert_eq!(short_id("短い"), "短い");
    }

    #[test]
    fn encodes_nip19_mentions_as_nip08_tags() {
        let npub = Keys::generate().public_key().to_bech32().unwrap();
        let (content, tags) = nip08_mentions(&format!("hello @{npub} and @{npub}!"));
        assert_eq!(content, "hello #[0] and #[0]!");
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].kind(), TagKind::p());
    }

    #[test]
    fn leaves_invalid_mentions_unchanged() {
        let (content, tags) = nip08_mentions("hello @npub1invalid");
        assert_eq!(content, "hello @npub1invalid");
        assert!(tags.is_empty());
    }

    #[test]
    fn image_decoder_normalizes_retained_size() {
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::new_rgba8(512, 256)
            .write_to(&mut encoded, ImageFormat::Png)
            .unwrap();

        let decoded = decode_image(encoded.into_inner()).unwrap();
        assert!(decoded.width() <= CACHED_IMAGE_SIZE);
        assert!(decoded.height() <= CACHED_IMAGE_SIZE);
    }

    #[test]
    fn image_decoder_rejects_excessive_dimensions() {
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::new_rgba8(2_049, 1)
            .write_to(&mut encoded, ImageFormat::Png)
            .unwrap();

        assert!(decode_image(encoded.into_inner()).is_err());
    }

    #[test]
    fn image_decoder_rasterizes_svg_custom_emoji() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="16">
            <rect width="32" height="16" fill="#ff0000"/>
        </svg>"##;

        let decoded = decode_image(svg.to_vec()).unwrap();

        assert_eq!(decoded.width(), CACHED_IMAGE_SIZE);
        assert_eq!(decoded.height(), CACHED_IMAGE_SIZE / 2);
        let pixel = decoded.to_rgba8().get_pixel(64, 32).0;
        assert!(pixel[0] > 240);
        assert!(pixel[1] < 16);
        assert_eq!(pixel[3], 255);
    }

    #[test]
    fn image_decoder_rejects_oversized_svg_dimensions() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="2049" height="16"/>"#;
        assert!(decode_image(svg.to_vec()).is_err());
    }
}
