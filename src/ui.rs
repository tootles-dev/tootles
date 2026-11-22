//! Responsive, terminal-native presentation. Every pointer target carries the
//! same semantic action used by the keyboard; no account state lives here.
use crate::{model::*, view::*};
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy)]
struct Palette {
    bg: Color,
    panel: Color,
    raised: Color,
    selected: Color,
    fg: Color,
    soft: Color,
    muted: Color,
    line: Color,
    accent: Color,
    green: Color,
    coral: Color,
    purple: Color,
}
impl Palette {
    fn for_theme(theme: Theme) -> Self {
        let rgb = |r, g, b| Color::Rgb(r, g, b);
        match theme {
            Theme::TokyoNight => Self {
                bg: rgb(26, 27, 38),
                panel: rgb(31, 33, 48),
                raised: rgb(36, 40, 59),
                selected: rgb(41, 48, 71),
                fg: rgb(192, 202, 245),
                soft: rgb(169, 177, 214),
                muted: rgb(86, 95, 137),
                line: rgb(54, 62, 89),
                accent: rgb(122, 162, 247),
                green: rgb(158, 206, 106),
                coral: rgb(247, 118, 142),
                purple: rgb(187, 154, 247),
            },
            Theme::Midnight => Self {
                bg: rgb(14, 23, 28),
                panel: rgb(20, 32, 38),
                raised: rgb(26, 40, 47),
                selected: rgb(28, 53, 57),
                fg: rgb(225, 235, 233),
                soft: rgb(162, 184, 186),
                muted: rgb(101, 130, 137),
                line: rgb(45, 66, 73),
                accent: rgb(133, 220, 194),
                green: rgb(160, 209, 154),
                coral: rgb(242, 159, 143),
                purple: rgb(184, 169, 219),
            },
            Theme::Paper => Self {
                bg: rgb(243, 240, 233),
                panel: rgb(234, 231, 222),
                raised: rgb(250, 248, 242),
                selected: rgb(222, 234, 225),
                fg: rgb(42, 53, 56),
                soft: rgb(75, 92, 96),
                muted: rgb(109, 120, 118),
                line: rgb(196, 203, 193),
                accent: rgb(25, 112, 101),
                green: rgb(57, 119, 66),
                coral: rgb(161, 62, 65),
                purple: rgb(111, 75, 145),
            },
            Theme::Contrast => Self {
                bg: Color::Black,
                panel: rgb(12, 12, 12),
                raised: rgb(24, 24, 24),
                selected: rgb(19, 42, 57),
                fg: Color::White,
                soft: rgb(220, 220, 220),
                muted: rgb(172, 172, 172),
                line: rgb(142, 142, 142),
                accent: rgb(100, 222, 255),
                green: rgb(151, 255, 148),
                coral: rgb(255, 151, 163),
                purple: rgb(226, 176, 255),
            },
        }
    }
    fn text(self) -> Style {
        Style::default().fg(self.fg)
    }
    fn muted(self) -> Style {
        Style::default().fg(self.muted)
    }
    fn accent(self) -> Style {
        Style::default().fg(self.accent)
    }
    fn bold(self) -> Style {
        self.text().add_modifier(Modifier::BOLD)
    }
}

pub fn render(frame: &mut Frame, view: &mut View) {
    let p = Palette::for_theme(view.theme);
    let area = frame.area();
    view.hits.clear();
    view.detail_area = None;
    frame.render_widget(
        Block::default().style(Style::default().bg(p.bg).fg(p.fg)),
        area,
    );
    if area.width == 0 || area.height == 0 {
        return;
    }
    let header_h = if area.height >= 12 { 3 } else { 1 };
    let footer_h = if area.height >= 12 { 2 } else { 1 };
    header(
        frame,
        view,
        Rect::new(area.x, area.y, area.width, header_h.min(area.height)),
        p,
    );
    let body = Rect::new(
        area.x,
        area.y.saturating_add(header_h),
        area.width,
        area.height.saturating_sub(header_h + footer_h),
    );
    if area.width >= 80 && body.height > 0 {
        let side_width = if area.width >= 115 { 23 } else { 20 };
        sidebar(
            frame,
            view,
            Rect::new(body.x, body.y, side_width, body.height),
            p,
        );
        let content = Rect::new(
            body.x + side_width + 1,
            body.y,
            body.width.saturating_sub(side_width + 2),
            body.height,
        );
        if area.width >= 115 && view.show_inspector {
            let detail_width = ((area.width as f32 * 0.28) as u16).clamp(31, 47);
            let feed_width = content.width.saturating_sub(detail_width + 2);
            timeline(
                frame,
                view,
                Rect::new(content.x, content.y, feed_width, content.height),
                p,
            );
            inspector(
                frame,
                view,
                Rect::new(
                    content.x + feed_width + 2,
                    content.y,
                    detail_width,
                    content.height,
                ),
                p,
            );
        } else {
            timeline(frame, view, content, p);
        }
    } else if body.height > 0 {
        let content = Rect::new(
            body.x + u16::from(body.width > 2),
            body.y,
            body.width
                .saturating_sub(if body.width > 2 { 2 } else { 0 }),
            body.height,
        );
        timeline(frame, view, content, p);
    }
    footer(
        frame,
        view,
        Rect::new(
            area.x,
            area.bottom().saturating_sub(footer_h),
            area.width,
            footer_h.min(area.height),
        ),
        p,
    );
    if !matches!(view.modal, Modal::None) {
        view.hits.clear();
        modal(frame, view, p);
    }
}

fn text<'a>(frame: &mut Frame, area: Rect, content: impl Into<Line<'a>>, style: Style) {
    if area.width > 0 && area.height > 0 {
        frame.render_widget(Paragraph::new(content.into()).style(style), area);
    }
}
fn row(area: Rect, y: u16) -> Rect {
    Rect::new(
        area.x,
        area.y.saturating_add(y),
        area.width,
        u16::from(y < area.height),
    )
}
fn inset(area: Rect, x: u16, y: u16) -> Rect {
    Rect::new(
        area.x.saturating_add(x.min(area.width)),
        area.y.saturating_add(y.min(area.height)),
        area.width.saturating_sub(x.saturating_mul(2)),
        area.height.saturating_sub(y.saturating_mul(2)),
    )
}
fn hit(view: &mut View, area: Rect, action: Hit) {
    if area.width > 0 && area.height > 0 {
        view.hits.push(Hitbox { area, hit: action });
    }
}
fn command(view: &mut View, area: Rect, id: &str) {
    hit(view, area, Hit::Command(id.into()));
}
fn chip(
    frame: &mut Frame,
    view: &mut View,
    area: Rect,
    label: &str,
    id: &str,
    active: bool,
    p: Palette,
) {
    let style = if active {
        Style::default()
            .fg(p.bg)
            .bg(p.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.soft).bg(p.raised)
    };
    frame.render_widget(
        Paragraph::new(label.to_owned())
            .alignment(Alignment::Center)
            .style(style),
        area,
    );
    command(view, area, id);
}
fn ellipsis(value: &str, width: usize) -> String {
    if value.width() <= width {
        return value.into();
    }
    if width == 0 {
        return String::new();
    }
    let mut s = String::new();
    for g in value.graphemes(true) {
        if s.width() + g.width() >= width {
            break;
        }
        s.push_str(g);
    }
    s.push('…');
    s
}
fn initials(name: &str) -> String {
    let value = name
        .split_whitespace()
        .take(2)
        .filter_map(|s| s.graphemes(true).next())
        .collect::<String>();
    if value.is_empty() {
        "?".into()
    } else {
        value.to_uppercase()
    }
}
fn handle(account: &Account) -> String {
    let acct = if account.acct.is_empty() {
        &account.username
    } else {
        &account.acct
    };
    if acct.is_empty() {
        "No account connected".into()
    } else {
        format!("@{acct}")
    }
}
fn short_date(value: &str) -> String {
    if let Ok(date) = chrono::DateTime::parse_from_rfc3339(value) {
        let seconds = chrono::Utc::now().signed_duration_since(date).num_seconds();
        return if seconds < 60 {
            "now".into()
        } else if seconds < 3600 {
            format!("{}m", seconds / 60)
        } else if seconds < 86400 {
            format!("{}h", seconds / 3600)
        } else if seconds < 604800 {
            format!("{}d", seconds / 86400)
        } else {
            date.format("%b %d").to_string()
        };
    }
    value.chars().take(16).collect()
}
fn hidden(post: &Post, view: &View) -> bool {
    !post.spoiler_text.is_empty() && !view.revealed.contains(&post.id)
}
fn hidden_media(post: &Post, view: &View) -> bool {
    hidden(post, view) || (post.sensitive && !view.revealed.contains(&post.id))
}

fn header(frame: &mut Frame, view: &mut View, area: Rect, p: Palette) {
    frame.render_widget(Block::default().style(Style::default().bg(p.panel)), area);
    let y = area.y + u16::from(area.height > 1);
    let brand_width = if area.width >= 80 { 39 } else { 13 };
    let brand = Line::from(vec![
        Span::styled("  ◉ ", p.accent()),
        Span::styled("tootles", p.bold()),
        Span::styled(
            if area.width >= 80 {
                "   a little more connected"
            } else {
                ""
            },
            p.muted(),
        ),
    ]);
    text(
        frame,
        Rect::new(area.x, y, brand_width.min(area.width), 1),
        brand,
        p.text(),
    );
    let network = if view.demo {
        "DEMO".to_owned()
    } else if view.connected {
        view.capabilities.name.to_uppercase()
    } else {
        "OFFLINE".into()
    };
    let status = if view.busy {
        "working"
    } else if view.loading {
        "syncing"
    } else if view.connected || view.demo {
        "connected"
    } else {
        "connect an account"
    };
    let right = if area.width >= 80 {
        format!("● {network}  ·  {status}  ")
    } else {
        format!("{network}  ")
    };
    let width = (right.width() as u16).min(area.width.saturating_sub(brand_width));
    let r = Rect::new(area.right().saturating_sub(width), y, width, 1);
    text(
        frame,
        r,
        right,
        Style::default().fg(if view.connected || view.demo {
            p.green
        } else {
            p.muted
        }),
    );
    command(view, r, "accounts");
}

fn sidebar(frame: &mut Frame, view: &mut View, area: Rect, p: Palette) {
    let inside = inset(area, 2, 1);
    text(frame, row(inside, 0), "YOUR SPACE", p.muted());
    let nav = [
        ("1", "Home", Feed::Home),
        ("2", "Local", Feed::Local),
        ("3", "Federated", Feed::Federated),
        ("4", "Notifications", Feed::Notifications),
        ("5", "Bookmarks", Feed::Bookmarks),
        ("6", "Favourites", Feed::Favourites),
    ];
    for (index, (key, label, feed)) in nav.into_iter().enumerate() {
        let r = row(inside, 2 + index as u16 * 2);
        let active = view.feed == feed;
        let line = Line::from(vec![
            Span::styled(
                format!(" {key}  "),
                if active { p.accent() } else { p.muted() },
            ),
            Span::styled(
                label,
                if active {
                    p.bold()
                } else {
                    Style::default().fg(p.soft)
                },
            ),
            Span::styled(if active { "  •" } else { "" }, p.accent()),
        ]);
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(if active { p.selected } else { p.bg })),
            r,
        );
        hit(view, r, Hit::Navigate(feed));
    }
    if inside.height > 16 {
        chip(
            frame,
            view,
            row(inside, 15),
            "+  Write a post",
            "compose",
            true,
            p,
        );
    }
    if inside.height > 22 {
        text(frame, row(inside, 18), "EXPLORE", p.muted());
        for (i, (label, id)) in [
            ("/  Search", "search"),
            ("l  Your lists", "lists"),
            ("d  Saved drafts", "drafts"),
        ]
        .into_iter()
        .enumerate()
        {
            let r = row(inside, 20 + i as u16 * 2);
            text(frame, r, label, Style::default().fg(p.soft));
            command(view, r, id);
        }
    }
    if inside.height > 31 {
        let y = inside.height - 6;
        text(
            frame,
            row(inside, y),
            "─".repeat(inside.width as usize),
            Style::default().fg(p.line),
        );
        text(
            frame,
            row(inside, y + 2),
            ellipsis(view.me.name(), inside.width as usize),
            p.bold(),
        );
        text(
            frame,
            row(inside, y + 3),
            ellipsis(&handle(&view.me), inside.width as usize),
            p.muted(),
        );
        let label = format!("{}  ·  a switch", view.capabilities.name);
        text(frame, row(inside, y + 4), label, p.accent());
        hit(
            view,
            Rect::new(inside.x, inside.y + y + 2, inside.width, 3),
            Hit::Command("accounts".into()),
        );
    }
}

fn feed_subtitle(view: &View) -> String {
    match &view.feed {
        Feed::Home => "Your circle, in the moment.".into(),
        Feed::Local => format!(
            "Around {}",
            if view.instance.is_empty() {
                "your community"
            } else {
                &view.instance
            }
        ),
        Feed::Federated => "A wider window into the fediverse.".into(),
        Feed::Notifications => "Conversations that come back to you.".into(),
        Feed::Bookmarks => "Good things, kept close.".into(),
        Feed::Favourites => "The posts you appreciated.".into(),
        Feed::Search(_) => format!(
            "{} people · {} hashtags · {} posts",
            view.people.len(),
            view.hashtags.len(),
            view.posts.len()
        ),
        Feed::Profile(_) => view
            .profile
            .as_ref()
            .map(handle)
            .unwrap_or_else(|| "Posts from this account".into()),
        Feed::Thread(_) => "Follow the conversation.".into(),
        Feed::List(_, _) => "A circle of your own.".into(),
        Feed::Tag(tag) => format!("The conversation around #{tag}"),
    }
}
fn timeline(frame: &mut Frame, view: &mut View, area: Rect, p: Palette) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let name = view.feed.title();
    text(
        frame,
        row(area, 0),
        Line::from(vec![
            Span::styled(name, p.bold()),
            Span::styled(format!("  {}", view.count()), p.muted()),
        ]),
        p.text(),
    );
    if area.width >= 36 {
        let r = Rect::new(area.right().saturating_sub(10), area.y, 10, 1);
        text(
            frame,
            r,
            if view.loading {
                "◌ syncing"
            } else {
                "R refresh"
            },
            p.accent(),
        );
        command(view, r, "refresh");
    }
    text(frame, row(area, 1), feed_subtitle(view), p.muted());
    let content = Rect::new(
        area.x,
        area.y.saturating_add(3),
        area.width,
        area.height.saturating_sub(3),
    );
    if content.height == 0 {
        return;
    }
    if view.count() == 0 {
        let title = if view.loading {
            "Finding your people…"
        } else if matches!(view.feed, Feed::Search(_)) {
            "No posts found"
        } else if matches!(view.feed, Feed::Bookmarks) {
            "A place for the keepers"
        } else if matches!(view.feed, Feed::Notifications) {
            "All caught up"
        } else {
            "A little quiet here"
        };
        let body = if view.loading {
            "Your timeline will appear in a moment."
        } else if !view.connected && !view.demo {
            "Add an account to start your conversation.\nPress a for accounts, or ? for a little help."
        } else if matches!(view.feed, Feed::Bookmarks) {
            "Press s on any post to save it for later."
        } else {
            "Try another timeline or press R to refresh."
        };
        let message = vec![
            Line::from(Span::styled("◌", p.accent())),
            Line::default(),
            Line::from(Span::styled(title, p.bold())),
            Line::default(),
            Line::from(body),
        ];
        frame.render_widget(
            Paragraph::new(message)
                .style(p.text())
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: false }),
            Rect::new(
                content.x,
                content.y + content.height.saturating_sub(7) / 2,
                content.width,
                7.min(content.height),
            ),
        );
        return;
    }
    view.selected = view.selected.min(view.count().saturating_sub(1));
    view.offset = view.offset.min(view.selected);
    let heights: Vec<u16> = if view.feed == Feed::Notifications {
        view.notifications
            .iter()
            .map(|n| notification_height(n, content.width, view))
            .collect()
    } else {
        view.posts
            .iter()
            .map(|post| post_height(post, content.width, view))
            .collect()
    };
    let available = content.height;
    while view.offset < view.selected
        && heights[view.offset..=view.selected]
            .iter()
            .map(|h| h.saturating_add(1) as u32)
            .sum::<u32>()
            .saturating_sub(1)
            > available as u32
    {
        view.offset += 1;
    }
    let mut y = content.y;
    for (index, height) in heights.iter().enumerate().skip(view.offset) {
        if y >= content.bottom() {
            break;
        }
        let h = (*height).min(content.bottom() - y);
        let r = Rect::new(content.x, y, content.width, h);
        if view.feed == Feed::Notifications {
            let notification = view.notifications[index].clone();
            notification_card(frame, view, &notification, index, r, p);
        } else {
            let post = view.posts[index].clone();
            post_card(frame, view, &post, index, r, p);
        }
        y = y.saturating_add(height.saturating_add(1));
    }
    if view.has_more && y < content.bottom() {
        text(
            frame,
            Rect::new(content.x, y, content.width, 1),
            "↓  More posts arrive as you move down",
            p.muted(),
        );
    }
}

fn wrapped(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for original in text.split('\n') {
        let mut line = String::new();
        for word in original.split_whitespace() {
            if !line.is_empty() && line.width() + word.width() + 1 > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            for grapheme in word.graphemes(true) {
                if !line.is_empty() && line.width() + grapheme.width() > width {
                    lines.push(std::mem::take(&mut line));
                }
                line.push_str(grapheme);
            }
        }
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}
fn body_lines(post: &Post, width: u16, view: &View) -> Vec<String> {
    let limit = if view.compact { 2 } else { 5 };
    if hidden(post, view) {
        return vec!["Content hidden · x to reveal".into()];
    }
    let mut lines = wrapped(&post.text, width.saturating_sub(4) as usize);
    if lines.len() > limit {
        lines.truncate(limit);
        while lines.len() > 1 && lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.pop();
        }
        let end = lines.last_mut().expect("nonempty body");
        *end = format!("{} …", ellipsis(end, width.saturating_sub(7) as usize));
    }
    lines
}
fn post_height(post: &Post, width: u16, view: &View) -> u16 {
    7 + body_lines(post, width, view).len() as u16
        + u16::from(post.boosted_by.is_some())
        + u16::from(!post.spoiler_text.is_empty())
        + u16::from(!post.media.is_empty() && !hidden(post, view))
        + u16::from(post.poll.is_some() && !hidden(post, view))
}
fn post_card(
    frame: &mut Frame,
    view: &mut View,
    post: &Post,
    index: usize,
    area: Rect,
    p: Palette,
) {
    let selected = index == view.selected;
    let bg = if selected { p.selected } else { p.panel };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if selected { p.accent } else { p.line }))
        .style(Style::default().bg(bg));
    frame.render_widget(block, area);
    let inside = inset(area, 2, 1);
    let mut y = 0;
    if let Some(name) = &post.boosted_by {
        text(
            frame,
            row(inside, y),
            format!("↻  {name} boosted"),
            Style::default().fg(p.green),
        );
        y += 1;
    }
    let timestamp = short_date(&post.created_at);
    let name_width = inside.width.saturating_sub(timestamp.width() as u16 + 3);
    let name = format!("{}  {}", initials(post.account.name()), post.account.name());
    text(
        frame,
        Rect::new(inside.x, inside.y + y, name_width, 1),
        ellipsis(&name, name_width as usize),
        p.bold(),
    );
    let time_width = timestamp.width() as u16;
    if time_width < inside.width {
        text(
            frame,
            Rect::new(
                inside.right().saturating_sub(time_width),
                inside.y + y,
                time_width,
                1,
            ),
            timestamp,
            p.muted(),
        );
    }
    y += 1;
    text(frame, row(inside, y), handle(&post.account), p.muted());
    y += 2;
    if !post.spoiler_text.is_empty() {
        text(
            frame,
            row(inside, y),
            format!("CW  {}", post.spoiler_text),
            Style::default().fg(p.coral),
        );
        command(view, row(inside, y), &format!("post:{index}:reveal"));
        y += 1;
    }
    for line in body_lines(post, area.width, view) {
        text(
            frame,
            row(inside, y),
            line,
            if hidden(post, view) {
                p.muted()
            } else {
                p.text()
            },
        );
        y += 1;
    }
    if !post.media.is_empty() && !hidden(post, view) {
        let media = if hidden_media(post, view) {
            "◇ Sensitive media · x to reveal".into()
        } else {
            format!(
                "◇ {} attachment{} · m to open",
                post.media.len(),
                if post.media.len() == 1 { "" } else { "s" }
            )
        };
        text(frame, row(inside, y), media, Style::default().fg(p.purple));
        command(
            view,
            row(inside, y),
            &format!(
                "post:{index}:{}",
                if hidden_media(post, view) {
                    "reveal"
                } else {
                    "media"
                }
            ),
        );
        y += 1;
    }
    if let Some(poll) = &post.poll
        && !hidden(post, view)
    {
        text(
            frame,
            row(inside, y),
            format!(
                "▥ Poll · {} votes · {}",
                poll.votes_count,
                if poll.expired {
                    "ended"
                } else if poll.voted {
                    "voted"
                } else {
                    "v to vote"
                }
            ),
            Style::default().fg(p.purple),
        );
        command(view, row(inside, y), &format!("post:{index}:vote"));
        y += 1;
    }
    y += 1;
    if y < inside.height {
        let mut x = inside.x;
        for (label, id, active) in [
            (format!("↩ {}", post.replies_count), "reply", false),
            (format!("↻ {}", post.reblogs_count), "boost", post.reblogged),
            (
                format!(
                    "{} {}",
                    if post.favourited { "♥" } else { "♡" },
                    post.favourites_count
                ),
                "favourite",
                post.favourited,
            ),
            (
                (if post.bookmarked {
                    "◆ saved"
                } else {
                    "◇ save"
                })
                .into(),
                "bookmark",
                post.bookmarked,
            ),
        ] {
            let width = (label.width() as u16 + 2).min(inside.right().saturating_sub(x));
            if width == 0 {
                break;
            }
            let r = Rect::new(x, inside.y + y, width, 1);
            text(
                frame,
                r,
                label,
                Style::default().fg(if active {
                    if id == "favourite" { p.coral } else { p.accent }
                } else {
                    p.muted
                }),
            );
            command(view, r, &format!("post:{index}:{id}"));
            x += width + 1;
        }
    }
    hit(view, area, Hit::Select(index));
}
fn notification_height(note: &Notification, width: u16, view: &View) -> u16 {
    let lines = note
        .post
        .as_ref()
        .map(|post| {
            if hidden(post, view) {
                1
            } else {
                wrapped(&post.text, width.saturating_sub(4) as usize)
                    .len()
                    .min(3)
            }
        })
        .unwrap_or(0);
    5 + lines as u16
}
fn notification_card(
    frame: &mut Frame,
    view: &mut View,
    note: &Notification,
    index: usize,
    area: Rect,
    p: Palette,
) {
    let selected = view.selected == index;
    frame.render_widget(
        Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(if selected { p.accent } else { p.line }))
            .style(Style::default().bg(if selected { p.selected } else { p.panel })),
        area,
    );
    let inner = inset(area, 2, 1);
    let verb = match note.kind.as_str() {
        "follow" => "followed you",
        "favourite" => "favourited your post",
        "reblog" => "boosted your post",
        "mention" => "mentioned you",
        "poll" => "poll has ended",
        "follow_request" => "requested to follow you",
        "update" => "edited a post",
        _ => "shared an update",
    };
    text(
        frame,
        row(inner, 0),
        note.account.name().to_owned(),
        p.bold(),
    );
    text(
        frame,
        row(inner, 1),
        format!("{verb} · {}", short_date(&note.created_at)),
        Style::default().fg(p.purple),
    );
    if let Some(post) = &note.post {
        let lines = if hidden(post, view) {
            vec![format!("CW: {} · x to reveal", post.spoiler_text)]
        } else {
            wrapped(&post.text, inner.width as usize)
                .into_iter()
                .take(3)
                .collect()
        };
        for (i, line) in lines.into_iter().enumerate() {
            text(
                frame,
                row(inner, 3 + i as u16),
                line,
                Style::default().fg(p.soft),
            );
        }
    }
    hit(view, area, Hit::SelectNotification(index));
}

fn inspector(frame: &mut Frame, view: &mut View, area: Rect, p: Palette) {
    view.detail_area = Some(area);
    frame.render_widget(
        Block::default()
            .borders(Borders::LEFT)
            .border_style(Style::default().fg(p.line)),
        area,
    );
    let area = Rect::new(
        area.x + 2,
        area.y,
        area.width.saturating_sub(2),
        area.height,
    );
    text(frame, row(area, 0), "IN FOCUS", p.muted());
    let Some(post) = view.selected_post().cloned() else {
        text(frame, row(area, 3), "Room for a closer look.", p.text());
        text(
            frame,
            row(area, 5),
            "Select a post to read the details.",
            p.muted(),
        );
        return;
    };
    let account = view.profile.as_ref().unwrap_or(&post.account).clone();
    let header = Line::from(vec![
        Span::styled(
            format!(" {} ", initials(account.name())),
            Style::default()
                .fg(p.bg)
                .bg(p.purple)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {}", account.name()), p.bold()),
    ]);
    text(frame, row(area, 2), header, p.text());
    text(frame, row(area, 3), handle(&account), p.muted());
    command(
        view,
        Rect::new(
            area.x,
            area.y + 2,
            area.width,
            2.min(area.height.saturating_sub(2)),
        ),
        "profile",
    );
    text(
        frame,
        row(area, 5),
        Line::from(vec![
            Span::styled(format!("{}", account.followers_count), p.bold()),
            Span::styled(" followers   ", p.muted()),
            Span::styled(format!("{}", account.following_count), p.bold()),
            Span::styled(" following", p.muted()),
        ]),
        p.text(),
    );
    if area.height > 8 {
        let following = view.profile.is_some() && view.relationship.following;
        let width = (area.width / 2).min(17);
        chip(
            frame,
            view,
            Rect::new(area.x, area.y + 7, width, 1),
            if following { "Following" } else { "F  Follow" },
            "follow",
            following,
            p,
        );
        chip(
            frame,
            view,
            Rect::new(
                area.x + width + 1,
                area.y + 7,
                area.width.saturating_sub(width + 1),
                1,
            ),
            "p  Profile",
            "profile",
            false,
            p,
        );
    }
    let body = Rect::new(
        area.x,
        area.y + 10,
        area.width,
        area.height.saturating_sub(13),
    );
    let mut lines: Vec<Line<'static>> = Vec::new();
    if view.profile.is_some() && !account.note.is_empty() {
        lines.push(Line::from(Span::styled("ABOUT", p.muted())));
        lines.extend(
            wrapped(&account.note, body.width as usize)
                .into_iter()
                .map(Line::from),
        );
        lines.push(Line::default());
    }
    lines.push(Line::from(Span::styled(
        format!(
            "{} · {}",
            post.visibility.label(),
            short_date(&post.created_at)
        ),
        p.muted(),
    )));
    lines.push(Line::default());
    if !post.spoiler_text.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("CW · {}", post.spoiler_text),
            Style::default().fg(p.coral),
        )));
        lines.push(Line::default());
    }
    if hidden(&post, view) {
        lines.push(Line::from(Span::styled(
            "This post has a content warning.",
            p.muted(),
        )));
        lines.push(Line::from(Span::styled(
            "Press x when you are ready.",
            p.accent(),
        )));
    } else {
        lines.extend(
            wrapped(&post.text, body.width as usize)
                .into_iter()
                .map(Line::from),
        );
        if !post.media.is_empty() {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled("ATTACHMENTS", p.muted())));
            if hidden_media(&post, view) {
                lines.push(Line::from(Span::styled(
                    "Sensitive media · x to reveal",
                    Style::default().fg(p.coral),
                )));
            } else {
                for media in &post.media {
                    lines.push(Line::from(Span::styled(
                        format!("◇ {}", media.kind),
                        Style::default().fg(p.purple),
                    )));
                    let description = if media.description.is_empty() {
                        "No description provided"
                    } else {
                        &media.description
                    };
                    lines.extend(
                        wrapped(description, body.width as usize)
                            .into_iter()
                            .map(|s| Line::from(Span::styled(s, Style::default().fg(p.soft)))),
                    );
                }
            }
        }
        if let Some(poll) = &post.poll {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled("POLL", p.muted())));
            for (i, option) in poll.options.iter().enumerate() {
                let chosen = poll.own_votes.contains(&i);
                lines.extend(
                    wrapped(
                        &format!("{} {}", if chosen { "✓" } else { "○" }, option.title),
                        body.width as usize,
                    )
                    .into_iter()
                    .map(Line::from),
                );
                if poll.voted || poll.expired {
                    let percent = option
                        .votes_count
                        .saturating_mul(100)
                        .checked_div(poll.votes_count)
                        .unwrap_or(0)
                        .min(100);
                    let bar_width = body.width.saturating_sub(6).min(24) as usize;
                    let fill = (bar_width as u64 * percent / 100).min(bar_width as u64) as usize;
                    lines.push(Line::from(vec![
                        Span::styled("━".repeat(fill), p.accent()),
                        Span::styled("─".repeat(bar_width - fill), Style::default().fg(p.line)),
                        Span::styled(format!(" {percent}%"), p.muted()),
                    ]));
                }
            }
            lines.push(Line::from(Span::styled(
                format!(
                    "{} votes · {}",
                    poll.votes_count,
                    if poll.expired {
                        "closed"
                    } else if poll.voted {
                        "you voted"
                    } else {
                        "v to vote"
                    }
                ),
                p.muted(),
            )));
        }
    }
    if post.edited_at.is_some() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("Edited", p.muted())));
    }
    let max_scroll = lines
        .len()
        .saturating_sub(body.height as usize)
        .min(u16::MAX as usize) as u16;
    view.detail_scroll = view.detail_scroll.min(max_scroll);
    frame.render_widget(
        Paragraph::new(lines)
            .style(p.text())
            .scroll((view.detail_scroll, 0)),
        body,
    );
    if area.height > 4 {
        let width = area.width.saturating_sub(12);
        chip(
            frame,
            view,
            Rect::new(area.x, area.y + area.height - 3, width, 1),
            "↵ Conversation",
            "thread",
            false,
            p,
        );
        chip(
            frame,
            view,
            Rect::new(area.x + width + 1, area.y + area.height - 3, 11, 1),
            "o Browser",
            "open",
            false,
            p,
        );
        let extra = if hidden(&post, view) || hidden_media(&post, view) {
            Some(("x  Reveal hidden content", "reveal"))
        } else if !post.media.is_empty() {
            Some(("m  Open media gallery", "media"))
        } else if post
            .poll
            .as_ref()
            .is_some_and(|poll| !poll.expired && !poll.voted)
        {
            Some(("v  Add your vote", "vote"))
        } else {
            None
        };
        if let Some((label, id)) = extra {
            text(frame, row(area, area.height - 2), label, p.accent());
            command(view, row(area, area.height - 2), id);
        }
        text(
            frame,
            row(area, area.height - 1),
            "J / K scroll details  ·  i hide",
            p.muted(),
        );
    }
}

fn footer(frame: &mut Frame, view: &mut View, area: Rect, p: Palette) {
    frame.render_widget(Block::default().style(Style::default().bg(p.panel)), area);
    let hints = if area.width >= 110 {
        "  j/k move   Enter thread   r reply   f favourite   b boost   s save   n compose   / search   Ctrl+K commands   ? help"
    } else if area.width >= 65 {
        "  j/k move  Enter thread  n compose  / search  Ctrl+K commands  ? help"
    } else {
        " j/k move  n compose  / search  ? help"
    };
    text(frame, row(area, 0), hints, Style::default().fg(p.soft));
    for (label, id) in [
        ("Enter thread", "thread"),
        ("r reply", "reply"),
        ("f favourite", "favourite"),
        ("b boost", "boost"),
        ("s save", "bookmark"),
        ("n compose", "compose"),
        ("/ search", "search"),
        ("Ctrl+K commands", "palette"),
        ("? help", "help"),
    ] {
        if let Some(column) = hints.find(label)
            && column < area.width as usize
        {
            command(
                view,
                Rect::new(
                    area.x + column as u16,
                    area.y,
                    (label.len() as u16).min(area.width - column as u16),
                    1,
                ),
                id,
            );
        }
    }
    if area.height > 1 {
        if let Some(notice) = &view.notice {
            text(
                frame,
                row(area, 1),
                format!(
                    "  {} {}",
                    if notice.error { "!" } else { "✓" },
                    notice.message
                ),
                Style::default().fg(if notice.error { p.coral } else { p.green }),
            );
        } else {
            let position = if view.count() == 0 {
                "0 posts".into()
            } else {
                format!("{} / {}", view.selected + 1, view.count())
            };
            let sync = if view.loading {
                "syncing…"
            } else if view.last_sync.is_empty() {
                "ready"
            } else {
                &view.last_sync
            };
            let left = format!("  {position}  ·  {}  ·  {sync}", view.theme.name());
            text(frame, row(area, 1), left, p.muted());
            if area.width > 92 {
                let label = if view.mouse_enabled {
                    "mouse on  ·  q quit  "
                } else {
                    "mouse off  ·  q quit  "
                };
                text(
                    frame,
                    Rect::new(
                        area.right().saturating_sub(label.width() as u16),
                        area.y + 1,
                        label.width() as u16,
                        1,
                    ),
                    label,
                    p.muted(),
                );
            }
        }
    }
}

fn centered(screen: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(
        screen
            .width
            .saturating_sub(if screen.width > 4 { 4 } else { 0 }),
    );
    let height = height.min(
        screen
            .height
            .saturating_sub(if screen.height > 4 { 2 } else { 0 }),
    );
    Rect::new(
        screen.x + (screen.width - width) / 2,
        screen.y + (screen.height - height) / 2,
        width,
        height,
    )
}
fn modal_frame(
    frame: &mut Frame,
    view: &mut View,
    width: u16,
    height: u16,
    title: &str,
    p: Palette,
) -> Rect {
    let area = centered(frame.area(), width, height);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::bordered()
            .border_type(BorderType::Rounded)
            .title(Line::from(Span::styled(format!("  {title}  "), p.bold())))
            .border_style(p.accent())
            .style(Style::default().bg(p.panel).fg(p.fg)),
        area,
    );
    if area.width > 8 && area.height > 0 {
        let close = Rect::new(area.right() - 5, area.y, 3, 1);
        text(frame, close, " × ", p.muted().bg(p.panel));
        command(view, close, "cancel");
    }
    inset(area, 2, 1)
}
fn modal(frame: &mut Frame, view: &mut View, p: Palette) {
    match view.modal.clone() {
        Modal::None => {}
        Modal::Help => help_modal(frame, view, p),
        Modal::Palette { query, selected } => palette_modal(frame, view, &query, selected, p),
        Modal::Compose => compose_modal(frame, view, p),
        Modal::Input {
            title,
            value,
            purpose,
        } => {
            let area = modal_frame(frame, view, 72, 9, &title, p);
            let hint = match purpose.as_str() {
                "search" => "Find posts, people, and hashtags across your network.",
                "tag" => "Type a hashtag without the #.",
                "create_list" => "Give your new circle a name.",
                _ => "Type a value below.",
            };
            text(frame, row(area, 0), hint, p.muted());
            single_input(frame, row(area, 2), &value, view.input_cursor, p);
            if area.height > 4 {
                chip(
                    frame,
                    view,
                    Rect::new(area.x, area.y + area.height - 3, 14.min(area.width), 1),
                    "Continue →",
                    "modal_activate",
                    true,
                    p,
                );
            }
            text(
                frame,
                row(area, area.height.saturating_sub(1)),
                "Enter continue  ·  Esc cancel",
                p.muted(),
            );
        }
        Modal::Confirm { title, detail, .. } => {
            let area = modal_frame(frame, view, 68, 12, &title, p);
            frame.render_widget(
                Paragraph::new(detail)
                    .wrap(Wrap { trim: false })
                    .style(p.text()),
                Rect::new(
                    area.x,
                    area.y + 1,
                    area.width,
                    area.height.saturating_sub(5),
                ),
            );
            let y = area.height.saturating_sub(2);
            if area.height > 3 {
                chip(
                    frame,
                    view,
                    Rect::new(area.x, area.y + y - 2, 14.min(area.width), 1),
                    "Confirm",
                    "confirm",
                    true,
                    p,
                );
                if area.width > 17 {
                    chip(
                        frame,
                        view,
                        Rect::new(area.x + 17, area.y + y - 2, 12.min(area.width - 17), 1),
                        "Cancel",
                        "cancel",
                        false,
                        p,
                    );
                }
            }
            text(
                frame,
                row(area, y),
                "Enter confirm  ·  Esc cancel",
                Style::default().fg(p.coral),
            );
        }
        Modal::Accounts { selected } => {
            let items = view
                .accounts
                .iter()
                .map(|a| {
                    (
                        format!("{} {}", if a.active { "●" } else { "○" }, a.label),
                        a.network.clone(),
                    )
                })
                .collect::<Vec<_>>();
            list_modal(
                frame,
                view,
                "Your accounts",
                "One workspace, every conversation.",
                &items,
                selected,
                "Enter switch  ·  tootles login to add  ·  Esc close",
                p,
            );
        }
        Modal::Lists { selected } => {
            let items = view
                .lists
                .iter()
                .map(|a| (a.title.clone(), "Curated timeline".into()))
                .collect::<Vec<_>>();
            list_modal(
                frame,
                view,
                "Your lists",
                "A smaller circle. A different perspective.",
                &items,
                selected,
                "Enter open  ·  + create list  ·  Esc close",
                p,
            );
        }
        Modal::People { selected } => {
            let items = view
                .people
                .iter()
                .map(|a| (a.name().to_owned(), handle(a)))
                .collect::<Vec<_>>();
            list_modal(
                frame,
                view,
                "People",
                "People found in your latest search.",
                &items,
                selected,
                "Enter view profile  ·  Esc close",
                p,
            );
        }
        Modal::Drafts { selected } => {
            let items = view
                .drafts
                .iter()
                .map(|d| {
                    let first = d.text.lines().next().unwrap_or("Empty draft");
                    (
                        if first.is_empty() {
                            "Empty draft".into()
                        } else {
                            first.into()
                        },
                        format!(
                            "{} · {} characters{}",
                            d.visibility.label(),
                            d.text.chars().count(),
                            if d.reply_to.is_some() {
                                " · reply"
                            } else {
                                ""
                            }
                        ),
                    )
                })
                .collect::<Vec<_>>();
            list_modal(
                frame,
                view,
                "Saved drafts",
                "A thought can wait until it is ready.",
                &items,
                selected,
                "Enter resume  ·  D remove  ·  Esc close",
                p,
            );
        }
        Modal::Poll { choices, selected } => {
            let concealed = view.selected_post().is_some_and(|post| hidden(post, view));
            let poll = view
                .selected_post()
                .filter(|post| !hidden(post, view))
                .and_then(|post| post.poll.clone());
            let items = poll
                .as_ref()
                .map(|poll| {
                    poll.options
                        .iter()
                        .enumerate()
                        .map(|(i, o)| {
                            (
                                format!(
                                    "{} {}",
                                    if choices.contains(&i) { "●" } else { "○" },
                                    o.title
                                ),
                                String::new(),
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let description = if concealed {
                "Close this window and press x to reveal the post first."
            } else if poll.is_some_and(|p| p.multiple) {
                "Choose one or more options."
            } else {
                "Choose one option."
            };
            list_modal(
                frame,
                view,
                "Add your vote",
                description,
                &items,
                selected,
                "Space select  ·  Enter vote  ·  Esc cancel",
                p,
            );
        }
    }
}
fn help_modal(frame: &mut Frame, view: &mut View, p: Palette) {
    let area = modal_frame(frame, view, 80, 32, "Make yourself at home", p);
    let lines = vec![
        ("MOVE AROUND", ""),
        ("j / k  or  ↑ / ↓", "Previous / next post"),
        ("PageUp / PageDown", "Move a page · Home / End jump"),
        ("1 · 2 · 3", "Home · Local · Federated"),
        ("4 · 5 · 6", "Notifications · Bookmarks · Favourites"),
        ("Enter", "Open conversation"),
        ("/   ·   #", "Search everything · browse hashtag"),
        ("", ""),
        ("JOIN THE CONVERSATION", ""),
        ("n   ·   r   ·   e", "New post · reply · edit your post"),
        ("f   ·   b   ·   s", "Favourite · boost · bookmark"),
        ("p   ·   F", "View author · follow / unfollow"),
        ("x   ·   v", "Reveal content warning · vote"),
        ("o   ·   m", "Open post · media gallery"),
        ("", ""),
        ("YOUR WORKSPACE", ""),
        ("Ctrl+K  or  :", "Find any command"),
        ("a   ·   l   ·   d", "Accounts · lists · saved drafts"),
        ("T   ·   c   ·   i", "Theme · compact view · detail pane"),
        ("J / K", "Scroll detail pane"),
        ("R   ·   q", "Refresh · quit"),
        ("", ""),
        ("COMPOSER", ""),
        ("Tab / Shift+Tab", "Move between fields"),
        ("Ctrl+Enter or F4 / Ctrl+S", "Publish / save draft"),
        ("Esc", "Close · keep your unfinished draft"),
    ];
    for (i, (key, description)) in lines.into_iter().enumerate() {
        if i as u16 >= area.height.saturating_sub(1) {
            break;
        }
        if description.is_empty() {
            text(frame, row(area, i as u16), key, p.accent());
        } else if area.width >= 53 {
            text(
                frame,
                row(area, i as u16),
                Line::from(vec![
                    Span::styled(format!("{key:<26}"), p.text()),
                    Span::styled(description, Style::default().fg(p.soft)),
                ]),
                p.text(),
            );
        } else {
            text(
                frame,
                row(area, i as u16),
                format!("{key}  {description}"),
                p.text(),
            );
        }
    }
    text(
        frame,
        row(area, area.height.saturating_sub(1)),
        "Click to select · scroll to move · Esc to return",
        p.muted(),
    );
}
fn palette_modal(frame: &mut Frame, view: &mut View, query: &str, selected: usize, p: Palette) {
    let area = modal_frame(frame, view, 74, 23, "A command away", p);
    single_input(frame, row(area, 0), query, view.input_cursor, p);
    text(
        frame,
        row(area, 1),
        "─".repeat(area.width as usize),
        Style::default().fg(p.line),
    );
    let options = commands(query);
    let visible = area.height.saturating_sub(4) as usize;
    let offset = selected.saturating_sub(visible.saturating_sub(1));
    if options.is_empty() {
        text(
            frame,
            row(area, 3),
            "No matching commands. Try another word.",
            p.muted(),
        );
    }
    for (i, item) in options.iter().enumerate().skip(offset).take(visible) {
        let r = row(area, 2 + (i - offset) as u16);
        let selected = i == selected;
        frame.render_widget(
            Paragraph::new(format!(" {}", item.label)).style(
                Style::default()
                    .fg(if selected { p.fg } else { p.soft })
                    .bg(if selected { p.selected } else { p.panel }),
            ),
            r,
        );
        let w = item.hint.width() as u16;
        if w < r.width {
            text(
                frame,
                Rect::new(r.right().saturating_sub(w + 1), r.y, w, 1),
                item.hint,
                p.accent(),
            );
        }
        hit(view, r, Hit::ModalSelect(i));
    }
    if area.height > 0 {
        text(
            frame,
            row(area, area.height - 1),
            "Type to filter  ·  ↑/↓ choose  ·  Enter run  ·  Esc close",
            p.muted(),
        );
    }
}

fn single_input(frame: &mut Frame, area: Rect, value: &str, cursor: usize, p: Palette) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let prefix = if area.width > 2 { 2 } else { 0 };
    let width = area.width.saturating_sub(prefix).max(1) as usize;
    let mut cursor = cursor.min(value.len());
    while !value.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let column = value[..cursor].width();
    let start = column.saturating_sub(width.saturating_sub(1));
    let mut skipped = 0;
    let mut byte = 0;
    for (index, grapheme) in value.grapheme_indices(true) {
        if skipped >= start {
            break;
        }
        skipped += grapheme.width();
        byte = index + grapheme.len();
    }
    text(
        frame,
        area,
        format!("{}{}", if prefix > 0 { "› " } else { "" }, &value[byte..]),
        p.text(),
    );
    frame.set_cursor_position((
        area.x + prefix + column.saturating_sub(skipped).min(width - 1) as u16,
        area.y,
    ));
}
#[allow(clippy::too_many_arguments)] // Explicit component inputs keep modal state in View.
fn list_modal(
    frame: &mut Frame,
    view: &mut View,
    title: &str,
    description: &str,
    items: &[(String, String)],
    selected: usize,
    hint: &str,
    p: Palette,
) {
    let height = ((items.len().min(8) * 3 + 6) as u16).max(11);
    let area = modal_frame(frame, view, 72, height, title, p);
    text(frame, row(area, 0), description, p.muted());
    if items.is_empty() {
        text(frame, row(area, 3), "Nothing here yet.", p.text());
    }
    let count = (area.height.saturating_sub(4) / 3) as usize;
    let offset = selected.saturating_sub(count.saturating_sub(1));
    for (i, (title, description)) in items.iter().enumerate().skip(offset).take(count) {
        let y = 2 + (i - offset) as u16 * 3;
        let r = Rect::new(
            area.x,
            area.y + y,
            area.width,
            2.min(area.height.saturating_sub(y)),
        );
        frame.render_widget(
            Block::default().style(Style::default().bg(if i == selected {
                p.selected
            } else {
                p.panel
            })),
            r,
        );
        text(
            frame,
            row(r, 0),
            format!(" {}", title),
            if i == selected { p.bold() } else { p.text() },
        );
        text(frame, row(r, 1), format!(" {}", description), p.muted());
        hit(view, r, Hit::ModalSelect(i));
    }
    if area.height > 3 {
        let (label, action) = if matches!(view.modal, Modal::Poll { .. }) {
            ("Cast vote", "submit_vote")
        } else {
            ("Open selected", "modal_activate")
        };
        if !items.is_empty() {
            chip(
                frame,
                view,
                Rect::new(area.x, area.y + area.height - 3, 16.min(area.width), 1),
                label,
                action,
                true,
                p,
            );
        }
        if matches!(view.modal, Modal::Lists { .. }) && area.width > 19 {
            chip(
                frame,
                view,
                Rect::new(
                    area.x + 19,
                    area.y + area.height - 3,
                    16.min(area.width - 19),
                    1,
                ),
                "+ Create list",
                "create_list",
                false,
                p,
            );
        }
    }
    text(
        frame,
        row(area, area.height.saturating_sub(1)),
        hint,
        p.muted(),
    );
}

fn editor_lines(value: &str, width: usize, cursor: usize) -> (Vec<String>, (usize, usize)) {
    let width = width.max(1);
    let mut lines = vec![String::new()];
    let mut cursor_position = (0, 0);
    let mut found = false;
    let mut word_start = true;
    for (byte, g) in value.grapheme_indices(true) {
        let whitespace = g.chars().all(char::is_whitespace);
        if word_start && !whitespace && !lines.last().unwrap().is_empty() {
            let word_width = value[byte..]
                .split_whitespace()
                .next()
                .unwrap_or("")
                .width();
            if word_width <= width && lines.last().unwrap().width() + word_width > width {
                lines.push(String::new());
            }
        }
        if g != "\n"
            && lines.last().unwrap().width() + g.width() > width
            && !lines.last().unwrap().is_empty()
        {
            lines.push(String::new());
        }
        if byte >= cursor && !found {
            cursor_position = (lines.len() - 1, lines.last().unwrap().width());
            found = true;
        }
        if g == "\n" {
            lines.push(String::new());
        } else {
            lines.last_mut().unwrap().push_str(g);
        }
        word_start = whitespace;
    }
    if !found {
        if lines.last().unwrap().width() >= width {
            lines.push(String::new());
        }
        cursor_position = (lines.len() - 1, lines.last().unwrap().width());
    }
    (lines, cursor_position)
}
#[allow(clippy::too_many_arguments)] // One shared editor paints all six composer fields.
fn editor(
    frame: &mut Frame,
    view: &mut View,
    area: Rect,
    label: &str,
    value: &str,
    index: usize,
    placeholder: &str,
    p: Palette,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let focused = view.compose_field == index;
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Line::from(Span::styled(
            format!(" {label} "),
            if focused {
                p.accent()
            } else {
                Style::default().fg(p.soft)
            },
        )))
        .border_style(Style::default().fg(if focused { p.accent } else { p.line }))
        .style(Style::default().bg(if focused { p.raised } else { p.panel }));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    hit(view, area, Hit::ComposerField(index));
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let (lines, (cy, cx)) = editor_lines(
        value,
        inner.width as usize,
        view.compose_cursor.min(value.len()),
    );
    let scroll = if focused {
        cy.saturating_sub(inner.height as usize - 1)
    } else {
        0
    };
    if value.is_empty() {
        text(frame, inner, placeholder.to_owned(), p.muted());
    } else {
        frame.render_widget(
            Paragraph::new(lines.into_iter().map(Line::from).collect::<Vec<_>>())
                .style(p.text())
                .scroll((scroll.min(u16::MAX as usize) as u16, 0)),
            inner,
        );
    }
    if focused {
        frame.set_cursor_position((
            inner.x + (cx as u16).min(inner.width - 1),
            inner.y + ((cy - scroll) as u16).min(inner.height - 1),
        ));
    }
}
fn compose_modal(frame: &mut Frame, view: &mut View, p: Palette) {
    let title = if view.composer.edit_id.is_some() {
        "Edit your post"
    } else if view.composer.reply_to.is_some() {
        "Keep the conversation going"
    } else {
        "What is on your mind?"
    };
    let area = modal_frame(frame, view, 92, 35, title, p);
    let count = view.composer.text.chars().count() + view.composer.spoiler_text.chars().count();
    let max = view.capabilities.max_characters;
    let account = format!(
        "{}  ·  {}",
        view.me.name(),
        view.composer.visibility.label()
    );
    text(frame, row(area, 0), account, p.muted());
    let count_label = format!("{count} / {max}");
    if area.width > count_label.width() as u16 + 20 {
        text(
            frame,
            Rect::new(
                area.right() - count_label.width() as u16,
                area.y,
                count_label.width() as u16,
                1,
            ),
            count_label,
            Style::default().fg(if count > max { p.coral } else { p.green }),
        );
    }
    // A small terminal shows the focused field and essential controls. All six
    // fields remain accessible with Tab, with the same editor and cursor rules.
    if area.height < 26 || area.width < 48 {
        let (label, value, placeholder) = match view.compose_field {
            0 => (
                "Your post",
                view.composer.text.clone(),
                "A thought, a question, a small discovery…",
            ),
            1 => (
                "Content warning",
                view.composer.spoiler_text.clone(),
                "Optional warning",
            ),
            2 => (
                "Attachment path",
                view.attachment_path.clone(),
                "/path/to/image.png",
            ),
            3 => (
                "Image description",
                view.attachment_alt.clone(),
                "Describe your attachment",
            ),
            4 => (
                "Poll options",
                view.poll_input.clone(),
                "First choice; Second choice",
            ),
            _ => ("Language", view.composer.language.clone(), "en"),
        };
        let h = area
            .height
            .saturating_sub(7)
            .max(3)
            .min(area.height.saturating_sub(2));
        editor(
            frame,
            view,
            Rect::new(area.x, area.y + 2, area.width, h),
            label,
            &value,
            view.compose_field,
            placeholder,
            p,
        );
        let y = 2 + h;
        compose_controls(frame, view, area, y, p);
        if area.height > 0 {
            text(
                frame,
                row(area, area.height - 1),
                "Tab field · F4 send · Esc keep",
                p.muted(),
            );
        }
        return;
    }
    let body_h = area.height.saturating_sub(24).clamp(5, 9);
    let post = view.composer.text.clone();
    editor(
        frame,
        view,
        Rect::new(area.x, area.y + 2, area.width, body_h),
        "Your post",
        &post,
        0,
        "A thought, a question, a small discovery…",
        p,
    );
    let mut y = 2 + body_h;
    let cw = view.composer.spoiler_text.clone();
    editor(
        frame,
        view,
        Rect::new(area.x, area.y + y, area.width, 3),
        "Content warning · optional",
        &cw,
        1,
        "Give people a choice before they read",
        p,
    );
    y += 3;
    let half = area.width / 2;
    let path = view.attachment_path.clone();
    let alt = view.attachment_alt.clone();
    editor(
        frame,
        view,
        Rect::new(area.x, area.y + y, half.saturating_sub(1), 3),
        "Attachment path",
        &path,
        2,
        "/path/to/image.png",
        p,
    );
    editor(
        frame,
        view,
        Rect::new(area.x + half, area.y + y, area.width - half, 3),
        "Alt text",
        &alt,
        3,
        "Describe what is in the attachment",
        p,
    );
    y += 3;
    let attached = view.composer.attachments.len();
    let attach_r = Rect::new(area.x, area.y + y, 17.min(area.width), 1);
    chip(
        frame,
        view,
        attach_r,
        "+ Add attachment",
        "attach",
        false,
        p,
    );
    if area.width > 19 {
        text(
            frame,
            Rect::new(area.x + 19, area.y + y, area.width - 19, 1),
            format!(
                "{attached} attached{}",
                if attached > 0 {
                    " · click to remove last"
                } else {
                    ""
                }
            ),
            Style::default().fg(p.purple),
        );
        if attached > 0 {
            command(
                view,
                Rect::new(area.x + 19, area.y + y, area.width - 19, 1),
                "remove_attachment",
            );
        }
    }
    y += 2;
    let poll = view.poll_input.clone();
    editor(
        frame,
        view,
        Rect::new(area.x, area.y + y, area.width, 3),
        "Poll · separate options with semicolons",
        &poll,
        4,
        "First choice; Second choice",
        p,
    );
    y += 3;
    let language = view.composer.language.clone();
    editor(
        frame,
        view,
        Rect::new(area.x, area.y + y, 12.min(area.width), 3),
        "Language",
        &language,
        5,
        "en",
        p,
    );
    if area.width > 15 {
        let poll_label = if view.composer.poll_multiple {
            "☑ Multiple choice"
        } else {
            "□ Multiple choice"
        };
        chip(
            frame,
            view,
            Rect::new(area.x + 15, area.y + y + 1, 20, 1),
            poll_label,
            "poll_multiple",
            false,
            p,
        );
        let seconds = view.composer.poll_expires_in;
        let duration = if seconds >= 86400 {
            let days = seconds / 86400;
            format!("{days} day{}", if days == 1 { "" } else { "s" })
        } else if seconds >= 3600 {
            let hours = seconds / 3600;
            format!("{hours} hour{}", if hours == 1 { "" } else { "s" })
        } else {
            format!("{} min", seconds.max(300) / 60)
        };
        if area.width > 38 {
            chip(
                frame,
                view,
                Rect::new(area.x + 37, area.y + y + 1, area.width - 37, 1),
                &format!("Ends in {duration}"),
                "poll_duration",
                false,
                p,
            );
        }
    }
    y += 4;
    compose_controls(frame, view, area, y, p);
    text(
        frame,
        row(area, area.height.saturating_sub(1)),
        "Tab fields · Ctrl+Enter / F4 publish · Ctrl+S save · Esc keep",
        p.muted(),
    );
}
fn compose_controls(frame: &mut Frame, view: &mut View, area: Rect, y: u16, p: Palette) {
    if y >= area.height {
        return;
    }
    let visibility = format!("◉ {}", view.composer.visibility.label());
    let vis_w = (visibility.width() as u16 + 2).min(area.width);
    chip(
        frame,
        view,
        Rect::new(area.x, area.y + y, vis_w, 1),
        &visibility,
        "visibility",
        false,
        p,
    );
    if area.width > vis_w + 3 {
        let sensitive = if view.composer.sensitive {
            "☑ Sensitive"
        } else {
            "□ Sensitive"
        };
        chip(
            frame,
            view,
            Rect::new(
                area.x + vis_w + 2,
                area.y + y,
                13.min(area.width - vis_w - 2),
                1,
            ),
            sensitive,
            "sensitive",
            false,
            p,
        );
    }
    if y + 2 < area.height {
        let publish = if view.busy {
            "Sending…"
        } else if view.composer.edit_id.is_some() {
            "Save changes"
        } else {
            "Publish post"
        };
        chip(
            frame,
            view,
            Rect::new(area.x, area.y + y + 2, 16.min(area.width), 1),
            publish,
            "send",
            true,
            p,
        );
        if area.width > 18 {
            chip(
                frame,
                view,
                Rect::new(area.x + 18, area.y + y + 2, 12.min(area.width - 18), 1),
                "Save draft",
                "save_draft",
                false,
                p,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editor_cursor_respects_unicode_and_newlines() {
        let (lines, position) = editor_lines("hi 🌊\n世界", 5, "hi 🌊\n世".len());
        assert_eq!(lines, vec!["hi 🌊", "世界"]);
        assert_eq!(position, (1, 2));
    }
    #[test]
    fn wrapping_never_splits_a_grapheme() {
        assert_eq!(wrapped("hello world", 6), vec!["hello", "world"]);
        assert_eq!(ellipsis("🌊🌊🌊", 5), "🌊🌊…");
    }
    #[test]
    fn editor_wraps_words_and_keeps_the_cursor_at_the_word_start() {
        let (lines, position) = editor_lines("hello world", 8, "hello ".len());
        assert_eq!(lines, vec!["hello ", "world"]);
        assert_eq!(position, (1, 0));
        let (_, end) = editor_lines("hello world", 8, "hello world".len());
        assert_eq!(end, (1, 5));
    }
}
