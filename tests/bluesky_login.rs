use std::{
    path::Path,
    process::{Output, Stdio},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{io::AsyncWriteExt, process::Command};
use tootles::{
    bluesky_auth::{self, Session, SessionClient},
    config::{self, Config, SavedAccount},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path},
};

const PASSWORD: &str = "fake-app-password-never-output";
const ACCESS: &str = "fake-access-never-output";
const REFRESH: &str = "fake-refresh-never-output";
const PRIVATE: &str = "private-server-content-never-output";
const DID: &str = "did:plc:abcdefghijklmnopqrstuvwx";
const HANDLE: &str = "alice.bsky.social";
const FEED: &str = "app.bsky.feed.getTimeline";

fn session(access: &str, refresh: &str) -> Value {
    json!({"did":DID,"handle":HANDLE,"accessJwt":access,"refreshJwt":refresh})
}

fn account(server: &MockServer) -> SavedAccount {
    SavedAccount {
        id: format!("bluesky:{DID}"),
        label: "Alice".into(),
        provider: "bluesky".into(),
        instance: server.uri(),
        token: session(ACCESS, REFRESH).to_string(),
    }
}

fn save_account(server: &MockServer, root: &Path) -> SessionClient {
    let account = account(server);
    Config {
        accounts: vec![account.clone()],
        active: Some(account.id.clone()),
        ..Config::default()
    }
    .save(Some(root))
    .unwrap();
    SessionClient::from_saved(&account, Some(root.to_path_buf())).unwrap()
}

async fn login_cli(server: &MockServer, root: &Path, stdin: bool, extra: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tootles"));
    command
        .arg("--config-dir")
        .arg(root)
        .args([
            "login",
            "--provider",
            "bluesky",
            HANDLE,
            "--pds",
            &server.uri(),
        ])
        .args(extra)
        .env_remove("TOOTLES_TOKEN")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if stdin {
        command.arg("--app-password-stdin");
    }
    let mut child = command.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    input
        .write_all(format!("{PASSWORD}\n").as_bytes())
        .await
        .unwrap();
    drop(input);
    tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
        .await
        .unwrap()
        .unwrap()
}

fn assert_private(text: &str) {
    for secret in [PASSWORD, ACCESS, REFRESH, PRIVATE] {
        assert!(!text.contains(secret), "secret leaked");
    }
}

async fn login_response(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/xrpc/com.atproto.server.createSession"))
        .and(body_json(json!({"identifier":HANDLE,"password":PASSWORD})))
        .respond_with(response)
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn cli_login_stores_tokens_without_password_and_preserves_other_accounts() {
    let server = MockServer::start().await;
    login_response(
        &server,
        ResponseTemplate::new(200).set_body_json(session(ACCESS, REFRESH)),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config");
    let mastodon = SavedAccount {
        id: "mastodon:other".into(),
        label: "Other".into(),
        provider: "mastodon".into(),
        instance: "https://example.social".into(),
        token: "other-secret".into(),
    };
    Config {
        accounts: vec![mastodon],
        theme: "paper".into(),
        ..Config::default()
    }
    .save(Some(&root))
    .unwrap();
    let output = login_cli(&server, &root, true, &["--label", "My Bluesky"]).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_private(&String::from_utf8_lossy(&output.stdout));
    assert_private(&String::from_utf8_lossy(&output.stderr));
    let stored = Config::load(Some(&root)).unwrap();
    assert_eq!(stored.accounts.len(), 2);
    assert_eq!(stored.theme, "paper");
    let account = stored.active_account().unwrap();
    assert_eq!(account.id, format!("bluesky:{DID}"));
    assert_eq!(account.label, "My Bluesky");
    assert_eq!(account.provider, "bluesky");
    let tokens: Session = serde_json::from_str(&account.token).unwrap();
    assert_eq!(tokens.access_jwt, ACCESS);
    assert_eq!(tokens.refresh_jwt, REFRESH);
    assert_private(&format!("{stored:?} {tokens:?}"));
    assert!(
        !std::fs::read_to_string(root.join("config.toml"))
            .unwrap()
            .contains(PASSWORD)
    );
    let request = &server.received_requests().await.unwrap()[0];
    assert!(!request.headers.contains_key("authorization"));
}

#[tokio::test]
async fn entryway_discovers_matching_pds_without_forwarding_password() {
    let entryway = MockServer::start().await;
    let pds = MockServer::start().await;
    let mut wire = session(ACCESS, REFRESH);
    wire["didDoc"] = json!({"id":DID,"service":[{"id":format!("{DID}#atproto_pds"),
        "type":"AtprotoPersonalDataServer","serviceEndpoint":pds.uri()}]});
    login_response(&entryway, ResponseTemplate::new(200).set_body_json(wire)).await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config");
    let result = login_cli(&entryway, &root, true, &[]).await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let saved = Config::load(Some(&root)).unwrap().accounts.remove(0);
    assert_eq!(saved.instance, pds.uri());
    assert_eq!(saved.label, format!("@{HANDLE} · Bluesky"));
    assert!(pds.received_requests().await.unwrap().is_empty());
    Mock::given(method("GET"))
        .and(path(format!("/xrpc/{FEED}")))
        .and(header("authorization", format!("Bearer {ACCESS}")))
        .and(header("atproto-proxy", "did:web:api.bsky.app#bsky_appview"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"feed":[]})))
        .expect(1)
        .mount(&pds)
        .await;
    SessionClient::from_saved(&saved, Some(root))
        .unwrap()
        .get(FEED, &[])
        .await
        .unwrap();
    assert_eq!(entryway.received_requests().await.unwrap().len(), 1);
    assert!(
        !String::from_utf8_lossy(&pds.received_requests().await.unwrap()[0].body)
            .contains(PASSWORD)
    );
}

#[tokio::test]
async fn invalid_session_and_foreign_or_insecure_pds_are_rejected_without_leaks() {
    let mut foreign = session(ACCESS, REFRESH);
    foreign["didDoc"] = json!({"id":"did:plc:someoneelse","service":[]});
    let mut insecure = session(ACCESS, REFRESH);
    insecure["didDoc"] = json!({"id":DID,"service":[{"id":"#atproto_pds",
        "type":"AtprotoPersonalDataServer","serviceEndpoint":"http://example.com"}]});
    let mut inactive = session(ACCESS, REFRESH);
    inactive["active"] = json!(false);
    let mut invalid_identity = session(ACCESS, REFRESH);
    invalid_identity["did"] = json!("");
    let responses = vec![
        ResponseTemplate::new(200).set_body_json(json!({})),
        ResponseTemplate::new(200).set_body_json(json!({"error":PRIVATE})),
        ResponseTemplate::new(200)
            .set_body_raw(format!("<html>{PRIVATE}{ACCESS}</html>"), "text/html"),
        ResponseTemplate::new(200)
            .set_body_raw(format!("{{{PRIVATE}{REFRESH}"), "application/json"),
        ResponseTemplate::new(200).set_body_json(foreign),
        ResponseTemplate::new(200).set_body_json(insecure),
        ResponseTemplate::new(200).set_body_json(inactive),
        ResponseTemplate::new(200).set_body_json(invalid_identity),
        ResponseTemplate::new(401).set_body_json(json!({"message":PRIVATE,"password":PASSWORD})),
    ];
    for response in responses {
        let server = MockServer::start().await;
        login_response(&server, response).await;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("config");
        let result = login_cli(&server, &root, true, &[]).await;
        assert!(!result.status.success());
        assert_private(&String::from_utf8_lossy(&result.stdout));
        assert_private(&String::from_utf8_lossy(&result.stderr));
        assert!(!root.join("config.toml").exists());
    }
}

#[tokio::test]
async fn login_redirect_is_not_followed_and_noninteractive_secret_requires_flag() {
    let destination = MockServer::start().await;
    let server = MockServer::start().await;
    login_response(
        &server,
        ResponseTemplate::new(307).insert_header("location", destination.uri()),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config");
    let output = login_cli(&server, &root, true, &[]).await;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("redirect"));
    assert!(destination.received_requests().await.unwrap().is_empty());
    let output = login_cli(&server, &root, false, &[]).await;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--app-password-stdin"));
    assert_private(&String::from_utf8_lossy(&output.stderr));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

async fn expired_and_refresh(server: &MockServer, status: u16, delay: Duration) {
    Mock::given(path(format!("/xrpc/{FEED}")))
        .and(header("authorization", format!("Bearer {ACCESS}")))
        .respond_with(
            ResponseTemplate::new(status)
                .set_body_json(json!({"error":"ExpiredToken","message":PRIVATE})),
        )
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/xrpc/com.atproto.server.refreshSession"))
        .and(header("authorization", format!("Bearer {REFRESH}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(session("rotated-access", "rotated-refresh"))
                .set_delay(delay),
        )
        .expect(1)
        .mount(server)
        .await;
    Mock::given(path(format!("/xrpc/{FEED}")))
        .and(header("authorization", "Bearer rotated-access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"feed":[]})))
        .mount(server)
        .await;
}

#[tokio::test]
async fn parallel_clients_refresh_once_and_persist_before_retried_requests() {
    for status in [400, 401] {
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("config");
        let client = save_account(&server, &root);
        let second_client =
            SessionClient::from_saved(&account(&server), Some(root.clone())).unwrap();
        expired_and_refresh(&server, status, Duration::from_millis(100)).await;
        let clone = client.clone();
        let check_root = root.clone();
        Mock::given(path(format!("/xrpc/{FEED}")))
            .and(header("authorization", "Bearer rotated-access"))
            .respond_with(move |_: &wiremock::Request| {
                let saved = Config::load(Some(&check_root)).unwrap();
                assert!(
                    saved.accounts[0].token.contains("rotated-refresh"),
                    "renewed tokens were used before persistence"
                );
                ResponseTemplate::new(200).set_body_json(json!({"feed":[]}))
            })
            .with_priority(1)
            .expect(3)
            .mount(&server)
            .await;
        let results = tokio::join!(
            client.get(FEED, &[]),
            clone.get(FEED, &[]),
            second_client.get(FEED, &[])
        );
        results.0.unwrap();
        results.1.unwrap();
        results.2.unwrap();
        let stored = Config::load(Some(&root)).unwrap();
        let token: Session = serde_json::from_str(&stored.accounts[0].token).unwrap();
        assert_eq!(token.access_jwt, "rotated-access");
        assert_eq!(token.refresh_jwt, "rotated-refresh");
        let requests = server.received_requests().await.unwrap();
        let refresh = requests
            .iter()
            .find(|r| r.url.path().ends_with("refreshSession"))
            .unwrap();
        assert!(!refresh.headers.contains_key("atproto-proxy"));
        assert!(refresh.body.is_empty());
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.url.path().ends_with("refreshSession"))
                .count(),
            1
        );
    }
}

async fn wait_for_refresh(server: &MockServer) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .any(|r| r.url.path().ends_with("refreshSession"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn refresh_does_not_restore_logout_or_overwrite_new_login() {
    for relogin in [false, true] {
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("config");
        let client = save_account(&server, &root);
        expired_and_refresh(&server, 401, Duration::from_millis(250)).await;
        let pending = tokio::spawn(async move { client.get(FEED, &[]).await });
        wait_for_refresh(&server).await;
        config::update(Some(&root), |config| {
            if relogin {
                config.accounts[0].token =
                    session("new-login-access", "new-login-refresh").to_string();
            } else {
                config.accounts.clear();
                config.active = None;
            }
            Ok(())
        })
        .unwrap();
        let error = pending.await.unwrap().unwrap_err();
        assert!(format!("{error:#}").contains("changed during refresh"));
        assert_private(&format!("{error:#}"));
        let stored = Config::load(Some(&root)).unwrap();
        if relogin {
            assert!(stored.accounts[0].token.contains("new-login-access"));
        } else {
            assert!(stored.accounts.is_empty());
        }
        assert!(!server.received_requests().await.unwrap().iter().any(|r| {
            r.headers
                .get("authorization")
                .is_some_and(|v| v == "Bearer rotated-access")
        }));
    }
}

#[tokio::test]
async fn refresh_preserves_concurrent_preferences_and_other_account_addition() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config");
    let client = save_account(&server, &root);
    expired_and_refresh(&server, 401, Duration::from_millis(200)).await;
    let pending = tokio::spawn(async move { client.get(FEED, &[]).await });
    wait_for_refresh(&server).await;
    config::update(Some(&root), |config| {
        let mut other = account(&server);
        other.id = "other".into();
        other.token = "other-token".into();
        config.accounts.push(other);
        config.active = Some("other".into());
        config.theme = "paper".into();
        Ok(())
    })
    .unwrap();
    pending.await.unwrap().unwrap();
    let saved = Config::load(Some(&root)).unwrap();
    assert_eq!(saved.accounts.len(), 2);
    assert_eq!(saved.accounts[1].token, "other-token");
    assert_eq!(saved.active.as_deref(), Some("other"));
    assert_eq!(saved.theme, "paper");
    assert!(saved.accounts[0].token.contains("rotated-access"));
}

#[tokio::test]
async fn canceled_request_still_saves_rotated_credentials() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config");
    let client = save_account(&server, &root);
    expired_and_refresh(&server, 401, Duration::from_millis(150)).await;
    let pending = tokio::spawn(async move { client.get(FEED, &[]).await });
    wait_for_refresh(&server).await;
    pending.abort();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !Config::load(Some(&root)).unwrap().accounts[0]
            .token
            .contains("rotated-access")
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn rejected_refresh_never_retries_or_reveals_response_contents() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config");
    let client = save_account(&server, &root);
    Mock::given(path(format!("/xrpc/{FEED}")))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/xrpc/com.atproto.server.refreshSession"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"message":format!("{PRIVATE}{REFRESH}")})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let error = client.get(FEED, &[]).await.unwrap_err();
    assert_private(&format!("{error:#}"));
    assert!(format!("{error:#}").contains("sign in again"));
    assert_eq!(
        Config::load(Some(&root)).unwrap().accounts[0].token,
        account(&server).token
    );
    assert!(
        client
            .get(FEED, &[])
            .await
            .unwrap_err()
            .to_string()
            .contains("sign in again")
    );
}

#[tokio::test]
async fn refreshed_identity_must_stay_the_same() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config");
    let client = save_account(&server, &root);
    Mock::given(path(format!("/xrpc/{FEED}")))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    let mut changed = session("rotated-access", "rotated-refresh");
    changed["did"] = json!("did:plc:differentaccount");
    Mock::given(path("/xrpc/com.atproto.server.refreshSession"))
        .respond_with(ResponseTemplate::new(200).set_body_json(changed))
        .expect(1)
        .mount(&server)
        .await;
    let error = client.get(FEED, &[]).await.unwrap_err();
    assert!(error.to_string().contains("changed session identity"));
    assert_private(&format!("{error:#}"));
    assert_eq!(
        Config::load(Some(&root)).unwrap().accounts[0].token,
        account(&server).token
    );
    assert!(client.get(FEED, &[]).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn failed_persistence_does_not_use_rotated_credentials() {
    use std::os::unix::fs::PermissionsExt;
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config");
    let client = save_account(&server, &root);
    expired_and_refresh(&server, 401, Duration::from_millis(150)).await;
    let request_client = client.clone();
    let pending = tokio::spawn(async move { request_client.get(FEED, &[]).await });
    wait_for_refresh(&server).await;
    std::fs::set_permissions(
        root.join("config.toml"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let error = pending.await.unwrap().unwrap_err();
    assert!(format!("{error:#}").contains("no request was retried"));
    assert_private(&format!("{error:#}"));
    std::fs::set_permissions(
        root.join("config.toml"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert_eq!(
        Config::load(Some(&root)).unwrap().accounts[0].token,
        account(&server).token
    );
    assert!(client.get(FEED, &[]).await.is_err());
    assert!(!server.received_requests().await.unwrap().iter().any(|r| {
        r.headers
            .get("authorization")
            .is_some_and(|v| v == "Bearer rotated-access")
    }));
}

#[tokio::test]
async fn upload_and_pds_calls_do_not_proxy_and_record_not_found_is_typed() {
    let server = MockServer::start().await;
    let client = SessionClient::from_saved(&account(&server), None).unwrap();
    Mock::given(method("POST"))
        .and(path("/xrpc/com.atproto.repo.uploadBlob"))
        .and(header("content-type", "image/png"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"blob":{"ref":{"$link":"fake"}}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    client
        .upload("image/png", b"test-only-bytes")
        .await
        .unwrap();
    Mock::given(method("GET"))
        .and(path("/xrpc/com.atproto.repo.getRecord"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({"error":"RecordNotFound","message":PRIVATE})),
        )
        .mount(&server)
        .await;
    let error = client
        .get("com.atproto.repo.getRecord", &[("repo", DID.into())])
        .await
        .unwrap_err();
    assert!(bluesky_auth::is_record_not_found(&error));
    assert_private(&format!("{error:#}"));
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| !r.headers.contains_key("atproto-proxy"))
    );
}

#[tokio::test]
async fn authenticated_redirect_and_invalid_methods_never_forward_credentials() {
    let server = MockServer::start().await;
    let destination = MockServer::start().await;
    let client = SessionClient::from_saved(&account(&server), None).unwrap();
    Mock::given(path(format!("/xrpc/{FEED}")))
        .respond_with(ResponseTemplate::new(307).insert_header("location", destination.uri()))
        .expect(1)
        .mount(&server)
        .await;
    assert!(
        client
            .get(FEED, &[])
            .await
            .unwrap_err()
            .to_string()
            .contains("redirect")
    );
    for invalid in [
        "https://example.com",
        "../xrpc/method",
        "app.bsky.feed.getTimeline?token=secret",
        "app..thing",
        "app.bsky.-thing",
    ] {
        assert!(client.get(invalid, &[]).await.is_err());
    }
    assert!(destination.received_requests().await.unwrap().is_empty());
}

#[test]
fn public_http_credentials_and_secret_config_debug_are_rejected() {
    for invalid in [
        "http://example.com",
        "https://u:secret@example.com",
        "https://example.com/path",
        "https://example.com?secret",
        "https://example.com/#secret",
    ] {
        let error = bluesky_auth::normalize_pds(invalid).unwrap_err();
        assert!(!error.to_string().contains("secret"));
    }
    assert_eq!(
        bluesky_auth::normalize_pds("bsky.social").unwrap(),
        "https://bsky.social"
    );
    assert!(bluesky_auth::normalize_pds("http://127.0.0.1:1234").is_ok());
    let mut bad = SavedAccount {
        id: format!("bluesky:{DID}"),
        label: "Alice".into(),
        provider: "bluesky".into(),
        instance: "https://bsky.social".into(),
        token: format!("{{{ACCESS}"),
    };
    let error = SessionClient::from_saved(&bad, None).err().unwrap();
    assert_private(&format!("{error:#}"));
    bad.token = session(ACCESS, REFRESH).to_string();
    bad.id = "bluesky:did:plc:other".into();
    assert!(SessionClient::from_saved(&bad, None).is_err());
}

#[test]
fn atomic_config_updates_merge_parallel_changes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config");
    Config::default().save(Some(&root)).unwrap();
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let root = root.clone();
            std::thread::spawn(move || {
                config::update(Some(&root), |config| {
                    config.accounts.push(SavedAccount {
                        id: format!("{i}"),
                        label: format!("{i}"),
                        provider: "mastodon".into(),
                        instance: "https://example.social".into(),
                        token: "test".into(),
                    });
                    Ok(())
                })
                .unwrap()
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(Config::load(Some(&root)).unwrap().accounts.len(), 8);
}
