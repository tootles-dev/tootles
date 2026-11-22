use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use tootles::{model::*, ui, view::*};

fn sample() -> View {
    let author = Account {
        id: "alexa".into(),
        username: "alexa".into(),
        acct: "alexa@example.social".into(),
        display_name: "Alexa Morgan".into(),
        note: "Making the web a little more human.".into(),
        followers_count: Some(328),
        following_count: Some(142),
        ..Default::default()
    };
    let post = Post {
        id: "first".into(), account: author.clone(), text: "A small reminder: you can build wonderful things at your own pace. 🌱\nWhat are you working on today?".into(),
        created_at: "2026-11-22T05:00:00Z".into(), favourites_count: 12, reblogs_count: 3,
        replies_count: 8, ..Default::default()
    };
    let mut view = View {
        me: author.clone(),
        connected: true,
        instance: "example.social".into(),
        ..Default::default()
    };
    view.posts = (0..12)
        .map(|i| Post {
            id: format!("post-{i}"),
            ..post.clone()
        })
        .collect();
    view.posts[0].poll = Some(Poll {
        id: "poll".into(),
        options: vec![
            PollOption {
                title: "A little project".into(),
                votes_count: 4,
            },
            PollOption {
                title: "A little rest".into(),
                votes_count: 6,
            },
        ],
        votes_count: 10,
        ..Default::default()
    });
    view.people.push(author.clone());
    view.accounts.push(AccountLabel {
        label: "alexa@example.social".into(),
        network: "Mastodon".into(),
        active: true,
    });
    view.lists.push(SocialList {
        id: "friends".into(),
        title: "Good company".into(),
    });
    view.drafts.push(Compose {
        text: "An unfinished thought".into(),
        ..Default::default()
    });
    view.notifications.push(Notification {
        id: "notification".into(),
        kind: "favourite".into(),
        account: author,
        post: Some(post),
        ..Default::default()
    });
    view.file_browser.directory = "/home/example/Pictures".into();
    view.file_browser.location = "/home/example/Pictures".into();
    view.file_browser.entries = (0..30)
        .map(|index| tootles::file_picker::Entry {
            path: format!("/home/example/Pictures/holiday-{index}.png").into(),
            name: format!("holiday-{index}.png"),
            kind: tootles::file_picker::FileKind::Image,
            bytes: 2048,
        })
        .collect();
    view
}
fn draw(view: &mut View, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| ui::render(frame, view)).unwrap();
    let buffer = terminal.backend().buffer();
    let mut result = String::new();
    for y in 0..height {
        for x in 0..width {
            result.push_str(buffer[(x, y)].symbol());
        }
        result.push('\n');
    }
    result
}
#[test]
fn responsive_layout_shows_only_available_panes() {
    let mut view = sample();
    let wide = draw(&mut view, 150, 48);
    assert!(wide.contains("YOUR SPACE"));
    assert!(wide.contains("IN FOCUS"));
    assert!(wide.contains("Alexa Morgan"));
    assert!(wide.contains("Tokyo Night"));
    let medium = draw(&mut view, 95, 32);
    assert!(medium.contains("YOUR SPACE"));
    assert!(!medium.contains("IN FOCUS"));
    let narrow = draw(&mut view, 48, 28);
    assert!(!narrow.contains("YOUR SPACE"));
    assert!(narrow.contains("Home"));
    assert!(narrow.contains("Alexa Morgan"));
}
#[test]
fn content_warnings_hide_text_media_and_poll_until_revealed() {
    let mut view = sample();
    view.posts.truncate(1);
    view.posts[0].spoiler_text = "A surprise".into();
    view.posts[0].text = "NEVER_LEAK_BODY".into();
    view.posts[0].media = vec![Media {
        description: "NEVER_LEAK_ALT".into(),
        ..Default::default()
    }];
    view.posts[0].poll = Some(Poll {
        options: vec![PollOption {
            title: "NEVER_LEAK_POLL".into(),
            votes_count: 0,
        }],
        ..Default::default()
    });
    let hidden = draw(&mut view, 150, 48);
    assert!(hidden.contains("A surprise"));
    assert!(!hidden.contains("NEVER_LEAK"));
    view.modal = Modal::Poll {
        choices: vec![],
        selected: 0,
    };
    assert!(!draw(&mut view, 150, 48).contains("NEVER_LEAK"));
    view.modal = Modal::None;
    view.revealed.insert(view.posts[0].id.clone());
    let visible = draw(&mut view, 150, 48);
    assert!(visible.contains("NEVER_LEAK_BODY"));
    assert!(visible.contains("NEVER_LEAK_ALT"));
    assert!(visible.contains("NEVER_LEAK_POLL"));
}

#[test]
fn sensitive_media_descriptions_wait_for_explicit_reveal() {
    let mut view = sample();
    view.posts.truncate(1);
    view.posts[0].sensitive = true;
    view.posts[0].media = vec![Media {
        description: "PRIVATE_ALT_DESCRIPTION".into(),
        ..Default::default()
    }];
    let screen = draw(&mut view, 150, 48);
    assert!(screen.contains("Sensitive media"));
    assert!(!screen.contains("PRIVATE_ALT_DESCRIPTION"));
    view.revealed.insert(view.posts[0].id.clone());
    assert!(draw(&mut view, 150, 48).contains("PRIVATE_ALT_DESCRIPTION"));
}

#[test]
fn inputs_show_the_actual_unicode_cursor_and_scroll_to_it() {
    let mut view = sample();
    let value = "one 🌊 two";
    view.modal = Modal::Input {
        title: "Search".into(),
        value: value.into(),
        purpose: "search".into(),
    };
    view.input_cursor = "one ".len();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|frame| ui::render(frame, &mut view)).unwrap();
    let before_emoji = terminal.get_cursor_position().unwrap();
    view.input_cursor = "one 🌊".len();
    terminal.draw(|frame| ui::render(frame, &mut view)).unwrap();
    let after_emoji = terminal.get_cursor_position().unwrap();
    assert_eq!(after_emoji.x, before_emoji.x + 2);
    assert_eq!(after_emoji.y, before_emoji.y);
    view.modal = Modal::Input {
        title: "Search".into(),
        value: "a".repeat(160),
        purpose: "search".into(),
    };
    view.input_cursor = 160;
    terminal.draw(|frame| ui::render(frame, &mut view)).unwrap();
    assert!(terminal.get_cursor_position().unwrap().x < 100);
}
#[test]
fn every_modal_and_theme_renders_at_every_size_without_invalid_hitboxes() {
    let modals = vec![
        Modal::None,
        Modal::Help,
        Modal::Palette {
            query: "post".into(),
            selected: 0,
        },
        Modal::Input {
            title: "Search".into(),
            value: "🌊 some words".into(),
            purpose: "search".into(),
        },
        Modal::Compose,
        Modal::FileBrowser,
        Modal::Confirm {
            title: "Delete?".into(),
            detail: "This cannot be undone.".into(),
            action: "delete".into(),
        },
        Modal::Accounts { selected: 0 },
        Modal::Lists { selected: 0 },
        Modal::People { selected: 0 },
        Modal::Poll {
            choices: vec![0],
            selected: 1,
        },
        Modal::Drafts { selected: 0 },
    ];
    for theme in [
        Theme::TokyoNight,
        Theme::Midnight,
        Theme::Paper,
        Theme::Contrast,
    ] {
        for modal in &modals {
            for (width, height) in [
                (160, 50),
                (115, 35),
                (80, 24),
                (48, 20),
                (25, 10),
                (8, 4),
                (1, 1),
            ] {
                let mut view = sample();
                view.theme = theme;
                view.modal = modal.clone();
                view.composer.text = "Unicode 🌊 compose\nSecond line".into();
                view.compose_cursor = view.composer.text.len();
                draw(&mut view, width, height);
                let screen = Rect::new(0, 0, width, height);
                for hit in &view.hits {
                    assert_eq!(
                        hit.area.intersection(screen),
                        hit.area,
                        "hit outside {width}x{height}: {:?} modal {modal:?}",
                        hit.area
                    );
                }
            }
        }
    }
}

#[test]
fn browser_shows_file_details_and_scrolls_selected_entry_into_view() {
    let mut view = sample();
    view.modal = Modal::FileBrowser;
    view.file_browser.selected = 25;
    let screen = draw(&mut view, 150, 48);
    assert!(screen.contains("Find an attachment"));
    assert!(screen.contains("holiday-25.png"));
    assert!(screen.contains("2.0 KiB"));
    assert!(view.file_browser.offset > 0);
    assert!(
        view.hits
            .iter()
            .any(|hit| matches!(hit.hit, Hit::FileSelect(25)))
    );
    assert!(
        view.hits
            .iter()
            .any(|hit| matches!(&hit.hit, Hit::Command(command) if command == "browse_preview"))
    );
    view.file_browser.error = Some("Permission denied reading this folder".into());
    assert!(draw(&mut view, 95, 32).contains("Permission denied"));
    view.file_browser.error = None;
    view.file_browser.loading = true;
    assert!(draw(&mut view, 95, 32).contains("Reading this folder"));
}
#[test]
fn selection_scrolls_into_view_and_mouse_actions_target_their_post() {
    let mut view = sample();
    view.selected = 9;
    draw(&mut view, 100, 35);
    assert!(view.offset > 0 && view.offset <= 9);
    assert!(view.hits.iter().any(|h| matches!(h.hit, Hit::Select(9))));
    assert!(
        view.hits
            .iter()
            .any(|h| matches!(&h.hit,Hit::Command(id) if id=="post:9:favourite"))
    );
    view.modal = Modal::Palette {
        query: "write".into(),
        selected: 0,
    };
    draw(&mut view, 100, 35);
    assert!(
        !view
            .hits
            .iter()
            .any(|h| matches!(h.hit, Hit::Select(_) | Hit::Navigate(_)))
    );
    assert!(
        view.hits
            .iter()
            .any(|h| matches!(h.hit, Hit::ModalSelect(0)))
    );
}
#[test]
fn notification_content_warning_is_respected() {
    let mut view = sample();
    view.feed = Feed::Notifications;
    let post = view.notifications[0].post.as_mut().unwrap();
    post.text = "NEVER_LEAK_NOTIFICATION".into();
    post.spoiler_text = "Hidden subject".into();
    let screen = draw(&mut view, 150, 40);
    assert!(screen.contains("Hidden subject"));
    assert!(!screen.contains("NEVER_LEAK_NOTIFICATION"));
}
#[test]
fn empty_loading_and_error_states_stay_readable() {
    let mut view = View::default();
    assert!(draw(&mut view, 80, 24).contains("A little quiet here"));
    view.loading = true;
    assert!(draw(&mut view, 80, 24).contains("Finding your people"));
    view.loading = false;
    view.notice = Some(Notice {
        message: "Connection could not be reached".into(),
        error: true,
    });
    assert!(draw(&mut view, 80, 24).contains("Connection could not be reached"));
}
