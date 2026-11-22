use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    pub id: String,
    pub username: String,
    pub acct: String,
    pub display_name: String,
    pub note: String,
    pub url: String,
    pub followers_count: Option<u64>,
    pub following_count: Option<u64>,
    pub statuses_count: Option<u64>,
    pub locked: bool,
}
impl Account {
    pub fn name(&self) -> &str {
        if self.display_name.is_empty() {
            &self.username
        } else {
            &self.display_name
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Media {
    pub id: String,
    pub kind: String,
    pub url: String,
    pub description: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PollOption {
    pub title: String,
    pub votes_count: u64,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Poll {
    pub id: String,
    pub options: Vec<PollOption>,
    pub expires_at: Option<String>,
    pub expired: bool,
    pub multiple: bool,
    pub voted: bool,
    pub own_votes: Vec<usize>,
    pub votes_count: u64,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Post {
    pub id: String,
    /// The outer timeline ID (different from id for boosts), used for pagination.
    pub timeline_id: String,
    pub account: Account,
    pub text: String,
    pub url: String,
    pub created_at: String,
    pub spoiler_text: String,
    pub sensitive: bool,
    pub visibility: Visibility,
    pub favourites_count: u64,
    pub reblogs_count: u64,
    pub replies_count: u64,
    pub favourited: bool,
    pub reblogged: bool,
    pub bookmarked: bool,
    pub boosted_by: Option<String>,
    pub in_reply_to_id: Option<String>,
    pub media: Vec<Media>,
    pub poll: Option<Poll>,
    pub mentions: Vec<String>,
    pub edited_at: Option<String>,
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    #[default]
    Public,
    Unlisted,
    Private,
    Direct,
}
impl Visibility {
    pub fn label(self) -> &'static str {
        match self {
            Self::Public => "Public",
            Self::Unlisted => "Unlisted",
            Self::Private => "Followers",
            Self::Direct => "Mentioned only",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::Public => Self::Unlisted,
            Self::Unlisted => Self::Private,
            Self::Private => Self::Direct,
            Self::Direct => Self::Public,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Unlisted => "unlisted",
            Self::Private => "private",
            Self::Direct => "direct",
        }
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    pub kind: String,
    pub account: Account,
    pub post: Option<Post>,
    pub created_at: String,
}
#[derive(Debug, Clone, Default)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Feed {
    #[default]
    Home,
    Local,
    Federated,
    Notifications,
    Bookmarks,
    Favourites,
    Search(String),
    Profile(String),
    Thread(String),
    List(String, String),
    Tag(String),
}
impl Feed {
    pub fn title(&self) -> String {
        match self {
            Self::Home => "Home".into(),
            Self::Local => "Local".into(),
            Self::Federated => "Federated".into(),
            Self::Notifications => "Notifications".into(),
            Self::Bookmarks => "Bookmarks".into(),
            Self::Favourites => "Favorites".into(),
            Self::Search(q) => format!("Search · {q}"),
            Self::Profile(_) => "Profile".into(),
            Self::Thread(_) => "Conversation".into(),
            Self::List(_, n) => n.clone(),
            Self::Tag(t) => format!("#{t}"),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct SearchResults {
    pub posts: Vec<Post>,
    pub accounts: Vec<Account>,
    pub hashtags: Vec<String>,
}
#[derive(Debug, Clone, Default)]
pub struct Thread {
    pub ancestors: Vec<Post>,
    pub descendants: Vec<Post>,
}
#[derive(Debug, Clone, Default)]
pub struct Relationship {
    pub following: bool,
    pub followed_by: bool,
    pub requested: bool,
    pub muting: bool,
    pub blocking: bool,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SocialList {
    pub id: String,
    pub title: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub path: String,
    pub description: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Compose {
    pub text: String,
    pub spoiler_text: String,
    pub visibility: Visibility,
    pub sensitive: bool,
    pub reply_to: Option<String>,
    pub edit_id: Option<String>,
    pub attachments: Vec<Attachment>,
    pub poll_options: Vec<String>,
    pub poll_multiple: bool,
    pub poll_expires_in: u64,
    pub language: String,
    pub idempotency_key: String,
}
#[derive(Debug, Clone, Copy)]
pub enum PostAction {
    Favourite(bool),
    Boost(bool),
    Bookmark(bool),
    Delete,
}
#[derive(Debug, Clone, Copy)]
pub enum AccountAction {
    Follow(bool),
    Mute(bool),
    Block(bool),
}
#[derive(Debug, Clone)]
pub struct Capabilities {
    pub name: &'static str,
    pub posting: bool,
    pub polls: bool,
    pub media: bool,
    pub lists: bool,
    pub max_characters: usize,
}
impl Default for Capabilities {
    fn default() -> Self {
        Self {
            name: "Mastodon",
            posting: true,
            polls: true,
            media: true,
            lists: true,
            max_characters: 500,
        }
    }
}
