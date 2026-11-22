//! Explicit, bounded attachment viewing. Media is fetched only after the user
//! opens the gallery; decoding and terminal encoding never run on the UI thread.
use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
};

use anyhow::{Context, Result};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use image::DynamicImage;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect, Size},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};
use ratatui_image::{
    Image, Resize,
    picker::{Picker, ProtocolType},
    protocol::Protocol,
};
use tokio::task::JoinHandle;

use crate::{
    auth, config,
    file_picker::{FileKind, media_kind},
    image_viewer, media_source,
    model::{Media, Post},
    player::{PlaybackMode, Player, Source},
};

pub use media_source::MAX_DOWNLOAD_BYTES;
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

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum ImageMode {
    #[default]
    Terminal,
    DefaultViewer,
    Browser,
}

impl ImageMode {
    fn label(self) -> &'static str {
        match self {
            Self::Terminal => "Terminal",
            Self::DefaultViewer => "Default image viewer",
            Self::Browser => "Browser",
        }
    }
}

struct PendingOriginal {
    receive: mpsc::Receiver<Result<PathBuf>>,
    task: JoinHandle<()>,
}

impl Drop for PendingOriginal {
    fn drop(&mut self) {
        self.task.abort();
    }
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
    buttons: [Rect; 5],
    notice: Option<String>,
    local: Option<PathBuf>,
    mode: PlaybackMode,
    image_mode: ImageMode,
    config_root: Option<PathBuf>,
    original: Option<PendingOriginal>,
    viewer_launcher: Option<image_viewer::Launcher>,
    player: Option<Player>,
    terminal_requested: Option<Source>,
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
            buttons: [Rect::default(); 5],
            notice: None,
            local: None,
            mode: PlaybackMode::default(),
            image_mode: ImageMode::default(),
            config_root: None,
            original: None,
            viewer_launcher: None,
            player: None,
            terminal_requested: None,
        }
    }

    pub fn with_config_root(mut self, root: Option<PathBuf>) -> Self {
        self.config_root = root;
        self
    }

    /// Preview an explicitly chosen local file. Reading and decoding are deferred
    /// to a worker; remote attachment URLs never enter this path.
    pub fn from_file(path: PathBuf, picker: Picker) -> Self {
        let kind = match media_kind(&path) {
            Some(FileKind::Video) => "video",
            Some(FileKind::Audio) => "audio",
            _ => "image",
        };
        let mut gallery = Self::new(
            Post {
                media: vec![Media {
                    kind: kind.into(),
                    description: "Local preview. This file has not been uploaded.".into(),
                    ..Media::default()
                }],
                ..Post::default()
            },
            picker,
        );
        gallery.author = display_text(
            &path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy(),
        );
        gallery.local = Some(path);
        gallery
    }

    pub fn take_terminal_request(&mut self) -> Option<Source> {
        self.terminal_requested.take()
    }

    pub fn playback_finished(&mut self, result: Result<()>) {
        self.notice = Some(match result {
            Ok(()) => "Playback finished. Enter plays again; Esc returns.".into(),
            Err(error) => error.to_string(),
        });
        // Terminal players may remove terminal graphics. Re-encode an image if
        // the selection changed to one immediately after returning.
        for cached in &mut self.cache {
            if let Cached::Ready(preview) = cached {
                preview.size = Size::default();
            }
        }
    }

    pub fn event(&mut self, event: &Event) -> bool {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => return true,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
                KeyCode::Left | KeyCode::Char('h') => self.navigate(false),
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => self.navigate(true),
                KeyCode::Enter => self.play(),
                KeyCode::Char('o') => self.open_external(),
                KeyCode::Char('v') => self.cycle_mode(),
                KeyCode::Char('R') => self.reload(),
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
                        self.play();
                    } else if self.buttons[2].contains(point) {
                        self.cycle_mode();
                    } else if self.buttons[3].contains(point) {
                        self.open_external();
                    } else if self.buttons[4].contains(point) {
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
            self.player = None;
            self.original = None;
            self.selected = if forward {
                (self.selected + 1) % self.attachments.len()
            } else {
                (self.selected + self.attachments.len() - 1) % self.attachments.len()
            };
            self.notice = None;
        }
    }

    fn is_image(&self) -> bool {
        self.attachments
            .get(self.selected)
            .is_some_and(|m| m.kind == "image")
    }

    fn reload(&mut self) {
        if let Some(state) = self.cache.get_mut(self.selected) {
            *state = Cached::Empty;
        }
        self.notice = None;
    }

    fn cycle_mode(&mut self) {
        self.original = None;
        if self.is_image() {
            self.image_mode = match self.image_mode {
                ImageMode::Terminal => ImageMode::DefaultViewer,
                ImageMode::DefaultViewer if self.local.is_none() => ImageMode::Browser,
                _ => ImageMode::Terminal,
            };
            self.notice = Some(format!(
                "View: {}. Press Enter to open.",
                self.image_mode.label()
            ));
        } else {
            self.mode = self.mode.next();
            self.notice = Some(format!(
                "Playback: {}. Press Enter to play.",
                self.mode.label()
            ));
        }
    }

    fn play(&mut self) {
        if self.is_image() {
            match self.image_mode {
                ImageMode::Terminal => self.reload(),
                ImageMode::DefaultViewer => self.prepare_original(),
                ImageMode::Browser => self.open_external(),
            }
            return;
        }
        if self.mode == PlaybackMode::Browser {
            self.open_external();
            return;
        }
        let Some(media) = self.attachments.get(self.selected) else {
            return;
        };
        let source = if let Some(path) = &self.local {
            Source::local(path)
        } else if media.url.is_empty() {
            self.notice = Some("In-memory demo attachment; the file is not uploaded".into());
            return;
        } else {
            Source::remote(&media.url)
        };
        let source = match source {
            Ok(source) => source,
            Err(error) => {
                self.notice = Some(error.to_string());
                return;
            }
        };
        self.player = None;
        if self.mode == PlaybackMode::Terminal {
            self.terminal_requested = Some(source);
        } else {
            match Player::spawn(&source) {
                Ok(player) => {
                    self.player = Some(player);
                    self.notice = Some("Playing in mpv. Space pauses; arrows seek; q closes playback. Closing this gallery also stops playback.".into());
                }
                Err(error) => self.notice = Some(error.to_string()),
            }
        }
    }

    fn open_external(&mut self) {
        if self.local.is_some() {
            if self.is_image() {
                self.prepare_original();
            } else {
                self.notice = Some(
                    "Local files play in Window or Terminal mode. Press v to change modes.".into(),
                );
            }
            return;
        }
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

    fn prepare_original(&mut self) {
        if self.original.is_some() {
            return;
        }
        let Some(media) = self.attachments.get(self.selected) else {
            return;
        };
        let source = match &self.local {
            Some(path) => media_source::Source::Local(path.clone()),
            None => media_source::Source::Remote(media.url.clone()),
        };
        let cache_root = match config::config_dir(self.config_root.as_deref()) {
            Ok(root) => root.join("image-previews"),
            Err(error) => {
                self.notice = Some(error.to_string());
                return;
            }
        };
        let (send, receive) = mpsc::channel();
        let task = tokio::spawn(async move {
            let result = async {
                let original = media_source::load_original(source).await?;
                tokio::task::spawn_blocking(move || image_viewer::store(&original, &cache_root))
                    .await
                    .context("Image preparation worker failed")?
            }
            .await;
            let _ = send.send(result);
        });
        self.original = Some(PendingOriginal { receive, task });
        self.notice = Some("Preparing original for your default image viewer… Esc cancels.".into());
    }

    pub fn tick(&mut self) {
        if let Some(result) = self
            .viewer_launcher
            .as_mut()
            .and_then(image_viewer::Launcher::poll)
        {
            self.viewer_launcher = None;
            if let Err(error) = result {
                self.notice = Some(error.to_string());
            }
        }
        if let Some(original) = &self.original {
            let result = match original.receive.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(anyhow::anyhow!(
                    "Image preparation worker stopped unexpectedly"
                ))),
                Err(mpsc::TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                // Only the active gallery can launch a viewer. Closing, changing
                // modes, or navigating drops this request before it can launch.
                self.original = None;
                self.notice = Some(match result.and_then(|path| image_viewer::start(&path)) {
                    Ok(launcher) => {
                        self.viewer_launcher = Some(launcher);
                        "Opening the original in your default image viewer".into()
                    }
                    Err(error) => format!("Could not open image: {error}"),
                });
            }
        }
        if let Some(result) = self.player.as_mut().and_then(Player::poll) {
            self.player = None;
            self.playback_finished(result);
        }
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
        if media.url.is_empty() && self.local.is_none() {
            self.cache[self.selected] =
                Cached::Failed("In-memory demo attachment; the file is not uploaded".into());
            return;
        }
        if media.kind != "image"
            || self.image_mode != ImageMode::Terminal
            || self.size.width == 0
            || self.size.height == 0
        {
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
        let local = self.local.clone();
        let (send, receive) = mpsc::channel();
        let task = tokio::spawn(async move {
            let result = async {
                let image = match existing {
                    Some(image) => image,
                    None => Arc::new(match local {
                        Some(path) => load_file(&path).await?,
                        None => fetch_image(&url).await?,
                    }),
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
                .title(if self.local.is_some() {
                    " LOCAL MEDIA "
                } else {
                    " ATTACHMENTS "
                }),
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
        let mode_hint = if media.kind == "image" {
            match self.image_mode {
                ImageMode::Terminal => match self.picker.protocol_type() {
                    ProtocolType::Halfblocks => {
                        "Block preview · v → Default image viewer for full detail"
                    }
                    ProtocolType::Kitty => "Kitty graphics · v changes viewer · R reloads",
                    ProtocolType::Sixel => "Sixel graphics · v changes viewer · R reloads",
                    ProtocolType::Iterm2 => "iTerm2 graphics · v changes viewer · R reloads",
                },
                ImageMode::DefaultViewer => "Default image viewer · Enter opens the original file",
                ImageMode::Browser => "Browser · Enter opens the original URL",
            }
        } else {
            ""
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    caption,
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ),
                Line::styled(mode_hint, Style::default().fg(muted)),
            ]),
            rows[0],
        );
        if media.kind == "image" && self.image_mode != ImageMode::Terminal {
            let explanation = match self.image_mode {
                ImageMode::DefaultViewer => {
                    "Open the original image in your default desktop viewer.\nFull resolution; zoom and animation depend on your viewer."
                }
                ImageMode::Browser => "Open the original image URL in your default browser.",
                ImageMode::Terminal => unreachable!(),
            };
            frame.render_widget(
                Paragraph::new(format!("\n{}\n\n{explanation}\n\nEnter  Open    v  Change mode\n\nEsc returns to Tootles", self.image_mode.label()))
                    .style(Style::default().fg(accent))
                    .alignment(Alignment::Center)
                    .wrap(Wrap { trim: true }),
                rows[1],
            );
        } else if media.kind == "image" {
            match &self.cache[self.selected] {
                Cached::Ready(preview) if preview.size == self.size => {
                    frame.render_widget(Image::new(&preview.protocol), rows[1]);
                }
                Cached::Failed(error) => {
                    let fallback = if self.local.is_some() {
                        "R retries; v changes viewer; o opens in your default image viewer."
                    } else {
                        "R retries; v changes viewer; o opens the original in your browser."
                    };
                    frame.render_widget(
                        Paragraph::new(format!(
                            "Image preview unavailable\n\n{error}\n\n{fallback}"
                        ))
                        .style(Style::default().fg(muted))
                        .alignment(Alignment::Center)
                        .wrap(Wrap { trim: true }),
                        rows[1],
                    );
                }
                _ => {
                    frame.render_widget(
                        Paragraph::new("Preparing image…\n\nEsc returns to Tootles")
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
            let explanation = match self.mode {
                PlaybackMode::Window => {
                    "Play in a separate mpv window.\nClosing this gallery stops playback."
                }
                PlaybackMode::Terminal => {
                    "Play in this terminal using mpv.\nPress q in mpv to return here."
                }
                PlaybackMode::Browser => {
                    "Open the original in your browser.\nAvailable for online attachments."
                }
            };
            let preview = format!(
                "\n▶  {label}\n\n{} mode\n{explanation}\n\nEnter  Play    v  Change mode    o  Browser\n\nmpv: Space pause · arrows seek · 9 / 0 volume · q return",
                self.mode.label()
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
            "No attachment description supplied".to_owned()
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
            Constraint::Percentage(21),
            Constraint::Percentage(20),
            Constraint::Percentage(25),
            Constraint::Percentage(20),
            Constraint::Percentage(14),
        ])
        .split(rows[3]);
        self.buttons = [
            button_areas[0],
            button_areas[1],
            button_areas[2],
            button_areas[3],
            button_areas[4],
        ];
        let play_label = if self.is_image() && self.image_mode == ImageMode::Terminal {
            "Enter  Reload"
        } else if self.is_image() {
            "Enter  Open"
        } else {
            "Enter  Play"
        };
        let mode_label = if self.is_image() {
            match self.image_mode {
                ImageMode::Terminal => "v  Terminal".into(),
                ImageMode::DefaultViewer => "v  Default viewer".into(),
                ImageMode::Browser => "v  Browser".into(),
            }
        } else {
            format!("v  {}", self.mode.label())
        };
        for (area, label) in self.buttons.into_iter().zip([
            "← h / l →",
            play_label,
            &mode_label,
            "o  Original",
            "Esc  Close",
        ]) {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(label, Style::default().fg(accent)))),
                area,
            );
        }
    }
}

/// Read an explicitly selected local image with the same budgets as a remote
/// preview. No networking or upload occurs, and special files are rejected.
pub async fn load_file(path: &Path) -> Result<DynamicImage> {
    Ok(
        media_source::load_original(media_source::Source::Local(path.to_path_buf()))
            .await?
            .into_preview(),
    )
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
    Ok(
        media_source::load_original(media_source::Source::Remote(value.to_owned()))
            .await?
            .into_preview(),
    )
}

/// Decode a bounded preview. Public for deterministic tests and future adapters.
pub fn decode_image(bytes: &[u8]) -> Result<DynamicImage> {
    media_source::decode_preview(bytes)
}

fn display_text(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control() || *character == '\n')
        .collect()
}
