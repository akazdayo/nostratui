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

mod image;

use image::fetch_image;
#[cfg(test)]
use image::{decode_image, CACHED_IMAGE_SIZE};
mod protocol;

use protocol::{nip08_mentions, short_id, verify_nip05};
#[cfg(test)]
mod tests;
