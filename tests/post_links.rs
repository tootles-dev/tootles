use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{Terminal, backend::TestBackend};
use tootles::{
    app::App,
    config::Config,
    model::Post,
    ui,
    view::{Hit, Modal},
};

fn key(app: &mut App, code: KeyCode) {
    app.event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)), 140);
}
fn app(root: &std::path::Path) -> App {
    let mut app = App::new(Config::default(), Some(root.join("private")), true).unwrap();
    app.view.posts = vec![Post {
        id: "one".into(),
        text: "Read this (https://example.org/first). Then HTTP://EXAMPLE.NET/second and https://example.org/first. file:///tmp/private".into(),
        ..Default::default()
    }, Post { id: "two".into(), text: "https://second-post.example/".into(), ..Default::default() }];
    app
}

#[test]
fn picker_captures_original_destinations_and_supports_keyboard_navigation() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app(root.path());
    key(&mut app, KeyCode::Char('L'));
    let Modal::Links { urls, selected } = &app.view.modal else {
        panic!("expected link picker")
    };
    assert_eq!(
        urls,
        &["https://example.org/first", "http://example.net/second"]
    );
    assert_eq!(*selected, 0);
    app.view.selected = 1; // A background refresh must not retarget this picker.
    key(&mut app, KeyCode::End);
    assert!(
        matches!(&app.view.modal, Modal::Links { urls, selected: 1 } if urls[1] == "http://example.net/second")
    );
    key(&mut app, KeyCode::Home);
    assert!(matches!(app.view.modal, Modal::Links { selected: 0, .. }));
    key(&mut app, KeyCode::PageDown);
    assert!(matches!(app.view.modal, Modal::Links { selected: 1, .. }));
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.view.modal, Modal::None));
    assert!(!app.quit);
}

#[test]
fn hidden_post_links_require_reveal_even_for_direct_link_commands() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app(root.path());
    app.view.posts[0].spoiler_text = "A content warning".into();
    for command in ["links", "link:0", "post:0:link:0"] {
        app.command(command);
        assert!(matches!(app.view.modal, Modal::None));
        assert!(app.view.notice.as_ref().unwrap().message.contains("reveal"));
    }
    app.command("reveal");
    app.command("links");
    assert!(matches!(app.view.modal, Modal::Links { .. }));
    app.command("cancel");
    app.view.posts[0].text = "file:///tmp/a https://user:secret@example.org/".into();
    app.command("links");
    assert!(matches!(app.view.modal, Modal::None));
    assert!(
        app.view
            .notice
            .as_ref()
            .unwrap()
            .message
            .contains("no HTTP")
    );
}

#[test]
fn clicking_link_count_selects_the_clicked_post_before_opening_picker() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app(root.path());
    let mut terminal = Terminal::new(TestBackend::new(140, 48)).unwrap();
    terminal
        .draw(|frame| ui::render(frame, &mut app.view))
        .unwrap();
    let button = app
        .view
        .hits
        .iter()
        .find(|h| matches!(&h.hit, Hit::Command(cmd) if cmd == "post:1:links"))
        .unwrap()
        .area;
    app.event(
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: button.x,
            row: button.y,
            modifiers: KeyModifiers::NONE,
        }),
        140,
    );
    assert_eq!(app.view.selected, 1);
    assert!(
        matches!(&app.view.modal, Modal::Links { urls, selected: 0 } if urls == &["https://second-post.example/"])
    );
}
