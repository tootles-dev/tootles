//! Bluesky AppView/AT Protocol adapter. The session transport owns authentication
//! and PDS routing; this module never constructs a second authenticated client.
mod wire;
use super::SocialProvider;
use crate::{
    bluesky_auth::{SessionClient, is_record_not_found},
    config::SavedAccount,
    model::*,
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use tokio::io::AsyncReadExt;
use unicode_segmentation::UnicodeSegmentation;
use wire::{account, array, post_uri, record_uri, required, strong};

const POST: &str = "app.bsky.feed.post";
const MAX_IMAGE_BYTES: u64 = 2_000_000;

pub struct Bluesky {
    client: SessionClient,
    publishing: tokio::sync::Mutex<()>,
    actions: tokio::sync::Mutex<()>,
    pending_refs: tokio::sync::Mutex<HashMap<(String, String), Option<String>>>,
    me: tokio::sync::RwLock<Option<Account>>,
}
impl Bluesky {
    pub fn from_saved(saved: &SavedAccount, root: Option<PathBuf>) -> Result<Self> {
        Ok(Self {
            client: SessionClient::from_saved(saved, root)?,
            publishing: tokio::sync::Mutex::new(()),
            actions: tokio::sync::Mutex::new(()),
            pending_refs: tokio::sync::Mutex::new(HashMap::new()),
            me: tokio::sync::RwLock::new(None),
        })
    }
    // AppView can lag behind a confirmed PDS write. Keep the created/deleted
    // reference until the view acknowledges it, so an immediate undo acts on
    // the real record instead of silently doing nothing or creating duplicates.
    async fn current_ref(
        &self,
        subject: &str,
        collection: &str,
        viewed: Option<&str>,
    ) -> Option<String> {
        let key = (subject.to_owned(), collection.to_owned());
        let mut pending = self.pending_refs.lock().await;
        if let Some(known) = pending.get(&key) {
            if known.as_deref() != viewed {
                return known.clone();
            }
            pending.remove(&key);
        }
        viewed.map(str::to_owned)
    }
    async fn remember_ref(&self, subject: &str, collection: &str, uri: Option<String>) {
        self.pending_refs
            .lock()
            .await
            .insert((subject.to_owned(), collection.to_owned()), uri);
    }
    async fn reconcile_post(&self, mut post: Post, view: &Value) -> Post {
        post.favourited = self
            .current_ref(
                &post.id,
                "app.bsky.feed.like",
                view["viewer"]["like"].as_str(),
            )
            .await
            .is_some();
        post.reblogged = self
            .current_ref(
                &post.id,
                "app.bsky.feed.repost",
                view["viewer"]["repost"].as_str(),
            )
            .await
            .is_some();
        post
    }
    fn page_query(cursor: Option<&str>) -> Result<Vec<(&'static str, String)>> {
        let mut q = vec![("limit", "30".into())];
        if let Some(c) = cursor {
            ensure!(c.len() <= 16384, "Bluesky pagination cursor is too large");
            q.push(("cursor", c.into()));
        }
        Ok(q)
    }
    async fn post_view(&self, id: &str) -> Result<Value> {
        post_uri(id)?;
        let value = self
            .client
            .get("app.bsky.feed.getPosts", &[("uris", id.into())])
            .await?;
        let p = array(&value, "posts")?
            .iter()
            .find(|p| p["uri"].as_str() == Some(id))
            .ok_or_else(|| anyhow!("This Bluesky post is unavailable or was deleted"))?;
        ensure!(!wire::hidden(p), "This Bluesky post is unavailable");
        wire::post(p)?;
        Ok(p.clone())
    }
    async fn actor_view(&self, id: &str) -> Result<Value> {
        wire::actor(id)?;
        let value = self
            .client
            .get("app.bsky.actor.getProfile", &[("actor", id.into())])
            .await?;
        let a = account(&value)?;
        if wire::did(id) {
            ensure!(
                a.id == id,
                "Bluesky returned a different account than requested"
            );
        }
        Ok(value)
    }
    async fn delete_record(&self, uri: &str, collection: &str) -> Result<()> {
        let r = record_uri(uri)?;
        ensure!(
            r.repo == self.client.did() && r.collection == collection,
            "Only your own matching Bluesky record can be deleted"
        );
        self.client
            .post(
                "com.atproto.repo.deleteRecord",
                &json!({"repo":self.client.did(),"collection":collection,"rkey":r.key}),
            )
            .await?;
        Ok(())
    }
    async fn create_record(
        &self,
        collection: &str,
        record: Value,
        key: Option<&str>,
    ) -> Result<Value> {
        let mut body = json!({"repo":self.client.did(),"collection":collection,"record":record,"validate":true});
        if let Some(key) = key {
            body["rkey"] = key.into();
        }
        let value = self
            .client
            .post("com.atproto.repo.createRecord", &body)
            .await?;
        let uri = record_uri(required(&value, "uri")?)?;
        ensure!(
            uri.repo == self.client.did()
                && uri.collection == collection
                && key.is_none_or(|key| uri.key == key),
            "Bluesky returned an inconsistent created record identity"
        );
        required(&value, "cid")?;
        Ok(value)
    }
    async fn existing_record(&self, key: &str) -> Result<Option<Value>> {
        match self
            .client
            .get(
                "com.atproto.repo.getRecord",
                &[
                    ("repo", self.client.did().into()),
                    ("collection", POST.into()),
                    ("rkey", key.into()),
                ],
            )
            .await
        {
            Ok(value) => {
                let id = required(&value, "uri")?;
                let uri = post_uri(id)?;
                ensure!(
                    uri.repo == self.client.did() && uri.key == key,
                    "Bluesky returned an inconsistent saved post identity"
                );
                strong(&value)?;
                ensure!(
                    value["value"].is_object(),
                    "Bluesky returned an incomplete saved post"
                );
                Ok(Some(value))
            }
            Err(error) if is_record_not_found(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }
    async fn published_post(
        &self,
        uri: &str,
        record: &Value,
        author: &Account,
        cid: &str,
    ) -> Result<Post> {
        // AppView indexing is asynchronous. A confirmed PDS write is successful
        // even if it takes a moment for the post to appear in the public feed.
        if let Ok(view) = self.post_view(uri).await {
            return wire::post(&view);
        }
        let mut post = wire::post(
            &json!({"uri":uri,"cid":cid,"author":{"did":author.id,"handle":author.acct,"displayName":author.display_name},"record":record}),
        )?;
        post.account = author.clone();
        Ok(post)
    }
    async fn facets(&self, text: &str) -> Result<Vec<Value>> {
        let links = crate::links::find(text);
        let mut facets = Vec::new();
        for link in &links {
            facets.push(json!({"index":{"byteStart":link.range.start,"byteEnd":link.range.end},"features":[{"$type":"app.bsky.richtext.facet#link","uri":link.url}]}));
        }
        let mut resolved: HashMap<String, String> = HashMap::new();
        for (start, c) in text.char_indices() {
            if facets.len() >= 64 {
                break;
            }
            if !matches!(c, '@' | '#') || links.iter().any(|l| l.range.contains(&start)) {
                continue;
            }
            if start > 0
                && text[..start]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '@' | '#' | '/'))
            {
                continue;
            }
            let tail = &text[start + 1..];
            let length = tail
                .char_indices()
                .take_while(|(_, c)| {
                    if c.is_ascii() && *c == '-' {
                        true
                    } else if text.as_bytes()[start] == b'@' {
                        c.is_ascii_alphanumeric() || *c == '.'
                    } else {
                        c.is_alphanumeric() || *c == '_'
                    }
                })
                .map(|(i, c)| i + c.len_utf8())
                .last()
                .unwrap_or_default();
            let token = tail[..length].trim_end_matches(['.', '-']);
            if token.is_empty() {
                continue;
            }
            let end = start + 1 + token.len();
            let feature = if c == '#' {
                if token.len() > 640
                    || token.graphemes(true).count() > 64
                    || token.chars().all(|c| c.is_numeric())
                {
                    continue;
                }
                json!({"$type":"app.bsky.richtext.facet#tag","tag":token})
            } else {
                if wire::actor(token).is_err() || !token.contains('.') {
                    continue;
                }
                let id = if let Some(id) = resolved.get(token) {
                    id.clone()
                } else {
                    let value=self.client.get("com.atproto.identity.resolveHandle",&[("handle",token.to_ascii_lowercase())]).await.context("Could not resolve a mentioned Bluesky handle; check its spelling or remove the @ mention")?;
                    let id = required(&value, "did")?;
                    ensure!(
                        wire::did(id),
                        "Bluesky returned an invalid mentioned account"
                    );
                    resolved.insert(token.to_owned(), id.to_owned());
                    id.to_owned()
                };
                json!({"$type":"app.bsky.richtext.facet#mention","did":id})
            };
            facets.push(json!({"index":{"byteStart":start,"byteEnd":end},"features":[feature]}));
        }
        facets.sort_by_key(|v| v["index"]["byteStart"].as_u64().unwrap_or_default());
        Ok(facets)
    }
}

#[async_trait]
impl SocialProvider for Bluesky {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            name: "Bluesky",
            posting: true,
            polls: false,
            media: true,
            lists: false,
            max_characters: 300,
            editing: false,
            bookmarks: true,
            local_timeline: false,
            federated_timeline: false,
            post_visibility: false,
            content_warnings: false,
            max_media_attachments: 4,
            image_only_media: true,
            grapheme_counting: true,
            max_text_bytes: Some(3000),
        }
    }
    async fn me(&self) -> Result<Account> {
        let a = account(&self.actor_view(self.client.did()).await?)?;
        *self.me.write().await = Some(a.clone());
        Ok(a)
    }
    async fn feed(&self, feed: &Feed, cursor: Option<&str>) -> Result<Page<Post>> {
        let mut query = Self::page_query(cursor)?;
        let (method, field, wrapped) = match feed {
            Feed::Home => ("app.bsky.feed.getTimeline", "feed", true),
            Feed::Profile(id) => {
                wire::actor(id)?;
                query.push(("actor", id.clone()));
                query.push(("filter", "posts_with_replies".into()));
                ("app.bsky.feed.getAuthorFeed", "feed", true)
            }
            Feed::Favourites => {
                query.push(("actor", self.client.did().into()));
                ("app.bsky.feed.getActorLikes", "feed", true)
            }
            Feed::Search(q) => {
                query.push(("q", q.clone()));
                query.push(("sort", "latest".into()));
                ("app.bsky.feed.searchPosts", "posts", false)
            }
            Feed::Tag(tag) => {
                let tag = tag.trim_start_matches('#');
                ensure!(!tag.is_empty(), "Enter a hashtag");
                query.push(("q", format!("#{tag}")));
                query.push(("tag", tag.into()));
                query.push(("sort", "latest".into()));
                ("app.bsky.feed.searchPosts", "posts", false)
            }
            Feed::Bookmarks => ("app.bsky.bookmark.getBookmarks", "bookmarks", false),
            Feed::Thread(id) => {
                let thread = self.context(id).await?;
                let mut items = thread.ancestors;
                items.push(self.post(id).await?);
                items.extend(thread.descendants);
                return Ok(Page { items, next: None });
            }
            Feed::Local | Feed::Federated => {
                bail!("Bluesky has no local or federated timeline; use Home or Search")
            }
            Feed::List(..) => bail!("Bluesky list feeds are not supported in Tootles yet"),
            Feed::Notifications => bail!("Use the notifications view for Bluesky activity"),
        };
        let value = self.client.get(method, &query).await?;
        let mut items = Vec::new();
        for item in array(&value, field)?.iter().take(100) {
            let v = if field == "bookmarks" {
                &item["item"]
            } else if wrapped {
                &item["post"]
            } else {
                item
            };
            if field == "bookmarks"
                && matches!(
                    v["$type"].as_str(),
                    Some("app.bsky.feed.defs#blockedPost" | "app.bsky.feed.defs#notFoundPost")
                )
            {
                continue;
            }
            if wire::hidden(v) {
                continue;
            }
            let mut post = if wrapped {
                wire::feed_post(item)?
            } else {
                wire::post(v)?
            };
            if field == "bookmarks" {
                post.bookmarked = true;
            }
            items.push(self.reconcile_post(post, v).await);
        }
        Ok(Page {
            items,
            next: wire::cursor(&value),
        })
    }
    async fn notifications(&self, cursor: Option<&str>) -> Result<Page<Notification>> {
        let value = self
            .client
            .get(
                "app.bsky.notification.listNotifications",
                &Self::page_query(cursor)?,
            )
            .await?;
        let entries = array(&value, "notifications")?;
        let mut uris = Vec::<String>::new();
        for entry in entries.iter().take(100) {
            let reason = entry["reason"].as_str().unwrap_or_default();
            let target = if matches!(reason, "reply" | "mention" | "quote" | "subscribed-post") {
                entry["uri"].as_str()
            } else {
                entry["reasonSubject"].as_str()
            };
            if let Some(uri) = target.filter(|s| post_uri(s).is_ok())
                && !uris.iter().any(|s| s == uri)
            {
                uris.push(uri.into())
            }
        }
        let mut posts = HashMap::new();
        for chunk in uris.chunks(25) {
            let query: Vec<_> = chunk.iter().map(|uri| ("uris", uri.clone())).collect();
            let value = self.client.get("app.bsky.feed.getPosts", &query).await?;
            for view in array(&value, "posts")? {
                if !wire::hidden(view) {
                    let post = self.reconcile_post(wire::post(view)?, view).await;
                    posts.insert(post.id.clone(), post);
                }
            }
        }
        let mut items = Vec::new();
        for entry in entries.iter().take(100) {
            let id = required(entry, "uri")?;
            record_uri(id)?;
            let reason = required(entry, "reason")?;
            let target = if matches!(reason, "reply" | "mention" | "quote" | "subscribed-post") {
                Some(id)
            } else {
                entry["reasonSubject"].as_str()
            };
            let kind = match reason {
                "like" | "like-via-repost" => "favourite",
                "repost" | "repost-via-repost" => "reblog",
                "reply" | "quote" => "mention",
                other => other,
            };
            items.push(Notification {
                id: format!("{id}#{reason}"),
                kind: wire::clean(kind),
                account: account(&entry["author"])?,
                post: target.and_then(|id| posts.get(id).cloned()),
                created_at: wire::string(entry, "indexedAt"),
            });
        }
        Ok(Page {
            items,
            next: wire::cursor(&value),
        })
    }
    async fn search(&self, query: &str) -> Result<SearchResults> {
        let feed = Feed::Search(query.into());
        let query_args = [
            ("q", query.trim_start_matches('@').into()),
            ("limit", "20".into()),
        ];
        let (posts, people) = tokio::try_join!(
            self.feed(&feed, None),
            self.client.get("app.bsky.actor.searchActors", &query_args)
        )?;
        Ok(SearchResults {
            posts: posts.items,
            accounts: array(&people, "actors")?
                .iter()
                .take(100)
                .map(account)
                .collect::<Result<_>>()?,
            hashtags: if let Some(tag) = query
                .strip_prefix('#')
                .filter(|s| !s.is_empty() && !s.contains(char::is_whitespace))
            {
                vec![wire::clean(tag)]
            } else {
                vec![]
            },
        })
    }
    async fn context(&self, id: &str) -> Result<Thread> {
        post_uri(id)?;
        let value = self
            .client
            .get(
                "app.bsky.feed.getPostThread",
                &[
                    ("uri", id.into()),
                    ("depth", "6".into()),
                    ("parentHeight", "80".into()),
                ],
            )
            .await?;
        let root = &value["thread"];
        ensure!(
            root["post"]["uri"].as_str() == Some(id),
            "This Bluesky conversation is unavailable"
        );
        wire::post(&root["post"])?;
        let mut thread = Thread::default();
        let mut current = &root["parent"];
        for _ in 0..80 {
            if !current["post"].is_object() {
                break;
            }
            if !wire::hidden(&current["post"]) {
                thread.ancestors.push(wire::post(&current["post"])?);
            }
            current = &current["parent"];
        }
        thread.ancestors.reverse();
        let mut queue: Vec<_> = root["replies"]
            .as_array()
            .map(|a| a.iter().rev().collect())
            .unwrap_or_default();
        let mut visited = std::collections::HashSet::new();
        visited.insert(id.to_owned());
        while let Some(node) = queue.pop() {
            if thread.descendants.len() >= 300 {
                break;
            }
            if node["post"].is_object() && !wire::hidden(&node["post"]) {
                let post = wire::post(&node["post"])?;
                if !visited.insert(post.id.clone()) {
                    continue;
                }
                thread.descendants.push(post);
                if let Some(replies) = node["replies"].as_array() {
                    queue.extend(replies.iter().rev().take(300));
                }
            }
        }
        Ok(thread)
    }
    async fn post(&self, id: &str) -> Result<Post> {
        let view = self.post_view(id).await?;
        Ok(self.reconcile_post(wire::post(&view)?, &view).await)
    }
    async fn profile(&self, id: &str) -> Result<Account> {
        account(&self.actor_view(id).await?)
    }
    async fn relationship(&self, id: &str) -> Result<Relationship> {
        let view = self.actor_view(id).await?;
        let who = account(&view)?;
        let mut relation = wire::relation(&view);
        relation.following = self
            .current_ref(
                &who.id,
                "app.bsky.graph.follow",
                view["viewer"]["following"].as_str(),
            )
            .await
            .is_some();
        relation.blocking = view["viewer"]["blockingByList"].is_object()
            || self
                .current_ref(
                    &who.id,
                    "app.bsky.graph.block",
                    view["viewer"]["blocking"].as_str(),
                )
                .await
                .is_some();
        Ok(relation)
    }
    async fn post_action(&self, id: &str, action: PostAction) -> Result<Option<Post>> {
        let _guard = self.actions.lock().await;
        if matches!(action, PostAction::Delete) {
            self.delete_record(id, POST).await?;
            return Ok(None);
        }
        let view = self.post_view(id).await?;
        let mut post = wire::post(&view)?;
        match action {
            PostAction::Favourite(want) | PostAction::Boost(want) => {
                let (field, collection) = if matches!(action, PostAction::Favourite(_)) {
                    ("like", "app.bsky.feed.like")
                } else {
                    ("repost", "app.bsky.feed.repost")
                };
                let existing = self
                    .current_ref(id, collection, view["viewer"][field].as_str())
                    .await;
                if want && existing.is_none() {
                    let created = self.create_record(collection,json!({"$type":collection,"subject":strong(&view)?,"createdAt":Utc::now().to_rfc3339()}),None).await?;
                    self.remember_ref(id, collection, Some(required(&created, "uri")?.into()))
                        .await;
                } else if !want && let Some(uri) = existing {
                    self.delete_record(&uri, collection).await?;
                    self.remember_ref(id, collection, None).await;
                }
                if field == "like" {
                    if post.favourited != want {
                        post.favourites_count = if want {
                            post.favourites_count.saturating_add(1)
                        } else {
                            post.favourites_count.saturating_sub(1)
                        }
                    }
                    post.favourited = want;
                } else {
                    if post.reblogged != want {
                        post.reblogs_count = if want {
                            post.reblogs_count.saturating_add(1)
                        } else {
                            post.reblogs_count.saturating_sub(1)
                        }
                    }
                    post.reblogged = want;
                }
            }
            PostAction::Bookmark(want) => {
                if want {
                    self.client
                        .post("app.bsky.bookmark.createBookmark", &strong(&view)?)
                        .await?;
                } else {
                    self.client
                        .post("app.bsky.bookmark.deleteBookmark", &json!({"uri":id}))
                        .await?;
                }
                post.bookmarked = want;
            }
            PostAction::Delete => unreachable!(),
        }
        Ok(Some(post))
    }
    async fn account_action(&self, id: &str, action: AccountAction) -> Result<Relationship> {
        let _guard = self.actions.lock().await;
        let view = self.actor_view(id).await?;
        let who = account(&view)?;
        ensure!(
            who.id != self.client.did(),
            "Choose another account for this action"
        );
        let mut relationship = wire::relation(&view);
        match action {
            AccountAction::Mute(want) => {
                self.client
                    .post(
                        if want {
                            "app.bsky.graph.muteActor"
                        } else {
                            "app.bsky.graph.unmuteActor"
                        },
                        &json!({"actor":who.id}),
                    )
                    .await?;
                relationship.muting = want;
            }
            AccountAction::Follow(want) | AccountAction::Block(want) => {
                let (field, collection) = if matches!(action, AccountAction::Follow(_)) {
                    ("following", "app.bsky.graph.follow")
                } else {
                    ("blocking", "app.bsky.graph.block")
                };
                let existing = self
                    .current_ref(&who.id, collection, view["viewer"][field].as_str())
                    .await;
                if want && existing.is_none() {
                    let created = self.create_record(collection,json!({"$type":collection,"subject":who.id,"createdAt":Utc::now().to_rfc3339()}),None).await?;
                    self.remember_ref(&who.id, collection, Some(required(&created, "uri")?.into()))
                        .await;
                } else if !want {
                    if let Some(uri) = existing {
                        self.delete_record(&uri, collection).await?;
                        self.remember_ref(&who.id, collection, None).await;
                    } else if field == "blocking" && relationship.blocking {
                        bail!(
                            "This account is blocked by a moderation list; manage that list in Bluesky"
                        );
                    }
                }
                if field == "following" {
                    relationship.following = want
                } else {
                    relationship.blocking = want
                }
            }
        }
        Ok(relationship)
    }
    async fn publish(&self, compose: &Compose) -> Result<Post> {
        validate_compose(compose)?;
        let _guard = self.publishing.lock().await;
        let key = publication_key(&compose.idempotency_key);
        let images = prepare_images(&compose.attachments).await?;
        let cached_author = self.me.read().await.clone();
        let author = if let Some(a) = cached_author {
            a
        } else {
            self.me().await?
        };
        if let Some(existing) = self.existing_record(&key).await? {
            validate_existing(&existing["value"], compose, &images)?;
            return self
                .published_post(
                    required(&existing, "uri")?,
                    &existing["value"],
                    &author,
                    required(&existing, "cid")?,
                )
                .await;
        }
        let mut record =
            json!({"$type":POST,"text":compose.text,"createdAt":Utc::now().to_rfc3339()});
        let facets = self.facets(&compose.text).await?;
        if !facets.is_empty() {
            record["facets"] = facets.into();
        }
        if !compose.language.is_empty() {
            record["langs"] = json!([compose.language]);
        }
        if let Some(parent) = &compose.reply_to {
            let view = self.post_view(parent).await?;
            ensure!(
                view["viewer"]["replyDisabled"].as_bool() != Some(true),
                "Replies are disabled for this Bluesky post"
            );
            let parent = strong(&view)?;
            let root = if view["record"]["reply"].is_object() {
                strong(&view["record"]["reply"]["root"])?
            } else {
                parent.clone()
            };
            record["reply"] = json!({"root":root,"parent":parent});
        }
        if !images.is_empty() {
            let mut embeds = Vec::new();
            for (image, attachment) in images.iter().zip(&compose.attachments) {
                let blob = self.client.upload(image.mime, &image.bytes).await?;
                ensure!(
                    blob["blob"]["$type"].as_str() == Some("blob")
                        && blob["blob"]["ref"]["$link"].as_str() == Some(image.cid.as_str())
                        && blob["blob"]["mimeType"].as_str() == Some(image.mime)
                        && blob["blob"]["size"].as_u64() == Some(image.bytes.len() as u64),
                    "Bluesky returned invalid uploaded image metadata"
                );
                embeds.push(json!({"image":blob["blob"],"alt":attachment.description,"aspectRatio":{"width":image.width,"height":image.height}}));
            }
            record["embed"] = json!({"$type":"app.bsky.embed.images","images":embeds});
        }
        let created = match self.create_record(POST, record.clone(), Some(&key)).await {
            Ok(value) => value,
            Err(error) => {
                // A timeout can occur after the PDS committed. Query the exact
                // caller-owned key; never issue a second record with a new key.
                if let Ok(Some(existing)) = self.existing_record(&key).await {
                    validate_existing(&existing["value"], compose, &images)?;
                    return self
                        .published_post(
                            required(&existing, "uri")?,
                            &existing["value"],
                            &author,
                            required(&existing, "cid")?,
                        )
                        .await;
                }
                return Err(error);
            }
        };
        self.published_post(
            required(&created, "uri")?,
            &record,
            &author,
            required(&created, "cid")?,
        )
        .await
    }
    async fn source(&self, _id: &str) -> Result<Compose> {
        bail!("Bluesky does not support editing published posts")
    }
    async fn vote(&self, _id: &str, _choices: &[usize]) -> Result<Poll> {
        bail!("Bluesky does not support polls")
    }
    async fn lists(&self) -> Result<Vec<SocialList>> {
        bail!("Bluesky list management is not supported in Tootles yet")
    }
}

fn validate_compose(c: &Compose) -> Result<()> {
    ensure!(
        c.edit_id.is_none(),
        "Bluesky does not support editing published posts"
    );
    ensure!(
        c.visibility == Visibility::Public,
        "Bluesky posts are public; private and unlisted posts are not supported"
    );
    ensure!(c.poll_options.is_empty(), "Bluesky does not support polls");
    ensure!(
        c.spoiler_text.is_empty() && !c.sensitive,
        "Bluesky does not support freeform content warnings; use the Bluesky app to apply content labels"
    );
    ensure!(
        c.text.graphemes(true).count() <= 300 && c.text.len() <= 3000,
        "Bluesky posts must fit 300 graphemes and 3,000 UTF-8 bytes"
    );
    ensure!(
        wire::clean(&c.text) == c.text,
        "Post text contains unsupported terminal control characters"
    );
    ensure!(
        !c.text.trim().is_empty() || !c.attachments.is_empty(),
        "Write a post or attach an image"
    );
    ensure!(
        c.attachments.len() <= 4,
        "Tootles supports up to four Bluesky images per post"
    );
    ensure!(
        !c.idempotency_key.is_empty() && c.idempotency_key.len() <= 256,
        "A stable draft key is required to publish safely"
    );
    ensure!(
        c.language.is_empty()
            || (c.language.len() <= 64
                && c.language
                    .split('-')
                    .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric()))),
        "Enter a valid language tag, such as en or en-US"
    );
    if let Some(parent) = &c.reply_to {
        post_uri(parent)?;
    }
    Ok(())
}
/// TID alphabet and shape follow the AT Protocol record-key specification. New
/// drafts carry creation microseconds; legacy keys get a deterministic fallback.
fn publication_key(key: &str) -> String {
    let hash = Sha256::digest(key.as_bytes());
    let entropy = u64::from_be_bytes(hash[..8].try_into().expect("digest length"));
    let value = key
        .split_once(':')
        .and_then(|(time, _)| time.parse::<u64>().ok())
        .filter(|time| *time < (1u64 << 53))
        .map(|time| (time << 10) | (entropy & 1023))
        .unwrap_or(entropy & i64::MAX as u64);
    let alphabet = b"234567abcdefghijklmnopqrstuvwxyz";
    (0..13)
        .rev()
        .map(|i| alphabet[((value >> (i * 5)) & 31) as usize] as char)
        .collect()
}
struct UploadImage {
    bytes: Vec<u8>,
    cid: String,
    mime: &'static str,
    width: u32,
    height: u32,
}
async fn prepare_images(attachments: &[Attachment]) -> Result<Vec<UploadImage>> {
    let mut result = Vec::new();
    // Validate every attachment before the first network upload, including a
    // bounded read in case a file grows after its metadata was inspected.
    for attachment in attachments {
        ensure!(
            attachment.description.len() <= 100_000
                && attachment.description.graphemes(true).count() <= 10_000,
            "Keep image alt text within 10,000 graphemes and 100,000 UTF-8 bytes"
        );
        ensure!(
            wire::clean(&attachment.description) == attachment.description,
            "Image alt text contains unsupported terminal control characters"
        );
        let path = Path::new(&attachment.path);
        let file = tokio::fs::File::open(path)
            .await
            .context("Cannot open Bluesky image attachment")?;
        let metadata = file
            .metadata()
            .await
            .context("Cannot read image metadata")?;
        ensure!(
            metadata.is_file(),
            "Attachments must be regular image files"
        );
        ensure!(
            metadata.len() <= MAX_IMAGE_BYTES,
            "Each Bluesky image must be at most 2,000,000 bytes (2 MB)"
        );
        let mut bytes = Vec::new();
        file.take(MAX_IMAGE_BYTES + 1)
            .read_to_end(&mut bytes)
            .await
            .context("Cannot read image attachment")?;
        ensure!(
            bytes.len() as u64 <= MAX_IMAGE_BYTES,
            "Each Bluesky image must be at most 2,000,000 bytes (2 MB)"
        );
        ensure!(
            matches!(
                image::guess_format(&bytes),
                Ok(image::ImageFormat::Png
                    | image::ImageFormat::Jpeg
                    | image::ImageFormat::Gif
                    | image::ImageFormat::WebP)
            ),
            "Bluesky attachments in Tootles must be PNG, JPEG, GIF, or WebP images; video and audio uploads are not supported"
        );
        let parsed =
            tokio::task::spawn_blocking(move || crate::media_source::decode_original(bytes))
                .await
                .context("Image validation worker failed")??;
        let mime = match parsed.extension() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            _ => bail!(
                "Bluesky attachments in Tootles must be PNG, JPEG, GIF, or WebP images; video and audio uploads are not supported"
            ),
        };
        // decode_original's preview may be resized; use encoded image dimensions
        // for the embed's aspect ratio, without another full decode.
        let dimensions = image::ImageReader::new(std::io::Cursor::new(parsed.bytes()))
            .with_guessed_format()?
            .into_dimensions()?;
        result.push(UploadImage {
            bytes: parsed.bytes().to_vec(),
            cid: blob_cid(parsed.bytes()),
            mime,
            width: dimensions.0,
            height: dimensions.1,
        });
    }
    Ok(result)
}
fn validate_existing(record: &Value, c: &Compose, images: &[UploadImage]) -> Result<()> {
    let reply = record["reply"]["parent"]["uri"].as_str();
    let same = record["text"].as_str() == Some(c.text.as_str())
        && reply == c.reply_to.as_deref()
        && record["langs"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .unwrap_or_default()
            == c.language;
    let existing = record["embed"]["images"].as_array();
    let media_same = if images.is_empty() {
        record["embed"].is_null()
    } else {
        existing.is_some_and(|existing| {
            existing.len() == images.len()
                && existing.iter().zip(images).zip(&c.attachments).all(
                    |((value, image), attachment)| {
                        value["alt"].as_str() == Some(attachment.description.as_str())
                            && value["image"]["size"].as_u64() == Some(image.bytes.len() as u64)
                            && value["image"]["mimeType"].as_str() == Some(image.mime)
                            && value["image"]["ref"]["$link"].as_str() == Some(image.cid.as_str())
                    },
                )
        })
    };
    ensure!(
        same && media_same,
        "This draft key already belongs to a published post with different content. Start a new draft to publish another post"
    );
    Ok(())
}

/// AT Protocol blobs use CIDv1, raw multicodec, and SHA-256. Compare the content
/// address itself: equal MIME/length is not proof that a retried file is equal.
fn blob_cid(bytes: &[u8]) -> String {
    let mut binary = vec![0x01, 0x55, 0x12, 0x20];
    binary.extend_from_slice(&Sha256::digest(bytes));
    let alphabet = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut output = String::from("b");
    let mut buffer = 0u16;
    let mut bits = 0;
    for byte in binary {
        buffer = (buffer << 8) | u16::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            output.push(alphabet[usize::from((buffer >> bits) & 31)] as char);
        }
    }
    if bits > 0 {
        output.push(alphabet[usize::from((buffer << (5 - bits)) & 31)] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    #[test]
    fn raw_sha256_cid_matches_independent_known_vector() {
        assert_eq!(
            super::blob_cid(b""),
            "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku"
        );
    }
}
