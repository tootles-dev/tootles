use std::{path::Path, process::Output, process::Stdio, time::Duration};

use serde_json::{Value, json};
use tokio::{io::AsyncWriteExt, process::Command};
use tootles::{
    config::Config,
    provider::{SocialProvider, mastodon::Mastodon},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

const TOKEN: &str = "fake-login-regression-token-never-print";
const SERVER_SENTINEL: &str = "private-server-response-must-not-be-printed";
const ACCOUNT_URI: &str = "https://example.social/users/alice";

fn credential_account() -> Value {
    json!({
        "id": "42",
        "username": "alice",
        "acct": "alice",
        "display_name": "Alice Example",
        "note": "<p>A test account</p>",
        "url": null,
        "uri": ACCOUNT_URI,
        "followers_count": 12,
        "following_count": 8,
        "statuses_count": 3,
        "locked": false,
        "source": {"privacy": "public", "sensitive": false, "language": "en"}
    })
}

async fn mount_credentials(server: &MockServer, response: ResponseTemplate, requests: u64) {
    Mock::given(method("GET"))
        .and(path("/api/v1/accounts/verify_credentials"))
        .and(header("authorization", format!("Bearer {TOKEN}")))
        .respond_with(response)
        .expect(requests)
        .mount(server)
        .await;
    // Credential verification may also discover the server's post limit.
    Mock::given(method("GET"))
        .and(path("/api/v2/instance"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "configuration": {"statuses": {"max_characters": 500}}
        })))
        .mount(server)
        .await;
}

async fn login(server: &MockServer, config_root: &Path, label: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tootles"));
    command
        .arg("--config-dir")
        .arg(config_root)
        .args(["login", &server.uri(), "--token-stdin"])
        .env_remove("TOOTLES_TOKEN")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(label) = label {
        command.args(["--label", label]);
    }
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(format!("{TOKEN}\n").as_bytes())
        .await
        .unwrap();
    drop(stdin);
    tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
        .await
        .expect("login subprocess timed out")
        .unwrap()
}

fn assert_private_output(output: &Output) {
    for stream in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(stream);
        assert!(!text.contains(TOKEN), "login disclosed its access token");
        assert!(
            !text.contains(SERVER_SENTINEL),
            "login disclosed server response contents"
        );
    }
}

fn assert_login_rejected(output: &Output, config_root: &Path) {
    assert!(
        !output.status.success(),
        "an invalid credential response was accepted"
    );
    assert_private_output(output);
    assert!(
        Config::load(Some(config_root)).unwrap().accounts.is_empty(),
        "a failed login must not save an account"
    );
    assert!(
        !config_root.join("config.toml").exists(),
        "a failed login must not create a credential file"
    );
    assert!(
        !output.stderr.is_empty(),
        "a failed login should explain the failure"
    );
}

#[tokio::test]
async fn token_stdin_login_accepts_null_account_url_and_uses_uri_fallback() {
    let server = MockServer::start().await;
    mount_credentials(
        &server,
        ResponseTemplate::new(200).set_body_json(credential_account()),
        2,
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("private");
    let output = login(&server, &root, None).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_private_output(&output);
    let config = Config::load(Some(&root)).unwrap();
    assert_eq!(config.accounts.len(), 1);
    let account = config.active_account().unwrap();
    assert_eq!(account.instance, server.uri());
    assert_eq!(account.label, "@alice@127.0.0.1");
    assert_eq!(account.provider, "mastodon");
    assert_eq!(account.token, TOKEN);
    assert_eq!(config.active.as_deref(), Some(account.id.as_str()));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Connected @alice@127.0.0.1"));

    // SavedAccount stores credentials rather than profile URLs, so verify the
    // URL conversion through the same provider that the CLI just authenticated.
    let profile = Mastodon::new(&server.uri(), TOKEN)
        .unwrap()
        .me()
        .await
        .unwrap();
    assert_eq!(profile.url, ACCOUNT_URI);
}

#[tokio::test]
async fn successful_cli_login_preserves_the_requested_account_label() {
    let server = MockServer::start().await;
    mount_credentials(
        &server,
        ResponseTemplate::new(200).set_body_json(credential_account()),
        1,
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("private");
    let output = login(&server, &root, Some("My infosec account")).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_private_output(&output);
    assert_eq!(
        Config::load(Some(&root)).unwrap().accounts[0].label,
        "My infosec account"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Connected My infosec account"));
}

#[tokio::test]
async fn successful_http_responses_without_verified_identity_are_rejected() {
    let mut responses = vec![
        json!({}),
        json!({"error": format!("{SERVER_SENTINEL}: {TOKEN}")}),
    ];
    for field in ["id", "username", "acct"] {
        let mut incomplete = credential_account();
        incomplete[field] = json!("");
        responses.push(incomplete);
    }
    for body in responses {
        let server = MockServer::start().await;
        mount_credentials(&server, ResponseTemplate::new(200).set_body_json(body), 1).await;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("private");
        assert_login_rejected(&login(&server, &root, None).await, &root);
    }
}

#[tokio::test]
async fn empty_success_response_is_rejected_without_saving_an_account() {
    let server = MockServer::start().await;
    mount_credentials(&server, ResponseTemplate::new(200), 1).await;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("private");
    assert_login_rejected(&login(&server, &root, None).await, &root);
}

#[tokio::test]
async fn malformed_json_success_response_is_rejected_without_echoing_body_or_token() {
    let server = MockServer::start().await;
    mount_credentials(
        &server,
        ResponseTemplate::new(200).set_body_raw(
            format!(r#"{{"id":"42","private":"{SERVER_SENTINEL}","token":"{TOKEN}""#),
            "application/json",
        ),
        1,
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("private");
    assert_login_rejected(&login(&server, &root, None).await, &root);
}

#[tokio::test]
async fn html_success_response_is_rejected_without_echoing_body_or_token() {
    let server = MockServer::start().await;
    mount_credentials(
        &server,
        ResponseTemplate::new(200).set_body_raw(
            format!("<!doctype html><h1>{SERVER_SENTINEL}</h1><p>{TOKEN}</p>"),
            "text/html; charset=utf-8",
        ),
        1,
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("private");
    assert_login_rejected(&login(&server, &root, None).await, &root);
}

#[tokio::test]
async fn wrong_schema_reports_the_field_without_echoing_its_sensitive_value() {
    let server = MockServer::start().await;
    let mut account = credential_account();
    account["followers_count"] = json!(format!("{SERVER_SENTINEL}: {TOKEN}"));
    mount_credentials(
        &server,
        ResponseTemplate::new(200).set_body_json(account),
        1,
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("private");
    let output = login(&server, &root, None).await;
    assert_login_rejected(&output, &root);
    assert!(String::from_utf8_lossy(&output.stderr).contains("followers_count"));
}
