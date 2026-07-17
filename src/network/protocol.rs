use super::*;

#[derive(Debug, Deserialize)]
struct Nip05Document {
    #[serde(default)]
    names: HashMap<String, String>,
}

pub(super) async fn verify_nip05(
    http: &HttpClient,
    pubkey: &str,
    address: &str,
) -> anyhow::Result<bool> {
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

pub(super) fn short_id(value: &str) -> &str {
    value.get(..8).unwrap_or(value)
}

/// Converts human-friendly NIP-19 mentions to the legacy NIP-08 indexed form.
pub(super) fn nip08_mentions(content: &str) -> (String, Vec<Tag>) {
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
