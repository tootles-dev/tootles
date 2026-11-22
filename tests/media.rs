use std::io::Cursor;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use image::{DynamicImage, ImageFormat};
use ratatui::{Terminal, backend::TestBackend};
use ratatui_image::picker::Picker;
use tootles::{
    media::{Gallery, MAX_DOWNLOAD_BYTES, decode_image, fetch_image},
    model::{Media, Post},
};

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::new_rgb8(width, height)
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
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
