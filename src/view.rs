use crate::file_picker::{Entry as FileEntry, MediaFilter};
use crate::model::*;
use ratatui::layout::Rect;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    TokyoNight,
    Midnight,
    Paper,
    Contrast,
}
impl Theme {
    pub fn next(self) -> Self {
        match self {
            Self::TokyoNight => Self::Midnight,
            Self::Midnight => Self::Paper,
            Self::Paper => Self::Contrast,
            Self::Contrast => Self::TokyoNight,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::TokyoNight => "Tokyo Night",
            Self::Midnight => "Midnight",
            Self::Paper => "Paper",
            Self::Contrast => "High contrast",
        }
    }
}
#[derive(Debug, Clone)]
pub enum Modal {
    None,
    Help,
    Palette {
        query: String,
        selected: usize,
    },
    Input {
        title: String,
        value: String,
        purpose: String,
    },
    Compose,
    FileBrowser,
    Confirm {
        title: String,
        detail: String,
        action: String,
    },
    Accounts {
        selected: usize,
    },
    Lists {
        selected: usize,
    },
    People {
        selected: usize,
    },
    Links {
        urls: Vec<String>,
        selected: usize,
    },
    Poll {
        choices: Vec<usize>,
        selected: usize,
    },
    Drafts {
        selected: usize,
    },
}
#[derive(Debug, Clone)]
pub enum Hit {
    Navigate(Feed),
    Select(usize),
    SelectNotification(usize),
    Command(String),
    ComposerField(usize),
    FileSelect(usize),
    ModalSelect(usize),
}
#[derive(Debug, Clone)]
pub struct Hitbox {
    pub area: Rect,
    pub hit: Hit,
}
#[derive(Debug, Clone)]
pub struct Notice {
    pub message: String,
    pub error: bool,
}
#[derive(Debug, Clone)]
pub struct AccountLabel {
    pub label: String,
    pub network: String,
    pub active: bool,
}
#[derive(Debug, Clone, Default)]
pub struct FileBrowser {
    pub directory: PathBuf,
    pub entries: Vec<FileEntry>,
    pub selected: usize,
    pub offset: usize,
    pub filter: MediaFilter,
    pub show_hidden: bool,
    pub location: String,
    pub editing_location: bool,
    pub cursor: usize,
    pub loading: bool,
    pub error: Option<String>,
    pub omitted: usize,
}
#[derive(Debug, Clone)]
pub struct Command {
    pub id: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
}
pub const COMMANDS: &[Command] = &[
    Command {
        id: "compose",
        label: "Write a post",
        hint: "n",
    },
    Command {
        id: "search",
        label: "Search posts, people & hashtags",
        hint: "/",
    },
    Command {
        id: "home",
        label: "Go to home",
        hint: "1",
    },
    Command {
        id: "local",
        label: "Go to local timeline",
        hint: "2",
    },
    Command {
        id: "federated",
        label: "Go to federated timeline",
        hint: "3",
    },
    Command {
        id: "notifications",
        label: "Open notifications",
        hint: "4",
    },
    Command {
        id: "bookmarks",
        label: "Open bookmarks",
        hint: "5",
    },
    Command {
        id: "favourites",
        label: "Open favorites",
        hint: "6",
    },
    Command {
        id: "refresh",
        label: "Refresh timeline",
        hint: "R",
    },
    Command {
        id: "thread",
        label: "Open conversation",
        hint: "Enter",
    },
    Command {
        id: "reply",
        label: "Reply to selected post",
        hint: "r",
    },
    Command {
        id: "favourite",
        label: "Toggle favorite",
        hint: "f",
    },
    Command {
        id: "boost",
        label: "Toggle boost",
        hint: "b",
    },
    Command {
        id: "bookmark",
        label: "Toggle bookmark",
        hint: "s",
    },
    Command {
        id: "profile",
        label: "View author profile",
        hint: "p",
    },
    Command {
        id: "follow",
        label: "Follow / unfollow author",
        hint: "F",
    },
    Command {
        id: "mute",
        label: "Mute / unmute author",
        hint: "",
    },
    Command {
        id: "block",
        label: "Block / unblock author",
        hint: "",
    },
    Command {
        id: "open",
        label: "Open post in browser",
        hint: "o",
    },
    Command {
        id: "links",
        label: "Open a link from the selected post",
        hint: "L",
    },
    Command {
        id: "media",
        label: "Open attached media",
        hint: "m",
    },
    Command {
        id: "reveal",
        label: "Reveal / hide content warning",
        hint: "x",
    },
    Command {
        id: "vote",
        label: "Vote in poll",
        hint: "v",
    },
    Command {
        id: "edit",
        label: "Edit your selected post",
        hint: "e",
    },
    Command {
        id: "delete",
        label: "Delete your selected post",
        hint: "",
    },
    Command {
        id: "accounts",
        label: "Switch account",
        hint: "a",
    },
    Command {
        id: "lists",
        label: "Browse your lists",
        hint: "l",
    },
    Command {
        id: "create_list",
        label: "Create a list",
        hint: "",
    },
    Command {
        id: "add_to_list",
        label: "Add author to a list",
        hint: "",
    },
    Command {
        id: "people",
        label: "Show people from search",
        hint: "",
    },
    Command {
        id: "tag",
        label: "Browse a hashtag",
        hint: "#",
    },
    Command {
        id: "drafts",
        label: "Open saved drafts",
        hint: "d",
    },
    Command {
        id: "theme",
        label: "Change color theme",
        hint: "T",
    },
    Command {
        id: "motion",
        label: "Toggle reduced motion",
        hint: "",
    },
    Command {
        id: "compact",
        label: "Toggle compact timeline",
        hint: "c",
    },
    Command {
        id: "inspector",
        label: "Toggle detail pane",
        hint: "i",
    },
    Command {
        id: "help",
        label: "Keyboard shortcuts",
        hint: "?",
    },
    Command {
        id: "quit",
        label: "Quit Tootles",
        hint: "q",
    },
];
pub fn commands(query: &str) -> Vec<&'static Command> {
    let q = query.to_lowercase();
    COMMANDS
        .iter()
        .filter(|c| c.label.to_lowercase().contains(&q))
        .collect()
}

/// Shared capability policy for the palette, mouse actions, and keyboard commands.
pub fn command_available(id: &str, caps: &Capabilities) -> bool {
    match id {
        "compose" | "reply" | "send" => caps.posting,
        "edit" => caps.editing && caps.posting,
        "bookmark" | "bookmarks" => caps.bookmarks,
        "local" => caps.local_timeline,
        "federated" => caps.federated_timeline,
        "lists" | "create_list" | "add_to_list" => caps.lists,
        "vote" | "submit_vote" | "poll_multiple" | "poll_duration" => caps.polls,
        "visibility" => caps.post_visibility,
        "sensitive" => caps.content_warnings,
        "attach" | "browse" => caps.media,
        _ => true,
    }
}
pub fn available_commands(query: &str, caps: &Capabilities) -> Vec<&'static Command> {
    commands(query)
        .into_iter()
        .filter(|c| command_available(c.id, caps))
        .collect()
}

pub struct View {
    pub feed: Feed,
    pub posts: Vec<Post>,
    pub notifications: Vec<Notification>,
    pub selected: usize,
    pub offset: usize,
    pub detail_scroll: u16,
    pub detail_area: Option<Rect>,
    pub me: Account,
    pub profile: Option<Account>,
    pub relationship: Relationship,
    pub people: Vec<Account>,
    pub hashtags: Vec<String>,
    pub lists: Vec<SocialList>,
    pub accounts: Vec<AccountLabel>,
    pub drafts: Vec<Compose>,
    pub composer: Compose,
    pub file_browser: FileBrowser,
    /// 0 body, 1 content warning, 2 attachment path, 3 alt text, 4 poll options, 5 language
    pub compose_field: usize,
    pub compose_cursor: usize,
    pub input_cursor: usize,
    pub attachment_path: String,
    pub attachment_alt: String,
    pub poll_input: String,
    pub modal: Modal,
    pub notice: Option<Notice>,
    pub loading: bool,
    pub busy: bool,
    pub demo: bool,
    pub connected: bool,
    pub instance: String,
    pub capabilities: Capabilities,
    pub theme: Theme,
    pub compact: bool,
    pub mouse_enabled: bool,
    pub reduced_motion: bool,
    pub show_inspector: bool,
    pub revealed: std::collections::HashSet<String>,
    pub hits: Vec<Hitbox>,
    pub tick: u64,
    pub last_sync: String,
    pub has_more: bool,
}
impl Default for View {
    fn default() -> Self {
        Self {
            feed: Feed::Home,
            posts: vec![],
            notifications: vec![],
            selected: 0,
            offset: 0,
            detail_scroll: 0,
            detail_area: None,
            me: Account::default(),
            profile: None,
            relationship: Relationship::default(),
            people: vec![],
            hashtags: vec![],
            lists: vec![],
            accounts: vec![],
            drafts: vec![],
            composer: Compose::default(),
            file_browser: FileBrowser::default(),
            compose_field: 0,
            compose_cursor: 0,
            input_cursor: 0,
            attachment_path: String::new(),
            attachment_alt: String::new(),
            poll_input: String::new(),
            modal: Modal::None,
            notice: None,
            loading: false,
            busy: false,
            demo: false,
            connected: false,
            instance: String::new(),
            capabilities: Capabilities::default(),
            theme: Theme::default(),
            compact: false,
            mouse_enabled: true,
            reduced_motion: false,
            show_inspector: true,
            revealed: Default::default(),
            hits: vec![],
            tick: 0,
            last_sync: String::new(),
            has_more: false,
        }
    }
}
impl View {
    pub fn selected_post(&self) -> Option<&Post> {
        if self.feed == Feed::Notifications {
            self.notifications
                .get(self.selected)
                .and_then(|n| n.post.as_ref())
        } else {
            self.posts.get(self.selected)
        }
    }
    pub fn count(&self) -> usize {
        if self.feed == Feed::Notifications {
            self.notifications.len()
        } else {
            self.posts.len()
        }
    }
}
