use crate::{
    auth,
    config::{self, Config},
    editor,
    file_picker::{self, FileKind, Listing},
    links,
    model::*,
    provider::{SocialProvider, demo::DemoProvider, mastodon::Mastodon},
    view::*,
};
use anyhow::{Result, bail};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

#[derive(Default)]
struct FeedData {
    posts: Vec<Post>,
    notifications: Vec<Notification>,
    next: Option<String>,
    people: Vec<Account>,
    hashtags: Vec<String>,
    profile: Option<Account>,
    relationship: Relationship,
}
enum Data {
    Feed(Box<FeedData>, bool),
    Identity(Account, Capabilities),
    Lists(Vec<SocialList>, u64),
    Post(String, Option<Box<Post>>),
    Published(Box<Post>, String),
    Source(Compose, u64),
    Relationship(String, Relationship),
    Poll(Poll),
    List(SocialList),
    Done(String),
    Files(u64, std::result::Result<Listing, String>),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Job {
    Feed(u64),
    Action,
    Background,
}
struct Message {
    generation: u64,
    job: Job,
    result: Result<Data>,
}

/// A confirmation owns the original target. Feed refreshes and selection changes
/// must never retarget a destructive action while the dialog is visible.
enum Confirmation {
    Delete(Box<Post>),
    Account(Box<Account>, String),
}

pub struct App {
    pub view: View,
    pub quit: bool,
    pub media_requested: Option<Post>,
    pub local_media_requested: Option<PathBuf>,
    provider: Arc<dyn SocialProvider>,
    config: Config,
    root: Option<PathBuf>,
    account_id: String,
    generation: u64,
    request: u64,
    intent: u64,
    picker_request: u64,
    confirmation: Option<Confirmation>,
    cursor: Option<String>,
    history: Vec<Feed>,
    tx: mpsc::UnboundedSender<Message>,
    rx: mpsc::UnboundedReceiver<Message>,
    notice_at: Instant,
    list_add_author: Option<String>,
    draft_dirty: bool,
    last_draft_save: Instant,
    refresh_at: Instant,
}
impl App {
    pub fn new(config: Config, root: Option<PathBuf>, demo: bool) -> Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut view = View {
            theme: parse_theme(&config.theme),
            reduced_motion: config.reduced_motion,
            compact: config.compact,
            ..View::default()
        };
        let (provider, account_id): (Arc<dyn SocialProvider>, String) =
            if !demo && let Some(a) = config.active_account() {
                if a.provider != "mastodon" {
                    bail!(
                        "Unknown provider '{}'. This build includes Mastodon and Demo adapters.",
                        a.provider
                    );
                }
                view.instance = display_instance(&a.instance);
                view.demo = false;
                (
                    Arc::new(Mastodon::new(&a.instance, &a.token)?),
                    a.id.clone(),
                )
            } else {
                view.demo = true;
                view.instance = "the open web · demo".into();
                (Arc::new(DemoProvider::new()), "demo".into())
            };
        view.capabilities = provider.capabilities();
        view.accounts = account_labels(&config, &account_id);
        view.drafts = config::load_drafts(root.as_deref(), &account_id)?;
        let mut app = Self {
            view,
            quit: false,
            provider,
            config,
            root,
            account_id,
            generation: 0,
            request: 0,
            intent: 0,
            picker_request: 0,
            confirmation: None,
            cursor: None,
            history: vec![],
            tx,
            rx,
            notice_at: Instant::now(),
            media_requested: None,
            local_media_requested: None,
            list_add_author: None,
            draft_dirty: false,
            last_draft_save: Instant::now(),
            refresh_at: Instant::now(),
        };
        if app.view.demo {
            app.notice(
                "Welcome to the demo. Explore freely · connect with `tootles login <server>`",
                false,
            );
        }
        Ok(app)
    }
    pub fn initialize(&mut self) {
        let p = self.provider.clone();
        self.spawn(Job::Background, async move {
            let me = p.me().await?;
            Ok(Data::Identity(me, p.capabilities()))
        });
        self.load(false);
    }
    pub fn config_root(&self) -> Option<&Path> {
        self.root.as_deref()
    }
    fn spawn(&self, job: Job, future: impl Future<Output = Result<Data>> + Send + 'static) {
        let tx = self.tx.clone();
        let generation = self.generation;
        tokio::spawn(async move {
            let result = future.await;
            let _ = tx.send(Message {
                generation,
                job,
                result,
            });
        });
    }
    pub fn notice(&mut self, message: impl Into<String>, error: bool) {
        self.view.notice = Some(Notice {
            message: message.into(),
            error,
        });
        self.notice_at = Instant::now();
    }
    fn load(&mut self, append: bool) {
        if append && (self.view.loading || self.cursor.is_none()) {
            return;
        }
        self.request += 1;
        self.view.loading = true;
        let p = self.provider.clone();
        let feed = self.view.feed.clone();
        let cursor = if append { self.cursor.clone() } else { None };
        self.spawn(Job::Feed(self.request), async move {
            Ok(Data::Feed(
                Box::new(fetch_feed(p, &feed, cursor.as_deref()).await?),
                append,
            ))
        });
        self.refresh_at = Instant::now();
    }
    fn navigate(&mut self, feed: Feed) {
        if self.view.feed != feed {
            self.history.push(self.view.feed.clone());
        }
        self.view.feed = feed;
        self.view.posts.clear();
        self.view.notifications.clear();
        self.view.people.clear();
        self.view.hashtags.clear();
        self.view.profile = None;
        self.view.relationship = Relationship::default();
        self.view.selected = 0;
        self.view.offset = 0;
        self.view.detail_scroll = 0;
        self.cursor = None;
        self.view.has_more = false;
        self.view.modal = Modal::None;
        self.load(false);
    }
    fn back(&mut self) {
        if let Some(feed) = self.history.pop() {
            self.navigate(feed);
            self.history.pop();
        }
    }
    pub fn drain(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            if msg.generation != self.generation {
                continue;
            }
            if let Job::Feed(n) = msg.job {
                if n != self.request {
                    continue;
                }
                self.view.loading = false;
            }
            if msg.job == Job::Action {
                self.view.busy = false;
            }
            match msg.result {
                Ok(Data::Files(request, result)) => {
                    if request != self.picker_request
                        || !matches!(self.view.modal, Modal::FileBrowser)
                    {
                        continue;
                    }
                    let picker = &mut self.view.file_browser;
                    picker.loading = false;
                    match result {
                        Ok(listing) => {
                            picker.directory = listing.directory;
                            if !picker.editing_location {
                                picker.location = picker.directory.to_string_lossy().into_owned();
                                picker.cursor = picker.location.len();
                            }
                            picker.entries = listing.entries;
                            picker.omitted = listing.omitted;
                            picker.error = None;
                        }
                        Err(error) => picker.error = Some(error),
                    }
                }
                Err(e) => {
                    if matches!(msg.job, Job::Feed(_)) {
                        self.view.connected = false;
                    }
                    self.notice(format!("{e:#}"), true);
                }
                Ok(Data::Identity(me, caps)) => {
                    self.view.me = me;
                    self.view.capabilities = caps;
                    self.view.connected = true;
                }
                Ok(Data::Feed(data, append)) => {
                    let selected_id = self.view.selected_post().map(|p| p.id.clone());
                    let notification_id = self
                        .view
                        .notifications
                        .get(self.view.selected)
                        .map(|n| n.id.clone());
                    self.cursor = data.next;
                    self.view.has_more = self.cursor.is_some();
                    if append {
                        // Use outer IDs: multiple boosts of a status are distinct timeline entries.
                        for p in data.posts {
                            if !self
                                .view
                                .posts
                                .iter()
                                .any(|x| x.timeline_id == p.timeline_id)
                            {
                                self.view.posts.push(p);
                            }
                        }
                        for n in data.notifications {
                            if !self.view.notifications.iter().any(|x| x.id == n.id) {
                                self.view.notifications.push(n);
                            }
                        }
                    } else {
                        self.view.posts = data.posts;
                        self.view.notifications = data.notifications;
                        self.view.people = data.people;
                        self.view.hashtags = data.hashtags;
                        self.view.profile = data.profile;
                        self.view.relationship = data.relationship;
                        if self.view.feed == Feed::Notifications {
                            if let Some(i) = notification_id.and_then(|id| {
                                self.view.notifications.iter().position(|n| n.id == id)
                            }) {
                                self.view.selected = i;
                            }
                        } else if let Some(i) = selected_id
                            .and_then(|id| self.view.posts.iter().position(|p| p.id == id))
                        {
                            self.view.selected = i;
                        }
                    }
                    self.view.selected =
                        self.view.selected.min(self.view.count().saturating_sub(1));
                    self.view.connected = true;
                    self.view.last_sync = chrono::Local::now().format("%H:%M").to_string();
                    if matches!(self.view.feed, Feed::Search(_)) && !self.view.people.is_empty() {
                        self.notice(
                            format!(
                                "Found {} posts and {} people · Ctrl+K → Show people from search",
                                self.view.posts.len(),
                                self.view.people.len()
                            ),
                            false,
                        );
                    }
                }
                Ok(Data::Lists(lists, intent)) => {
                    self.view.lists = lists;
                    if intent == self.intent {
                        self.view.modal = Modal::Lists { selected: 0 };
                    }
                }
                Ok(Data::Post(id, post)) => {
                    if let Some(post) = post {
                        self.replace_post(*post);
                    } else {
                        self.view.posts.retain(|p| p.id != id);
                        self.view
                            .notifications
                            .retain(|n| n.post.as_ref().is_none_or(|p| p.id != id));
                    }
                    self.view.selected =
                        self.view.selected.min(self.view.count().saturating_sub(1));
                    self.load(false);
                    self.notice("Updated", false);
                }
                Ok(Data::Published(post, key)) => {
                    self.view.drafts.retain(|d| d.idempotency_key != key);
                    self.persist_drafts();
                    self.view.composer = Compose::default();
                    self.view.modal = Modal::None;
                    self.draft_dirty = false;
                    self.replace_post(*post);
                    self.load(false);
                    self.notice(
                        if self.view.demo {
                            "Posted in your local demo"
                        } else {
                            "Your post is published"
                        },
                        false,
                    );
                }
                Ok(Data::Source(compose, intent)) => {
                    if intent == self.intent {
                        self.open_composer(compose);
                    }
                }
                Ok(Data::Relationship(account, rel)) => {
                    if self.view.profile.as_ref().is_some_and(|p| p.id == account) {
                        self.view.relationship = rel;
                    }
                    self.load(false);
                    self.notice("Account relationship updated", false);
                }
                Ok(Data::Poll(poll)) => {
                    for p in &mut self.view.posts {
                        if p.poll.as_ref().is_some_and(|x| x.id == poll.id) {
                            p.poll = Some(poll.clone());
                        }
                    }
                    for n in &mut self.view.notifications {
                        if let Some(p) = &mut n.post
                            && p.poll.as_ref().is_some_and(|x| x.id == poll.id)
                        {
                            p.poll = Some(poll.clone());
                        }
                    }
                    self.view.modal = Modal::None;
                    self.load(false);
                    self.notice("Vote submitted", false);
                }
                Ok(Data::List(list)) => {
                    self.notice(format!("Created list: {}", list.title), false);
                    self.view.lists.push(list);
                }
                Ok(Data::Done(message)) => self.notice(message, false),
            }
        }
    }
    fn replace_post(&mut self, post: Post) {
        for p in &mut self.view.posts {
            if p.id == post.id {
                let timeline = p.timeline_id.clone();
                let boost = p.boosted_by.clone();
                *p = post.clone();
                p.timeline_id = timeline;
                p.boosted_by = boost;
            }
        }
        for n in &mut self.view.notifications {
            if let Some(p) = &mut n.post
                && p.id == post.id
            {
                *p = post.clone();
            }
        }
    }
    pub fn tick(&mut self) {
        self.view.tick = self.view.tick.wrapping_add(1);
        self.drain();
        if self.view.notice.as_ref().is_some_and(|n| !n.error)
            && self.notice_at.elapsed() > Duration::from_secs(10)
        {
            self.view.notice = None;
        }
        if self.draft_dirty && self.last_draft_save.elapsed() > Duration::from_secs(2) {
            self.save_current_draft(false);
        }
        let interval = Duration::from_secs(self.config.refresh_seconds.max(15));
        if !self.view.demo
            && self.refresh_at.elapsed() > interval
            && !self.view.loading
            && !self.view.busy
            && matches!(self.view.modal, Modal::None)
            && self.view.selected == 0
            && matches!(
                self.view.feed,
                Feed::Home
                    | Feed::Local
                    | Feed::Federated
                    | Feed::Notifications
                    | Feed::List(..)
                    | Feed::Tag(_)
            )
        {
            self.load(false);
        }
    }
    pub fn event(&mut self, event: Event, _width: u16) {
        match event {
            Event::Key(key) if key.kind != crossterm::event::KeyEventKind::Release => self.key(key),
            Event::Paste(text) => self.paste(&text),
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    let hit = self
                        .view
                        .hits
                        .iter()
                        .find(|h| h.area.contains((mouse.column, mouse.row).into()))
                        .map(|h| h.hit.clone());
                    if let Some(hit) = hit {
                        self.hit(hit);
                    }
                }
                MouseEventKind::ScrollDown => {
                    if matches!(self.view.modal, Modal::None)
                        && self
                            .view
                            .detail_area
                            .is_some_and(|area| area.contains((mouse.column, mouse.row).into()))
                    {
                        self.view.detail_scroll = self.view.detail_scroll.saturating_add(3);
                    } else {
                        self.move_selection(1);
                    }
                }
                MouseEventKind::ScrollUp => {
                    if matches!(self.view.modal, Modal::None)
                        && self
                            .view
                            .detail_area
                            .is_some_and(|area| area.contains((mouse.column, mouse.row).into()))
                    {
                        self.view.detail_scroll = self.view.detail_scroll.saturating_sub(3);
                    } else {
                        self.move_selection(-1);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    fn hit(&mut self, hit: Hit) {
        self.intent = self.intent.wrapping_add(1);
        if self.view.busy && matches!(self.view.modal, Modal::Compose) {
            return;
        }
        match hit {
            Hit::Navigate(f) => self.navigate(f),
            Hit::Select(i) | Hit::SelectNotification(i) => {
                self.view.selected = i;
                self.view.detail_scroll = 0;
            }
            Hit::Command(cmd) => self.command(&cmd),
            Hit::ComposerField(i) => {
                self.view.compose_field = i.min(5);
                self.view.compose_cursor = self.compose_text().len();
            }
            Hit::FileSelect(i) => {
                self.view.file_browser.selected =
                    i.min(self.view.file_browser.entries.len().saturating_sub(1));
                self.view.file_browser.editing_location = false;
                self.view.file_browser.error = None;
            }
            Hit::ModalSelect(i) => {
                self.set_modal_selection(i);
                self.activate_modal();
            }
        }
    }
    fn set_modal_selection(&mut self, i: usize) {
        match &mut self.view.modal {
            Modal::FileBrowser => self.view.file_browser.selected = i,
            Modal::Palette { selected, .. }
            | Modal::Accounts { selected }
            | Modal::Lists { selected }
            | Modal::People { selected }
            | Modal::Links { selected, .. }
            | Modal::Poll { selected, .. }
            | Modal::Drafts { selected } => *selected = i,
            _ => {}
        }
    }
    fn move_selection(&mut self, delta: isize) {
        if matches!(self.view.modal, Modal::FileBrowser) {
            self.view.file_browser.editing_location = false;
            if !self.view.file_browser.entries.is_empty() {
                self.view.file_browser.error = None;
            }
        }
        let (selected, len) = match &self.view.modal {
            Modal::FileBrowser => (
                self.view.file_browser.selected,
                self.view.file_browser.entries.len(),
            ),
            Modal::Palette { query, selected } => (*selected, commands(query).len()),
            Modal::Accounts { selected } => (*selected, self.view.accounts.len()),
            Modal::Lists { selected } => (*selected, self.view.lists.len()),
            Modal::People { selected } => (*selected, self.view.people.len()),
            Modal::Links { urls, selected } => (*selected, urls.len()),
            Modal::Drafts { selected } => (*selected, self.view.drafts.len()),
            Modal::Poll { selected, .. } => (
                *selected,
                self.view
                    .selected_post()
                    .and_then(|p| p.poll.as_ref())
                    .map_or(0, |p| p.options.len()),
            ),
            Modal::None => (self.view.selected, self.view.count()),
            _ => return,
        };
        let next = selected
            .saturating_add_signed(delta)
            .min(len.saturating_sub(1));
        if matches!(self.view.modal, Modal::None) {
            self.view.selected = next;
            self.view.detail_scroll = 0;
            if delta > 0 && next + 1 >= len {
                self.load(true);
            }
        } else {
            self.set_modal_selection(next);
        }
    }
    fn key(&mut self, key: KeyEvent) {
        self.intent = self.intent.wrapping_add(1);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.command("quit");
            return;
        }
        if matches!(self.view.modal, Modal::FileBrowser) {
            self.file_browser_key(key);
            return;
        }
        if matches!(self.view.modal, Modal::Compose) {
            if self.view.busy {
                if key.code == KeyCode::Esc {
                    self.notice("Publishing… please wait for the server response", false);
                }
                return;
            }
            match key.code {
                KeyCode::Esc => {
                    if self.save_current_draft(true) {
                        self.view.modal = Modal::None;
                    }
                }
                KeyCode::Enter if ctrl => self.command("send"),
                KeyCode::Char('s') if ctrl => self.command("save_draft"),
                KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::ALT) => {
                    self.command("visibility")
                }
                KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::ALT) => {
                    self.command("attach")
                }
                KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::ALT) => {
                    self.command("browse")
                }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::ALT) => {
                    self.command("poll_multiple")
                }
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::ALT) => {
                    self.command("poll_duration")
                }
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::ALT) => {
                    self.command("sensitive")
                }
                KeyCode::F(2) => self.command("visibility"),
                KeyCode::F(3) => self.command("browse"),
                KeyCode::F(4) => self.command("send"),
                KeyCode::Tab => {
                    self.view.compose_field = (self.view.compose_field + 1) % 6;
                    self.view.compose_cursor = self.compose_text().len();
                }
                KeyCode::BackTab => {
                    self.view.compose_field = (self.view.compose_field + 5) % 6;
                    self.view.compose_cursor = self.compose_text().len();
                }
                _ => {
                    let multiline = self.view.compose_field == 0;
                    let mut cursor = self.view.compose_cursor;
                    editor::edit(self.compose_text_mut(), &mut cursor, key, multiline);
                    self.view.compose_cursor = cursor;
                    self.draft_dirty = true;
                }
            }
            return;
        }
        if key.code == KeyCode::Esc {
            self.view.modal = Modal::None;
            self.list_add_author = None;
            self.confirmation = None;
            return;
        }
        match self.view.modal.clone() {
            Modal::None => {}
            Modal::Help => {
                if matches!(key.code, KeyCode::Char('?') | KeyCode::Enter) {
                    self.view.modal = Modal::None;
                }
                return;
            }
            Modal::Confirm { action, .. } => {
                if matches!(key.code, KeyCode::Char('y') | KeyCode::Enter) {
                    self.view.modal = Modal::None;
                    self.command(&action);
                } else if key.code == KeyCode::Char('n') {
                    self.view.modal = Modal::None;
                    self.confirmation = None;
                }
                return;
            }
            Modal::Input {
                mut value,
                title,
                purpose,
            } => {
                if key.code == KeyCode::Enter {
                    self.view.modal = Modal::None;
                    self.submit_input(&purpose, value);
                } else {
                    editor::edit(&mut value, &mut self.view.input_cursor, key, false);
                    self.view.modal = Modal::Input {
                        title,
                        value,
                        purpose,
                    };
                }
                return;
            }
            Modal::Palette {
                mut query,
                selected,
            } => {
                if key.code == KeyCode::Enter {
                    self.activate_modal();
                } else if key.code == KeyCode::Down || (ctrl && key.code == KeyCode::Char('n')) {
                    self.move_selection(1);
                } else if key.code == KeyCode::Up || (ctrl && key.code == KeyCode::Char('p')) {
                    self.move_selection(-1);
                } else {
                    editor::edit(&mut query, &mut self.view.input_cursor, key, false);
                    let len = commands(&query).len();
                    self.view.modal = Modal::Palette {
                        query,
                        selected: selected.min(len.saturating_sub(1)),
                    };
                }
                return;
            }
            Modal::Poll { .. } => {
                match key.code {
                    KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
                    KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
                    KeyCode::Char(' ') => self.activate_modal(),
                    KeyCode::Enter => self.submit_vote(),
                    _ => {}
                }
                return;
            }
            _ => {
                match key.code {
                    KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
                    KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
                    KeyCode::Enter => self.activate_modal(),
                    KeyCode::PageDown => self.move_selection(5),
                    KeyCode::PageUp => self.move_selection(-5),
                    KeyCode::Home => self.move_selection(isize::MIN),
                    KeyCode::End => self.move_selection(isize::MAX),
                    KeyCode::Char('D') if matches!(self.view.modal, Modal::Drafts { .. }) => {
                        if let Modal::Drafts { selected } = self.view.modal
                            && selected < self.view.drafts.len()
                        {
                            let removed = self.view.drafts.remove(selected);
                            if self.view.composer.idempotency_key == removed.idempotency_key {
                                self.view.composer = Compose::default();
                                self.view.poll_input.clear();
                                self.draft_dirty = false;
                            }
                            self.persist_drafts();
                            self.set_modal_selection(
                                selected.min(self.view.drafts.len().saturating_sub(1)),
                            );
                        }
                    }
                    _ => {}
                }
                return;
            }
        }
        if ctrl && matches!(key.code, KeyCode::Char('k') | KeyCode::Char('p')) {
            self.command("palette");
            return;
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.move_selection(5),
            KeyCode::PageUp => self.move_selection(-5),
            KeyCode::Home | KeyCode::Char('g') => {
                self.view.selected = 0;
                self.view.offset = 0;
                self.view.detail_scroll = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.view.selected = self.view.count().saturating_sub(1);
                self.load(true);
            }
            KeyCode::Char('J') => {
                self.view.detail_scroll = self.view.detail_scroll.saturating_add(3)
            }
            KeyCode::Char('K') => {
                self.view.detail_scroll = self.view.detail_scroll.saturating_sub(3)
            }
            KeyCode::Backspace | KeyCode::Left => self.back(),
            KeyCode::Enter | KeyCode::Right => self.command("thread"),
            KeyCode::Tab => self.command("inspector"),
            KeyCode::Char(c) => {
                if let Some(cmd) = match c {
                    'q' => Some("quit"),
                    'n' => Some("compose"),
                    '/' => Some("search"),
                    ':' => Some("palette"),
                    '?' => Some("help"),
                    '1' => Some("home"),
                    '2' => Some("local"),
                    '3' => Some("federated"),
                    '4' => Some("notifications"),
                    '5' => Some("bookmarks"),
                    '6' => Some("favourites"),
                    'r' => Some("reply"),
                    'R' => Some("refresh"),
                    'f' => Some("favourite"),
                    'b' => Some("boost"),
                    's' => Some("bookmark"),
                    'p' => Some("profile"),
                    'F' => Some("follow"),
                    'o' => Some("open"),
                    'L' => Some("links"),
                    'm' => Some("media"),
                    'x' => Some("reveal"),
                    'v' => Some("vote"),
                    'e' => Some("edit"),
                    'a' => Some("accounts"),
                    'l' => Some("lists"),
                    '#' => Some("tag"),
                    'd' => Some("drafts"),
                    'T' => Some("theme"),
                    'c' => Some("compact"),
                    'i' => Some("inspector"),
                    _ => None,
                } {
                    self.command(cmd);
                }
            }
            _ => {}
        }
    }
    fn paste(&mut self, text: &str) {
        self.intent = self.intent.wrapping_add(1);
        match &mut self.view.modal {
            Modal::FileBrowser if self.view.file_browser.editing_location => {
                let picker = &mut self.view.file_browser;
                editor::insert(&mut picker.location, &mut picker.cursor, text, false);
            }
            Modal::Compose if !self.view.busy => {
                let mut c = self.view.compose_cursor;
                let multi = self.view.compose_field == 0;
                editor::insert(self.compose_text_mut(), &mut c, text, multi);
                self.view.compose_cursor = c;
                self.draft_dirty = true;
            }
            Modal::Palette { query, selected } => {
                editor::insert(query, &mut self.view.input_cursor, text, false);
                *selected = 0;
            }
            Modal::Input { value, .. } => {
                editor::insert(value, &mut self.view.input_cursor, text, false)
            }
            _ => {}
        }
    }
    fn input(&mut self, title: &str, purpose: &str) {
        self.view.input_cursor = 0;
        self.view.modal = Modal::Input {
            title: title.into(),
            value: String::new(),
            purpose: purpose.into(),
        };
    }
    fn submit_input(&mut self, purpose: &str, value: String) {
        let value = value.trim().to_string();
        if value.is_empty() {
            return;
        }
        match purpose {
            "search" => self.navigate(Feed::Search(value)),
            "tag" => self.navigate(Feed::Tag(value.trim_start_matches('#').to_string())),
            "create_list" if self.begin_action() => {
                let p = self.provider.clone();
                self.spawn(Job::Action, async move {
                    Ok(Data::List(p.create_list(&value).await?))
                });
            }
            _ => {}
        }
    }
    fn activate_modal(&mut self) {
        match self.view.modal.clone() {
            Modal::FileBrowser => self.file_browser_activate(),
            Modal::Input { purpose, value, .. } => {
                self.view.modal = Modal::None;
                self.submit_input(&purpose, value);
            }
            Modal::Palette { query, selected } => {
                if let Some(c) = commands(&query).get(selected) {
                    let id = c.id;
                    self.view.modal = Modal::None;
                    self.command(id);
                }
            }
            Modal::Accounts { selected } => {
                self.switch_account(selected);
            }
            Modal::Lists { selected } => {
                if let Some(list) = self.view.lists.get(selected).cloned() {
                    self.view.modal = Modal::None;
                    if let Some(author) = self.list_add_author.take() {
                        if self.begin_action() {
                            let p = self.provider.clone();
                            self.spawn(Job::Action, async move {
                                p.list_member(&list.id, &author, true).await?;
                                Ok(Data::Done(format!("Added author to {}", list.title)))
                            });
                        }
                    } else {
                        self.navigate(Feed::List(list.id, list.title));
                    }
                }
            }
            Modal::People { selected } => {
                if let Some(a) = self.view.people.get(selected) {
                    self.navigate(Feed::Profile(a.id.clone()));
                }
            }
            Modal::Links { urls, selected } => {
                if let Some(url) = urls.get(selected) {
                    self.open_browser(url.clone());
                }
            }
            Modal::Drafts { selected } => {
                if let Some(d) = self.view.drafts.get(selected).cloned() {
                    self.open_composer(d);
                }
            }
            Modal::Poll {
                mut choices,
                selected,
            } => {
                if let Some(poll) = self.view.selected_post().and_then(|p| p.poll.as_ref())
                    && selected < poll.options.len()
                {
                    if choices.contains(&selected) {
                        choices.retain(|v| *v != selected);
                    } else {
                        if !poll.multiple {
                            choices.clear();
                        }
                        choices.push(selected);
                    }
                }
                self.view.modal = Modal::Poll { choices, selected };
            }
            _ => {}
        }
    }
    fn begin_action(&mut self) -> bool {
        if self.view.busy {
            self.notice("Waiting for the current action to finish…", false);
            false
        } else {
            self.view.busy = true;
            true
        }
    }
    pub fn command(&mut self, cmd: &str) {
        self.intent = self.intent.wrapping_add(1);
        if self.view.busy && matches!(self.view.modal, Modal::Compose) {
            self.notice("Publishing… please wait for the server response", false);
            return;
        }
        if let Some(rest) = cmd.strip_prefix("post:")
            && let Some((index, action)) = rest.split_once(':')
            && let Ok(i) = index.parse::<usize>()
        {
            if i < self.view.count() {
                self.view.selected = i;
                self.command(action);
            }
            return;
        }
        if let Some(index) = cmd
            .strip_prefix("link:")
            .and_then(|s| s.parse::<usize>().ok())
        {
            if let Some(url) = self.post_links().and_then(|urls| urls.get(index).cloned()) {
                self.open_browser(url);
            }
            return;
        }
        match cmd {
            "confirm" => {
                if let Modal::Confirm { action, .. } = self.view.modal.clone() {
                    self.view.modal = Modal::None;
                    self.command(&action);
                }
            }
            "modal_activate" => self.activate_modal(),
            "submit_vote" => self.submit_vote(),
            "quit" => {
                if self.view.busy {
                    self.notice(
                        "An action is still in progress. Wait for its result before quitting.",
                        true,
                    );
                    return;
                }
                if self.save_current_draft(false) {
                    self.quit = true;
                }
            }
            "palette" => {
                self.view.input_cursor = 0;
                self.view.modal = Modal::Palette {
                    query: String::new(),
                    selected: 0,
                };
            }
            "help" => self.view.modal = Modal::Help,
            "home" => self.navigate(Feed::Home),
            "local" => self.navigate(Feed::Local),
            "federated" => self.navigate(Feed::Federated),
            "notifications" => self.navigate(Feed::Notifications),
            "bookmarks" => self.navigate(Feed::Bookmarks),
            "favourites" => self.navigate(Feed::Favourites),
            "refresh" => self.load(false),
            "search" => self.input("Search posts, people & hashtags", "search"),
            "tag" => self.input("Explore a hashtag", "tag"),
            "compose" => {
                if !self.view.capabilities.posting {
                    self.notice("This provider does not support posting", true);
                    return;
                }
                self.open_composer(Compose::default());
            }
            "reply" => {
                if let Some(post) = self.view.selected_post().cloned() {
                    let mut mentions = vec![post.account.acct.clone()];
                    mentions.extend(post.mentions.clone());
                    mentions.retain(|a| a != &self.view.me.acct);
                    mentions.dedup();
                    let text = mentions
                        .iter()
                        .map(|a| format!("@{a} "))
                        .collect::<String>();
                    self.open_composer(Compose {
                        text,
                        spoiler_text: post.spoiler_text,
                        visibility: post.visibility,
                        reply_to: Some(post.id),
                        ..Compose::default()
                    });
                }
            }
            "thread" => {
                if let Some(p) = self.view.selected_post() {
                    self.navigate(Feed::Thread(p.id.clone()));
                }
            }
            "profile" => {
                let account = if self.view.feed == Feed::Notifications {
                    self.view
                        .notifications
                        .get(self.view.selected)
                        .map(|n| n.account.id.clone())
                } else {
                    self.view.selected_post().map(|p| p.account.id.clone())
                };
                if let Some(id) = account {
                    self.navigate(Feed::Profile(id));
                }
            }
            "my_profile" => {
                if !self.view.me.id.is_empty() {
                    self.navigate(Feed::Profile(self.view.me.id.clone()));
                }
            }
            "reveal" => {
                if let Some(id) = self.view.selected_post().map(|p| p.id.clone())
                    && !self.view.revealed.remove(&id)
                {
                    self.view.revealed.insert(id);
                }
            }
            "favourite" | "boost" | "bookmark" => {
                if let Some(post) = self.view.selected_post().cloned() {
                    if cmd == "boost"
                        && matches!(post.visibility, Visibility::Direct | Visibility::Private)
                    {
                        self.notice("Only public and unlisted posts can be boosted", true);
                        return;
                    }
                    let action = match cmd {
                        "favourite" => PostAction::Favourite(!post.favourited),
                        "boost" => PostAction::Boost(!post.reblogged),
                        _ => PostAction::Bookmark(!post.bookmarked),
                    };
                    if self.begin_action() {
                        let p = self.provider.clone();
                        self.spawn(Job::Action, async move {
                            let updated = p.post_action(&post.id, action).await?;
                            Ok(Data::Post(post.id, updated.map(Box::new)))
                        });
                    }
                }
            }
            "delete" => {
                if let Some(p) = self.view.selected_post().cloned() {
                    if p.account.id == self.view.me.id {
                        self.confirmation = Some(Confirmation::Delete(Box::new(p)));
                        self.view.modal = Modal::Confirm {
                            title: "Delete this post?".into(),
                            detail:
                                "This removes your post from the server. This cannot be undone."
                                    .into(),
                            action: "confirm_action".into(),
                        };
                    } else {
                        self.notice("You can only delete your own posts", true);
                    }
                }
            }
            "confirm_action" => self.confirm_action(),
            "edit" => {
                if let Some(post) = self.view.selected_post().cloned() {
                    if post.account.id != self.view.me.id {
                        self.notice("You can only edit your own posts", true);
                        return;
                    }
                    if self.begin_action() {
                        let p = self.provider.clone();
                        let intent = self.intent;
                        self.spawn(Job::Action, async move {
                            let mut c = p.source(&post.id).await?;
                            c.edit_id = Some(post.id);
                            Ok(Data::Source(c, intent))
                        });
                    }
                }
            }
            "follow" => self.account_action("follow"),
            "mute" | "block" => {
                if let Some(account) = self.action_account() {
                    if account.id == self.view.me.id {
                        self.notice("Choose another account", false);
                        return;
                    }
                    self.confirmation =
                        Some(Confirmation::Account(Box::new(account), cmd.to_owned()));
                    self.view.modal = Modal::Confirm {
                        title: format!("Toggle {cmd} for this account?"),
                        detail: "This changes which posts and interactions you receive.".into(),
                        action: "confirm_action".into(),
                    };
                }
            }
            "open" => {
                if let Some(p) = self.view.selected_post() {
                    self.open_browser(p.url.clone());
                }
            }
            "links" => {
                if let Some(urls) = self.post_links() {
                    self.view.modal = Modal::Links { urls, selected: 0 };
                }
            }
            "media" => {
                if let Some(p) = self.view.selected_post() {
                    if p.media.is_empty() {
                        self.notice("This post has no attached media", false);
                    } else if (!p.spoiler_text.is_empty() || p.sensitive)
                        && !self.view.revealed.contains(&p.id)
                    {
                        self.notice(
                            "Press x to reveal this post before opening its media",
                            false,
                        );
                    } else {
                        self.media_requested = Some(p.clone());
                    }
                }
            }
            "vote" => {
                if self.view.selected_post().is_some_and(|post| {
                    (!post.spoiler_text.is_empty() || post.sensitive)
                        && !self.view.revealed.contains(&post.id)
                }) {
                    self.notice("Press x to reveal this post before voting", false);
                    return;
                }
                if let Some(p) = self.view.selected_post().and_then(|p| p.poll.as_ref()) {
                    if p.voted || p.expired {
                        self.notice("This poll has already been voted on or has ended", false);
                    } else {
                        self.view.modal = Modal::Poll {
                            choices: vec![],
                            selected: 0,
                        };
                    }
                } else {
                    self.notice("Select a post with a poll", false);
                }
            }
            "accounts" => self.view.modal = Modal::Accounts { selected: 0 },
            "lists" | "add_to_list" => {
                if !self.view.capabilities.lists {
                    self.notice("This network does not support lists", true);
                    return;
                }
                self.list_add_author = if cmd == "add_to_list" {
                    self.view.selected_post().map(|p| p.account.id.clone())
                } else {
                    None
                };
                let p = self.provider.clone();
                let intent = self.intent;
                self.spawn(Job::Background, async move {
                    Ok(Data::Lists(p.lists().await?, intent))
                });
            }
            "create_list" => self.input("Name your new list", "create_list"),
            "people" => self.view.modal = Modal::People { selected: 0 },
            "drafts" => self.view.modal = Modal::Drafts { selected: 0 },
            "theme" => {
                self.view.theme = self.view.theme.next();
                if self.persist_preferences(None) {
                    self.notice(format!("Theme · {}", self.view.theme.name()), false);
                }
            }
            "motion" => {
                self.view.reduced_motion = !self.view.reduced_motion;
                if self.persist_preferences(None) {
                    self.notice(
                        if self.view.reduced_motion {
                            "Reduced motion enabled"
                        } else {
                            "Motion enabled"
                        },
                        false,
                    );
                }
            }
            "compact" => {
                self.view.compact = !self.view.compact;
                self.persist_preferences(None);
            }
            "inspector" => self.view.show_inspector = !self.view.show_inspector,
            "send" => self.publish(),
            "save_draft" => {
                self.save_current_draft(true);
            }
            "cancel" => {
                if matches!(self.view.modal, Modal::FileBrowser) {
                    self.close_file_browser();
                    return;
                }
                if matches!(self.view.modal, Modal::Compose) && !self.save_current_draft(true) {
                    return;
                }
                self.view.modal = Modal::None;
                self.confirmation = None;
                self.list_add_author = None;
            }
            "visibility" => {
                self.view.composer.visibility = self.view.composer.visibility.next();
                self.draft_dirty = true;
            }
            "sensitive" => {
                self.view.composer.sensitive = !self.view.composer.sensitive;
                self.draft_dirty = true;
            }
            "poll_multiple" => {
                self.view.composer.poll_multiple = !self.view.composer.poll_multiple;
                self.draft_dirty = true;
            }
            "poll_duration" => {
                self.view.composer.poll_expires_in = match self.view.composer.poll_expires_in {
                    300 => 3600,
                    3600 => 86400,
                    86400 => 604800,
                    _ => 300,
                };
                self.draft_dirty = true;
            }
            "attach" => {
                let value = self.view.attachment_path.as_str();
                if value.trim().is_empty() {
                    self.open_file_browser();
                    return;
                }
                let directory = std::env::current_dir().unwrap_or_default();
                let path = file_picker::resolve_location(value, &directory);
                if !path.is_file() {
                    self.notice("Attachment path must point to an existing file", true);
                    return;
                }
                if self.view.composer.attachments.len() >= 4 {
                    self.notice("A post can contain up to four attachments", true);
                    return;
                }
                self.view.composer.attachments.push(Attachment {
                    path: path.to_string_lossy().into_owned(),
                    description: self.view.attachment_alt.trim().into(),
                });
                self.view.attachment_path.clear();
                self.view.attachment_alt.clear();
                self.view.compose_cursor = 0;
                self.draft_dirty = true;
                self.notice("Attachment added. Alt text travels with your media.", false);
            }
            "browse" => self.open_file_browser(),
            "browse_open" => self.file_browser_activate(),
            "browse_preview" => {
                if let Some(entry) = self
                    .view
                    .file_browser
                    .entries
                    .get(self.view.file_browser.selected)
                    && entry.kind != FileKind::Directory
                {
                    self.local_media_requested = Some(entry.path.clone());
                }
            }
            "preview_attachment" => {
                let value = if self.view.attachment_path.trim().is_empty() {
                    self.view
                        .composer
                        .attachments
                        .last()
                        .map(|attachment| attachment.path.as_str())
                } else {
                    Some(self.view.attachment_path.as_str())
                };
                if let Some(value) = value {
                    let path = file_picker::resolve_location(
                        value,
                        &std::env::current_dir().unwrap_or_default(),
                    );
                    self.local_media_requested = Some(path);
                } else {
                    self.notice("Choose a file or add an attachment to preview it", false);
                }
            }
            "browse_up" => self.file_browser_up(),
            "browse_home" => self.browse_directory(file_picker::initial_directory(None)),
            "browse_location" => {
                self.view.file_browser.editing_location = true;
                self.view.file_browser.cursor = self.view.file_browser.location.len();
            }
            "browse_go" => self.file_browser_location(),
            "browse_filter" => {
                self.view.file_browser.filter = self.view.file_browser.filter.next();
                self.browse_directory(self.view.file_browser.directory.clone());
            }
            "browse_hidden" => {
                self.view.file_browser.show_hidden = !self.view.file_browser.show_hidden;
                self.browse_directory(self.view.file_browser.directory.clone());
            }
            "remove_attachment" => {
                self.view.composer.attachments.pop();
                self.draft_dirty = true;
            }
            _ => {}
        }
    }
    fn open_file_browser(&mut self) {
        if !matches!(self.view.modal, Modal::Compose) {
            return;
        }
        if !self.view.capabilities.media {
            self.notice("This network does not support attachments", true);
            return;
        }
        if self.view.composer.attachments.len() >= 4 {
            self.notice("A post can contain up to four attachments", true);
            return;
        }
        let last = &self.view.file_browser.directory;
        let mut directory = file_picker::initial_directory(if last.as_os_str().is_empty() {
            None
        } else {
            Some(last)
        });
        if !self.view.attachment_path.trim().is_empty() {
            let current = std::env::current_dir().unwrap_or_else(|_| directory.clone());
            let entered = file_picker::resolve_location(&self.view.attachment_path, &current);
            if entered.is_dir() {
                directory = entered;
            } else if let Some(parent) = entered.parent()
                && parent.is_dir()
            {
                directory = parent.to_owned();
            }
        }
        self.view.modal = Modal::FileBrowser;
        self.browse_directory(directory);
    }
    fn browse_directory(&mut self, directory: PathBuf) {
        if !matches!(self.view.modal, Modal::FileBrowser) {
            return;
        }
        self.picker_request = self.picker_request.wrapping_add(1);
        let request = self.picker_request;
        let picker = &mut self.view.file_browser;
        picker.directory = directory.clone();
        picker.location = directory.to_string_lossy().into_owned();
        picker.cursor = picker.location.len();
        picker.editing_location = false;
        picker.selected = 0;
        picker.offset = 0;
        picker.entries.clear();
        picker.omitted = 0;
        picker.error = None;
        picker.loading = true;
        let filter = picker.filter;
        let hidden = picker.show_hidden;
        self.spawn(Job::Background, async move {
            let result =
                tokio::task::spawn_blocking(move || file_picker::scan(&directory, filter, hidden))
                    .await;
            let result = match result {
                Ok(result) => result.map_err(|error| format!("{error:#}")),
                Err(_) => Err("Could not finish reading this folder. Try another location.".into()),
            };
            Ok(Data::Files(request, result))
        });
    }
    fn close_file_browser(&mut self) {
        self.picker_request = self.picker_request.wrapping_add(1);
        self.view.file_browser.loading = false;
        self.view.file_browser.editing_location = false;
        self.view.modal = Modal::Compose;
    }
    fn file_browser_up(&mut self) {
        if let Some(parent) = self.view.file_browser.directory.parent() {
            self.browse_directory(parent.to_owned());
        }
    }
    fn file_browser_activate(&mut self) {
        if self.view.file_browser.loading {
            return;
        }
        if self.view.file_browser.editing_location {
            self.file_browser_location();
            return;
        }
        if let Some(entry) = self
            .view
            .file_browser
            .entries
            .get(self.view.file_browser.selected)
            .cloned()
        {
            if entry.kind == FileKind::Directory {
                self.browse_directory(entry.path);
            } else {
                self.choose_attachment(&entry.path);
            }
        }
    }
    fn choose_attachment(&mut self, path: &Path) {
        let Some(value) = path.to_str() else {
            self.view.file_browser.error =
                Some("This filename is not valid Unicode. Rename it before attaching.".into());
            return;
        };
        if !path.is_file() {
            self.view.file_browser.error =
                Some("This file is no longer available. Refresh the folder with R.".into());
            return;
        }
        self.view.attachment_path = value.to_owned();
        self.close_file_browser();
        self.view.compose_field = 3;
        self.view.compose_cursor = self.view.attachment_alt.len();
        self.notice(
            "File selected. Add a description, then Alt+A or Add file to attach it.",
            false,
        );
    }
    fn file_browser_location(&mut self) {
        let picker = &self.view.file_browser;
        if picker.location.trim().is_empty() {
            return;
        }
        let path = file_picker::resolve_location(&picker.location, &picker.directory);
        if path.is_file() {
            if file_picker::media_kind(&path).is_some() {
                self.choose_attachment(&path);
            } else {
                self.view.file_browser.error =
                    Some("Choose an image, video, or audio file.".into());
            }
        } else {
            self.browse_directory(path);
        }
    }
    fn file_browser_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.close_file_browser();
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('l') {
            self.command("browse_location");
            return;
        }
        if self.view.file_browser.editing_location {
            match key.code {
                KeyCode::Enter => self.file_browser_location(),
                KeyCode::Tab => self.view.file_browser.editing_location = false,
                _ => {
                    let picker = &mut self.view.file_browser;
                    editor::edit(&mut picker.location, &mut picker.cursor, key, false);
                }
            }
            return;
        }
        match key.code {
            KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => self.file_browser_up(),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::Home => self.view.file_browser.selected = 0,
            KeyCode::End => {
                self.view.file_browser.selected =
                    self.view.file_browser.entries.len().saturating_sub(1)
            }
            KeyCode::Enter | KeyCode::Right => self.file_browser_activate(),
            KeyCode::Backspace | KeyCode::Left => self.file_browser_up(),
            KeyCode::Tab | KeyCode::Char('f') => self.command("browse_filter"),
            KeyCode::Char('.') => self.command("browse_hidden"),
            KeyCode::Char('p') => self.command("browse_preview"),
            KeyCode::Char('~') => self.command("browse_home"),
            KeyCode::F(5) | KeyCode::Char('R') => {
                self.browse_directory(self.view.file_browser.directory.clone())
            }
            _ => {}
        }
    }
    fn action_account(&self) -> Option<Account> {
        if self.view.feed == Feed::Notifications {
            self.view
                .notifications
                .get(self.view.selected)
                .map(|n| n.account.clone())
        } else {
            self.view
                .profile
                .as_ref()
                .or_else(|| self.view.selected_post().map(|p| &p.account))
                .cloned()
        }
    }
    fn account_action(&mut self, kind: &str) {
        if let Some(account) = self.action_account() {
            self.account_action_on(account, kind);
        }
    }
    fn account_action_on(&mut self, account: Account, kind: &str) {
        if account.id == self.view.me.id {
            self.notice("Choose another account", false);
            return;
        }
        if self.begin_action() {
            let p = self.provider.clone();
            let kind = kind.to_string();
            self.spawn(Job::Action, async move {
                let rel = p.relationship(&account.id).await?;
                let action = match kind.as_str() {
                    "follow" => AccountAction::Follow(!(rel.following || rel.requested)),
                    "mute" => AccountAction::Mute(!rel.muting),
                    _ => AccountAction::Block(!rel.blocking),
                };
                let relationship = p.account_action(&account.id, action).await?;
                Ok(Data::Relationship(account.id, relationship))
            });
        }
    }
    fn confirm_action(&mut self) {
        match self.confirmation.take() {
            Some(Confirmation::Delete(post)) => {
                if post.account.id != self.view.me.id {
                    self.notice("You can only delete your own posts", true);
                    return;
                }
                if self.begin_action() {
                    let p = self.provider.clone();
                    self.spawn(Job::Action, async move {
                        p.post_action(&post.id, PostAction::Delete).await?;
                        Ok(Data::Post(post.id, None))
                    });
                }
            }
            Some(Confirmation::Account(account, kind)) => self.account_action_on(*account, &kind),
            None => {}
        }
    }
    fn post_links(&mut self) -> Option<Vec<String>> {
        let Some(post) = self.view.selected_post() else {
            self.notice("Select a post to open its links", false);
            return None;
        };
        if !post.spoiler_text.is_empty() && !self.view.revealed.contains(&post.id) {
            self.notice(
                "Press x to reveal this post before opening its links",
                false,
            );
            return None;
        }
        let urls = links::extract(&post.text);
        if urls.is_empty() {
            self.notice("This post has no HTTP or HTTPS links", false);
            None
        } else {
            Some(urls)
        }
    }
    fn open_browser(&mut self, url: String) {
        self.spawn(Job::Background, async move {
            tokio::task::spawn_blocking(move || auth::open_url(&url)).await??;
            Ok(Data::Done("Opened in your browser".into()))
        });
    }
    fn open_composer(&mut self, mut compose: Compose) {
        if self.view.busy {
            return;
        }
        if compose.idempotency_key.is_empty() {
            compose.idempotency_key = uuid::Uuid::new_v4().to_string();
        }
        if compose.poll_expires_in == 0 {
            compose.poll_expires_in = 86400;
        }
        self.view.poll_input = compose.poll_options.join("; ");
        self.view.compose_cursor = compose.text.len();
        self.view.composer = compose;
        self.view.attachment_path.clear();
        self.view.attachment_alt.clear();
        self.view.compose_field = 0;
        self.view.modal = Modal::Compose;
        self.draft_dirty = false;
    }
    fn compose_text(&self) -> &str {
        match self.view.compose_field {
            0 => &self.view.composer.text,
            1 => &self.view.composer.spoiler_text,
            2 => &self.view.attachment_path,
            3 => &self.view.attachment_alt,
            4 => &self.view.poll_input,
            _ => &self.view.composer.language,
        }
    }
    fn compose_text_mut(&mut self) -> &mut String {
        match self.view.compose_field {
            0 => &mut self.view.composer.text,
            1 => &mut self.view.composer.spoiler_text,
            2 => &mut self.view.attachment_path,
            3 => &mut self.view.attachment_alt,
            4 => &mut self.view.poll_input,
            _ => &mut self.view.composer.language,
        }
    }
    fn current_compose(&self) -> Compose {
        let mut c = self.view.composer.clone();
        c.poll_options = self
            .view
            .poll_input
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        c
    }
    fn save_current_draft(&mut self, notify: bool) -> bool {
        let c = self.current_compose();
        if c.idempotency_key.is_empty()
            || (c.text.trim().is_empty()
                && c.attachments.is_empty()
                && c.spoiler_text.is_empty()
                && c.poll_options.is_empty())
        {
            return true;
        }
        if let Some(d) = self
            .view
            .drafts
            .iter_mut()
            .find(|d| d.idempotency_key == c.idempotency_key)
        {
            *d = c;
        } else {
            self.view.drafts.insert(0, c);
        }
        if self.persist_drafts() {
            self.draft_dirty = false;
            self.last_draft_save = Instant::now();
            if notify {
                self.notice("Draft saved on this device · d to resume", false);
            }
            true
        } else {
            false
        }
    }
    fn persist_drafts(&mut self) -> bool {
        match config::save_drafts(self.root.as_deref(), &self.account_id, &self.view.drafts) {
            Ok(()) => true,
            Err(e) => {
                self.notice(format!("Could not save drafts: {e:#}"), true);
                false
            }
        }
    }
    fn persist_preferences(&mut self, active: Option<&str>) -> bool {
        // CLI login/logout may change credentials while this TUI is open.
        // Merge only preferences into the latest file; do not write a stale
        // account snapshot back over credentials added or removed elsewhere.
        let result = (|| -> Result<Config> {
            let mut latest = Config::load(self.root.as_deref())?;
            if let Some(id) = active {
                if !latest.accounts.iter().any(|account| account.id == id) {
                    bail!(
                        "That account was removed from the saved configuration; restart Tootles to refresh the account list"
                    );
                }
                latest.active = Some(id.to_owned());
            }
            latest.theme = theme_key(self.view.theme).into();
            latest.reduced_motion = self.view.reduced_motion;
            latest.compact = self.view.compact;
            latest.save(self.root.as_deref())?;
            Ok(latest)
        })();
        match result {
            Ok(latest) => {
                self.config = latest;
                self.view.accounts = account_labels(&self.config, &self.account_id);
                true
            }
            Err(error) => {
                self.notice(format!("Could not save settings: {error:#}"), true);
                false
            }
        }
    }
    fn publish(&mut self) {
        if !matches!(self.view.modal, Modal::Compose) {
            return;
        }
        if !self.view.attachment_path.trim().is_empty() {
            self.notice(
                "Use Alt+A or click Add file to add the selected attachment before publishing",
                true,
            );
            return;
        }
        let compose = self.current_compose();
        if let Err(e) = validate_compose(&compose, &self.view.capabilities) {
            self.notice(e.to_string(), true);
            return;
        }
        self.save_current_draft(false);
        if self.begin_action() {
            let p = self.provider.clone();
            self.spawn(Job::Action, async move {
                let post = p.publish(&compose).await?;
                Ok(Data::Published(Box::new(post), compose.idempotency_key))
            });
        }
    }
    fn submit_vote(&mut self) {
        if let Modal::Poll { choices, .. } = &self.view.modal {
            let choices = choices.clone();
            if choices.is_empty() {
                self.notice("Select an option with Space first", false);
                return;
            }
            if let Some(poll) = self
                .view
                .selected_post()
                .and_then(|p| p.poll.as_ref())
                .cloned()
                && self.begin_action()
            {
                let p = self.provider.clone();
                self.spawn(Job::Action, async move {
                    Ok(Data::Poll(p.vote(&poll.id, &choices).await?))
                });
            }
        }
    }
    fn switch_account(&mut self, index: usize) {
        if self.view.busy {
            self.notice(
                "Wait for the current action before switching accounts",
                false,
            );
            return;
        }
        if !self.save_current_draft(false) {
            return;
        }
        let (provider, id, instance, demo): (Arc<dyn SocialProvider>, String, String, bool) =
            if index == 0 {
                (
                    Arc::new(DemoProvider::new()),
                    "demo".into(),
                    "the open web · demo".into(),
                    true,
                )
            } else if let Some(a) = self.config.accounts.get(index - 1) {
                match Mastodon::new(&a.instance, &a.token) {
                    Ok(p) => (
                        Arc::new(p),
                        a.id.clone(),
                        display_instance(&a.instance),
                        false,
                    ),
                    Err(e) => {
                        self.notice(e.to_string(), true);
                        return;
                    }
                }
            } else {
                return;
            };
        let drafts = match config::load_drafts(self.root.as_deref(), &id) {
            Ok(d) => d,
            Err(e) => {
                self.notice(format!("Cannot load account drafts: {e}"), true);
                return;
            }
        };
        if !demo && !self.persist_preferences(Some(&id)) {
            return;
        }
        self.generation += 1;
        self.provider = provider;
        self.account_id = id.clone();
        self.view.accounts = account_labels(&self.config, &id);
        self.view.demo = demo;
        self.view.instance = instance;
        self.view.me = Account::default();
        self.view.connected = false;
        self.view.drafts = drafts;
        self.view.composer = Compose::default();
        self.view.poll_input.clear();
        self.view.revealed.clear();
        self.view.lists.clear();
        self.view.capabilities = self.provider.capabilities();
        self.draft_dirty = false;
        self.history.clear();
        self.view.modal = Modal::None;
        self.view.busy = false;
        self.confirmation = None;
        self.list_add_author = None;
        self.media_requested = None;
        self.navigate(Feed::Home);
        let p = self.provider.clone();
        self.spawn(Job::Background, async move {
            let me = p.me().await?;
            Ok(Data::Identity(me, p.capabilities()))
        });
    }
    /// Deterministic offline initialization for screenshots and terminal render tests.
    pub async fn prepare_snapshot(&mut self) -> Result<()> {
        self.view.me = self.provider.me().await?;
        self.view.capabilities = self.provider.capabilities();
        let data = fetch_feed(self.provider.clone(), &self.view.feed, None).await?;
        self.view.posts = data.posts;
        self.view.notifications = data.notifications;
        self.view.connected = true;
        self.view.last_sync = "just now".into();
        self.view.notice = None;
        Ok(())
    }
}

async fn fetch_feed(
    p: Arc<dyn SocialProvider>,
    feed: &Feed,
    cursor: Option<&str>,
) -> Result<FeedData> {
    let mut data = FeedData::default();
    match feed {
        Feed::Notifications => {
            let page = p.notifications(cursor).await?;
            data.notifications = page.items;
            data.next = page.next;
        }
        Feed::Search(q) => {
            let r = p.search(q).await?;
            data.posts = r.posts;
            data.people = r.accounts;
            data.hashtags = r.hashtags;
        }
        Feed::Thread(id) => {
            let (post, context) = tokio::try_join!(p.post(id), p.context(id))?;
            data.posts = context.ancestors;
            data.posts.push(post);
            data.posts.extend(context.descendants);
        }
        Feed::Profile(id) => {
            let (account, rel, page) =
                tokio::try_join!(p.profile(id), p.relationship(id), p.feed(feed, cursor))?;
            data.posts = page.items;
            data.next = page.next;
            data.profile = Some(account);
            data.relationship = rel;
        }
        _ => {
            let page = p.feed(feed, cursor).await?;
            data.posts = page.items;
            data.next = page.next;
        }
    }
    Ok(data)
}
pub fn parse_theme(name: &str) -> Theme {
    match name {
        "midnight" => Theme::Midnight,
        "paper" => Theme::Paper,
        "contrast" => Theme::Contrast,
        _ => Theme::TokyoNight,
    }
}
pub fn theme_key(theme: Theme) -> &'static str {
    match theme {
        Theme::TokyoNight => "tokyo-night",
        Theme::Midnight => "midnight",
        Theme::Paper => "paper",
        Theme::Contrast => "contrast",
    }
}
fn account_labels(config: &Config, active: &str) -> Vec<AccountLabel> {
    let mut accounts = vec![AccountLabel {
        label: "Explore the demo".into(),
        network: "Offline playground".into(),
        active: active == "demo",
    }];
    accounts.extend(config.accounts.iter().map(|a| AccountLabel {
        label: a.label.clone(),
        network: display_instance(&a.instance),
        active: active == a.id,
    }));
    accounts
}
fn display_instance(instance: &str) -> String {
    url::Url::parse(instance)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| instance.to_string())
}
pub fn validate_compose(c: &Compose, caps: &Capabilities) -> Result<()> {
    if !caps.posting {
        bail!("This network does not support publishing");
    }
    if c.text.trim().is_empty() && c.attachments.is_empty() {
        bail!("Write something or attach media before publishing");
    }
    let count = c.text.chars().count() + c.spoiler_text.chars().count();
    if count > caps.max_characters {
        bail!(
            "Post is too long: {count}/{} characters (including content warning)",
            caps.max_characters
        );
    }
    if !c.attachments.is_empty() && !caps.media {
        bail!("This network does not support attachments");
    }
    if c.attachments.len() > 4 {
        bail!("A post can contain at most four attachments");
    }
    if !c.poll_options.is_empty() {
        if !caps.polls {
            bail!("This network does not support polls");
        }
        if !c.attachments.is_empty() {
            bail!("Choose either media attachments or a poll for this post");
        }
        if !(2..=4).contains(&c.poll_options.len()) {
            bail!("Polls need 2–4 options, separated by semicolons");
        }
        if c.poll_options.iter().any(|o| o.chars().count() > 50) {
            bail!("Poll options must be 50 characters or fewer");
        }
        if !(300..=2_629_746).contains(&c.poll_expires_in) {
            bail!("Poll duration must be between 5 minutes and one month");
        }
    }
    if !c.language.is_empty()
        && (c.language.len() > 3 || !c.language.chars().all(|c| c.is_ascii_alphabetic()))
    {
        bail!("Use a language code such as en, de, or ja");
    }
    if c.visibility == Visibility::Direct && !c.text.contains('@') {
        bail!("Mention a recipient for a mentioned-only post");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reject_media_poll_mix_and_overlong_cw() {
        let caps = Capabilities::default();
        let mut c = Compose {
            text: "hello".into(),
            spoiler_text: "x".repeat(500),
            ..Compose::default()
        };
        assert!(validate_compose(&c, &caps).is_err());
        c.spoiler_text.clear();
        c.attachments.push(Attachment {
            path: "a".into(),
            description: "b".into(),
        });
        c.poll_options = vec!["one".into(), "two".into()];
        assert!(validate_compose(&c, &caps).is_err());
    }
    #[test]
    fn reject_blank_and_direct_without_recipient() {
        let caps = Capabilities::default();
        assert!(validate_compose(&Compose::default(), &caps).is_err());
        let c = Compose {
            text: "hello".into(),
            visibility: Visibility::Direct,
            ..Compose::default()
        };
        assert!(validate_compose(&c, &caps).is_err());
    }
    #[test]
    fn tokyo_night_first_and_theme_cycle() {
        let mut t = Theme::default();
        assert_eq!(t, Theme::TokyoNight);
        for _ in 0..4 {
            t = t.next();
        }
        assert_eq!(t, Theme::TokyoNight);
    }
    #[tokio::test]
    async fn demo_keyboard_draft_roundtrip_and_mutation() {
        let root = tempfile::tempdir().unwrap();
        let private_root = root.path().join("private");
        let mut app = App::new(Config::default(), Some(private_root.clone()), true).unwrap();
        app.prepare_snapshot().await.unwrap();
        assert!(!app.view.posts.is_empty());
        app.command("compose");
        app.paste("A draft with Unicode 🌌");
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.view.drafts.len(), 1);
        assert_eq!(
            config::load_drafts(Some(&private_root), "demo").unwrap()[0].text,
            "A draft with Unicode 🌌"
        );
        app.command("drafts");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.view.composer.text, "A draft with Unicode 🌌");
    }
}
