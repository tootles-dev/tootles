//! Mastodon REST adapter. Pagination deliberately keeps only the opaque `max_id`
//! from Link headers: a server can never redirect an authenticated request to an
//! unrelated origin. Mutations are never automatically retried.
use std::{
    net::IpAddr,
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use async_trait::async_trait;
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode, header};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use url::Url;

use super::SocialProvider;
use crate::model::*;

const PAGE_SIZE: &str = "30";
const MAX_UPLOAD_BYTES: u64 = 80 * 1024 * 1024;

/// An authenticated adapter. Debug is intentionally not derived for credentials.
pub struct Mastodon {
    client: Client,
    base: Url,
    max_characters: AtomicUsize,
}

impl Mastodon {
    pub fn new(instance: &str, token: &str) -> Result<Self> {
        ensure!(!token.trim().is_empty(), "An access token is required");
        let input = instance.trim();
        let base = Url::parse(&if input.contains("://") {
            input.to_owned()
        } else {
            format!("https://{input}")
        })
        .map_err(|_| anyhow!("Enter a valid Mastodon server URL"))?;
        ensure!(base.host_str().is_some(), "Server URL must have a hostname");
        ensure!(
            base.username().is_empty() && base.password().is_none(),
            "Server URL must not contain credentials"
        );
        ensure!(
            base.query().is_none() && base.fragment().is_none() && matches!(base.path(), "" | "/"),
            "Use your server's origin, without a path, query, or fragment"
        );
        let loopback = base.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .trim_matches(['[', ']'])
                    .parse::<IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        ensure!(
            base.scheme() == "https" || (base.scheme() == "http" && loopback),
            "HTTPS is required except for local loopback test servers"
        );
        let mut authorization = header::HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| anyhow!("Access token contains invalid characters"))?;
        authorization.set_sensitive(true);
        let mut headers = header::HeaderMap::new();
        headers.insert(header::AUTHORIZATION, authorization);
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/json"),
        );
        let client = Client::builder()
            .default_headers(headers)
            .user_agent(concat!("Tootles/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(45))
            .build()
            .context("Could not initialize HTTPS client")?;
        Ok(Self {
            client,
            base,
            max_characters: AtomicUsize::new(500),
        })
    }

    /// Each component is encoded as exactly one path segment, including IDs.
    fn url(&self, parts: &[&str]) -> Result<Url> {
        ensure!(
            parts
                .iter()
                .all(|p| !p.is_empty() && *p != "." && *p != ".."),
            "Invalid empty or relative API identifier"
        );
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| anyhow!("Invalid server origin"))?
            .clear()
            .extend(parts);
        Ok(url)
    }

    fn request(&self, method: Method, parts: &[&str]) -> Result<RequestBuilder> {
        Ok(self.client.request(method, self.url(parts)?))
    }

    async fn send(&self, request: RequestBuilder, retry_read: bool) -> Result<Response> {
        // Only read requests are replayed. Publishing uses a caller-owned stable
        // idempotency key so a deliberate user retry is safe as well.
        for attempt in 0..3 {
            let current = request
                .try_clone()
                .ok_or_else(|| anyhow!("Request cannot be replayed"))?;
            match current.send().await {
                Ok(response)
                    if retry_read
                        && attempt < 2
                        && matches!(response.status().as_u16(), 502..=504) =>
                {
                    tokio::time::sleep(Duration::from_millis(200 * (1 << attempt))).await;
                }
                Ok(response) => return check_status(response),
                Err(error)
                    if retry_read && attempt < 2 && (error.is_connect() || error.is_timeout()) =>
                {
                    tokio::time::sleep(Duration::from_millis(200 * (1 << attempt))).await;
                }
                Err(error) => {
                    // Do not expose reqwest's raw error/URL or untrusted server
                    // bodies, either of which may contain credentials or escapes.
                    bail!(if error.is_timeout() {
                        "The server timed out; check your connection and try again"
                    } else {
                        "Could not reach the server; check its address, TLS certificate, and your connection"
                    });
                }
            }
        }
        unreachable!()
    }

    async fn get<T: DeserializeOwned>(&self, parts: &[&str]) -> Result<T> {
        decode(self.send(self.request(Method::GET, parts)?, true).await?).await
    }

    async fn post_page(
        &self,
        parts: &[&str],
        cursor: Option<&str>,
        extra: &[(&str, &str)],
    ) -> Result<Page<Post>> {
        let mut request = self
            .request(Method::GET, parts)?
            .query(&[("limit", PAGE_SIZE)])
            .query(extra);
        if let Some(cursor) = cursor {
            request = request.query(&[("max_id", cursor)]);
        }
        let response = self.send(request, true).await?;
        let next = next_cursor(response.headers());
        let statuses: Vec<WirePost> = decode(response).await?;
        Ok(Page {
            items: statuses
                .into_iter()
                .filter(WirePost::visible)
                .map(Post::from)
                .collect(),
            next,
        })
    }

    async fn upload(&self, attachment: &Attachment) -> Result<String> {
        let path = Path::new(&attachment.path);
        let metadata = tokio::fs::metadata(path)
            .await
            .context("Cannot read attachment")?;
        ensure!(metadata.is_file(), "Attachment must be a regular file");
        ensure!(
            metadata.len() <= MAX_UPLOAD_BYTES,
            "Attachments must be at most 80 MiB"
        );
        let bytes = tokio::fs::read(path)
            .await
            .context("Cannot read attachment")?;
        ensure!(
            bytes.len() as u64 <= MAX_UPLOAD_BYTES,
            "Attachment grew beyond the upload limit"
        );
        let mime = match path
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "avif" => "image/avif",
            "mp4" | "m4v" => "video/mp4",
            "webm" => "video/webm",
            "mov" => "video/quicktime",
            "mp3" => "audio/mpeg",
            "ogg" | "oga" => "audio/ogg",
            "wav" => "audio/wav",
            "flac" => "audio/flac",
            _ => "application/octet-stream",
        };
        let file = reqwest::multipart::Part::bytes(bytes)
            .file_name(
                path.file_name()
                    .and_then(|x| x.to_str())
                    .unwrap_or("attachment")
                    .to_owned(),
            )
            .mime_str(mime)?;
        let form = reqwest::multipart::Form::new()
            .part("file", file)
            .text("description", attachment.description.clone());
        // Multipart bodies cannot always be cloned; upload directly, with exactly
        // the same status/error policy as JSON requests and no implicit retry.
        let response = self
            .request(Method::POST, &["api", "v2", "media"])?
            .multipart(form)
            .send()
            .await
            .map_err(|_| anyhow!("Attachment upload failed; check your connection"))?;
        let media: WireMedia = decode(check_status(response)?).await?;
        if media.url.as_ref().is_some_and(|url| !url.is_empty()) {
            return Ok(media.id);
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            ensure!(
                tokio::time::Instant::now() < deadline,
                "Attachment is still processing; wait a moment before trying again"
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
            let response = self
                .send(
                    self.request(Method::GET, &["api", "v1", "media", &media.id])?,
                    true,
                )
                .await?;
            if response.status() == StatusCode::PARTIAL_CONTENT {
                continue;
            }
            let ready: WireMedia = decode(response).await?;
            if ready.url.as_ref().is_some_and(|url| !url.is_empty()) {
                return Ok(ready.id);
            }
        }
    }
}

fn check_status(response: Response) -> Result<Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let explanation = match status.as_u16() {
        301..=399 => "Server redirected the API request; use the canonical server address",
        400 => "Server rejected the request; check its fields",
        401 => "Access token is expired or invalid; sign in again",
        403 => "Your account or token does not have permission for this action",
        404 => "This item no longer exists, is private, or is unavailable on this server",
        409 => "This action conflicts with the current server state; refresh and try again",
        413 => "The attachment is larger than this server permits",
        422 => {
            "The server could not validate this action; check text length, poll options, media format, or ownership"
        }
        429 => "Server rate limit reached; wait before trying again",
        500..=599 => "The server is temporarily unavailable; try again later",
        _ => "The server could not complete this request",
    };
    let retry = response
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if status == StatusCode::TOO_MANY_REQUESTS
        && let Some(seconds) = retry
    {
        bail!("{explanation} (HTTP 429; retry in {seconds}s)");
    }
    bail!("{explanation} (HTTP {})", status.as_u16());
}

async fn decode<T: DeserializeOwned>(response: Response) -> Result<T> {
    response
        .json()
        .await
        .map_err(|_| anyhow!("The server returned an unexpected response format"))
}

fn next_cursor(headers: &header::HeaderMap) -> Option<String> {
    for value in headers.get_all(header::LINK) {
        for part in value.to_str().ok()?.split(',') {
            let mut fields = part.split(';');
            let link = fields.next()?.trim().strip_prefix('<')?.strip_suffix('>')?;
            if fields.any(|field| {
                field.trim().strip_prefix("rel=").is_some_and(|rel| {
                    rel.trim_matches('"')
                        .split_whitespace()
                        .any(|r| r == "next")
                })
            }) {
                let url = Url::parse(link).ok()?;
                return url
                    .query_pairs()
                    .find(|(key, _)| key == "max_id")
                    .map(|(_, value)| value.into_owned());
            }
        }
    }
    None
}

#[async_trait]
impl SocialProvider for Mastodon {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            max_characters: self.max_characters.load(Ordering::Relaxed),
            ..Capabilities::default()
        }
    }

    async fn me(&self) -> Result<Account> {
        let account: WireAccount = self
            .get(&["api", "v1", "accounts", "verify_credentials"])
            .await?;
        // Older compatible servers may not expose the v2 instance endpoint.
        // Authentication still succeeds and the conservative default remains.
        let instance_request = self
            .request(Method::GET, &["api", "v2", "instance"])?
            .timeout(Duration::from_secs(5));
        if let Ok(response) = self.send(instance_request, false).await
            && let Ok(instance) = decode::<Value>(response).await
            && let Some(limit) = instance
                .pointer("/configuration/statuses/max_characters")
                .and_then(Value::as_u64)
            && (1..=1_000_000).contains(&limit)
        {
            self.max_characters.store(limit as usize, Ordering::Relaxed);
        }
        Ok(account.into())
    }

    async fn feed(&self, feed: &Feed, cursor: Option<&str>) -> Result<Page<Post>> {
        match feed {
            Feed::Home => {
                self.post_page(&["api", "v1", "timelines", "home"], cursor, &[])
                    .await
            }
            Feed::Local => {
                self.post_page(
                    &["api", "v1", "timelines", "public"],
                    cursor,
                    &[("local", "true")],
                )
                .await
            }
            Feed::Federated => {
                self.post_page(&["api", "v1", "timelines", "public"], cursor, &[])
                    .await
            }
            Feed::Bookmarks => {
                self.post_page(&["api", "v1", "bookmarks"], cursor, &[])
                    .await
            }
            Feed::Favourites => {
                self.post_page(&["api", "v1", "favourites"], cursor, &[])
                    .await
            }
            Feed::Profile(id) => {
                self.post_page(&["api", "v1", "accounts", id, "statuses"], cursor, &[])
                    .await
            }
            Feed::List(id, _) => {
                self.post_page(&["api", "v1", "timelines", "list", id], cursor, &[])
                    .await
            }
            Feed::Tag(tag) => {
                self.post_page(
                    &["api", "v1", "timelines", "tag", tag.trim_start_matches('#')],
                    cursor,
                    &[],
                )
                .await
            }
            Feed::Search(query) => {
                let mut request = self
                    .request(Method::GET, &["api", "v2", "search"])?
                    .query(&[
                        ("q", query.as_str()),
                        ("type", "statuses"),
                        ("limit", PAGE_SIZE),
                    ]);
                if let Some(cursor) = cursor {
                    request = request.query(&[("max_id", cursor)]);
                }
                let response = self.send(request, true).await?;
                let next = next_cursor(response.headers());
                let result: WireSearch = decode(response).await?;
                Ok(Page {
                    items: result
                        .statuses
                        .into_iter()
                        .filter(WirePost::visible)
                        .map(Into::into)
                        .collect(),
                    next,
                })
            }
            Feed::Thread(id) => {
                let thread = self.context(id).await?;
                let mut items = thread.ancestors;
                items.push(self.post(id).await?);
                items.extend(thread.descendants);
                Ok(Page { items, next: None })
            }
            Feed::Notifications => bail!("Use the notification stream for notifications"),
        }
    }

    async fn notifications(&self, cursor: Option<&str>) -> Result<Page<Notification>> {
        let mut request = self
            .request(Method::GET, &["api", "v1", "notifications"])?
            .query(&[("limit", PAGE_SIZE)]);
        if let Some(cursor) = cursor {
            request = request.query(&[("max_id", cursor)]);
        }
        let response = self.send(request, true).await?;
        let next = next_cursor(response.headers());
        let items: Vec<WireNotification> = decode(response).await?;
        Ok(Page {
            items: items.into_iter().map(Into::into).collect(),
            next,
        })
    }

    async fn search(&self, query: &str) -> Result<SearchResults> {
        let request = self
            .request(Method::GET, &["api", "v2", "search"])?
            .query(&[("q", query), ("resolve", "true"), ("limit", PAGE_SIZE)]);
        let result: WireSearch = decode(self.send(request, true).await?).await?;
        Ok(SearchResults {
            posts: result
                .statuses
                .into_iter()
                .filter(WirePost::visible)
                .map(Into::into)
                .collect(),
            accounts: result.accounts.into_iter().map(Into::into).collect(),
            hashtags: result
                .hashtags
                .into_iter()
                .map(|t| clean(&t.name))
                .collect(),
        })
    }

    async fn context(&self, id: &str) -> Result<Thread> {
        let result: WireContext = self.get(&["api", "v1", "statuses", id, "context"]).await?;
        Ok(Thread {
            ancestors: result
                .ancestors
                .into_iter()
                .filter(WirePost::visible)
                .map(Into::into)
                .collect(),
            descendants: result
                .descendants
                .into_iter()
                .filter(WirePost::visible)
                .map(Into::into)
                .collect(),
        })
    }

    async fn post(&self, id: &str) -> Result<Post> {
        Ok(self
            .get::<WirePost>(&["api", "v1", "statuses", id])
            .await?
            .into())
    }

    async fn profile(&self, id: &str) -> Result<Account> {
        Ok(self
            .get::<WireAccount>(&["api", "v1", "accounts", id])
            .await?
            .into())
    }

    async fn relationship(&self, id: &str) -> Result<Relationship> {
        let request = self
            .request(Method::GET, &["api", "v1", "accounts", "relationships"])?
            .query(&[("id[]", id)]);
        let relationships: Vec<WireRelationship> = decode(self.send(request, true).await?).await?;
        relationships
            .into_iter()
            .next()
            .map(Into::into)
            .ok_or_else(|| anyhow!("The server returned no relationship for this account"))
    }

    async fn post_action(&self, id: &str, action: PostAction) -> Result<Option<Post>> {
        let verb = match action {
            PostAction::Favourite(true) => "favourite",
            PostAction::Favourite(false) => "unfavourite",
            PostAction::Boost(true) => "reblog",
            PostAction::Boost(false) => "unreblog",
            PostAction::Bookmark(true) => "bookmark",
            PostAction::Bookmark(false) => "unbookmark",
            PostAction::Delete => {
                self.send(
                    self.request(Method::DELETE, &["api", "v1", "statuses", id])?,
                    false,
                )
                .await?;
                return Ok(None);
            }
        };
        let response = self
            .send(
                self.request(Method::POST, &["api", "v1", "statuses", id, verb])?,
                false,
            )
            .await?;
        Ok(Some(decode::<WirePost>(response).await?.into()))
    }

    async fn account_action(&self, id: &str, action: AccountAction) -> Result<Relationship> {
        let verb = match action {
            AccountAction::Follow(true) => "follow",
            AccountAction::Follow(false) => "unfollow",
            AccountAction::Mute(true) => "mute",
            AccountAction::Mute(false) => "unmute",
            AccountAction::Block(true) => "block",
            AccountAction::Block(false) => "unblock",
        };
        let response = self
            .send(
                self.request(Method::POST, &["api", "v1", "accounts", id, verb])?,
                false,
            )
            .await?;
        Ok(decode::<WireRelationship>(response).await?.into())
    }

    async fn publish(&self, compose: &Compose) -> Result<Post> {
        ensure!(
            !compose.text.trim().is_empty()
                || !compose.attachments.is_empty()
                || compose.edit_id.is_some(),
            "Write a post or attach media first"
        );
        ensure!(
            compose.attachments.len() <= 4,
            "Attach at most four media files"
        );
        ensure!(
            compose.poll_options.is_empty() || compose.attachments.is_empty(),
            "A post can have media or a poll, but not both"
        );
        ensure!(
            compose.poll_options.is_empty()
                || ((2..=4).contains(&compose.poll_options.len())
                    && compose.poll_options.iter().all(|o| !o.trim().is_empty())),
            "Polls need two to four nonempty options"
        );
        ensure!(
            compose.poll_options.is_empty() || compose.poll_expires_in >= 300,
            "Poll duration must be at least five minutes"
        );
        if compose.edit_id.is_none() {
            ensure!(
                !compose.idempotency_key.is_empty(),
                "A stable idempotency key is required to publish safely"
            );
            header::HeaderValue::from_str(&compose.idempotency_key)
                .map_err(|_| anyhow!("Invalid idempotency key"))?;
        }
        let mut payload = json!({
            "status": compose.text, "spoiler_text": compose.spoiler_text,
            "sensitive": compose.sensitive, "visibility": compose.visibility.as_str(),
        });
        if !compose.language.trim().is_empty() {
            payload["language"] = json!(compose.language);
        }
        if let Some(id) = &compose.reply_to {
            payload["in_reply_to_id"] = json!(id);
        }
        let previous: Option<Post> = if let Some(id) = &compose.edit_id {
            Some(self.post(id).await?)
        } else {
            None
        };
        let mut media_ids: Vec<String> = previous
            .as_ref()
            .map(|p| p.media.iter().map(|m| m.id.clone()).collect())
            .unwrap_or_default();
        ensure!(
            media_ids.len() + compose.attachments.len() <= 4,
            "This edit would exceed four media attachments"
        );
        ensure!(
            compose.poll_options.is_empty() || media_ids.is_empty(),
            "Remove existing media before adding a poll"
        );
        ensure!(
            compose.attachments.is_empty()
                || previous.as_ref().is_none_or(|post| post.poll.is_none()),
            "A post with a poll cannot also have media"
        );
        for attachment in &compose.attachments {
            media_ids.push(self.upload(attachment).await?);
        }
        if !media_ids.is_empty() {
            payload["media_ids"] = json!(media_ids);
        }
        if !compose.poll_options.is_empty() {
            payload["poll"] = json!({ "options": compose.poll_options, "multiple": compose.poll_multiple, "expires_in": compose.poll_expires_in });
        }
        // Omitting poll on a status edit preserves the original poll.
        let request = if let Some(id) = &compose.edit_id {
            self.request(Method::PUT, &["api", "v1", "statuses", id])?
        } else {
            let key = header::HeaderValue::from_str(&compose.idempotency_key)
                .map_err(|_| anyhow!("Invalid idempotency key"))?;
            self.request(Method::POST, &["api", "v1", "statuses"])?
                .header("Idempotency-Key", key)
        };
        let result: WirePost = decode(self.send(request.json(&payload), false).await?).await?;
        Ok(result.into())
    }

    async fn source(&self, id: &str) -> Result<Compose> {
        let source: WireSource = self.get(&["api", "v1", "statuses", id, "source"]).await?;
        let status: WirePost = self.get(&["api", "v1", "statuses", id]).await?;
        Ok(Compose {
            text: clean(&source.text),
            spoiler_text: clean(&source.spoiler_text),
            sensitive: status.sensitive,
            visibility: status.visibility,
            reply_to: status.in_reply_to_id,
            edit_id: Some(id.to_owned()),
            language: status.language.unwrap_or_default(),
            idempotency_key: uuid::Uuid::new_v4().to_string(),
            ..Compose::default()
        })
    }

    async fn vote(&self, id: &str, choices: &[usize]) -> Result<Poll> {
        ensure!(!choices.is_empty(), "Choose at least one poll option");
        let response = self
            .send(
                self.request(Method::POST, &["api", "v1", "polls", id, "votes"])?
                    .json(&json!({ "choices": choices })),
                false,
            )
            .await?;
        Ok(decode::<WirePoll>(response).await?.into())
    }

    async fn lists(&self) -> Result<Vec<SocialList>> {
        let mut lists: Vec<SocialList> = self.get(&["api", "v1", "lists"]).await?;
        for list in &mut lists {
            list.title = clean(&list.title);
        }
        Ok(lists)
    }

    async fn create_list(&self, title: &str) -> Result<SocialList> {
        ensure!(!title.trim().is_empty(), "Give your list a name");
        let mut list: SocialList = decode(
            self.send(
                self.request(Method::POST, &["api", "v1", "lists"])?
                    .json(&json!({"title": title.trim()})),
                false,
            )
            .await?,
        )
        .await?;
        list.title = clean(&list.title);
        Ok(list)
    }

    async fn list_member(&self, list: &str, account: &str, add: bool) -> Result<()> {
        let method = if add { Method::POST } else { Method::DELETE };
        self.send(
            self.request(method, &["api", "v1", "lists", list, "accounts"])?
                .json(&json!({"account_ids": [account]})),
            false,
        )
        .await?;
        Ok(())
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct WireAccount {
    id: String,
    username: String,
    acct: String,
    display_name: String,
    note: String,
    url: String,
    followers_count: u64,
    following_count: u64,
    statuses_count: u64,
    locked: bool,
}
impl From<WireAccount> for Account {
    fn from(a: WireAccount) -> Self {
        Self {
            id: a.id,
            username: clean(&a.username),
            acct: clean(&a.acct),
            display_name: clean(&a.display_name),
            note: html_text(&a.note),
            url: clean(&a.url),
            followers_count: a.followers_count,
            following_count: a.following_count,
            statuses_count: a.statuses_count,
            locked: a.locked,
        }
    }
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct WirePost {
    id: String,
    account: WireAccount,
    content: String,
    url: Option<String>,
    uri: String,
    created_at: String,
    spoiler_text: String,
    sensitive: bool,
    visibility: Visibility,
    favourites_count: u64,
    reblogs_count: u64,
    replies_count: u64,
    favourited: Option<bool>,
    reblogged: Option<bool>,
    bookmarked: Option<bool>,
    reblog: Option<Box<WirePost>>,
    in_reply_to_id: Option<String>,
    media_attachments: Vec<WireMedia>,
    poll: Option<WirePoll>,
    mentions: Vec<WireMention>,
    edited_at: Option<String>,
    language: Option<String>,
    filtered: Vec<WireFilter>,
}
impl WirePost {
    fn visible(&self) -> bool {
        self.filtered
            .iter()
            .all(|f| f.filter.filter_action != "hide")
            && self.reblog.as_ref().is_none_or(|p| p.visible())
    }
}
impl From<WirePost> for Post {
    fn from(mut p: WirePost) -> Self {
        let timeline_id = p.id.clone();
        let boosted_by = p.reblog.take().map(|original| {
            let name = if p.account.display_name.is_empty() {
                clean(&p.account.acct)
            } else {
                clean(&p.account.display_name)
            };
            p = *original;
            name
        });
        let mut warnings: Vec<String> = p
            .filtered
            .iter()
            .filter(|f| f.filter.filter_action == "warn")
            .map(|f| format!("Filtered: {}", clean(&f.filter.title)))
            .collect();
        if !p.spoiler_text.is_empty() {
            warnings.push(clean(&p.spoiler_text));
        }
        Self {
            id: p.id,
            timeline_id,
            account: p.account.into(),
            text: html_text(&p.content),
            url: clean(&p.url.unwrap_or(p.uri)),
            created_at: clean(&p.created_at),
            spoiler_text: warnings.join(" · "),
            sensitive: p.sensitive,
            visibility: p.visibility,
            favourites_count: p.favourites_count,
            reblogs_count: p.reblogs_count,
            replies_count: p.replies_count,
            favourited: p.favourited.unwrap_or(false),
            reblogged: p.reblogged.unwrap_or(false),
            bookmarked: p.bookmarked.unwrap_or(false),
            boosted_by,
            in_reply_to_id: p.in_reply_to_id,
            media: p.media_attachments.into_iter().map(Into::into).collect(),
            poll: p.poll.map(Into::into),
            mentions: p.mentions.into_iter().map(|m| clean(&m.acct)).collect(),
            edited_at: p.edited_at.map(|s| clean(&s)),
        }
    }
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct WireMedia {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    url: Option<String>,
    remote_url: Option<String>,
    description: Option<String>,
}
impl From<WireMedia> for Media {
    fn from(m: WireMedia) -> Self {
        Self {
            id: m.id,
            kind: clean(&m.kind),
            url: clean(&m.url.or(m.remote_url).unwrap_or_default()),
            description: clean(&m.description.unwrap_or_default()),
        }
    }
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct WirePoll {
    id: String,
    options: Vec<WirePollOption>,
    expires_at: Option<String>,
    expired: bool,
    multiple: bool,
    voted: Option<bool>,
    own_votes: Option<Vec<usize>>,
    votes_count: u64,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct WirePollOption {
    title: String,
    votes_count: Option<u64>,
}
impl From<WirePoll> for Poll {
    fn from(p: WirePoll) -> Self {
        Self {
            id: p.id,
            options: p
                .options
                .into_iter()
                .map(|o| PollOption {
                    title: clean(&o.title),
                    votes_count: o.votes_count.unwrap_or_default(),
                })
                .collect(),
            expires_at: p.expires_at,
            expired: p.expired,
            multiple: p.multiple,
            voted: p.voted.unwrap_or(false),
            own_votes: p.own_votes.unwrap_or_default(),
            votes_count: p.votes_count,
        }
    }
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct WireMention {
    acct: String,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct WireFilter {
    filter: WireFilterRule,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct WireFilterRule {
    title: String,
    filter_action: String,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct WireNotification {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    account: WireAccount,
    status: Option<WirePost>,
    created_at: String,
}
impl From<WireNotification> for Notification {
    fn from(n: WireNotification) -> Self {
        Self {
            id: n.id,
            kind: clean(&n.kind),
            account: n.account.into(),
            post: n.status.filter(WirePost::visible).map(Into::into),
            created_at: clean(&n.created_at),
        }
    }
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct WireRelationship {
    following: bool,
    followed_by: bool,
    requested: bool,
    muting: bool,
    blocking: bool,
}
impl From<WireRelationship> for Relationship {
    fn from(r: WireRelationship) -> Self {
        Self {
            following: r.following,
            followed_by: r.followed_by,
            requested: r.requested,
            muting: r.muting,
            blocking: r.blocking,
        }
    }
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct WireSearch {
    accounts: Vec<WireAccount>,
    statuses: Vec<WirePost>,
    hashtags: Vec<WireTag>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct WireTag {
    name: String,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct WireContext {
    ancestors: Vec<WirePost>,
    descendants: Vec<WirePost>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct WireSource {
    text: String,
    spoiler_text: String,
}

/// Strip terminal control characters, including C1 controls, direction overrides
/// and isolates. Newlines and tabs remain readable text, never terminal commands.
fn clean(input: &str) -> String {
    input
        .chars()
        .filter(|c| {
            (*c == '\n' || *c == '\t' || !c.is_control())
                && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect()
}

/// Decode numeric entities explicitly as well as named HTML entities. Some
/// HTML decoders deliberately leave invalid HTML control references unchanged;
/// decoding them here lets `clean` remove the underlying terminal controls.
fn decode_entities(input: &str) -> String {
    let decoded = html_escape::decode_html_entities(input);
    let mut out = String::new();
    let mut rest = decoded.as_ref();
    while let Some(start) = rest.find("&#") {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find(';').filter(|end| *end <= 12) else {
            out.push_str("&#");
            rest = &rest[2..];
            continue;
        };
        let digits = &rest[2..end];
        let number = if let Some(hex) = digits
            .strip_prefix('x')
            .or_else(|| digits.strip_prefix('X'))
        {
            u32::from_str_radix(hex, 16).ok()
        } else {
            digits.parse::<u32>().ok()
        };
        if let Some(character) = number.and_then(char::from_u32) {
            out.push(character);
        } else {
            out.push_str(&rest[..=end]);
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Mastodon's HTML is a tiny subset. Preserve paragraph boundaries and the full
/// destination of shortened links, without emitting ANSI or terminal hyperlinks.
fn html_text(input: &str) -> String {
    let mut out = String::new();
    let mut rest = input;
    let mut anchor: Option<(String, usize)> = None;
    let mut hidden_tag: Option<String> = None;
    while let Some(start) = rest.find('<') {
        if hidden_tag.is_none() {
            out.push_str(&rest[..start]);
        }
        let Some(end) = rest[start..].find('>') else {
            rest = &rest[start..];
            break;
        };
        let raw = rest[start + 1..start + end].trim();
        let name = raw
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_ascii_lowercase();
        if matches!(name.as_str(), "script" | "style") {
            hidden_tag = Some(name.clone());
        }
        if hidden_tag
            .as_ref()
            .is_some_and(|tag| name == format!("/{tag}"))
        {
            hidden_tag = None;
        }
        if hidden_tag.is_none() {
            match name.as_str() {
                "br" | "/p" | "/div" | "/li" | "/blockquote" => {
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                    if name == "/p" {
                        out.push('\n');
                    }
                }
                "li" => out.push_str("• "),
                "a" => {
                    if let Some(href) = attribute(raw, "href") {
                        let href = decode_entities(&href);
                        if Url::parse(&href)
                            .is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
                        {
                            anchor = Some((href, out.len()));
                        }
                    }
                }
                "/a" => {
                    if let Some((href, start)) = anchor.take() {
                        let label = decode_entities(&out[start..]);
                        if !label.starts_with('@')
                            && !label.starts_with('#')
                            && label.trim() != href
                            && label.trim_end_matches('/')
                                != href
                                    .trim_start_matches("https://")
                                    .trim_start_matches("http://")
                                    .trim_end_matches('/')
                        {
                            out.push_str(" (");
                            out.push_str(&href);
                            out.push(')');
                        }
                    }
                }
                "img" => {
                    if let Some(alt) = attribute(raw, "alt") {
                        out.push_str(&alt);
                    }
                }
                _ => {}
            }
        }
        rest = &rest[start + end + 1..];
    }
    if hidden_tag.is_none() {
        out.push_str(rest);
    }
    clean(&decode_entities(&out)).trim().to_owned()
}

fn attribute(tag: &str, key: &str) -> Option<String> {
    let mut rest = tag.split_once(char::is_whitespace)?.1.trim_start();
    while !rest.is_empty() {
        let end = rest
            .find(|c: char| c == '=' || c.is_whitespace())
            .unwrap_or(rest.len());
        let name = &rest[..end];
        rest = rest[end..].trim_start();
        if !rest.starts_with('=') {
            if rest.is_empty() {
                break;
            }
            continue;
        }
        rest = rest[1..].trim_start();
        let (value, tail) = if rest.starts_with(['\'', '"']) {
            let quote = rest.chars().next()?;
            let end = rest[1..].find(quote)? + 1;
            (&rest[1..end], &rest[end + 1..])
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            (&rest[..end], &rest[end..])
        };
        if name.eq_ignore_ascii_case(key) {
            return Some(value.to_owned());
        }
        rest = tail.trim_start();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readable_safe_html() {
        assert_eq!(
            html_text(
                "<p>Hello &amp; welcome</p><p><a href='https://example.org/long'>read this</a><br>next&#27;\u{202e}</p>"
            ),
            "Hello & welcome\n\nread this (https://example.org/long)\nnext"
        );
        assert_eq!(
            html_text(
                "<script>ignored</script><p><img alt=':blob:'> <a href='https://example.org/@alice'>@alice</a></p>"
            ),
            ":blob: @alice"
        );
    }
    #[test]
    fn refuses_insecure_or_credentialed_origins() {
        for origin in [
            "http://example.org",
            "https://user:secret@example.org",
            "https://example.org/api",
            "file:///tmp/token",
            "https://example.org/?token=secret",
        ] {
            assert!(Mastodon::new(origin, "secret").is_err());
        }
        assert!(Mastodon::new("https://example.org", "secret\n").is_err());
        assert!(Mastodon::new("http://127.0.0.1:1234", "secret").is_ok());
    }
}
