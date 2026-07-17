use std::{collections::HashMap, time::Duration};

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
    let client = match config.secret_key {
        Some(secret) => {
            let keys = Keys::parse(&secret)?;
            ui_tx
                .send(UiEvent::Identity(keys.public_key().to_bech32()?))
                .await?;
            Client::new(keys)
        }
        None => {
            ui_tx
                .send(UiEvent::Identity("read-only".to_owned()))
                .await?;
            Client::default()
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
    let filter = Filter::new()
        .kinds([Kind::Metadata, Kind::TextNote, Kind::Repost, Kind::Reaction])
        .limit(config.limit);
    client.subscribe(filter, None).await?;
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
                        ui_tx.send(UiEvent::Event(event)).await?;
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
            client
                .send_event_builder(builder)
                .await
                .map(|output| format!("published {}", short_id(&output.id().to_string())))
        }
        Command::React { event, reaction } => client
            .send_event_builder(EventBuilder::reaction(&*event, reaction))
            .await
            .map(|_| "reaction sent".to_owned()),
        Command::Repost(event) => client
            .send_event_builder(EventBuilder::repost(&event, None))
            .await
            .map(|_| "reposted".to_owned()),
        Command::VerifyNip05 { .. } | Command::FetchProfile(_) | Command::Quit => unreachable!(),
    };

    let status = match result {
        Ok(message) => message,
        Err(error) => format!("send failed: {error}"),
    };
    let _ = ui_tx.send(UiEvent::Status(status)).await;
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
    use nostr_sdk::prelude::*;

    use super::{nip08_mentions, short_id};

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
}
