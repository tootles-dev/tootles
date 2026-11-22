use tootles::{
    config::{Config, SavedAccount, config_dir, load_drafts, save_drafts},
    model::Compose,
};

fn account(id: &str, token: &str) -> SavedAccount {
    SavedAccount {
        id: id.into(),
        label: format!("@{id}@example.social"),
        provider: "mastodon".into(),
        instance: "https://example.social".into(),
        token: token.into(),
    }
}

#[test]
fn configuration_round_trip_preserves_accounts_and_preferences() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("private");
    let config = Config {
        accounts: vec![
            account("alice", "alice-secret"),
            account("bob", "bob-secret"),
        ],
        active: Some("bob".into()),
        theme: "paper".into(),
        reduced_motion: true,
        compact: true,
        refresh_seconds: 90,
    };
    config.save(Some(&root)).unwrap();
    let loaded = Config::load(Some(&root)).unwrap();
    assert_eq!(loaded.active_account().unwrap().token, "bob-secret");
    assert_eq!(loaded.accounts.len(), 2);
    assert_eq!(loaded.theme, "paper");
    assert!(loaded.reduced_motion && loaded.compact);
    assert_eq!(loaded.refresh_seconds, 90);
    assert!(!format!("{loaded:?}").contains("bob-secret"));
    assert!(!format!("{loaded:?}").contains("alice-secret"));
    config.save(Some(&root)).unwrap();
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
    assert!(root.join(".config.lock").is_file());
}

#[test]
fn missing_configuration_is_read_without_creating_it() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("missing");
    let config = Config::load(Some(&root)).unwrap();
    assert_eq!(config.theme, "tokyo-night");
    assert!(config.accounts.is_empty());
    assert!(load_drafts(Some(&root), "alice").unwrap().is_empty());
    assert!(!root.exists());
}

#[test]
fn drafts_are_scoped_to_account_and_paths_cannot_traverse() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("private");
    let draft = Compose {
        text: "An unfinished thought 🌙".into(),
        spoiler_text: "draft warning".into(),
        reply_to: Some("123".into()),
        ..Compose::default()
    };
    save_drafts(Some(&root), "../alice", &[draft]).unwrap();
    assert!(load_drafts(Some(&root), "alice").unwrap().is_empty());
    let drafts = load_drafts(Some(&root), "../alice").unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].text, "An unfinished thought 🌙");
    assert_eq!(drafts[0].reply_to.as_deref(), Some("123"));
    save_drafts(Some(&root), "../alice", &[]).unwrap();
    assert!(load_drafts(Some(&root), "../alice").unwrap().is_empty());
}

#[test]
fn long_opaque_account_ids_do_not_exceed_filename_limits() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("private");
    let account_id = "long-domain-and-account-id/".repeat(40);
    save_drafts(Some(&root), &account_id, &[]).unwrap();
    assert!(load_drafts(Some(&root), &account_id).unwrap().is_empty());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
}

#[test]
fn malformed_configuration_errors_do_not_disclose_tokens() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("private");
    Config::default().save(Some(&root)).unwrap();
    std::fs::write(root.join("config.toml"), "token = \"private-value").unwrap();
    let error = Config::load(Some(&root)).unwrap_err();
    assert!(!format!("{error:#}").contains("private-value"));
}

#[test]
fn parent_components_in_config_directory_are_rejected() {
    assert!(config_dir(Some(std::path::Path::new("/tmp/../tootles"))).is_err());
}

#[cfg(unix)]
#[test]
fn configuration_and_drafts_are_private_and_unsafe_modes_are_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("private");
    Config::default().save(Some(&root)).unwrap();
    save_drafts(Some(&root), "alice", &[]).unwrap();
    assert_eq!(
        std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for entry in std::fs::read_dir(&root).unwrap() {
        assert_eq!(
            entry.unwrap().metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let path = root.join("config.toml");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        Config::load(Some(&root))
            .unwrap_err()
            .to_string()
            .contains("chmod 600")
    );
    assert!(Config::default().save(Some(&root)).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        Config::load(Some(&root))
            .unwrap_err()
            .to_string()
            .contains("chmod 700")
    );
}

#[cfg(unix)]
#[test]
fn credential_symlinks_and_hard_links_are_rejected() {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("private");
    Config::default().save(Some(&root)).unwrap();
    let path = root.join("config.toml");
    let copy = root.join("original.toml");
    std::fs::rename(&path, &copy).unwrap();
    symlink(&copy, &path).unwrap();
    assert!(Config::load(Some(&root)).is_err());
    assert!(Config::default().save(Some(&root)).is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::hard_link(&copy, &path).unwrap();
    assert!(Config::load(Some(&root)).is_err());
    assert!(Config::default().save(Some(&root)).is_err());
    let linked_root = directory.path().join("linked");
    symlink(&root, &linked_root).unwrap();
    assert!(Config::load(Some(&linked_root)).is_err());
}

#[test]
fn active_account_uses_first_account_when_selection_is_stale() {
    let config = Config {
        accounts: vec![account("alice", "secret")],
        active: Some("removed".into()),
        ..Config::default()
    };
    assert_eq!(config.active_account().unwrap().id, "alice");
}

#[tokio::test]
async fn token_login_verifies_credentials_without_registering_an_oauth_app() {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path},
    };
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/accounts/verify_credentials"))
        .and(header("authorization", "Bearer private-test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "42", "username": "alice", "acct": "alice",
            "display_name": "Alice", "note": "", "url": "https://example.social/@alice",
            "followers_count": 12, "following_count": 8, "statuses_count": 3,
            "locked": false
        })))
        .expect(1)
        .mount(&server)
        .await;
    let account = tootles::auth::login(
        &server.uri(),
        Some("private-test-token".into()),
        Some("Personal".into()),
    )
    .await
    .unwrap();
    assert_eq!(account.label, "Personal");
    assert_eq!(account.instance, server.uri());
    assert_eq!(account.provider, "mastodon");
    assert_eq!(account.token, "private-test-token");
    assert!(account.id.ends_with(":42"));
    let requests = server.received_requests().await.unwrap();
    assert!(requests.iter().all(|request| request.method == "GET"));
    assert!(requests.iter().all(
        |request| !request.url.path().contains("oauth") && request.url.path() != "/api/v1/apps"
    ));
}

#[tokio::test]
async fn invalid_access_token_does_not_create_a_saved_account() {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
    let server = MockServer::start().await;
    Mock::given(path("/api/v1/accounts/verify_credentials"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": "The access token is invalid"
        })))
        .mount(&server)
        .await;
    assert!(
        tootles::auth::login(&server.uri(), Some("bad-token".into()), None)
            .await
            .is_err()
    );
}
