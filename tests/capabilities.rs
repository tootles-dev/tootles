use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{Terminal, backend::TestBackend};
use tootles::{
    app::{App, validate_compose},
    config::Config,
    model::{Attachment, Capabilities, Compose, Feed, Post, Visibility},
    ui,
    view::{Hit, Modal, View, available_commands},
};

fn bluesky() -> Capabilities {
    Capabilities {
        name: "Bluesky",
        polls: false,
        lists: false,
        editing: false,
        local_timeline: false,
        federated_timeline: false,
        post_visibility: false,
        content_warnings: false,
        image_only_media: true,
        grapheme_counting: true,
        max_characters: 300,
        max_text_bytes: Some(3000),
        ..Capabilities::default()
    }
}
fn key(app: &mut App, code: KeyCode) {
    app.event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)), 150);
}
fn screenshot(view: &mut View, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| ui::render(frame, view)).unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[test]
fn bluesky_composer_counts_graphemes_and_rejects_unsupported_payloads() {
    let caps = bluesky();
    let mut compose = Compose {
        text: "👩‍💻".repeat(300),
        ..Compose::default()
    };
    assert_eq!(caps.compose_length(&compose), 300);
    assert!(
        validate_compose(&compose, &caps).is_err(),
        "300 complex emoji exceed UTF-8 byte limit"
    );
    compose.text = "e\u{301}".repeat(300);
    assert!(validate_compose(&compose, &caps).is_ok());
    compose.text.push('!');
    assert!(validate_compose(&compose, &caps).is_err());
    compose.text = "A public thought".into();
    compose.visibility = Visibility::Private;
    assert!(
        validate_compose(&compose, &caps)
            .unwrap_err()
            .to_string()
            .contains("public")
    );
    compose.visibility = Visibility::Public;
    compose.spoiler_text = "Warning".into();
    assert!(validate_compose(&compose, &caps).is_err());
    compose.spoiler_text.clear();
    compose.sensitive = true;
    assert!(validate_compose(&compose, &caps).is_err());
    compose.sensitive = false;
    compose.edit_id = Some("old".into());
    assert!(validate_compose(&compose, &caps).is_err());
    compose.edit_id = None;
    compose.poll_options = vec!["Yes".into(), "No".into()];
    assert!(validate_compose(&compose, &caps).is_err());
    compose.poll_options.clear();
    compose.attachments.push(Attachment {
        path: "clip.mp4".into(),
        description: "A video".into(),
    });
    assert!(validate_compose(&compose, &caps).is_err());
    compose.attachments[0].path = "image.png".into();
    assert!(validate_compose(&compose, &caps).is_ok());
}

#[test]
fn unsupported_keyboard_and_mouse_navigation_do_not_dispatch_or_clear_feed() {
    let root = tempfile::tempdir().unwrap();
    let mut app = App::new(Config::default(), Some(root.path().join("private")), true).unwrap();
    app.view.capabilities = bluesky();
    app.view.posts = vec![Post {
        id: "keep".into(),
        text: "Keep the current feed".into(),
        ..Post::default()
    }];
    for code in [
        KeyCode::Char('2'),
        KeyCode::Char('3'),
        KeyCode::Char('e'),
        KeyCode::Char('l'),
        KeyCode::Char('v'),
    ] {
        key(&mut app, code);
        assert!(!app.view.busy);
        assert!(matches!(app.view.modal, Modal::None));
        assert_eq!(app.view.feed, Feed::Home);
        assert_eq!(app.view.posts[0].id, "keep");
        assert!(
            app.view
                .notice
                .as_ref()
                .unwrap()
                .message
                .contains("not available")
        );
    }
    app.view.hits.push(tootles::view::Hitbox {
        area: ratatui::layout::Rect::new(1, 1, 1, 1),
        hit: Hit::Navigate(Feed::Local),
    });
    app.event(
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        }),
        150,
    );
    assert_eq!(app.view.feed, Feed::Home);
    assert_eq!(app.view.posts[0].id, "keep");
    app.command("compose");
    assert!(app.view.composer.idempotency_key.contains(':'));
    for expected in [2, 3, 5, 0] {
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.view.compose_field, expected);
    }
    for command in [
        "visibility",
        "sensitive",
        "poll_multiple",
        "poll_duration",
        "create_list",
    ] {
        app.command(command);
        assert!(
            app.view
                .notice
                .as_ref()
                .unwrap()
                .message
                .contains("not available")
        );
    }
    assert_eq!(app.view.composer.visibility, Visibility::Public);
    assert!(!app.view.composer.sensitive);
    assert!(!app.view.composer.poll_multiple);
}

#[test]
fn unavailable_navigation_actions_and_composer_fields_are_absent_at_all_sizes() {
    for (width, height) in [(150, 48), (95, 35), (48, 22), (1, 1)] {
        let mut view = View {
            capabilities: bluesky(),
            ..View::default()
        };
        screenshot(&mut view, width, height);
        assert!(
            !view
                .hits
                .iter()
                .any(|hit| matches!(hit.hit, Hit::Navigate(Feed::Local | Feed::Federated)))
        );
        assert!(
            !view
                .hits
                .iter()
                .any(|hit| matches!(&hit.hit, Hit::Command(id) if id == "lists"))
        );
        view.modal = Modal::Compose;
        view.composer.text = "e\u{301}".repeat(5);
        let output = screenshot(&mut view, width, height);
        assert!(
            !view
                .hits
                .iter()
                .any(|hit| matches!(hit.hit, Hit::ComposerField(1 | 4)))
        );
        assert!(!view.hits.iter().any(|hit| matches!(&hit.hit, Hit::Command(id) if ["visibility", "sensitive", "poll_multiple", "poll_duration"].contains(&id.as_str()))));
        if width == 150 {
            assert!(output.contains("5 / 300"));
            assert!(output.contains("Public posts"));
            assert!(!output.contains("Content warning"));
            assert!(!output.contains("Poll ·"));
        }
    }
    let commands = available_commands("", &bluesky());
    for id in [
        "edit",
        "local",
        "federated",
        "lists",
        "vote",
        "create_list",
        "add_to_list",
    ] {
        assert!(!commands.iter().any(|command| command.id == id));
    }
    assert!(commands.iter().any(|command| command.id == "bookmarks"));
    assert!(commands.iter().any(|command| command.id == "compose"));
}

#[test]
fn bluesky_factory_is_offline_and_switcher_picks_up_new_network_accounts() {
    use tootles::{bluesky_auth::Session, config::SavedAccount};
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    let account = SavedAccount {
        id: "bluesky:did:plc:alice".into(),
        label: "alice.bsky.social".into(),
        provider: "bluesky".into(),
        instance: "https://bsky.social".into(),
        token: serde_json::to_string(&Session {
            did: "did:plc:alice".into(),
            handle: "alice.bsky.social".into(),
            access_jwt: "dummy-access".into(),
            refresh_jwt: "dummy-refresh".into(),
        })
        .unwrap(),
    };
    let config = Config {
        accounts: vec![account],
        ..Config::default()
    };
    let app = App::new(config.clone(), Some(directory.clone()), false).unwrap();
    assert_eq!(app.view.capabilities.name, "Bluesky");
    assert!(!app.view.capabilities.post_visibility);
    assert!(app.view.accounts[1].network.contains("Bluesky"));
    let mut demo = App::new(Config::default(), Some(directory.clone()), true).unwrap();
    config.save(Some(&directory)).unwrap();
    demo.command("accounts");
    assert_eq!(demo.view.accounts.len(), 2);
    assert!(demo.view.accounts[1].network.contains("Bluesky"));
}
