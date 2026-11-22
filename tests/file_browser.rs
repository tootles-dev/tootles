use std::{path::Path, time::Duration};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{Terminal, backend::TestBackend};
use tootles::{
    app::App,
    config::Config,
    file_picker::FileKind,
    ui,
    view::{Hit, Modal},
};

fn key(app: &mut App, code: KeyCode) {
    app.event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)), 140);
}
fn shortcut(app: &mut App, letter: char, modifiers: KeyModifiers) {
    app.event(
        Event::Key(KeyEvent::new(KeyCode::Char(letter), modifiers)),
        140,
    );
}
fn demo(root: &Path) -> App {
    App::new(Config::default(), Some(root.join("private-config")), true).unwrap()
}
async fn loaded(app: &mut App) {
    for _ in 0..200 {
        tokio::time::sleep(Duration::from_millis(5)).await;
        app.drain();
        if !app.view.file_browser.loading {
            return;
        }
    }
    panic!("file listing never finished");
}
async fn open(app: &mut App, directory: &Path) {
    app.command("compose");
    app.view.attachment_path = directory.to_string_lossy().into_owned();
    key(app, KeyCode::F(3));
    loaded(app).await;
    assert!(matches!(app.view.modal, Modal::FileBrowser));
}
fn click(app: &mut App, predicate: impl Fn(&Hit) -> bool) {
    let mut terminal = Terminal::new(TestBackend::new(140, 45)).unwrap();
    terminal
        .draw(|frame| ui::render(frame, &mut app.view))
        .unwrap();
    let target = app
        .view
        .hits
        .iter()
        .find(|hit| predicate(&hit.hit))
        .expect("visible mouse target")
        .area;
    app.event(
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: target.x,
            row: target.y,
            modifiers: KeyModifiers::NONE,
        }),
        140,
    );
}

#[tokio::test]
async fn browse_choose_describe_and_attach_preserves_the_composition() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("A photo.png");
    std::fs::write(&file, b"test image placeholder").unwrap();
    let mut app = demo(root.path());
    app.command("compose");
    app.view.composer.text = "A thought with a photograph".into();
    app.view.attachment_alt = "A description already in progress".into();
    app.view.attachment_path = file.to_string_lossy().into_owned();
    key(&mut app, KeyCode::F(3));
    loaded(&mut app).await;
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.view.modal, Modal::Compose));
    assert_eq!(app.view.compose_field, 3);
    assert_eq!(app.view.composer.text, "A thought with a photograph");
    assert_eq!(app.view.attachment_alt, "A description already in progress");
    assert!(
        app.view.composer.attachments.is_empty(),
        "choosing does not upload or attach implicitly"
    );
    shortcut(&mut app, 'a', KeyModifiers::ALT);
    assert_eq!(app.view.composer.attachments.len(), 1);
    assert_eq!(
        app.view.composer.attachments[0].path,
        file.to_str().unwrap()
    );
    assert_eq!(
        app.view.composer.attachments[0].description,
        "A description already in progress"
    );
}

#[tokio::test]
async fn escape_returns_to_composer_and_late_listing_cannot_replace_it() {
    let root = tempfile::tempdir().unwrap();
    let mut app = demo(root.path());
    app.command("compose");
    app.view.composer.text = "Do not discard this".into();
    app.view.attachment_alt = "Keep this too".into();
    app.view.file_browser.directory = root.path().to_owned();
    key(&mut app, KeyCode::F(3));
    key(&mut app, KeyCode::Esc);
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(5)).await;
        app.drain();
    }
    assert!(matches!(app.view.modal, Modal::Compose));
    assert_eq!(app.view.composer.text, "Do not discard this");
    assert_eq!(app.view.attachment_alt, "Keep this too");
    assert!(app.view.drafts.is_empty());
}

#[tokio::test]
async fn navigation_filters_hidden_files_and_editable_locations_work() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("album");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(root.path().join("photo.png"), b"x").unwrap();
    std::fs::write(root.path().join("movie.mp4"), b"x").unwrap();
    std::fs::write(root.path().join(".hidden.png"), b"x").unwrap();
    std::fs::write(folder.join("song.mp3"), b"x").unwrap();
    let mut app = demo(root.path());
    open(&mut app, root.path()).await;
    assert_eq!(app.view.file_browser.entries[0].kind, FileKind::Directory);
    key(&mut app, KeyCode::Enter);
    loaded(&mut app).await;
    assert_eq!(
        app.view.file_browser.directory,
        folder.canonicalize().unwrap()
    );
    key(&mut app, KeyCode::Backspace);
    loaded(&mut app).await;
    key(&mut app, KeyCode::Char('f'));
    loaded(&mut app).await;
    assert!(
        app.view
            .file_browser
            .entries
            .iter()
            .all(|e| matches!(e.kind, FileKind::Directory | FileKind::Image))
    );
    assert!(
        !app.view
            .file_browser
            .entries
            .iter()
            .any(|e| e.name.starts_with('.'))
    );
    key(&mut app, KeyCode::Char('.'));
    loaded(&mut app).await;
    assert!(
        app.view
            .file_browser
            .entries
            .iter()
            .any(|e| e.name == ".hidden.png")
    );
    shortcut(&mut app, 'l', KeyModifiers::CONTROL);
    shortcut(&mut app, 'u', KeyModifiers::CONTROL);
    app.event(Event::Paste(folder.to_string_lossy().into_owned()), 140);
    key(&mut app, KeyCode::Enter);
    loaded(&mut app).await;
    assert_eq!(
        app.view.file_browser.directory,
        folder.canonicalize().unwrap()
    );
    assert!(
        app.view.file_browser.entries.is_empty(),
        "image filter excludes audio"
    );
}

#[tokio::test]
async fn mouse_selection_preview_and_choose_keep_the_browser_context() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("a.png");
    let video = root.path().join("b.mp4");
    std::fs::write(&first, b"x").unwrap();
    std::fs::write(&video, b"x").unwrap();
    let mut app = demo(root.path());
    open(&mut app, root.path()).await;
    click(&mut app, |hit| matches!(hit, Hit::FileSelect(1)));
    assert!(matches!(app.view.modal, Modal::FileBrowser));
    assert_eq!(app.view.file_browser.selected, 1);
    click(
        &mut app,
        |hit| matches!(hit, Hit::Command(id) if id == "browse_preview"),
    );
    assert_eq!(app.local_media_requested.take(), Some(video.clone()));
    assert!(matches!(app.view.modal, Modal::FileBrowser));
    click(
        &mut app,
        |hit| matches!(hit, Hit::Command(id) if id == "browse_open"),
    );
    assert!(matches!(app.view.modal, Modal::Compose));
    assert_eq!(app.view.attachment_path, video.to_str().unwrap());
}

#[tokio::test]
async fn newer_scan_wins_and_invalid_location_has_a_recoverable_error() {
    let root = tempfile::tempdir().unwrap();
    let one = root.path().join("one");
    let two = root.path().join("two");
    std::fs::create_dir(&one).unwrap();
    std::fs::create_dir(&two).unwrap();
    for i in 0..30 {
        std::fs::write(two.join(format!("{i:02}.png")), b"x").unwrap();
    }
    let mut app = demo(root.path());
    app.command("compose");
    app.view.file_browser.directory = one;
    app.command("browse");
    app.view.file_browser.location = two.to_string_lossy().into_owned();
    app.command("browse_go");
    loaded(&mut app).await;
    assert_eq!(app.view.file_browser.directory, two.canonicalize().unwrap());
    assert_eq!(app.view.file_browser.entries.len(), 30);
    key(&mut app, KeyCode::End);
    assert_eq!(app.view.file_browser.selected, 29);
    key(&mut app, KeyCode::PageUp);
    assert_eq!(app.view.file_browser.selected, 19);
    key(&mut app, KeyCode::Home);
    assert_eq!(app.view.file_browser.selected, 0);
    key(&mut app, KeyCode::PageDown);
    assert_eq!(app.view.file_browser.selected, 10);
    app.view.file_browser.location = root.path().join("not-here").to_string_lossy().into_owned();
    app.command("browse_go");
    loaded(&mut app).await;
    assert!(app.view.file_browser.error.is_some());
    assert!(matches!(app.view.modal, Modal::FileBrowser));
    key(&mut app, KeyCode::Backspace);
    loaded(&mut app).await;
    assert!(app.view.file_browser.error.is_none());
}

#[tokio::test]
async fn exact_whitespace_paths_and_last_attachment_preview_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("photo.png ");
    std::fs::write(&file, b"x").unwrap();
    let mut app = demo(root.path());
    app.command("compose");
    app.view.attachment_path = file.to_string_lossy().into_owned();
    app.command("preview_attachment");
    assert_eq!(app.local_media_requested.take(), Some(file.clone()));
    app.command("attach");
    assert_eq!(
        app.view.composer.attachments[0].path,
        file.to_str().unwrap()
    );
    assert!(app.view.attachment_path.is_empty());
    app.command("preview_attachment");
    assert_eq!(app.local_media_requested.take(), Some(file));
}

#[tokio::test]
async fn selection_errors_keep_other_files_visible_and_recoverable() {
    let root = tempfile::tempdir().unwrap();
    let removed = root.path().join("a.png");
    std::fs::write(&removed, b"x").unwrap();
    std::fs::write(root.path().join("b.png"), b"x").unwrap();
    let mut app = demo(root.path());
    open(&mut app, root.path()).await;
    std::fs::remove_file(removed).unwrap();
    key(&mut app, KeyCode::Enter);
    assert!(app.view.file_browser.error.is_some());
    let mut terminal = Terminal::new(TestBackend::new(140, 45)).unwrap();
    terminal
        .draw(|frame| ui::render(frame, &mut app.view))
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("b.png"));
    assert!(screen.contains("no longer available"));
    key(&mut app, KeyCode::Down);
    assert!(app.view.file_browser.error.is_none());
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.view.modal, Modal::Compose));
}
