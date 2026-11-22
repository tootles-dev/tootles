//! Conversion from AppView JSON. Identity is strict; optional counters and future
//! embed variants are tolerated. Facet offsets are checked against original UTF-8.
use crate::model::*;
use anyhow::{Result, anyhow, ensure};
use serde_json::Value;
use url::Url;

pub(super) fn required<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("Bluesky returned an incomplete response (missing {key})"))
}
pub(super) fn array<'a>(v: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    v.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Bluesky returned an incomplete response (missing {key})"))
}
pub(super) fn string(v: &Value, key: &str) -> String {
    clean(v.get(key).and_then(Value::as_str).unwrap_or_default())
}
pub(super) fn clean(s: &str) -> String {
    s.chars().filter(|c| (!c.is_control() || matches!(c, '\n' | '\t')) && !matches!(*c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')).collect()
}
pub(super) fn did(s: &str) -> bool {
    s.len() <= 2048
        && s.starts_with("did:")
        && s.split(':').count() >= 3
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b":._%-".contains(&b))
        && !s.ends_with(':')
}
pub(super) fn actor(s: &str) -> Result<()> {
    ensure!(
        did(s)
            || (s.len() <= 253
                && s.contains('.')
                && s.split('.').all(|part| !part.is_empty()
                    && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))),
        "Invalid Bluesky account identifier"
    );
    Ok(())
}
pub(super) struct RecordUri<'a> {
    pub repo: &'a str,
    pub collection: &'a str,
    pub key: &'a str,
}
pub(super) fn record_uri(s: &str) -> Result<RecordUri<'_>> {
    let parts: Vec<_> = s
        .strip_prefix("at://")
        .unwrap_or_default()
        .split('/')
        .collect();
    ensure!(
        parts.len() == 3 && did(parts[0]),
        "Invalid Bluesky record URI"
    );
    ensure!(
        !parts[1].is_empty()
            && parts[1].contains('.')
            && parts[1]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b)),
        "Invalid Bluesky record collection"
    );
    ensure!(
        !parts[2].is_empty()
            && parts[2].len() <= 512
            && !matches!(parts[2], "." | "..")
            && parts[2]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._~:-".contains(&b)),
        "Invalid Bluesky record key"
    );
    Ok(RecordUri {
        repo: parts[0],
        collection: parts[1],
        key: parts[2],
    })
}
pub(super) fn post_uri(s: &str) -> Result<RecordUri<'_>> {
    let uri = record_uri(s)?;
    ensure!(
        uri.collection == "app.bsky.feed.post",
        "Expected a Bluesky post record"
    );
    Ok(uri)
}
pub(super) fn strong(v: &Value) -> Result<Value> {
    let uri = required(v, "uri")?;
    post_uri(uri)?;
    let cid = required(v, "cid")?;
    ensure!(
        cid.len() <= 256 && cid.bytes().all(|b| b.is_ascii_alphanumeric()),
        "Invalid Bluesky record fingerprint"
    );
    Ok(serde_json::json!({"uri":uri,"cid":cid}))
}
pub(super) fn safe_url(s: &str) -> Option<String> {
    if s.chars().any(|c| c.is_control()) || clean(s) != s {
        return None;
    }
    let u = Url::parse(s).ok()?;
    (matches!(u.scheme(), "http" | "https")
        && u.host_str().is_some()
        && u.username().is_empty()
        && u.password().is_none())
    .then(|| u.into())
}
pub(super) fn account(v: &Value) -> Result<Account> {
    let id = required(v, "did")?;
    ensure!(did(id), "Bluesky returned an invalid account DID");
    let handle = required(v, "handle")?;
    actor(handle)?;
    Ok(Account {
        id: id.into(),
        username: handle.into(),
        acct: handle.into(),
        display_name: string(v, "displayName"),
        note: string(v, "description"),
        url: format!("https://bsky.app/profile/{id}"),
        followers_count: v["followersCount"].as_u64(),
        following_count: v["followsCount"].as_u64(),
        statuses_count: v["postsCount"].as_u64(),
        locked: false,
    })
}
pub(super) fn relation(v: &Value) -> Relationship {
    let v = &v["viewer"];
    Relationship {
        following: v["following"].as_str().is_some(),
        followed_by: v["followedBy"].as_str().is_some(),
        muting: v["muted"].as_bool().unwrap_or(false),
        blocking: v["blocking"].as_str().is_some() || v["blockingByList"].is_object(),
        requested: false,
    }
}
fn labels(v: &Value) -> Vec<String> {
    let mut result = Vec::new();
    if let Some(xs) = v.as_array() {
        for x in xs.iter().take(64) {
            if x["neg"].as_bool() == Some(true) {
                continue;
            }
            if let Some(s) = x["val"]
                .as_str()
                .filter(|s| !s.is_empty() && *s != "!no-unauthenticated")
            {
                let s = clean(s);
                if !result.contains(&s) {
                    result.push(s)
                }
            }
        }
    }
    result
}
pub(super) fn hidden(v: &Value) -> bool {
    v["author"]["viewer"]["blockedBy"].as_bool() == Some(true)
        || v["author"]["viewer"]["blocking"].is_string()
        || v["author"]["viewer"]["blockingByList"].is_object()
        || labels(&v["labels"])
            .iter()
            .chain(labels(&v["author"]["labels"]).iter())
            .any(|s| matches!(s.as_str(), "!hide" | "!takedown"))
}
fn facet_text(record: &Value) -> (String, Vec<String>) {
    let original = record["text"].as_str().unwrap_or_default();
    let mut additions = Vec::new();
    let mut mentions = Vec::new();
    if let Some(facets) = record["facets"].as_array() {
        for facet in facets.iter().take(128) {
            let (Some(start), Some(end)) = (
                facet["index"]["byteStart"].as_u64(),
                facet["index"]["byteEnd"].as_u64(),
            ) else {
                continue;
            };
            let (Ok(start), Ok(end)) = (usize::try_from(start), usize::try_from(end)) else {
                continue;
            };
            if start >= end {
                continue;
            }
            let Some(slice) = original.get(start..end) else {
                continue;
            };
            if let Some(features) = facet["features"].as_array() {
                for feature in features.iter().take(8) {
                    match feature["$type"].as_str().unwrap_or_default() {
                        "app.bsky.richtext.facet#link" => {
                            if let Some(url) = feature["uri"].as_str().and_then(safe_url)
                                && safe_url(slice).as_deref() != Some(&url)
                                && !additions.contains(&url)
                            {
                                additions.push(url);
                            }
                        }
                        "app.bsky.richtext.facet#mention"
                            if feature["did"].as_str().is_some_and(did)
                                && slice.starts_with('@') =>
                        {
                            let name = clean(&slice[1..]);
                            if !mentions.contains(&name) {
                                mentions.push(name)
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    let mut text = clean(original);
    for url in additions {
        text.push_str("\n↗ ");
        text.push_str(&url);
    }
    (text, mentions)
}
fn append(text: &mut String, s: &str) {
    if !s.is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n")
        };
        text.push_str(s)
    }
}
fn embed(v: &Value, post: &mut Post, depth: usize) {
    if depth > 3 {
        return;
    }
    match v["$type"].as_str().unwrap_or_default() {
        "app.bsky.embed.images#view" | "app.bsky.embed.gallery#view" => {
            let gallery = v["$type"].as_str() == Some("app.bsky.embed.gallery#view");
            if let Some(images) = v[if gallery { "items" } else { "images" }].as_array() {
                for image in images.iter().take(20) {
                    if gallery
                        && image["$type"].as_str() != Some("app.bsky.embed.gallery#viewImage")
                    {
                        continue;
                    }
                    if let Some(url) = image["fullsize"].as_str().and_then(safe_url) {
                        post.media.push(Media {
                            id: format!("{}:image:{}", post.id, post.media.len()),
                            kind: "image".into(),
                            url,
                            description: string(image, "alt"),
                        })
                    }
                }
            }
        }
        "app.bsky.embed.video#view" => {
            if let Some(url) = v["playlist"].as_str().and_then(safe_url) {
                post.media.push(Media {
                    id: string(v, "cid"),
                    kind: "video".into(),
                    url,
                    description: string(v, "alt"),
                })
            }
        }
        "app.bsky.embed.external#view" => {
            let v = &v["external"];
            let mut text = string(v, "title");
            let desc = string(v, "description");
            if !desc.is_empty() {
                text.push('\n');
                text.push_str(&desc)
            }
            if let Some(url) = v["uri"].as_str().and_then(safe_url) {
                text.push('\n');
                text.push_str(&url)
            }
            append(&mut post.text, &text);
        }
        "app.bsky.embed.recordWithMedia#view" => {
            embed(&v["media"], post, depth + 1);
            embed(&v["record"], post, depth + 1)
        }
        "app.bsky.embed.record#view" => {
            let quote = &v["record"];
            match quote["$type"].as_str().unwrap_or_default() {
                "app.bsky.embed.record#viewRecord" => {
                    if hidden(quote) {
                        append(&mut post.text, "[Quoted post unavailable]");
                        return;
                    }
                    let mut warnings = labels(&quote["labels"]);
                    warnings.extend(labels(&quote["author"]["labels"]));
                    warnings.extend(labels(&quote["value"]["labels"]["values"]));
                    if quote["author"]["viewer"]["muted"].as_bool() == Some(true)
                        || quote["author"]["viewer"]["mutedByList"].is_object()
                    {
                        warnings.push("Muted account".into());
                    }
                    warnings.sort();
                    warnings.dedup();
                    // A quoted warning must cover the containing post too; its text
                    // and images must never bypass the existing reveal control.
                    if !warnings.is_empty() {
                        post.sensitive = true;
                        append(
                            &mut post.spoiler_text,
                            &format!("Quoted content: {}", warnings.join(", ")),
                        );
                    }
                    if warnings
                        .iter()
                        .any(|s| matches!(s.as_str(), "!hide" | "!takedown"))
                    {
                        append(&mut post.text, "[Quoted post unavailable]");
                        return;
                    }
                    let who = account(&quote["author"])
                        .map(|a| a.name().to_owned())
                        .unwrap_or_else(|_| "Unknown account".into());
                    let (body, _) = facet_text(&quote["value"]);
                    append(&mut post.text, &format!("↳ {who}\n{body}"));
                    if let Some(uri) = quote["uri"].as_str().and_then(|uri| post_uri(uri).ok()) {
                        append(
                            &mut post.text,
                            &format!("https://bsky.app/profile/{}/post/{}", uri.repo, uri.key),
                        );
                    }
                    if let Some(embeds) = quote["embeds"].as_array() {
                        for e in embeds.iter().take(4) {
                            embed(e, post, depth + 1)
                        }
                    }
                }
                "app.bsky.embed.record#viewBlocked"
                | "app.bsky.embed.record#viewNotFound"
                | "app.bsky.embed.record#viewDetached" => {
                    append(&mut post.text, "[Quoted post unavailable]")
                }
                _ => {
                    let title = string(quote, "name");
                    if !title.is_empty() {
                        append(&mut post.text, &format!("Shared: {title}"));
                    }
                }
            }
        }
        _ => {}
    }
}
pub(super) fn post(v: &Value) -> Result<Post> {
    let id = required(v, "uri")?;
    let uri = post_uri(id)?;
    strong(v)?;
    let author = account(&v["author"])?;
    ensure!(
        author.id == uri.repo,
        "Bluesky returned a post with inconsistent author identity"
    );
    let record = &v["record"];
    ensure!(
        record["text"].is_string(),
        "Bluesky returned an incomplete post (missing text)"
    );
    let (text, mentions) = facet_text(record);
    let mut warnings = labels(&v["labels"]);
    warnings.extend(labels(&v["author"]["labels"]));
    if v["author"]["viewer"]["muted"].as_bool() == Some(true)
        || v["author"]["viewer"]["mutedByList"].is_object()
    {
        warnings.push("Muted account".into());
    }
    warnings.extend(labels(&record["labels"]["values"]));
    warnings.sort();
    warnings.dedup();
    let mut p = Post {
        id: id.into(),
        timeline_id: id.into(),
        account: author,
        text,
        url: format!("https://bsky.app/profile/{}/post/{}", uri.repo, uri.key),
        created_at: record["createdAt"]
            .as_str()
            .or_else(|| v["indexedAt"].as_str())
            .map(clean)
            .unwrap_or_default(),
        spoiler_text: if warnings.is_empty() {
            String::new()
        } else {
            format!("Content labels: {}", warnings.join(", "))
        },
        sensitive: !warnings.is_empty(),
        visibility: Visibility::Public,
        favourites_count: v["likeCount"].as_u64().unwrap_or_default(),
        reblogs_count: v["repostCount"].as_u64().unwrap_or_default(),
        replies_count: v["replyCount"].as_u64().unwrap_or_default(),
        favourited: v["viewer"]["like"].is_string(),
        reblogged: v["viewer"]["repost"].is_string(),
        bookmarked: v["viewer"]["bookmarked"].as_bool().unwrap_or(false),
        in_reply_to_id: record["reply"]["parent"]["uri"]
            .as_str()
            .filter(|id| post_uri(id).is_ok())
            .map(str::to_owned),
        mentions,
        ..Post::default()
    };
    embed(&v["embed"], &mut p, 0);
    Ok(p)
}
pub(super) fn feed_post(v: &Value) -> Result<Post> {
    let mut post = post(&v["post"])?;
    if v["reason"]["$type"].as_str() == Some("app.bsky.feed.defs#reasonRepost") {
        post.boosted_by = Some(account(&v["reason"]["by"])?.name().into());
        post.timeline_id = v["reason"]["uri"]
            .as_str()
            .filter(|s| record_uri(s).is_ok())
            .unwrap_or(&post.id)
            .to_owned();
    }
    Ok(post)
}
pub(super) fn cursor(v: &Value) -> Option<String> {
    v["cursor"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 16384)
        .map(str::to_owned)
}
