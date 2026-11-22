//! Explicit, bounded attachment viewing. Media is fetched only after the user
//! opens the gallery; decoding and terminal encoding never run on the UI thread.
use std::{
    io::Cursor,
    sync::{Arc, mpsc},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use image::{DynamicImage, ImageReader, Limits};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect, Size},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};
use ratatui_image::{Image, Resize, picker::Picker, protocol::Protocol};
use tokio::task::JoinHandle;
use url::Url;

use crate::{
    auth,
    model::{Media, Post},
};

pub const MAX_DOWNLOAD_BYTES: usize = 16 * 1024 * 1024;
const MAX_PIXELS: u64 = 40_000_000;
const MAX_DIMENSION: u32 = 16_384;
const MAX_PREVIEW_DIMENSION: u32 = 2_048;
const MAX_ATTACHMENTS: usize = 4;

struct Preview {
    image: Arc<DynamicImage>,
    protocol: Protocol,
    size: Size,
}

enum Cached {
    Empty,
    Ready(Preview),
    Failed(String),
}

struct Completed {
    index: usize,
    result: Result<Preview>,
}

struct Pending {
    index: usize,
    receive: mpsc::Receiver<Completed>,
    task: JoinHandle<()>,
}

/// A modal attachment gallery. Call `tick` from the existing event loop, then
/// `render`; `event` returns true when the user wants to close the gallery.
pub struct Gallery {
    attachments: Vec<Media>,
    selected: usize,
    author: String,
    picker: Picker,
    cache: Vec<Cached>,
    pending: Option<Pending>,
    size: Size,
    bounds: Rect,
    buttons: [Rect; 3],
    notice: Option<String>,
}

impl Gallery {
    pub fn new(post: Post, picker: Picker) -> Self {
        let attachments: Vec<_> = post.media.into_iter().take(MAX_ATTACHMENTS).collect();
        let cache = (0..attachments.len()).map(|_| Cached::Empty).collect();
        Self {
            attachments,
            selected: 0,
            author: display_text(post.account.name()),
            picker,
            cache,
            pending: None,
            size: Size::new(70, 22),
            bounds: Rect::default(),
            buttons: [Rect::default(); 3],
            notice: None,
        }
    }

    pub fn event(&mut self, event: &Event) -> bool {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => return true,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
                KeyCode::Left | KeyCode::Char('h') => self.navigate(false),
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => self.navigate(true),
                KeyCode::Enter | KeyCode::Char('o') => self.open_external(),
                KeyCode::Char('R') => {
                    if let Some(state) = self.cache.get_mut(self.selected) {
                        *state = Cached::Empty;
                    }
                    self.notice = None;
                }
                _ => {}
            },
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    let point = (mouse.column, mouse.row).into();
                    if self.buttons[0].contains(point) {
                        self.navigate(
                            mouse.column >= self.buttons[0].x + self.buttons[0].width / 2,
                        );
                    } else if self.buttons[1].contains(point) {
                        self.open_external();
                    } else if self.buttons[2].contains(point) {
                        return true;
                    } else if self.bounds.contains(point)
                        && mouse.column > self.bounds.x + self.bounds.width / 2
                    {
                        self.navigate(true);
                    } else if self.bounds.contains(point) {
                        self.navigate(false);
                    }
                }
                MouseEventKind::ScrollDown | MouseEventKind::ScrollRight => self.navigate(true),
                MouseEventKind::ScrollUp | MouseEventKind::ScrollLeft => self.navigate(false),
                _ => {}
            },
            _ => {}
        }
        false
    }

    fn navigate(&mut self, forward: bool) {
        if !self.attachments.is_empty() {
            self.selected = if forward {
                (self.selected + 1) % self.attachments.len()
            } else {
                (self.selected + self.attachments.len() - 1) % self.attachments.len()
            };
            self.notice = None;
        }
    }

    fn open_external(&mut self) {
        if let Some(media) = self.attachments.get(self.selected) {
            if media.url.is_empty() {
                self.notice = Some("In-memory demo attachment; the file is not uploaded".into());
                return;
            }
            if media.url.starts_with("demo://") {
                self.notice =
                    Some("This demo image is bundled locally and has no browser link".into());
                return;
            }
            self.notice = Some(match auth::open_url(&media.url) {
                Ok(()) => "Opened in your default browser".into(),
                Err(error) => format!("Could not open attachment: {error}"),
            });
        }
    }

    pub fn tick(&mut self) {
        if let Some(pending) = &self.pending {
            match pending.receive.try_recv() {
                Ok(completed) => {
                    if let Some(state) = self.cache.get_mut(completed.index) {
                        *state = match completed.result {
                            Ok(preview) => Cached::Ready(preview),
                            Err(error) => Cached::Failed(error.to_string()),
                        };
                    }
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.cache[pending.index] =
                        Cached::Failed("The image worker stopped unexpectedly".into());
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Empty) => return,
            }
        }
        let Some(media) = self.attachments.get(self.selected) else {
            return;
        };
        if media.url.is_empty() {
            self.cache[self.selected] =
                Cached::Failed("In-memory demo attachment; the file is not uploaded".into());
            return;
        }
        if media.kind != "image" || self.size.width == 0 || self.size.height == 0 {
            return;
        }
        let existing = match &self.cache[self.selected] {
            Cached::Ready(preview) if preview.size != self.size => Some(Arc::clone(&preview.image)),
            Cached::Empty => None,
            _ => return,
        };
        let index = self.selected;
        let size = self.size;
        let picker = self.picker.clone();
        let url = media.url.clone();
        let (send, receive) = mpsc::channel();
        let task = tokio::spawn(async move {
            let result = async {
                let image = match existing {
                    Some(image) => image,
                    None => Arc::new(fetch_image(&url).await?),
                };
                tokio::task::spawn_blocking(move || {
                    let protocol = picker
                        .new_protocol((*image).clone(), size, Resize::Fit(None))
                        .context("Could not prepare this image for the terminal")?;
                    Ok(Preview {
                        image,
                        protocol,
                        size,
                    })
                })
                .await
                .context("Image rendering worker failed")?
            }
            .await;
            let _ = send.send(Completed { index, result });
        });
        self.pending = Some(Pending {
            index,
            receive,
            task,
        });
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let bg = Color::Rgb(26, 27, 38);
        let fg = Color::Rgb(192, 202, 245);
        let muted = Color::Rgb(128, 144, 180);
        let accent = Color::Rgb(122, 162, 247);
        let area = frame.area();
        frame.render_widget(Clear, area);
        frame.render_widget(Block::default().style(Style::default().bg(bg).fg(fg)), area);
        let margin_x = if area.width > 30 { 3 } else { 0 };
        let margin_y = if area.height > 12 { 1 } else { 0 };
        let outer = area.inner(ratatui::layout::Margin::new(margin_x, margin_y));
        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(accent))
                .title(" ATTACHMENTS "),
            outer,
        );
        let inner = outer.inner(ratatui::layout::Margin::new(2, 1));
        let rows = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(3),
            Constraint::Length(2),
        ])
        .split(inner);
        self.bounds = rows[1];
        self.size = Size::new(rows[1].width, rows[1].height);
        let Some(media) = self.attachments.get(self.selected) else {
            frame.render_widget(
                Paragraph::new("This post has no attachments. Press Esc to return.")
                    .wrap(Wrap { trim: true }),
                inner,
            );
            return;
        };
        let caption = format!(
            "{}  ·  {} / {}  ·  {}",
            self.author,
            self.selected + 1,
            self.attachments.len(),
            media.kind
        );
        frame.render_widget(
            Paragraph::new(caption).style(Style::default().fg(accent).add_modifier(Modifier::BOLD)),
            rows[0],
        );
        if media.kind == "image" {
            match &self.cache[self.selected] {
                Cached::Ready(preview) if preview.size == self.size => {
                    frame.render_widget(Image::new(&preview.protocol), rows[1]);
                }
                Cached::Failed(error) => {
                    frame.render_widget(Paragraph::new(format!("Image preview unavailable\n\n{error}\n\nPress R to retry or Enter to open in your browser."))
                        .style(Style::default().fg(muted)).alignment(Alignment::Center).wrap(Wrap { trim: true }), rows[1]);
                }
                _ => {
                    frame.render_widget(
                        Paragraph::new("Preparing image…\n\nEsc returns to your timeline")
                            .style(Style::default().fg(muted))
                            .alignment(Alignment::Center),
                        rows[1],
                    );
                }
            }
        } else {
            let label = match media.kind.as_str() {
                "video" | "gifv" => "VIDEO",
                "audio" => "AUDIO",
                _ => "ATTACHMENT",
            };
            let preview = format!(
                "\n\n▶  {label}\n\nPress Enter to play in your browser.\nPlayback uses your browser's native controls."
            );
            frame.render_widget(
                Paragraph::new(preview)
                    .style(Style::default().fg(accent))
                    .alignment(Alignment::Center)
                    .wrap(Wrap { trim: true }),
                rows[1],
            );
        }
        let alt = if media.description.is_empty() {
            "No image description supplied".to_owned()
        } else {
            display_text(&media.description)
        };
        frame.render_widget(
            Paragraph::new(self.notice.as_deref().unwrap_or(&alt))
                .style(Style::default().fg(muted))
                .wrap(Wrap { trim: true }),
            rows[2],
        );
        let button_areas = Layout::horizontal([
            Constraint::Percentage(34),
            Constraint::Percentage(40),
            Constraint::Percentage(26),
        ])
        .split(rows[3]);
        self.buttons = [button_areas[0], button_areas[1], button_areas[2]];
        for (area, label) in self.buttons.into_iter().zip([
            "← h / l →  Navigate",
            "Enter  Open / play",
            "Esc  Close",
        ]) {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(label, Style::default().fg(accent)))),
                area,
            );
        }
    }
}

impl Drop for Gallery {
    fn drop(&mut self) {
        if let Some(pending) = &self.pending {
            pending.task.abort();
        }
    }
}

/// Download a public image without account credentials, with both declared and
/// actual byte limits. GIF images use the first frame; gifv/video stays external.
pub async fn fetch_image(value: &str) -> Result<DynamicImage> {
    let demo: Option<&'static [u8]> = match value {
        "demo://desktop" => Some(include_bytes!("../assets/demo-desktop.png")),
        "demo://trail" => Some(include_bytes!("../assets/demo-trail.png")),
        _ => None,
    };
    if let Some(bytes) = demo {
        return tokio::task::spawn_blocking(move || decode_image(bytes))
            .await
            .context("Demo image decoding worker failed")?;
    }
    let url = media_url(value)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(25))
        .connect_timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > 3 || media_url(attempt.url().as_str()).is_err() {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .user_agent(concat!("tootles/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let mut response = client
        .get(url)
        .send()
        .await
        .context("Could not download image")?;
    if !response.status().is_success() {
        bail!("Image server returned HTTP {}", response.status());
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_DOWNLOAD_BYTES as u64)
    {
        bail!("Image exceeds the 16 MiB preview limit; open it in your browser");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("Image download was interrupted")?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_DOWNLOAD_BYTES {
            bail!("Image exceeds the 16 MiB preview limit; open it in your browser");
        }
        bytes.extend_from_slice(&chunk);
    }
    tokio::task::spawn_blocking(move || decode_image(&bytes))
        .await
        .context("Image decoding worker failed")?
}

/// Decode a bounded preview. Public for deterministic tests and future adapters.
pub fn decode_image(bytes: &[u8]) -> Result<DynamicImage> {
    if bytes.len() > MAX_DOWNLOAD_BYTES {
        bail!("Image exceeds the 16 MiB preview limit");
    }
    let probe = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("Unrecognized image format")?;
    let (width, height) = probe
        .into_dimensions()
        .context("Could not read image dimensions")?;
    if u64::from(width) * u64::from(height) > MAX_PIXELS
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
    {
        bail!("Image exceeds the 40 megapixel preview limit");
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(256 * 1024 * 1024);
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(limits);
    let image = reader.decode().context("Could not decode image")?;
    Ok(
        if image.width() > MAX_PREVIEW_DIMENSION || image.height() > MAX_PREVIEW_DIMENSION {
            image.thumbnail(MAX_PREVIEW_DIMENSION, MAX_PREVIEW_DIMENSION)
        } else {
            image
        },
    )
}

fn media_url(value: &str) -> Result<Url> {
    if value.chars().any(char::is_control) {
        bail!("Invalid image URL");
    }
    let url = Url::parse(value).context("Invalid image URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("Images must use an HTTP or HTTPS URL without embedded credentials");
    }
    Ok(url)
}

fn display_text(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control() || *character == '\n')
        .collect()
}
