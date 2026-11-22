//! Render the actual interface to reviewable, offline SVG previews.
//!
//! Run `cargo run --example render_previews -- docs` from the repository root.
//! This uses temporary configuration and bundled demo media; it never connects
//! to an account, publishes a post, or writes to the user's configuration.
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use ratatui::{Terminal, backend::TestBackend};
use ratatui_image::picker::Picker;
use tootles::{
    app::App,
    config::Config,
    media::Gallery,
    model::{Attachment, Feed, Post},
    provider::{SocialProvider, demo::DemoProvider},
    snapshot, ui,
    view::{Modal, Theme},
};

const WIDTH: u16 = 150;
const HEIGHT: u16 = 48;

#[tokio::main]
async fn main() -> Result<()> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("docs"));
    std::fs::create_dir_all(&output).context("Could not create the preview output directory")?;
    let config = tempfile::tempdir().context("Could not isolate preview configuration")?;
    let mut app = App::new(
        Config::default(),
        Some(config.path().join("private-config")),
        true,
    )?;
    app.prepare_snapshot().await?;
    app.view.theme = Theme::TokyoNight;
    app.view.reduced_motion = true;
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT))?;

    save_ui(&mut terminal, &mut app, &output.join("home.svg"))?;

    app.command("compose");
    app.view.composer.text = "A little field note from the open web.\n\nSpent the morning making software feel a bit more human. Fewer distractions. Better conversations.\n\nWhat makes a tool feel like home to you?".into();
    app.view.composer.language = "en".into();
    app.view.composer.attachments.push(Attachment {
        path: "assets/demo-trail.png".into(),
        description:
            "A golden trail winds through green foothills beneath blue mountains and an amber moon."
                .into(),
    });
    app.view.poll_input = String::new();
    app.view.compose_field = 0;
    app.view.compose_cursor = app.view.composer.text.len();
    save_ui(&mut terminal, &mut app, &output.join("compose.svg"))?;

    app.view.modal = Modal::Palette {
        query: "post".into(),
        selected: 1,
    };
    app.view.input_cursor = "post".len();
    save_ui(&mut terminal, &mut app, &output.join("palette.svg"))?;

    app.view.modal = Modal::None;
    app.view.feed = Feed::Notifications;
    app.view.offset = 0;
    app.view.detail_scroll = 0;
    app.prepare_snapshot().await?;
    app.view.selected = app
        .view
        .notifications
        .iter()
        .position(|note| note.post.is_some())
        .unwrap_or(0);
    save_ui(&mut terminal, &mut app, &output.join("notifications.svg"))?;

    let post = bundled_image_post().await?;
    let mut gallery = Gallery::new(post, Picker::halfblocks());
    // Initial render establishes the exact pixel-to-cell target before loading.
    terminal.draw(|frame| gallery.render(frame))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        gallery.tick();
        terminal.draw(|frame| gallery.render(frame))?;
        let buffer = terminal.backend().buffer();
        if buffer
            .content()
            .iter()
            .any(|cell| matches!(cell.symbol(), "▀" | "▄" | "█"))
        {
            snapshot::save(buffer, &output.join("media.svg"))?;
            break;
        }
        let text: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        if text.contains("Image preview unavailable") {
            bail!("Bundled demo image failed to render: {text}");
        }
        if Instant::now() >= deadline {
            bail!("Bundled demo image did not render within two seconds");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    println!(
        "Rendered home, compose, palette, notifications, and media previews in {}",
        output.display()
    );
    Ok(())
}

fn save_ui(terminal: &mut Terminal<TestBackend>, app: &mut App, path: &Path) -> Result<()> {
    terminal.draw(|frame| ui::render(frame, &mut app.view))?;
    snapshot::save(terminal.backend().buffer(), path)
}

async fn bundled_image_post() -> Result<Post> {
    let provider = DemoProvider::new();
    let mut cursor = None;
    loop {
        let page = provider.feed(&Feed::Federated, cursor.as_deref()).await?;
        if let Some(post) = page
            .items
            .into_iter()
            .find(|post| post.media.iter().any(|media| media.url == "demo://trail"))
        {
            return Ok(post);
        }
        cursor = page.next;
        if cursor.is_none() {
            bail!("The demo timeline has no bundled trail image");
        }
    }
}
