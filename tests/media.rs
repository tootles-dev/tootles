use std::io::Cursor;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use image::{DynamicImage, ImageFormat};
use ratatui::{Terminal, backend::TestBackend};
use ratatui_image::picker::Picker;
use tootles::{
    media::{Gallery, MAX_DOWNLOAD_BYTES, decode_image, fetch_image, load_file},
    model::{Media, Post},
};

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::new_rgb8(width, height)
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

#[tokio::test]
async fn local_preview_reads_selected_images_with_download_and_decode_budgets() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("a local photo.png");
    std::fs::write(&path, png(30, 20)).unwrap();
    let image = load_file(&path).await.unwrap();
    assert_eq!((image.width(), image.height()), (30, 20));
    assert!(load_file(directory.path()).await.is_err());
    assert!(
        load_file(&directory.path().join("missing.png"))
            .await
            .is_err()
    );
    std::fs::write(&path, b"not an image").unwrap();
    assert!(load_file(&path).await.is_err());
    std::fs::File::create(&path)
        .unwrap()
        .set_len((MAX_DOWNLOAD_BYTES + 1) as u64)
        .unwrap();
    assert!(
        load_file(&path)
            .await
            .unwrap_err()
            .to_string()
            .contains("16 MiB")
    );
}

fn screen(gallery: &mut Gallery) -> String {
    let mut terminal = Terminal::new(TestBackend::new(100, 32)).unwrap();
    terminal.draw(|frame| gallery.render(frame)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

fn press(gallery: &mut Gallery, code: KeyCode) {
    gallery.event(&Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

#[tokio::test]
async fn local_image_gallery_renders_inline_and_returns_without_uploading() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("photo.png");
    std::fs::write(&path, png(30, 20)).unwrap();
    let mut gallery = Gallery::from_file(path, Picker::halfblocks());
    assert!(screen(&mut gallery).contains("LOCAL MEDIA"));
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        gallery.tick();
        let text = screen(&mut gallery);
        if !text.contains("Preparing image") {
            assert!(!text.contains("preview unavailable"));
            assert!(text.contains("This file has not been uploaded"));
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(gallery.take_terminal_request().is_none());
    assert!(screen(&mut gallery).contains("Block preview"));
    press(&mut gallery, KeyCode::Char('v'));
    assert!(screen(&mut gallery).contains("Default image viewer"));
    // Merely selecting a mode must not open an application or read the original.
    assert!(!screen(&mut gallery).contains("Preparing original"));
    assert!(gallery.event(&Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))));
}

#[tokio::test]
async fn image_view_modes_are_explicit_and_canceled_requests_do_not_launch() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("photo.png");
    std::fs::write(&path, png(30, 20)).unwrap();
    let mut gallery = Gallery::from_file(path, Picker::halfblocks())
        .with_config_root(Some(directory.path().join("config")));
    press(&mut gallery, KeyCode::Char('v'));
    assert!(screen(&mut gallery).contains("Default image viewer"));
    gallery.tick();
    assert!(!directory.path().join("config").exists());
    press(&mut gallery, KeyCode::Enter);
    assert!(screen(&mut gallery).contains("Preparing original"));
    // Cancel synchronously before the async preparation worker can run.
    press(&mut gallery, KeyCode::Char('v'));
    assert!(screen(&mut gallery).contains("Block preview"));
    tokio::task::yield_now().await;
    gallery.tick();
    assert!(!directory.path().join("config").exists());
    assert!(!screen(&mut gallery).contains("Opened the original"));
    press(&mut gallery, KeyCode::Char('o'));
    assert!(screen(&mut gallery).contains("Preparing original"));
    drop(gallery);
    tokio::task::yield_now().await;
    assert!(!directory.path().join("config").exists());
}

#[test]
fn remote_image_gallery_cycles_all_view_modes_with_mouse_controls() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut gallery = Gallery::new(
        Post {
            media: vec![Media {
                kind: "image".into(),
                url: "https://example.org/photo.png".into(),
                ..Media::default()
            }],
            ..Post::default()
        },
        Picker::halfblocks(),
    );
    let mut terminal = Terminal::new(TestBackend::new(100, 32)).unwrap();
    terminal.draw(|frame| gallery.render(frame)).unwrap();
    let buffer = terminal.backend().buffer();
    let index = buffer
        .content()
        .iter()
        .rposition(|cell| cell.symbol() == "v")
        .unwrap();
    gallery.event(&Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: (index % 100) as u16,
        row: (index / 100) as u16,
        modifiers: KeyModifiers::NONE,
    }));
    assert!(screen(&mut gallery).contains("Default image viewer ·"));
    press(&mut gallery, KeyCode::Char('v'));
    assert!(screen(&mut gallery).contains("Browser · Enter"));
    press(&mut gallery, KeyCode::Char('v'));
    assert!(screen(&mut gallery).contains("Block preview"));
}

#[test]
fn video_gallery_requires_explicit_play_and_hands_terminal_requests_to_main() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("video.mp4");
    std::fs::write(&path, b"fake local clip").unwrap();
    let mut gallery = Gallery::from_file(path, Picker::halfblocks());
    assert!(screen(&mut gallery).contains("Window mode"));
    assert!(gallery.take_terminal_request().is_none());
    press(&mut gallery, KeyCode::Char('v'));
    assert!(screen(&mut gallery).contains("Terminal mode"));
    assert!(gallery.take_terminal_request().is_none());
    press(&mut gallery, KeyCode::Enter);
    assert!(gallery.take_terminal_request().is_some());
    assert!(gallery.take_terminal_request().is_none());
    gallery.playback_finished(Err(anyhow::anyhow!("mpv is unavailable")));
    assert!(screen(&mut gallery).contains("mpv is unavailable"));
    press(&mut gallery, KeyCode::Char('v'));
    press(&mut gallery, KeyCode::Enter);
    assert!(screen(&mut gallery).contains("Local files play in Window or Terminal mode"));
}

#[test]
fn remote_video_cannot_access_local_files_or_embedded_credentials() {
    for url in [
        "file:///tmp/movie.mp4",
        "https://name:secret@example.com/movie.mp4",
    ] {
        let mut gallery = Gallery::new(
            Post {
                media: vec![Media {
                    kind: "video".into(),
                    url: url.into(),
                    ..Media::default()
                }],
                ..Post::default()
            },
            Picker::halfblocks(),
        );
        press(&mut gallery, KeyCode::Char('v'));
        press(&mut gallery, KeyCode::Enter);
        assert!(gallery.take_terminal_request().is_none());
        assert!(screen(&mut gallery).contains("HTTP or HTTPS URL"));
    }
}

#[test]
fn image_decode_limits_preview_size_and_preserves_aspect_ratio() {
    let decoded = decode_image(&png(3000, 100)).unwrap();
    assert_eq!(decoded.width(), 2048);
    assert!(decoded.height() <= 69 && decoded.height() >= 67);
    assert!(decode_image(b"not an image").is_err());
    assert!(decode_image(&vec![0; MAX_DOWNLOAD_BYTES + 1]).is_err());
}

#[test]
fn images_with_extreme_dimensions_are_rejected_before_full_decode() {
    let bytes = png(16_385, 1);
    assert!(
        decode_image(&bytes)
            .unwrap_err()
            .to_string()
            .contains("preview limit")
    );
}

#[tokio::test]
async fn bundled_demo_images_load_without_network_access() {
    for url in ["demo://desktop", "demo://trail"] {
        let image = fetch_image(url).await.unwrap();
        assert!(image.width() > 0 && image.height() > 0);
    }
    assert!(fetch_image("demo://unknown").await.is_err());
}

#[tokio::test]
async fn image_fetch_does_not_send_account_credentials() {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
    let server = MockServer::start().await;
    Mock::given(path("/photo.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(png(3, 2)))
        .expect(1)
        .mount(&server)
        .await;
    let image = fetch_image(&format!("{}/photo.png", server.uri()))
        .await
        .unwrap();
    assert_eq!((image.width(), image.height()), (3, 2));
    let requests = server.received_requests().await.unwrap();
    assert!(!requests[0].headers.contains_key("authorization"));
}

#[tokio::test]
async fn oversized_and_invalid_media_responses_fail_cleanly() {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
    let server = MockServer::start().await;
    Mock::given(path("/too-big"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0; MAX_DOWNLOAD_BYTES + 1]))
        .mount(&server)
        .await;
    let error = fetch_image(&format!("{}/too-big", server.uri()))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("16 MiB"));
    for url in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "https://name:secret@example.com/image",
        "https://example.com/\n",
    ] {
        assert!(fetch_image(url).await.is_err());
    }
}

#[tokio::test]
async fn chunked_images_are_limited_even_without_a_content_length() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let byte = stream.read_u8().await.unwrap();
            request.push(byte);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let chunk = vec![0_u8; 64 * 1024];
        for _ in 0..(MAX_DOWNLOAD_BYTES / chunk.len() + 1) {
            if stream.write_all(b"10000\r\n").await.is_err()
                || stream.write_all(&chunk).await.is_err()
                || stream.write_all(b"\r\n").await.is_err()
            {
                return;
            }
        }
        let _ = stream.write_all(b"0\r\n\r\n").await;
    });
    let result = fetch_image(&format!("http://{address}/chunked")).await;
    server.abort();
    let error = result.unwrap_err();
    assert!(error.to_string().contains("16 MiB"), "{error:#}");
}

#[test]
fn gallery_handles_empty_posts_small_terminals_and_keyboard_navigation() {
    let mut empty = Gallery::new(Post::default(), Picker::halfblocks());
    for (width, height) in [(1, 1), (20, 6), (80, 24)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| empty.render(frame)).unwrap();
    }
    assert!(empty.event(&Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))));
    let post = Post {
        media: vec![
            Media {
                kind: "video".into(),
                description: "first video".into(),
                ..Media::default()
            },
            Media {
                kind: "audio".into(),
                description: "second audio".into(),
                ..Media::default()
            },
        ],
        ..Post::default()
    };
    let mut gallery = Gallery::new(post, Picker::halfblocks());
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    gallery.event(&Event::Key(KeyEvent::new(
        KeyCode::Right,
        KeyModifiers::NONE,
    )));
    terminal.draw(|frame| gallery.render(frame)).unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("second audio"));
    assert!(screen.contains("2 / 2"));
    gallery.event(&Event::Key(KeyEvent::new(
        KeyCode::Left,
        KeyModifiers::NONE,
    )));
    terminal.draw(|frame| gallery.render(frame)).unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("first video"));
}

/// Wait until the background worker has stored the encoded original, then
/// remove it before ticking the gallery. This makes cancellation regressions
/// fail with a local missing-file error instead of launching a desktop viewer.
async fn remove_prepared_original(root: &std::path::Path, expected: &[u8]) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if let Ok(entries) = std::fs::read_dir(root.join("image-previews")) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("image-")
                    && std::fs::read(&path).is_ok_and(|bytes| bytes == expected)
                    && std::fs::remove_file(&path).is_ok()
                {
                    // Let an already completed worker enqueue its result. The
                    // gallery intentionally has not ticked yet, so no opener
                    // can run while the valid stored file exists.
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                    return;
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "original was not prepared"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn prepared_native_image_is_canceled_when_navigating_or_changing_mode() {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
    let server = MockServer::start().await;
    let bytes = png(4, 3);
    Mock::given(path("/first.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.clone()))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path("/second.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.clone()))
        .expect(0)
        .mount(&server)
        .await;
    for cancel in [KeyCode::Right, KeyCode::Char('v')] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("private");
        let mut gallery = Gallery::new(
            Post {
                media: vec![
                    Media {
                        kind: "image".into(),
                        url: format!("{}/first.png", server.uri()),
                        description: "first original".into(),
                        ..Media::default()
                    },
                    Media {
                        kind: "image".into(),
                        url: format!("{}/second.png", server.uri()),
                        description: "second original".into(),
                        ..Media::default()
                    },
                ],
                ..Post::default()
            },
            Picker::halfblocks(),
        )
        .with_config_root(Some(root.clone()));
        press(&mut gallery, KeyCode::Char('v'));
        press(&mut gallery, KeyCode::Enter);
        remove_prepared_original(&root, &bytes).await;
        press(&mut gallery, cancel);
        for _ in 0..4 {
            gallery.tick();
            tokio::task::yield_now().await;
        }
        let text = screen(&mut gallery);
        assert!(!text.contains("Opened the original"));
        assert!(!text.contains("Could not open image"));
        assert!(!text.contains("Preparing original"));
        if cancel == KeyCode::Right {
            assert!(text.contains("second original"));
            assert!(text.contains("2 / 2"));
        } else {
            assert!(text.contains("Browser · Enter"));
        }
    }
}

#[tokio::test]
async fn native_prepare_deduplicates_pending_enter_and_allows_retry_after_failure() {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
    let server = MockServer::start().await;
    Mock::given(path("/invalid.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(b"Not an image".to_vec())
                .set_delay(std::time::Duration::from_millis(40)),
        )
        .expect(2)
        .mount(&server)
        .await;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("private");
    let mut gallery = Gallery::new(
        Post {
            media: vec![Media {
                kind: "image".into(),
                url: format!("{}/invalid.png", server.uri()),
                ..Media::default()
            }],
            ..Post::default()
        },
        Picker::halfblocks(),
    )
    .with_config_root(Some(root.clone()));
    press(&mut gallery, KeyCode::Char('v'));
    for attempt in 1..=2 {
        press(&mut gallery, KeyCode::Enter);
        press(&mut gallery, KeyCode::Enter);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            gallery.tick();
            if screen(&mut gallery).contains("Could not open image") {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(server.received_requests().await.unwrap().len(), attempt);
        assert!(!root.join("image-previews").exists());
    }
}

#[test]
fn populated_image_modes_render_at_tiny_sizes_without_starting_a_viewer() {
    let mut gallery = Gallery::new(
        Post {
            media: vec![Media {
                kind: "image".into(),
                url: "demo://desktop".into(),
                description: "A very small terminal can still close the gallery".into(),
                ..Media::default()
            }],
            ..Post::default()
        },
        Picker::halfblocks(),
    );
    for _mode in 0..3 {
        for (width, height) in [(1, 1), (2, 2), (20, 6), (40, 12), (100, 32)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| gallery.render(frame)).unwrap();
        }
        press(&mut gallery, KeyCode::Char('v'));
    }
    assert!(screen(&mut gallery).contains("Block preview"));
    assert!(gallery.take_terminal_request().is_none());
}
