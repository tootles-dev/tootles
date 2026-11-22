//! Browser-based Mastodon authorization; Tootles never asks for a password.
use std::{
    io::{self, IsTerminal, Write},
    process::{Command, Stdio},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use url::Url;

use crate::{
    config::SavedAccount,
    provider::{SocialProvider, mastodon::Mastodon},
};

const REDIRECT_URI: &str = "urn:ietf:wg:oauth:2.0:oob";
const SCOPES: &str = "read write follow";

#[derive(Deserialize)]
struct RegisteredApp {
    client_id: String,
    client_secret: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

/// Add a verified account using an existing token or browser OAuth.
pub async fn login(
    instance: &str,
    token: Option<String>,
    label: Option<String>,
) -> Result<SavedAccount> {
    let instance = normalize_instance(instance)?;
    let token = match token {
        Some(token) => token.trim().to_owned(),
        None => browser_authorize(&instance).await?,
    };
    if token.is_empty() {
        bail!("The access token is empty");
    }
    let provider = Mastodon::new(&instance, &token)?;
    let account = provider
        .me()
        .await
        .context("Could not verify the Mastodon account")?;
    let origin = Url::parse(&instance)?;
    let account_label = if account.acct.contains('@') {
        format!("@{}", account.acct)
    } else {
        format!("@{}@{}", account.acct, origin.host_str().unwrap_or(""))
    };
    Ok(SavedAccount {
        id: format!("mastodon:{}:{}", instance, account.id),
        label: label
            .filter(|label| !label.trim().is_empty())
            .unwrap_or(account_label),
        provider: "mastodon".into(),
        instance,
        token,
    })
}

/// Accept a server hostname or HTTPS origin. Plain HTTP is restricted to local
/// development servers, so credentials are never sent over a public HTTP link.
pub fn normalize_instance(instance: &str) -> Result<String> {
    let instance = instance.trim();
    if instance.is_empty() || instance.chars().any(char::is_control) {
        bail!("Enter a Mastodon server hostname, for example mastodon.social");
    }
    let input = if instance.contains("://") {
        instance.to_owned()
    } else {
        format!("https://{instance}")
    };
    let url = Url::parse(&input).context("Invalid Mastodon server address")?;
    let local = match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    };
    if url.scheme() != "https" && !(url.scheme() == "http" && local) {
        bail!("Mastodon servers must use HTTPS (HTTP is allowed only for localhost)");
    }
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        bail!("Use only the server hostname or origin, without a profile path or credentials");
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

async fn browser_authorize(instance: &str) -> Result<String> {
    if !io::stdin().is_terminal() {
        bail!(
            "Browser login needs an interactive terminal; use an access token for unattended login"
        );
    }
    let client = reqwest::Client::builder()
        .user_agent(concat!("tootles/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        // Never forward an OAuth code or client secret through a redirect.
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = client
        .post(format!("{instance}/api/v1/apps"))
        .form(&[
            ("client_name", "Tootles"),
            ("redirect_uris", REDIRECT_URI),
            ("scopes", SCOPES),
        ])
        .send()
        .await
        .context("Could not register Tootles with this server")?;
    let response = check_response(response, "Application registration")?;
    let app: RegisteredApp = response
        .json()
        .await
        .context("The server returned an invalid application registration")?;
    if app.client_id.is_empty() || app.client_secret.is_empty() {
        bail!("The server returned incomplete application credentials");
    }
    let mut authorize = Url::parse(&format!("{instance}/oauth/authorize"))?;
    authorize.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("client_id", app.client_id.as_str()),
        ("redirect_uri", REDIRECT_URI),
        ("scope", SCOPES),
        ("force_login", "true"),
    ]);
    eprintln!("Authorize Tootles in your browser:\n{authorize}\n");
    if open_url(authorize.as_str()).is_err() {
        eprintln!("A browser could not be opened. Open the address above manually.");
    }
    eprint!("Paste the authorization code shown by Mastodon: ");
    io::stderr().flush()?;
    let code = tokio::task::spawn_blocking(read_authorization_code)
        .await
        .context("Could not read the authorization code")??;
    let response = client
        .post(format!("{instance}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", app.client_id.as_str()),
            ("client_secret", app.client_secret.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("code", code.as_str()),
        ])
        .send()
        .await
        .context("Could not exchange the authorization code")?;
    let response = check_response(response, "Authorization")?;
    let token: TokenResponse = response
        .json()
        .await
        .context("The server returned an invalid token response")?;
    Ok(token.access_token)
}

fn check_response(response: reqwest::Response, operation: &str) -> Result<reqwest::Response> {
    if !response.status().is_success() {
        // An arbitrary server error body can echo tokens or authorization codes.
        bail!(
            "{operation} failed (HTTP {}); check the server address and try login again",
            response.status()
        );
    }
    Ok(response)
}

fn read_authorization_code() -> Result<String> {
    let mut code = String::new();
    io::stdin().read_line(&mut code)?;
    let code = code.trim();
    if code.is_empty() || code.chars().any(char::is_control) {
        bail!("No valid authorization code was entered");
    }
    Ok(code.to_owned())
}

/// Launch a web URL without invoking a command shell. Other URI schemes are
/// deliberately rejected because posts and profiles are untrusted network data.
pub fn open_url(value: &str) -> Result<()> {
    let url = safe_web_url(value)?;
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = Command::new("xdg-open");
    let mut child = command
        .arg(url.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Could not launch the default browser")?;
    // Reap the launcher without blocking the TUI on the browser's lifetime.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn safe_web_url(value: &str) -> Result<Url> {
    if value.chars().any(char::is_control) {
        bail!("Cannot open a URL containing control characters");
    }
    let url = Url::parse(value).context("Invalid web URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("Only HTTP and HTTPS web links without embedded credentials can be opened");
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_links_cannot_execute_local_schemes() {
        for value in [
            "file:///tmp/a",
            "javascript:alert(1)",
            "mailto:a@example.com",
            "--help",
            "https://example.com\n--help",
            "https://user:pass@example.com",
        ] {
            assert!(safe_web_url(value).is_err(), "accepted {value}");
        }
        assert!(safe_web_url("https://example.com/@alice?foo=a&bar=b").is_ok());
    }

    #[test]
    fn instance_addresses_require_secure_origins() {
        assert_eq!(
            normalize_instance(" Mastodon.Social ").unwrap(),
            "https://mastodon.social"
        );
        assert_eq!(
            normalize_instance("http://127.0.0.1:8090/").unwrap(),
            "http://127.0.0.1:8090"
        );
        assert_eq!(
            normalize_instance("http://[::1]:8090/").unwrap(),
            "http://[::1]:8090"
        );
        for value in [
            "",
            "http://mastodon.social",
            "ftp://example.com",
            "https://example.com/@alice",
            "https://example.com?token=secret",
            "https://user:pass@example.com",
            "https://example.com/#fragment",
        ] {
            assert!(normalize_instance(value).is_err(), "accepted {value}");
        }
    }
}
