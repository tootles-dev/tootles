//! App-password sessions for the personal CLI. Credentials are opaque and all
//! authenticated requests stay on the validated PDS; AppView calls use its proxy.
use std::{fmt, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Client, Method, Response, StatusCode, header};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use url::Url;

use crate::config::{self, Config, SavedAccount};

pub const DEFAULT_PDS: &str = "https://bsky.social";
const APPVIEW: &str = "did:web:api.bsky.app#bsky_appview";
const RESPONSE_LIMIT: usize = 16 * 1024 * 1024;

/// Stored inside SavedAccount.token to preserve the existing config format.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub did: String,
    pub handle: String,
    pub access_jwt: String,
    pub refresh_jwt: String,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("did", &self.did)
            .field("handle", &self.handle)
            .field("credentials", &"[REDACTED]")
            .finish()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionResponse {
    #[serde(flatten)]
    session: Session,
    did_doc: Option<Value>,
    active: Option<bool>,
}

impl Session {
    fn validate(&self) -> Result<()> {
        let did = self
            .did
            .strip_prefix("did:")
            .and_then(|s| s.split_once(':'));
        if self.did.len() > 2048
            || !did.is_some_and(|(method, id)| {
                !method.is_empty()
                    && method.bytes().all(|b| b.is_ascii_lowercase())
                    && !id.is_empty()
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._:%-".contains(&b))
                    && id
                        .bytes()
                        .last()
                        .is_some_and(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            })
            || self.handle.is_empty()
            || self.handle.len() > 253
            || !self
                .handle
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
        {
            bail!("Bluesky returned an invalid account identity; sign in again");
        }
        for token in [&self.access_jwt, &self.refresh_jwt] {
            if token.is_empty()
                || token.len() > 16384
                || !token.bytes().all(|b| b.is_ascii_graphic())
            {
                bail!("Bluesky returned an invalid session token; sign in again");
            }
        }
        Ok(())
    }
}

/// Accept an HTTPS origin or a loopback HTTP server for local development.
pub fn normalize_pds(input: &str) -> Result<String> {
    let input = input.trim();
    if input.is_empty() || input.chars().any(char::is_control) {
        bail!("Enter a Bluesky PDS hostname or HTTPS origin");
    }
    let input = if input.contains("://") {
        input.to_owned()
    } else {
        format!("https://{input}")
    };
    let url = Url::parse(&input).map_err(|_| anyhow!("Invalid Bluesky PDS address"))?;
    let loopback = match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!(
            "Bluesky PDS must be an HTTPS origin without credentials or a path (HTTP is allowed only on loopback)"
        );
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

fn http_client() -> Result<Client> {
    Client::builder()
        .user_agent(concat!("Tootles/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("Could not create Bluesky HTTP client")
}

/// Only the explicitly chosen login server receives the app password. A hosted
/// entryway may return the account's PDS in its matching DID document.
pub async fn login(
    identifier: &str,
    pds: &str,
    app_password: String,
    label: Option<String>,
) -> Result<SavedAccount> {
    let pds = normalize_pds(pds)?;
    let identifier = identifier.trim().trim_start_matches('@');
    if identifier.is_empty() || identifier.len() > 2048 || identifier.chars().any(char::is_control)
    {
        bail!("Enter your Bluesky handle, for example alice.bsky.social");
    }
    let password = app_password.trim();
    if password.is_empty() || password.len() > 1024 || password.chars().any(char::is_control) {
        bail!("Enter a Bluesky app password from Settings → Privacy and security → App passwords");
    }
    let response = http_client()?
        .post(format!("{pds}/xrpc/com.atproto.server.createSession"))
        .json(&json!({"identifier": identifier, "password": password}))
        .send()
        .await
        .map_err(transport_error)?;
    // Drop the password as soon as the request completes; it is never persisted.
    drop(app_password);
    let session = decode_success::<SessionResponse>(response).await?;
    validate_response(&session)?;
    let pds = discover_pds(&session, &pds)?;
    let label = label
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("@{} · Bluesky", session.session.handle));
    Ok(SavedAccount {
        id: format!("bluesky:{}", session.session.did),
        label,
        provider: "bluesky".into(),
        instance: pds,
        token: serde_json::to_string(&session.session)
            .context("Could not serialize Bluesky session")?,
    })
}

fn validate_response(response: &SessionResponse) -> Result<()> {
    response.session.validate()?;
    if response.active == Some(false) {
        bail!("This Bluesky account is not active; check your account status on the server");
    }
    Ok(())
}

fn discover_pds(response: &SessionResponse, login_origin: &str) -> Result<String> {
    let Some(doc) = &response.did_doc else {
        return Ok(login_origin.to_owned());
    };
    if doc.get("id").and_then(Value::as_str) != Some(response.session.did.as_str()) {
        bail!("Bluesky returned a DID document for a different account");
    }
    let full_id = format!("{}#atproto_pds", response.session.did);
    let entry = doc
        .get("service")
        .and_then(Value::as_array)
        .and_then(|services| {
            services.iter().find(|entry| {
                matches!(
                    entry.get("id").and_then(Value::as_str),
                    Some("#atproto_pds")
                ) || entry.get("id").and_then(Value::as_str) == Some(full_id.as_str())
            })
        })
        .context("Bluesky returned a DID document without its account PDS")?;
    if entry.get("type").and_then(Value::as_str) != Some("AtprotoPersonalDataServer") {
        bail!("Bluesky returned an invalid PDS service in its DID document");
    }
    let raw = entry
        .get("serviceEndpoint")
        .and_then(Value::as_str)
        .context("Bluesky returned an invalid PDS endpoint")?;
    if !raw.starts_with("https://") && !raw.starts_with("http://") {
        bail!("Bluesky returned an invalid PDS endpoint");
    }
    let pds = normalize_pds(raw)?;
    if pds.starts_with("http://") && !login_origin.starts_with("http://") {
        bail!("Bluesky returned an insecure PDS endpoint");
    }
    Ok(pds)
}

struct State {
    session: Session,
    saved_token: String,
    invalid: bool,
}

#[derive(Clone)]
pub struct SessionClient {
    http: Client,
    pds: String,
    did: String,
    account_id: String,
    root: Option<PathBuf>,
    state: Arc<Mutex<State>>,
}

impl SessionClient {
    pub fn from_saved(account: &SavedAccount, root: Option<PathBuf>) -> Result<Self> {
        if account.provider != "bluesky" {
            bail!("This is not a Bluesky account");
        }
        let session = parse_saved(account)?;
        Ok(Self {
            http: http_client()?,
            pds: normalize_pds(&account.instance)?,
            did: session.did.clone(),
            account_id: account.id.clone(),
            root,
            state: Arc::new(Mutex::new(State {
                session,
                saved_token: account.token.clone(),
                invalid: false,
            })),
        })
    }

    pub fn did(&self) -> &str {
        &self.did
    }

    pub async fn get(&self, method: &str, query: &[(&str, String)]) -> Result<Value> {
        self.request(method, Method::GET, query, Body::Empty).await
    }

    pub async fn post(&self, method: &str, body: &Value) -> Result<Value> {
        self.request(method, Method::POST, &[], Body::Json(body))
            .await
    }

    pub async fn upload(&self, mime: &str, bytes: &[u8]) -> Result<Value> {
        if bytes.len() > 16 * 1024 * 1024 {
            bail!("Bluesky attachment exceeds the 16 MiB client limit");
        }
        if !matches!(
            mime,
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        ) {
            bail!("Bluesky supports PNG, JPEG, GIF, or WebP image uploads in Tootles");
        }
        self.request(
            "com.atproto.repo.uploadBlob",
            Method::POST,
            &[],
            Body::Bytes(mime, bytes),
        )
        .await
    }

    async fn request(
        &self,
        nsid: &str,
        verb: Method,
        query: &[(&str, String)],
        body: Body<'_>,
    ) -> Result<Value> {
        validate_nsid(nsid)?;
        for attempt in 0..2 {
            let token = {
                let state = self.state.lock().await;
                if state.invalid {
                    bail!("Bluesky session is unavailable; sign in again");
                }
                state.session.access_jwt.clone()
            };
            let mut request = self
                .http
                .request(verb.clone(), format!("{}/xrpc/{nsid}", self.pds))
                .bearer_auth(&token)
                .query(query);
            if nsid.starts_with("app.bsky.")
                && !matches!(
                    nsid,
                    "app.bsky.actor.getPreferences" | "app.bsky.actor.putPreferences"
                )
            {
                request = request.header("atproto-proxy", APPVIEW);
            }
            request = match body {
                Body::Empty => request,
                Body::Json(json) => request.json(json),
                Body::Bytes(mime, bytes) => request
                    .header(header::CONTENT_TYPE, mime)
                    .body(bytes.to_vec()),
            };
            let response = request.send().await.map_err(transport_error)?;
            if response.status().is_success() {
                return decode_success(response).await;
            }
            let status = response.status();
            let bytes = read_body(response).await?;
            let code = serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_owned));
            let expired = status == StatusCode::UNAUTHORIZED
                || (status == StatusCode::BAD_REQUEST
                    && code
                        .as_deref()
                        .is_some_and(|code| matches!(code, "ExpiredToken" | "InvalidToken")));
            if expired && attempt == 0 {
                // The refresh transaction must finish even if its initiating UI
                // request is canceled after the server rotates the refresh token.
                let client = self.clone();
                tokio::spawn(async move { client.refresh(&token).await })
                    .await
                    .map_err(|_| {
                        anyhow!("Bluesky session refresh was interrupted; sign in again")
                    })??;
                continue;
            }
            if matches!(status.as_u16(), 400 | 404) && code.as_deref() == Some("RecordNotFound") {
                return Err(RecordNotFound.into());
            }
            return Err(status_error(status));
        }
        unreachable!()
    }

    async fn refresh(&self, rejected: &str) -> Result<()> {
        let mut state = self.state.lock().await;
        if state.invalid {
            bail!("Bluesky session is unavailable; sign in again");
        }
        if state.session.access_jwt != rejected {
            return Ok(());
        }
        let root = self.root.clone();
        let id = self.account_id.clone();
        let _guard =
            tokio::task::spawn_blocking(move || config::session_lock(root.as_deref(), &id))
                .await
                .map_err(|_| anyhow!("Could not lock the Bluesky session"))??;
        let latest = Config::load(self.root.as_deref())?;
        let Some(account) = latest
            .accounts
            .iter()
            .find(|a| a.id == self.account_id && a.provider == "bluesky" && a.instance == self.pds)
        else {
            state.invalid = true;
            bail!("The Bluesky account was removed or changed; sign in again");
        };
        if account.token != state.saved_token {
            // Another process already refreshed or explicitly logged in again.
            let updated = parse_saved(account)?;
            state.session = updated;
            state.saved_token = account.token.clone();
            if state.session.access_jwt != rejected {
                return Ok(());
            }
        }
        let response = self
            .http
            .post(format!(
                "{}/xrpc/com.atproto.server.refreshSession",
                self.pds
            ))
            .bearer_auth(&state.session.refresh_jwt)
            .send()
            .await
            .map_err(transport_error)?;
        let renewed = match decode_success::<SessionResponse>(response).await {
            Ok(renewed) => renewed,
            Err(error) => {
                state.invalid = true;
                return Err(error).context(
                    "Could not refresh the Bluesky session; sign in again if it has expired",
                );
            }
        };
        if let Err(error) = validate_response(&renewed) {
            state.invalid = true;
            return Err(error);
        }
        let renewed_pds = match discover_pds(&renewed, &self.pds) {
            Ok(pds) => pds,
            Err(error) => {
                state.invalid = true;
                return Err(error)
                    .context("Bluesky returned invalid renewed session data; sign in again");
            }
        };
        if renewed.session.did != self.did || renewed_pds != self.pds {
            state.invalid = true;
            bail!("Bluesky returned a changed session identity or PDS; sign in again");
        }
        let token = serde_json::to_string(&renewed.session)
            .context("Could not serialize renewed Bluesky session")?;
        let result = config::update(self.root.as_deref(), |latest| {
            let account = latest
                .accounts
                .iter_mut()
                .find(|a| {
                    a.id == self.account_id
                        && a.provider == "bluesky"
                        && a.instance == self.pds
                        && a.token == state.saved_token
                })
                .context("The Bluesky account changed during refresh; sign in again")?;
            account.token.clone_from(&token);
            Ok(())
        });
        if let Err(error) = result {
            state.invalid = true;
            return Err(error)
                .context("Could not save renewed Bluesky credentials; no request was retried");
        }
        state.session = renewed.session;
        state.saved_token = token;
        Ok(())
    }
}

fn parse_saved(account: &SavedAccount) -> Result<Session> {
    let session: Session = serde_json::from_str(&account.token)
        .map_err(|_| anyhow!("Saved Bluesky session is invalid; sign in again"))?;
    session.validate()?;
    if account.id != format!("bluesky:{}", session.did) {
        bail!("Saved Bluesky account identity does not match its session");
    }
    Ok(session)
}

enum Body<'a> {
    Empty,
    Json(&'a Value),
    Bytes(&'a str, &'a [u8]),
}

#[derive(Debug)]
struct RecordNotFound;
impl fmt::Display for RecordNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Bluesky record was not found")
    }
}
impl std::error::Error for RecordNotFound {}

/// Distinguish a missing record from network, permission, or decoding failures.
pub fn is_record_not_found(error: &anyhow::Error) -> bool {
    error.downcast_ref::<RecordNotFound>().is_some()
}

fn validate_nsid(nsid: &str) -> Result<()> {
    let parts: Vec<_> = nsid.split('.').collect();
    if nsid.len() > 317
        || parts.len() < 3
        || parts.iter().any(|s| {
            s.is_empty()
                || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                || !s.as_bytes()[0].is_ascii_alphabetic()
                || !s.as_bytes()[s.len() - 1].is_ascii_alphanumeric()
        })
    {
        bail!("Invalid Bluesky API method");
    }
    Ok(())
}

fn transport_error(error: reqwest::Error) -> anyhow::Error {
    if error.is_timeout() {
        anyhow!("Bluesky request timed out; try again")
    } else {
        anyhow!("Could not connect to the Bluesky PDS; check its address and your connection")
    }
}

fn status_error(status: StatusCode) -> anyhow::Error {
    match status.as_u16() {
        400 => {
            anyhow!("Bluesky rejected the request (HTTP 400); check the content or account details")
        }
        401 => {
            anyhow!("Bluesky session was rejected (HTTP 401); sign in again with an app password")
        }
        403 => anyhow!("Bluesky did not permit this operation (HTTP 403)"),
        404 => anyhow!("Bluesky could not find the requested item (HTTP 404)"),
        429 => anyhow!("Bluesky rate limit reached (HTTP 429); wait before trying again"),
        300..=399 => anyhow!(
            "Bluesky returned an HTTP redirect; it was not followed to protect your credentials"
        ),
        _ => anyhow!("Bluesky request failed (HTTP {})", status.as_u16()),
    }
}

async fn read_body(mut response: Response) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|size| size > RESPONSE_LIMIT as u64)
    {
        bail!("Bluesky response exceeds the client size limit");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("Could not read the Bluesky response"))?
    {
        if bytes.len().saturating_add(chunk.len()) > RESPONSE_LIMIT {
            bail!("Bluesky response exceeds the client size limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn decode_success<T: DeserializeOwned>(response: Response) -> Result<T> {
    let status = response.status();
    if !status.is_success() {
        return Err(status_error(status));
    }
    if status == StatusCode::NO_CONTENT {
        return serde_json::from_value(json!({})).map_err(|_| {
            anyhow!("Bluesky returned an empty response where account data was required")
        });
    }
    let bytes = read_body(response).await?;
    // Some successful procedures intentionally have no output schema/body.
    let bytes = if bytes.is_empty() {
        b"{}".as_slice()
    } else {
        &bytes
    };
    serde_json::from_slice(bytes).map_err(|error| match error.classify() {
        serde_json::error::Category::Data => anyhow!(
            "Bluesky returned an unexpected JSON schema (HTTP {})",
            status.as_u16()
        ),
        _ => anyhow!(
            "Bluesky returned invalid or incomplete JSON (HTTP {})",
            status.as_u16()
        ),
    })
}

/// Read a secret without echoing it. Noninteractive callers must opt into stdin.
pub fn prompt_app_password() -> Result<String> {
    use crossterm::{
        event::{self, Event, KeyCode, KeyModifiers},
        terminal,
    };
    use std::io::{self, IsTerminal, Write};
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        bail!(
            "Bluesky login needs a terminal, or use --app-password-stdin to read an app password from standard input"
        );
    }
    eprintln!("Create an app password in Bluesky Settings → Privacy and security → App passwords.");
    terminal::enable_raw_mode().context("Could not hide app-password input")?;
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = terminal::disable_raw_mode();
            eprintln!();
        }
    }
    let _restore = Restore;
    eprint!("App password (hidden): ");
    io::stderr().flush()?;
    let mut password = String::new();
    loop {
        if let Event::Key(key) = event::read()? {
            if key.kind == event::KeyEventKind::Release {
                continue;
            }
            match key.code {
                KeyCode::Enter => return Ok(password),
                KeyCode::Esc => bail!("Bluesky login canceled"),
                KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    bail!("Bluesky login canceled")
                }
                KeyCode::Backspace => {
                    password.pop();
                }
                KeyCode::Char(c)
                    if !c.is_control()
                        && !key.modifiers.contains(KeyModifiers::CONTROL)
                        && password.len() < 1024 =>
                {
                    password.push(c);
                }
                _ => {}
            }
        }
    }
}
