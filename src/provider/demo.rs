//! A useful offline sandbox: every action changes the same in-memory network.
//! Nothing in this provider opens a socket or reads an attachment from disk.
use std::{
    collections::{HashMap, HashSet},
    sync::{Mutex, MutexGuard},
};

use anyhow::{Result, anyhow, ensure};
use async_trait::async_trait;
use chrono::{Duration, Utc};

use super::SocialProvider;
use crate::model::*;

pub struct DemoProvider {
    state: Mutex<State>,
}
struct State {
    me: Account,
    accounts: Vec<Account>,
    posts: Vec<Post>,
    notifications: Vec<Notification>,
    relationships: HashMap<String, Relationship>,
    lists: Vec<SocialList>,
    members: HashMap<String, HashSet<String>>,
    sources: HashMap<String, Compose>,
    idempotency: HashMap<String, String>,
    serial: u64,
}

impl Default for DemoProvider {
    fn default() -> Self {
        Self::new()
    }
}
impl DemoProvider {
    pub fn new() -> Self {
        let accounts = vec![
            account(
                "you",
                "you",
                "You",
                "tootles.social",
                "A little corner of the open web. This is your offline demo account.",
            ),
            account(
                "maya",
                "maya",
                "Maya Chen",
                "tootles.social",
                "Designing quieter software. Type, texture, and a healthy respect for whitespace. #design",
            ),
            account(
                "sam",
                "samir",
                "Samir Patel",
                "tootles.social",
                "Rust, tiny tools, and walking the long way home. Building in the open. #rust",
            ),
            account(
                "sol",
                "sol",
                "Sol Rivera",
                "fosstodon.org",
                "Linux on the desk, sunlight in the room. Hyprland tinkerer. #linux #hyprland",
            ),
            account(
                "ada",
                "ada",
                "Ada Okafor",
                "tootles.social",
                "Community gardens, community networks. The web belongs to its people. #openweb",
            ),
            account(
                "noor",
                "noor",
                "Noor Williams",
                "hachyderm.io",
                "Accessibility is a design material. I write about interfaces and the people using them.",
            ),
            account(
                "leo",
                "leon",
                "Leon Fischer",
                "tootles.social",
                "Photographs, field notes, slow mornings. Based somewhere near a forest.",
            ),
            account(
                "june",
                "junebug",
                "June Park",
                "social.coop",
                "Cooperative software and good soup. Here to listen.",
            ),
        ];
        let now = Utc::now();
        let samples = [
            (
                "1016",
                1,
                2,
                "The nicest thing about the open web? You can rearrange the furniture.\n\nA small tool, made with care, can feel more like a room than a feed. That's the kind of software I want to build.\n\n#OpenWeb #Design\nhttps://joinmastodon.org/",
            ),
            (
                "1015",
                2,
                5,
                "Today's tiny Rust victory: replaced a spinner with a progress message that actually says what's happening.\n\nThe compiler helps me be precise with code. I'm trying to be just as precise with the people using it. #Rust",
            ),
            (
                "1014",
                3,
                9,
                "A terminal, two workspaces, and a very good cup of coffee.\n\nMy Hyprland setup finally disappears when I'm working. The best configuration is the one you stop thinking about. #Hyprland #Linux",
            ),
            (
                "1013",
                5,
                14,
                "@maya This! A clear focus indicator and a way back are two of the kindest things an interface can offer. Keyboard access is a first-class experience. #Accessibility",
            ),
            (
                "1012",
                7,
                19,
                "@maya And a front door that doesn't require an invitation from a platform. I'd love more software that feels like a shared workshop.",
            ),
            (
                "1011",
                4,
                26,
                "A question for the timeline: what makes a social space feel like home?\n\nThere are no wrong answers. I'm collecting ideas for our local community network. #OpenWeb",
            ),
            (
                "1010",
                6,
                34,
                "A little illustrated escape: a winding golden trail, green foothills, and a moon above the mountains.\n\nLeaving this quiet scene here for anyone who needs a pause. #Illustration",
            ),
            (
                "1009",
                0,
                46,
                "Making a little space for curiosity.\n\nToday I learned that a good terminal interface can be both useful and unhurried. Hello, fediverse! #Introductions #Rust",
            ),
            (
                "1008",
                1,
                63,
                "A field note on interface design:\n\nMake the next action obvious. Make mistakes reversible. Leave enough space for people to breathe.\n\nWhat would you add? #Design",
            ),
            (
                "1007",
                2,
                79,
                "The smallest useful abstraction in my side project is a provider trait: one shared vocabulary, many different networks.\n\nGood boundaries make room for things you haven't imagined yet. #Rust #OpenSource",
            ),
            (
                "1006",
                5,
                95,
                "Reminder from today's usability session: color alone isn't a status message.\n\nPair the dot with a word. Pair the animation with a setting to turn it off. Let the interface meet people where they are. #Accessibility #Design",
            ),
            (
                "1005",
                3,
                113,
                "A satisfying evening: removed six keybindings I never used and wrote down the four I always forget.\n\nMy future self is already grateful. #Hyprland",
            ),
            (
                "1004",
                4,
                142,
                "We planted a community orchard today. Five kinds of apple tree, thirty neighbors, one spectacularly muddy dog.\n\nInfrastructure doesn't always look like a server rack. Sometimes it looks like a place to sit together.",
            ),
            (
                "1003",
                7,
                167,
                "Small reminder: you don't have to keep up with the whole internet.\n\nRead a friend. Follow a thread. Close the tab when you've had enough. The conversations will still be here.",
            ),
            (
                "1002",
                6,
                194,
                "Sketchbook, train ticket, pencil. An afternoon with no deliverables.\n\nI think this is what they mean by a personal project. #CreativeCoding",
            ),
            (
                "1001",
                2,
                238,
                "Weekend reading: the Rust book, a paper on distributed systems, and approximately eighteen open tabs I swear I'll come back to.\n\nWhat are you learning this week? #Rust #Learning",
            ),
        ];
        let mut posts: Vec<Post> = samples
            .into_iter()
            .map(|(id, account, minutes, text)| Post {
                id: id.into(),
                timeline_id: id.into(),
                account: accounts[account].clone(),
                text: text.into(),
                url: format!(
                    "https://{}/@{}/{id}",
                    accounts[account]
                        .acct
                        .split('@')
                        .next_back()
                        .unwrap_or("tootles.social"),
                    accounts[account].username
                ),
                created_at: (now - Duration::minutes(minutes)).to_rfc3339(),
                favourites_count: (minutes as u64 / 3) + 2,
                reblogs_count: minutes as u64 / 11,
                replies_count: if id == "1008" { 2 } else { 0 },
                ..Post::default()
            })
            .collect();
        for id in ["1013", "1012"] {
            let post = posts.iter_mut().find(|p| p.id == id).unwrap();
            post.in_reply_to_id = Some("1008".into());
            post.mentions = vec!["maya@tootles.social".into()];
        }
        posts[2].media.push(Media {
            id: "media-desktop".into(), kind: "image".into(), url: "demo://desktop".into(),
            description: "Screenshot of Tootles in Tokyo Night: navigation on the left, a home timeline in the center, and the selected post in a detail pane on the right.".into(),
        });
        posts[5].poll = Some(Poll {
            id: "poll-home".into(),
            multiple: false,
            options: [
                ("Familiar people", 48),
                ("Room to be curious", 36),
                ("Thoughtful moderation", 29),
                ("A little of everything", 67),
            ]
            .into_iter()
            .map(|(title, votes_count)| PollOption {
                title: title.into(),
                votes_count,
            })
            .collect(),
            votes_count: 180,
            expires_at: Some((now + Duration::hours(18)).to_rfc3339()),
            ..Poll::default()
        });
        posts[6].media.push(Media {
            id: "media-trail".into(), kind: "image".into(), url: "demo://trail".into(),
            description: "An illustration of a golden trail winding across green foothills beneath blue mountains and a large amber moon, with dark evergreen trees in the foreground.".into(),
        });
        posts[10].boosted_by = Some("Maya Chen".into());
        posts[10].timeline_id = "boost-1006".into();
        posts[12].spoiler_text = "A tiny story about loss and new beginnings".into();
        posts[12].text.push_str("\n\nThe first tree is for a neighbor we lost this spring. Her granddaughter chose the spot. I hope she gets to eat the first apple.");
        posts[0].bookmarked = true;
        posts[7].bookmarked = true;
        posts[7].replies_count = 1;
        posts[8].favourited = true;
        posts[9].bookmarked = true;
        posts[10].favourited = true;
        let notifications = vec![
            Notification { id: "n6".into(), kind: "mention".into(), account: accounts[1].clone(), post: Some(Post { id:"mention-1".into(), timeline_id:"mention-1".into(), account: accounts[1].clone(), text:"@you Welcome! Glad to see another curious person in this corner of the web. What are you building?".into(), mentions:vec![accounts[0].acct.clone()], in_reply_to_id:Some("1009".into()), created_at:(now - Duration::minutes(3)).to_rfc3339(), ..Post::default() }), created_at:(now - Duration::minutes(3)).to_rfc3339() },
            Notification { id: "n5".into(), kind: "favourite".into(), account: accounts[2].clone(), post: Some(posts[7].clone()), created_at:(now - Duration::minutes(8)).to_rfc3339() },
            Notification { id: "n4".into(), kind: "follow".into(), account: accounts[5].clone(), post: None, created_at:(now - Duration::minutes(15)).to_rfc3339() },
            Notification { id: "n3".into(), kind: "reblog".into(), account: accounts[4].clone(), post: Some(posts[7].clone()), created_at:(now - Duration::minutes(23)).to_rfc3339() },
            Notification { id: "n2".into(), kind: "favourite".into(), account: accounts[7].clone(), post: Some(posts[7].clone()), created_at:(now - Duration::minutes(38)).to_rfc3339() },
            Notification { id: "n1".into(), kind: "follow".into(), account: accounts[6].clone(), post: None, created_at:(now - Duration::hours(2)).to_rfc3339() },
        ];
        if let Some(post) = &notifications[0].post {
            posts.push(post.clone());
        }
        posts.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        let relationships = accounts
            .iter()
            .skip(1)
            .map(|a| {
                (
                    a.id.clone(),
                    Relationship {
                        following: a.id != "june",
                        followed_by: a.id != "sol",
                        ..Relationship::default()
                    },
                )
            })
            .collect();
        let lists = vec![
            SocialList {
                id: "design".into(),
                title: "Design & craft".into(),
            },
            SocialList {
                id: "builders".into(),
                title: "Builders".into(),
            },
        ];
        let members = HashMap::from([
            (
                "design".into(),
                HashSet::from(["maya".into(), "noor".into(), "leo".into()]),
            ),
            (
                "builders".into(),
                HashSet::from(["sam".into(), "sol".into(), "ada".into()]),
            ),
        ]);
        let me = accounts[0].clone();
        Self {
            state: Mutex::new(State {
                me,
                accounts,
                posts,
                notifications,
                relationships,
                lists,
                members,
                sources: HashMap::new(),
                idempotency: HashMap::new(),
                serial: 2000,
            }),
        }
    }

    fn state(&self) -> Result<MutexGuard<'_, State>> {
        self.state
            .lock()
            .map_err(|_| anyhow!("The demo state is unavailable; restart the demo"))
    }
}

fn account(id: &str, username: &str, display_name: &str, domain: &str, note: &str) -> Account {
    Account {
        id: id.into(),
        username: username.into(),
        display_name: display_name.into(),
        acct: format!("{username}@{domain}"),
        note: note.into(),
        url: format!("https://{domain}/@{username}"),
        followers_count: Some(128),
        following_count: Some(86),
        statuses_count: Some(247),
        locked: false,
    }
}
fn page<T: Clone>(items: Vec<T>, cursor: Option<&str>, id: impl Fn(&T) -> &str) -> Page<T> {
    let start = cursor
        .and_then(|c| items.iter().position(|item| id(item) == c))
        .map_or(0, |i| i + 1);
    let end = (start + 8).min(items.len());
    let next = if end < items.len() {
        items.get(end - 1).map(|item| id(item).to_owned())
    } else {
        None
    };
    Page {
        items: items[start..end].to_vec(),
        next,
    }
}
fn post_source(post: &Post) -> Compose {
    Compose {
        text: post.text.clone(),
        spoiler_text: post.spoiler_text.clone(),
        sensitive: post.sensitive,
        visibility: post.visibility,
        reply_to: post.in_reply_to_id.clone(),
        edit_id: Some(post.id.clone()),
        language: "en".into(),
        idempotency_key: uuid::Uuid::new_v4().to_string(),
        ..Compose::default()
    }
}
fn refresh_notifications(state: &mut State) {
    for notification in &mut state.notifications {
        if let Some(post) = &notification.post {
            notification.post = state.posts.iter().find(|p| p.id == post.id).cloned();
        }
    }
}

#[async_trait]
impl SocialProvider for DemoProvider {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            name: "Demo",
            ..Capabilities::default()
        }
    }
    async fn me(&self) -> Result<Account> {
        Ok(self.state()?.me.clone())
    }

    async fn feed(&self, feed: &Feed, cursor: Option<&str>) -> Result<Page<Post>> {
        if let Feed::Thread(id) = feed {
            let thread = self.context(id).await?;
            let mut items = thread.ancestors;
            items.push(self.post(id).await?);
            items.extend(thread.descendants);
            return Ok(Page { items, next: None });
        }
        let state = self.state()?;
        let items = state
            .posts
            .iter()
            .filter(|p| {
                let relationship = state.relationships.get(&p.account.id);
                if relationship.is_some_and(|r| r.muting || r.blocking) {
                    return false;
                }
                match feed {
                    Feed::Home => {
                        p.account.id == state.me.id || relationship.is_some_and(|r| r.following)
                    }
                    Feed::Local => {
                        p.account.acct.ends_with("@tootles.social")
                            && p.visibility == Visibility::Public
                    }
                    Feed::Federated => p.visibility == Visibility::Public,
                    Feed::Bookmarks => p.bookmarked,
                    Feed::Favourites => p.favourited,
                    Feed::Profile(id) => p.account.id == *id,
                    Feed::Tag(tag) => p
                        .text
                        .split_whitespace()
                        .filter_map(|word| word.strip_prefix('#'))
                        .any(|word| {
                            word.trim_end_matches(|c: char| !c.is_alphanumeric())
                                .eq_ignore_ascii_case(tag.trim_start_matches('#'))
                        }),
                    Feed::List(id, _) => state
                        .members
                        .get(id)
                        .is_some_and(|members| members.contains(&p.account.id)),
                    Feed::Search(query) => {
                        p.text.to_lowercase().contains(&query.to_lowercase())
                            || p.account
                                .acct
                                .to_lowercase()
                                .contains(&query.to_lowercase())
                    }
                    Feed::Notifications | Feed::Thread(_) => false,
                }
            })
            .cloned()
            .collect();
        Ok(page(items, cursor, |p| &p.timeline_id))
    }

    async fn notifications(&self, cursor: Option<&str>) -> Result<Page<Notification>> {
        let state = self.state()?;
        let items = state
            .notifications
            .iter()
            .filter(|n| {
                !state
                    .relationships
                    .get(&n.account.id)
                    .is_some_and(|r| r.muting || r.blocking)
            })
            .cloned()
            .collect();
        Ok(page(items, cursor, |n| &n.id))
    }

    async fn search(&self, query: &str) -> Result<SearchResults> {
        let state = self.state()?;
        let needle = query.trim_start_matches(['@', '#']).to_lowercase();
        let posts = state
            .posts
            .iter()
            .filter(|p| {
                p.text.to_lowercase().contains(&needle)
                    && !state
                        .relationships
                        .get(&p.account.id)
                        .is_some_and(|r| r.blocking || r.muting)
            })
            .cloned()
            .collect();
        let accounts = state
            .accounts
            .iter()
            .filter(|a| {
                a.acct.to_lowercase().contains(&needle)
                    || a.display_name.to_lowercase().contains(&needle)
            })
            .cloned()
            .collect();
        let mut tags: Vec<String> = state
            .posts
            .iter()
            .flat_map(|p| p.text.split_whitespace())
            .filter_map(|word| word.strip_prefix('#'))
            .map(|word| {
                word.trim_end_matches(|c: char| !c.is_alphanumeric())
                    .to_owned()
            })
            .filter(|tag| tag.to_lowercase().contains(&needle))
            .collect();
        tags.sort();
        tags.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        Ok(SearchResults {
            posts,
            accounts,
            hashtags: tags,
        })
    }

    async fn context(&self, id: &str) -> Result<Thread> {
        let state = self.state()?;
        let original = state
            .posts
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| anyhow!("This demo post was removed"))?;
        let mut ancestors = Vec::new();
        let mut parent = original.in_reply_to_id.clone();
        let mut visited = HashSet::from([id.to_owned()]);
        while let Some(parent_id) = parent {
            if !visited.insert(parent_id.clone()) {
                break;
            }
            if let Some(post) = state.posts.iter().find(|p| p.id == parent_id) {
                ancestors.push(post.clone());
                parent = post.in_reply_to_id.clone();
            } else {
                break;
            }
        }
        ancestors.reverse();
        let mut descendants = Vec::new();
        let mut parents = vec![id.to_owned()];
        while !parents.is_empty() {
            let level: Vec<Post> = state
                .posts
                .iter()
                .filter(|p| {
                    p.in_reply_to_id
                        .as_ref()
                        .is_some_and(|parent| parents.contains(parent))
                        && !visited.contains(&p.id)
                })
                .cloned()
                .collect();
            parents.clear();
            for post in level {
                visited.insert(post.id.clone());
                parents.push(post.id.clone());
                descendants.push(post);
            }
        }
        Ok(Thread {
            ancestors,
            descendants,
        })
    }

    async fn post(&self, id: &str) -> Result<Post> {
        self.state()?
            .posts
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or_else(|| anyhow!("This demo post was removed"))
    }
    async fn profile(&self, id: &str) -> Result<Account> {
        self.state()?
            .accounts
            .iter()
            .find(|a| a.id == id)
            .cloned()
            .ok_or_else(|| anyhow!("This demo account does not exist"))
    }
    async fn relationship(&self, id: &str) -> Result<Relationship> {
        Ok(self
            .state()?
            .relationships
            .get(id)
            .cloned()
            .unwrap_or_default())
    }

    async fn post_action(&self, id: &str, action: PostAction) -> Result<Option<Post>> {
        let mut state = self.state()?;
        let index = state
            .posts
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| anyhow!("This demo post was removed"))?;
        if matches!(action, PostAction::Delete) {
            ensure!(
                state.posts[index].account.id == state.me.id,
                "You can only delete your own posts"
            );
            let removed = state.posts.remove(index);
            if let Some(parent) = &removed.in_reply_to_id
                && let Some(post) = state.posts.iter_mut().find(|p| p.id == *parent)
            {
                post.replies_count = post.replies_count.saturating_sub(1);
            }
            state.sources.remove(id);
            refresh_notifications(&mut state);
            return Ok(None);
        }
        let post = &mut state.posts[index];
        match action {
            PostAction::Favourite(value) => {
                if value != post.favourited {
                    post.favourites_count = if value {
                        post.favourites_count + 1
                    } else {
                        post.favourites_count.saturating_sub(1)
                    };
                }
                post.favourited = value;
            }
            PostAction::Boost(value) => {
                ensure!(
                    matches!(post.visibility, Visibility::Public | Visibility::Unlisted),
                    "Only public and unlisted posts can be boosted"
                );
                if value != post.reblogged {
                    post.reblogs_count = if value {
                        post.reblogs_count + 1
                    } else {
                        post.reblogs_count.saturating_sub(1)
                    };
                }
                post.reblogged = value;
            }
            PostAction::Bookmark(value) => post.bookmarked = value,
            PostAction::Delete => unreachable!(),
        }
        let result = post.clone();
        refresh_notifications(&mut state);
        Ok(Some(result))
    }

    async fn account_action(&self, id: &str, action: AccountAction) -> Result<Relationship> {
        let mut state = self.state()?;
        ensure!(
            state.accounts.iter().any(|a| a.id == id),
            "This demo account does not exist"
        );
        ensure!(
            state.me.id != id,
            "You cannot perform this action on your own account"
        );
        let relationship = state.relationships.entry(id.to_owned()).or_default();
        match action {
            AccountAction::Follow(value) => {
                ensure!(
                    !relationship.blocking || !value,
                    "Unblock this account before following"
                );
                relationship.following = value;
                relationship.requested = false;
            }
            AccountAction::Mute(value) => relationship.muting = value,
            AccountAction::Block(value) => {
                relationship.blocking = value;
                if value {
                    relationship.following = false;
                    relationship.followed_by = false;
                }
            }
        }
        Ok(relationship.clone())
    }

    async fn publish(&self, compose: &Compose) -> Result<Post> {
        ensure!(
            !compose.text.trim().is_empty()
                || !compose.attachments.is_empty()
                || compose.edit_id.is_some(),
            "Write a post or attach media first"
        );
        ensure!(
            compose.text.chars().count() + compose.spoiler_text.chars().count() <= 500,
            "This demo accepts up to 500 characters including the content warning"
        );
        ensure!(
            compose.attachments.len() <= 4,
            "Attach at most four media files"
        );
        ensure!(
            compose.poll_options.is_empty() || compose.attachments.is_empty(),
            "A post can have media or a poll, but not both"
        );
        ensure!(
            compose.poll_options.is_empty()
                || ((2..=4).contains(&compose.poll_options.len())
                    && compose.poll_options.iter().all(|o| !o.trim().is_empty())),
            "Polls need two to four nonempty options"
        );
        ensure!(
            compose.poll_options.is_empty() || (300..=2_592_000).contains(&compose.poll_expires_in),
            "Poll duration must be between five minutes and thirty days"
        );
        let mut state = self.state()?;
        if compose.edit_id.is_none()
            && let Some(id) = state.idempotency.get(&compose.idempotency_key)
        {
            return state
                .posts
                .iter()
                .find(|p| p.id == *id)
                .cloned()
                .ok_or_else(|| {
                    anyhow!("This post was already published and deleted; start a new draft")
                });
        }
        let now = Utc::now();
        let mut post = if let Some(id) = &compose.edit_id {
            let post = state
                .posts
                .iter()
                .find(|p| p.id == *id)
                .cloned()
                .ok_or_else(|| anyhow!("This demo post was removed"))?;
            ensure!(
                post.account.id == state.me.id,
                "You can only edit your own posts"
            );
            post
        } else {
            ensure!(
                !compose.idempotency_key.is_empty(),
                "A stable idempotency key is required to publish safely"
            );
            state.serial += 1;
            let id = state.serial.to_string();
            Post {
                id: id.clone(),
                timeline_id: id.clone(),
                account: state.me.clone(),
                url: format!("https://tootles.social/@you/{id}"),
                created_at: now.to_rfc3339(),
                ..Post::default()
            }
        };
        post.text = compose.text.clone();
        post.spoiler_text = compose.spoiler_text.clone();
        post.visibility = compose.visibility;
        post.sensitive = compose.sensitive;
        if compose.edit_id.is_none() {
            post.in_reply_to_id = compose.reply_to.clone();
        }
        ensure!(
            post.media.len() + compose.attachments.len() <= 4,
            "This edit would exceed four media attachments"
        );
        ensure!(
            compose.poll_options.is_empty() || post.media.is_empty(),
            "Remove existing media before adding a poll"
        );
        ensure!(
            compose.attachments.is_empty() || post.poll.is_none(),
            "A post with a poll cannot also have media"
        );
        for (index, attachment) in compose.attachments.iter().enumerate() {
            post.media.push(Media {
                id: format!("{}-media-{index}", post.id),
                kind: "image".into(),
                url: String::new(),
                description: attachment.description.clone(),
            });
        }
        if !compose.poll_options.is_empty() {
            post.poll = Some(Poll {
                id: format!("poll-{}", post.id),
                options: compose
                    .poll_options
                    .iter()
                    .map(|title| PollOption {
                        title: title.clone(),
                        votes_count: 0,
                    })
                    .collect(),
                multiple: compose.poll_multiple,
                expires_at: Some(
                    (now + Duration::seconds(compose.poll_expires_in.min(i64::MAX as u64) as i64))
                        .to_rfc3339(),
                ),
                ..Poll::default()
            });
        }
        if let Some(id) = &compose.edit_id {
            post.edited_at = Some(now.to_rfc3339());
            *state.posts.iter_mut().find(|p| p.id == *id).unwrap() = post.clone();
        } else {
            if let Some(parent) = &compose.reply_to {
                let target = state
                    .posts
                    .iter_mut()
                    .find(|p| p.id == *parent)
                    .ok_or_else(|| anyhow!("The post you are replying to was removed"))?;
                target.replies_count += 1;
            }
            state.posts.insert(0, post.clone());
            state
                .idempotency
                .insert(compose.idempotency_key.clone(), post.id.clone());
        }
        let mut source = compose.clone();
        source.edit_id = Some(post.id.clone());
        source.attachments.clear();
        source.poll_options.clear();
        state.sources.insert(post.id.clone(), source);
        refresh_notifications(&mut state);
        Ok(post)
    }

    async fn source(&self, id: &str) -> Result<Compose> {
        let state = self.state()?;
        let post = state
            .posts
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| anyhow!("This demo post was removed"))?;
        ensure!(
            post.account.id == state.me.id,
            "You can only edit your own posts"
        );
        Ok(state
            .sources
            .get(id)
            .cloned()
            .unwrap_or_else(|| post_source(post)))
    }

    async fn vote(&self, id: &str, choices: &[usize]) -> Result<Poll> {
        let mut state = self.state()?;
        let poll = state
            .posts
            .iter_mut()
            .filter_map(|p| p.poll.as_mut())
            .find(|p| p.id == id)
            .ok_or_else(|| anyhow!("This demo poll does not exist"))?;
        ensure!(
            !poll.expired && !poll.voted,
            "This poll has ended or you already voted"
        );
        ensure!(
            !choices.is_empty() && (poll.multiple || choices.len() == 1),
            "Select a valid number of poll options"
        );
        ensure!(
            choices.iter().all(|i| *i < poll.options.len()),
            "A selected option does not exist"
        );
        ensure!(
            choices.iter().collect::<HashSet<_>>().len() == choices.len(),
            "Each poll option can be selected only once"
        );
        for choice in choices {
            poll.options[*choice].votes_count += 1;
        }
        poll.votes_count += choices.len() as u64;
        poll.voted = true;
        poll.own_votes = choices.to_vec();
        Ok(poll.clone())
    }
    async fn lists(&self) -> Result<Vec<SocialList>> {
        Ok(self.state()?.lists.clone())
    }
    async fn create_list(&self, title: &str) -> Result<SocialList> {
        ensure!(!title.trim().is_empty(), "Give your list a name");
        let mut state = self.state()?;
        state.serial += 1;
        let list = SocialList {
            id: format!("list-{}", state.serial),
            title: title.trim().into(),
        };
        state.lists.push(list.clone());
        state.members.insert(list.id.clone(), HashSet::new());
        Ok(list)
    }
    async fn list_member(&self, list: &str, account: &str, add: bool) -> Result<()> {
        let mut state = self.state()?;
        ensure!(
            state.accounts.iter().any(|a| a.id == account),
            "This demo account does not exist"
        );
        ensure!(
            !add || state
                .relationships
                .get(account)
                .is_some_and(|r| r.following),
            "Follow this account before adding it to a list"
        );
        let members = state
            .members
            .get_mut(list)
            .ok_or_else(|| anyhow!("This demo list does not exist"))?;
        if add {
            members.insert(account.to_owned());
        } else {
            members.remove(account);
        }
        Ok(())
    }
}
