use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};
use tootles::{
    app::App,
    config::{self, Config, SavedAccount},
    view::Modal,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};

fn key(app: &mut App, code: KeyCode) {
    app.event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)), 140);
}
async fn settle(app: &mut App) {
    for _ in 0..200 {
        tokio::time::sleep(Duration::from_millis(5)).await;
        app.drain();
        if !app.view.loading && !app.view.busy && !app.view.me.id.is_empty() {
            return;
        }
    }
    panic!("app did not settle: {:?}", app.view.notice);
}
async fn pump(app: &mut App, millis: u64) {
    for _ in 0..(millis / 5).max(1) {
        tokio::time::sleep(Duration::from_millis(5)).await;
        app.drain();
    }
}
fn demo(root: &std::path::Path) -> App {
    App::new(Config::default(), Some(root.join("private")), true).unwrap()
}

#[tokio::test]
async fn late_list_result_does_not_replace_new_composer() {
    let root = tempfile::tempdir().unwrap();
    let mut app = demo(root.path());
    app.prepare_snapshot().await.unwrap();
    app.command("lists");
    app.command("compose");
    app.event(Event::Paste("A thought worth keeping".into()), 140);
    pump(&mut app, 30).await;
    assert!(matches!(app.view.modal, Modal::Compose));
    assert_eq!(app.view.composer.text, "A thought worth keeping");
    assert!(
        !app.view.lists.is_empty(),
        "the response still updates cached lists"
    );
}

#[tokio::test]
async fn deleting_current_saved_draft_then_quitting_does_not_resurrect_it() {
    let root = tempfile::tempdir().unwrap();
    let mut app = demo(root.path());
    app.command("compose");
    app.event(Event::Paste("This draft can be discarded".into()), 140);
    app.command("cancel");
    assert_eq!(app.view.drafts.len(), 1);
    app.command("drafts");
    key(&mut app, KeyCode::Char('D'));
    assert!(app.view.drafts.is_empty());
    app.command("quit");
    assert!(app.quit);
    assert!(
        config::load_drafts(Some(&root.path().join("private")), "demo")
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn notification_account_actions_target_actor_not_referenced_post_author() {
    let root = tempfile::tempdir().unwrap();
    let mut app = demo(root.path());
    app.initialize();
    settle(&mut app).await;
    app.command("notifications");
    settle(&mut app).await;
    app.view.selected = app
        .view
        .notifications
        .iter()
        .position(|n| n.kind == "favourite")
        .unwrap();
    let actor = app.view.notifications[app.view.selected].account.id.clone();
    assert_ne!(actor, app.view.me.id);
    assert_eq!(app.view.selected_post().unwrap().account.id, app.view.me.id);
    app.command("follow");
    assert!(app.view.busy, "must act on notification sender");
    settle(&mut app).await;
    app.command("profile");
    settle(&mut app).await;
    assert_eq!(app.view.profile.as_ref().unwrap().id, actor);
    assert!(
        !app.view.relationship.following,
        "the previously followed demo sender was unfollowed"
    );
}

#[tokio::test]
async fn confirmation_keeps_original_post_when_selection_changes() {
    let root = tempfile::tempdir().unwrap();
    let mut app = demo(root.path());
    app.initialize();
    settle(&mut app).await;
    for message in ["First test post", "Second test post"] {
        app.command("compose");
        app.view.composer.text = message.into();
        app.command("send");
        settle(&mut app).await;
    }
    let first = app
        .view
        .posts
        .iter()
        .position(|p| p.text == "First test post")
        .unwrap();
    let first_id = app.view.posts[first].id.clone();
    let second = app
        .view
        .posts
        .iter()
        .position(|p| p.text == "Second test post")
        .unwrap();
    let second_id = app.view.posts[second].id.clone();
    app.view.selected = first;
    app.command("delete");
    assert!(matches!(app.view.modal, Modal::Confirm { .. }));
    // This can happen when an already pending refresh finishes under the dialog.
    app.view.selected = second;
    app.command("confirm");
    settle(&mut app).await;
    assert!(app.view.posts.iter().all(|p| p.id != first_id));
    assert!(app.view.posts.iter().any(|p| p.id == second_id));
}

#[tokio::test]
async fn removing_bookmark_refreshes_saved_feed() {
    let root = tempfile::tempdir().unwrap();
    let mut app = demo(root.path());
    app.initialize();
    settle(&mut app).await;
    app.command("bookmarks");
    settle(&mut app).await;
    let id = app.view.selected_post().unwrap().id.clone();
    app.command("bookmark");
    settle(&mut app).await;
    assert!(app.view.posts.iter().all(|p| p.id != id));
}

fn saved(server: &MockServer, id: &str) -> SavedAccount {
    SavedAccount {
        id: id.into(),
        label: id.into(),
        provider: "mastodon".into(),
        instance: server.uri(),
        token: format!("token-{id}"),
    }
}
fn account(id: &str) -> Value {
    json!({"id":id,"username":id,"acct":id,"display_name":id})
}
fn post(id: &str, author: &str) -> Value {
    json!({"id":id,"account":account(author),"content":format!("<p>Post {id}</p>"),"visibility":"public"})
}
async fn mount_identity(server: &MockServer, id: &str, delay: u64) {
    Mock::given(path("/api/v1/accounts/verify_credentials"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(account(id))
                .set_delay(Duration::from_millis(delay)),
        )
        .mount(server)
        .await;
    Mock::given(path("/api/v2/instance"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(server)
        .await;
}

#[tokio::test]
async fn late_identity_and_feed_cannot_cross_account_boundary() {
    let a = MockServer::start().await;
    let b = MockServer::start().await;
    mount_identity(&a, "alice", 250).await;
    mount_identity(&b, "bob", 0).await;
    Mock::given(path("/api/v1/timelines/home"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([post("alice-post", "alice")]))
                .set_delay(Duration::from_millis(250)),
        )
        .mount(&a)
        .await;
    Mock::given(path("/api/v1/timelines/home"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([post("bob-post", "bob")])))
        .mount(&b)
        .await;
    let root = tempfile::tempdir().unwrap();
    let config = Config {
        accounts: vec![saved(&a, "alice"), saved(&b, "bob")],
        active: Some("alice".into()),
        ..Config::default()
    };
    config.save(Some(&root.path().join("private"))).unwrap();
    let mut app = App::new(config, Some(root.path().join("private")), false).unwrap();
    app.initialize();
    pump(&mut app, 20).await;
    app.view.modal = Modal::Accounts { selected: 2 };
    app.command("modal_activate");
    settle(&mut app).await;
    pump(&mut app, 300).await;
    assert_eq!(app.view.me.id, "bob");
    assert_eq!(app.view.posts.len(), 1);
    assert_eq!(app.view.posts[0].id, "bob-post");
    assert!(!app.view.demo);
}

#[tokio::test]
async fn late_relationship_result_does_not_overwrite_another_profile() {
    let server = MockServer::start().await;
    mount_identity(&server, "me", 0).await;
    Mock::given(path("/api/v1/timelines/home"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([post("a-post", "alice"), post("b-post", "bob")])),
        )
        .mount(&server)
        .await;
    for id in ["alice", "bob"] {
        Mock::given(method("GET"))
            .and(path("/api/v1/accounts/relationships"))
            .and(query_param("id[]", id))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"following":false}])))
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path("/api/v1/accounts/alice/follow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"following":true}))
                .set_delay(Duration::from_millis(150)),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/accounts/bob"))
        .respond_with(ResponseTemplate::new(200).set_body_json(account("bob")))
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/accounts/bob/statuses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([post("b-post", "bob")])))
        .mount(&server)
        .await;
    let root = tempfile::tempdir().unwrap();
    let config = Config {
        accounts: vec![saved(&server, "me")],
        ..Config::default()
    };
    let mut app = App::new(config, Some(root.path().join("private")), false).unwrap();
    app.initialize();
    settle(&mut app).await;
    app.command("follow");
    app.view.selected = 1;
    app.command("profile");
    settle(&mut app).await;
    assert_eq!(app.view.profile.as_ref().unwrap().id, "bob");
    assert!(!app.view.relationship.following);
}

#[tokio::test]
async fn delayed_edit_source_does_not_replace_new_user_intent() {
    let server = MockServer::start().await;
    mount_identity(&server, "me", 0).await;
    Mock::given(path("/api/v1/timelines/home"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([post("own", "me")])))
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/statuses/own/source"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"text":"Original text","spoiler_text":""}))
                .set_delay(Duration::from_millis(100)),
        )
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/statuses/own"))
        .respond_with(ResponseTemplate::new(200).set_body_json(post("own", "me")))
        .mount(&server)
        .await;
    let root = tempfile::tempdir().unwrap();
    let config = Config {
        accounts: vec![saved(&server, "me")],
        ..Config::default()
    };
    let mut app = App::new(config, Some(root.path().join("private")), false).unwrap();
    app.initialize();
    settle(&mut app).await;
    app.command("edit");
    app.command("help");
    settle(&mut app).await;
    assert!(matches!(app.view.modal, Modal::Help));
    assert!(app.view.composer.text.is_empty());
}

fn offline_saved(id: &str) -> SavedAccount {
    SavedAccount {
        id: id.into(),
        label: id.into(),
        provider: "mastodon".into(),
        instance: "https://example.invalid".into(),
        token: format!("dummy-{id}"),
    }
}

#[test]
fn preference_save_preserves_accounts_added_by_cli_and_its_active_selection() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("private");
    let original = Config {
        accounts: vec![offline_saved("alice")],
        active: Some("alice".into()),
        ..Config::default()
    };
    original.save(Some(&root)).unwrap();
    let mut app = App::new(original.clone(), Some(root.clone()), true).unwrap();
    let mut updated = original;
    updated.accounts.push(offline_saved("bob"));
    updated.active = Some("bob".into());
    updated.refresh_seconds = 91;
    updated.save(Some(&root)).unwrap();
    app.command("theme");
    let saved = Config::load(Some(&root)).unwrap();
    assert_eq!(saved.accounts.len(), 2);
    assert_eq!(saved.accounts[1].token, "dummy-bob");
    assert_eq!(saved.active.as_deref(), Some("bob"));
    assert_eq!(saved.refresh_seconds, 91);
    assert_eq!(saved.theme, "midnight");
    assert_eq!(
        app.view.accounts.len(),
        3,
        "the switcher includes the newly saved account"
    );
}

#[test]
fn preference_save_does_not_resurrect_account_logged_out_by_cli() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("private");
    let original = Config {
        accounts: vec![offline_saved("alice")],
        active: Some("alice".into()),
        ..Config::default()
    };
    original.save(Some(&root)).unwrap();
    let mut app = App::new(original, Some(root.clone()), true).unwrap();
    Config::default().save(Some(&root)).unwrap();
    app.command("theme");
    let saved = Config::load(Some(&root)).unwrap();
    assert!(saved.accounts.is_empty());
    assert!(saved.active.is_none());
    assert_eq!(saved.theme, "midnight");
    assert_eq!(app.view.accounts.len(), 1);
}

#[test]
fn switching_to_an_externally_removed_account_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("private");
    let original = Config {
        accounts: vec![offline_saved("alice")],
        ..Config::default()
    };
    original.save(Some(&root)).unwrap();
    let mut app = App::new(original, Some(root.clone()), true).unwrap();
    Config::default().save(Some(&root)).unwrap();
    app.view.modal = Modal::Accounts { selected: 1 };
    app.command("modal_activate");
    assert!(app.view.demo, "a removed account must not become active");
    assert!(app.view.notice.as_ref().unwrap().error);
    assert!(Config::load(Some(&root)).unwrap().accounts.is_empty());
}

#[test]
fn preference_write_failure_remains_visible_after_theme_and_motion_commands() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("private");
    let original = Config::default();
    original.save(Some(&root)).unwrap();
    let mut app = App::new(original, Some(root.clone()), true).unwrap();
    std::fs::write(root.join("config.toml"), "invalid TOML content").unwrap();
    for command in ["theme", "motion"] {
        app.command(command);
        let notice = app.view.notice.as_ref().unwrap();
        assert!(notice.error);
        assert!(notice.message.contains("Could not save settings"));
    }
}

#[tokio::test]
async fn failed_draft_save_keeps_editor_and_prevents_accidental_quit() {
    let root = tempfile::tempdir().unwrap();
    let mut app = demo(root.path());
    app.command("compose");
    app.event(
        Event::Paste("Keep this even when the disk fails".into()),
        140,
    );
    app.command("save_draft");
    let dir = root.path().join("private");
    let draft = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("drafts-")
        })
        .unwrap();
    std::fs::remove_file(&draft).unwrap();
    std::fs::create_dir(&draft).unwrap();
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.view.modal, Modal::Compose));
    assert!(app.view.notice.as_ref().unwrap().error);
    app.command("quit");
    assert!(!app.quit);
    assert_eq!(app.view.composer.text, "Keep this even when the disk fails");
}
