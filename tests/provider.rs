use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use serde_json::{Value, json};
use tootles::{
    model::*,
    provider::{SocialProvider, demo::DemoProvider, mastodon::Mastodon},
};
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{body_json, body_partial_json, header, method, path, query_param},
};

fn wire_account() -> Value {
    json!({"id":"a1", "username":"alice", "acct":"alice@example.org", "display_name":"Alice", "note":"<p>Hello &amp; welcome</p>", "url":"https://example.org/@alice", "followers_count":12})
}
fn wire_post(id: &str) -> Value {
    json!({"id":id, "account":wire_account(), "content":"<p>Hello &amp; welcome</p>", "url":format!("https://example.org/@alice/{id}"), "created_at":"2026-11-21T18:30:00Z", "visibility":"public", "favourites_count":3, "favourited":null, "reblogged":null, "bookmarked":null, "media_attachments":[], "mentions":[], "poll":null})
}
fn client(server: &MockServer) -> Mastodon {
    Mastodon::new(&server.uri(), "test-secret-token").unwrap()
}

#[tokio::test]
async fn parses_boosts_html_nullable_polls_and_correct_bookmark_cursor() {
    let server = MockServer::start().await;
    let mut post = wire_post("post-id");
    post["content"] = json!(
        "<p>Hello &amp; welcome</p><p><a href=\"https://example.org/story\">full story</a>&#27;</p>"
    );
    post["poll"] = json!({"id":"p1", "options":[{"title":"yes", "votes_count":null},{"title":"no", "votes_count":4}], "own_votes":null, "voted":null, "votes_count":4});
    post["media_attachments"] = json!([{"id":"m1", "type":"image", "url":null, "remote_url":"https://example.org/a.png", "description":null}]);
    let boost = json!({"id":"outer-id", "account":{"id":"b1","display_name":"Bob"}, "reblog":post});
    Mock::given(method("GET")).and(path("/api/v1/bookmarks")).and(query_param("limit", "30")).and(header("authorization", "Bearer test-secret-token"))
        .respond_with(ResponseTemplate::new(200).insert_header("Link", format!("<https://other.example/api/v1/bookmarks?max_id=bookmark-cursor>; rel=\"next\", <{}?min_id=other>; rel=\"prev\"", server.uri())).set_body_json(json!([boost])))
        .expect(1).mount(&server).await;
    let page = client(&server).feed(&Feed::Bookmarks, None).await.unwrap();
    assert_eq!(page.next.as_deref(), Some("bookmark-cursor"));
    let post = &page.items[0];
    assert_eq!(post.id, "post-id");
    assert_eq!(post.timeline_id, "outer-id");
    assert_eq!(post.boosted_by.as_deref(), Some("Bob"));
    assert_eq!(
        post.text,
        "Hello & welcome\n\nfull story (https://example.org/story)"
    );
    assert!(!post.favourited);
    assert_eq!(post.poll.as_ref().unwrap().options[0].votes_count, 0);
    assert_eq!(post.media[0].url, "https://example.org/a.png");
}

#[tokio::test]
async fn pagination_stays_on_original_origin_and_uses_link_id() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/favourites"))
        .and(query_param("max_id", "opaque/bookmark?cursor"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([wire_post("1")])))
        .expect(1)
        .mount(&server)
        .await;
    let page = client(&server)
        .feed(&Feed::Favourites, Some("opaque/bookmark?cursor"))
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(
        page.next.is_none(),
        "no invented pagination ID from a referenced status"
    );
}

#[tokio::test]
async fn server_filters_hide_posts_and_warn_with_content_warning() {
    let server = MockServer::start().await;
    let mut hidden = wire_post("hidden");
    hidden["filtered"] = json!([{"filter":{"filter_action":"hide","title":"hide this"}}]);
    let mut warned = wire_post("warned");
    warned["filtered"] = json!([{"filter":{"filter_action":"warn","title":"spiders"}}]);
    Mock::given(path("/api/v1/timelines/home"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([hidden, warned])))
        .mount(&server)
        .await;
    let page = client(&server).feed(&Feed::Home, None).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].spoiler_text, "Filtered: spiders");
}

#[tokio::test]
async fn instance_character_limit_and_account_fields_are_loaded() {
    let server = MockServer::start().await;
    Mock::given(path("/api/v1/accounts/verify_credentials"))
        .respond_with(ResponseTemplate::new(200).set_body_json(wire_account()))
        .mount(&server)
        .await;
    Mock::given(path("/api/v2/instance"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"configuration":{"statuses":{"max_characters":1500}}})),
        )
        .mount(&server)
        .await;
    let provider = client(&server);
    let me = provider.me().await.unwrap();
    assert_eq!(me.note, "Hello & welcome");
    assert_eq!(me.followers_count, 12);
    assert_eq!(provider.capabilities().max_characters, 1500);
}

#[tokio::test]
async fn read_requests_retry_transient_gateway_errors_only() {
    let server = MockServer::start().await;
    let attempts = Arc::new(AtomicUsize::new(0));
    let count = attempts.clone();
    Mock::given(path("/api/v1/statuses/p1"))
        .respond_with(move |_: &Request| {
            if count.fetch_add(1, Ordering::SeqCst) < 2 {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(200).set_body_json(wire_post("p1"))
            }
        })
        .expect(3)
        .mount(&server)
        .await;
    assert_eq!(client(&server).post("p1").await.unwrap().id, "p1");
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn errors_never_echo_server_content_or_token_and_rate_limits_are_useful() {
    let server = MockServer::start().await;
    Mock::given(path("/api/v1/statuses/rate-limit"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "42")
                .set_body_json(json!({"error":"test-secret-token \u{1b}[31m"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let error = client(&server)
        .post("rate-limit")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("42s"));
    assert!(error.contains("429"));
    assert!(!error.contains("test-secret-token"));
    assert!(!error.contains('\u{1b}'));
}

#[tokio::test]
async fn never_follows_redirects_with_bearer_credentials() {
    let server = MockServer::start().await;
    let destination = MockServer::start().await;
    Mock::given(path("/leak"))
        .respond_with(ResponseTemplate::new(200).set_body_json(wire_post("p1")))
        .expect(0)
        .mount(&destination)
        .await;
    Mock::given(path("/api/v1/statuses/p1"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("{}/leak", destination.uri())),
        )
        .expect(1)
        .mount(&server)
        .await;
    let error = client(&server).post("p1").await.unwrap_err().to_string();
    assert!(error.contains("redirected"));
    assert!(destination.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn encodes_arbitrary_ids_as_one_path_component() {
    let server = MockServer::start().await;
    Mock::given(path("/api/v1/statuses/remote%2Fa%3Fb%23c"))
        .respond_with(ResponseTemplate::new(200).set_body_json(wire_post("encoded")))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        client(&server).post("remote/a?b#c").await.unwrap().id,
        "encoded"
    );
    assert!(client(&server).post("..").await.is_err());
}

#[tokio::test]
async fn status_actions_account_actions_relationships_and_lists_use_correct_verbs() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/statuses/p1/unfavourite"))
        .respond_with(ResponseTemplate::new(200).set_body_json(wire_post("p1")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/accounts/a1/follow"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"following":true,"followed_by":true})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/accounts/relationships"))
        .and(query_param("id[]", "a1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"following":true}])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/lists/l1/accounts"))
        .and(body_json(json!({"account_ids":["a1"]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/statuses/own"))
        .respond_with(ResponseTemplate::new(200).set_body_json(wire_post("own")))
        .expect(1)
        .mount(&server)
        .await;
    let provider = client(&server);
    assert!(
        !provider
            .post_action("p1", PostAction::Favourite(false))
            .await
            .unwrap()
            .unwrap()
            .favourited
    );
    assert!(
        provider
            .account_action("a1", AccountAction::Follow(true))
            .await
            .unwrap()
            .following
    );
    assert!(provider.relationship("a1").await.unwrap().following);
    provider.list_member("l1", "a1", false).await.unwrap();
    assert!(
        provider
            .post_action("own", PostAction::Delete)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn writes_are_never_implicitly_retried() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/statuses/p1/favourite"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    assert!(
        client(&server)
            .post_action("p1", PostAction::Favourite(true))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn publish_sends_poll_privacy_reply_cw_and_stable_idempotency_key() {
    let server = MockServer::start().await;
    let body = json!({"status":"Which one?", "spoiler_text":"A choice", "sensitive":true, "visibility":"private", "language":"en", "in_reply_to_id":"parent", "poll":{"options":["One","Two"],"multiple":true,"expires_in":3600}});
    Mock::given(method("POST"))
        .and(path("/api/v1/statuses"))
        .and(header("idempotency-key", "stable-draft-key"))
        .and(body_json(body))
        .respond_with(ResponseTemplate::new(200).set_body_json(wire_post("new")))
        .expect(1)
        .mount(&server)
        .await;
    let compose = Compose {
        text: "Which one?".into(),
        spoiler_text: "A choice".into(),
        sensitive: true,
        visibility: Visibility::Private,
        language: "en".into(),
        reply_to: Some("parent".into()),
        poll_options: vec!["One".into(), "Two".into()],
        poll_multiple: true,
        poll_expires_in: 3600,
        idempotency_key: "stable-draft-key".into(),
        ..Compose::default()
    };
    assert_eq!(client(&server).publish(&compose).await.unwrap().id, "new");
}

#[tokio::test]
async fn poll_votes_preserve_multiple_choices() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/api/v1/polls/p1/votes")).and(body_json(json!({"choices":[0,2]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"p1","multiple":true,"voted":true,"own_votes":[0,2],"votes_count":2,"options":[{"title":"A","votes_count":1},{"title":"B","votes_count":0},{"title":"C","votes_count":1}]})))
        .expect(1).mount(&server).await;
    let poll = client(&server).vote("p1", &[0, 2]).await.unwrap();
    assert_eq!(poll.own_votes, [0, 2]);
    assert!(poll.voted);
}

#[tokio::test]
async fn editing_uses_original_source_and_retains_existing_media() {
    let server = MockServer::start().await;
    let mut original = wire_post("own");
    original["content"] = json!("<p>Rendered text</p>");
    original["visibility"] = json!("private");
    original["media_attachments"] = json!([{"id":"existing-media","type":"image","url":"https://example.org/photo.png","description":"Alt text"}]);
    Mock::given(method("GET"))
        .and(path("/api/v1/statuses/own/source"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"text":"Original **source**","spoiler_text":"CW"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/statuses/own"))
        .respond_with(ResponseTemplate::new(200).set_body_json(original))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("PUT")).and(path("/api/v1/statuses/own")).and(body_partial_json(json!({"status":"Edited source", "spoiler_text":"CW","visibility":"private","media_ids":["existing-media"]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(wire_post("own"))).expect(1).mount(&server).await;
    let provider = client(&server);
    let mut compose = provider.source("own").await.unwrap();
    assert_eq!(compose.text, "Original **source**");
    assert_eq!(compose.visibility, Visibility::Private);
    compose.text = "Edited source".into();
    provider.publish(&compose).await.unwrap();
}

#[tokio::test]
async fn multipart_upload_waits_for_async_processing_and_includes_alt_text() {
    let server = MockServer::start().await;
    let directory = tempfile::tempdir().unwrap();
    let attachment = directory.path().join("image.png");
    std::fs::write(&attachment, b"test-image-bytes").unwrap();
    Mock::given(method("POST"))
        .and(path("/api/v2/media"))
        .and(|request: &Request| {
            let body = String::from_utf8_lossy(&request.body);
            body.contains("name=\"file\"")
                && body.contains("image/png")
                && body.contains("name=\"description\"")
                && body.contains("A forest path")
                && body.contains("test-image-bytes")
        })
        .respond_with(
            ResponseTemplate::new(202).set_body_json(json!({"id":"m1","type":"image","url":null})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let count = Arc::new(AtomicUsize::new(0));
    let attempts = count.clone();
    Mock::given(method("GET"))
        .and(path("/api/v1/media/m1"))
        .respond_with(move |_: &Request| {
            if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(206)
            } else {
                ResponseTemplate::new(200)
                    .set_body_json(json!({"id":"m1","url":"https://example.org/processed.png"}))
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/statuses"))
        .and(body_partial_json(json!({"media_ids":["m1"]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(wire_post("uploaded")))
        .expect(1)
        .mount(&server)
        .await;
    let compose = Compose {
        text: "Picture".into(),
        attachments: vec![Attachment {
            path: attachment.to_string_lossy().into_owned(),
            description: "A forest path".into(),
        }],
        idempotency_key: "media-key".into(),
        ..Compose::default()
    };
    assert_eq!(
        client(&server).publish(&compose).await.unwrap().id,
        "uploaded"
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn local_tag_search_and_notification_routes_are_distinct() {
    let server = MockServer::start().await;
    Mock::given(path("/api/v1/timelines/public"))
        .and(query_param("local", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/timelines/tag/Rust"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/api/v2/search")).and(query_param("q","Rust")).and(query_param("resolve","true")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"accounts":[wire_account()],"statuses":[wire_post("p1")],"hashtags":[{"name":"Rust"}]}))).expect(1).mount(&server).await;
    Mock::given(path("/api/v1/notifications"))
        .and(query_param("max_id", "n4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!([{"id":"n3","type":"follow","account":wire_account(),"status":null}]),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let provider = client(&server);
    provider.feed(&Feed::Local, None).await.unwrap();
    provider
        .feed(&Feed::Tag("#Rust".into()), None)
        .await
        .unwrap();
    let results = provider.search("Rust").await.unwrap();
    assert_eq!(results.accounts.len(), 1);
    assert_eq!(results.posts.len(), 1);
    assert_eq!(results.hashtags, ["Rust"]);
    assert!(
        provider.notifications(Some("n4")).await.unwrap().items[0]
            .post
            .is_none()
    );
}

#[tokio::test]
async fn demo_pagination_actions_and_relationships_are_persistent() {
    let demo = DemoProvider::new();
    let first = demo.feed(&Feed::Home, None).await.unwrap();
    assert_eq!(first.items.len(), 8);
    let second = demo.feed(&Feed::Home, first.next.as_deref()).await.unwrap();
    assert!(!second.items.is_empty());
    assert!(
        first
            .items
            .iter()
            .all(|p| second.items.iter().all(|q| p.id != q.id))
    );
    let post = &first.items[1];
    let updated = demo
        .post_action(&post.id, PostAction::Favourite(true))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.favourites_count, post.favourites_count + 1);
    demo.post_action(&post.id, PostAction::Favourite(true))
        .await
        .unwrap();
    assert_eq!(
        demo.post(&post.id).await.unwrap().favourites_count,
        updated.favourites_count
    );
    demo.post_action(&post.id, PostAction::Bookmark(true))
        .await
        .unwrap();
    assert!(
        demo.feed(&Feed::Bookmarks, None)
            .await
            .unwrap()
            .items
            .iter()
            .any(|p| p.id == post.id)
    );
    demo.account_action(&post.account.id, AccountAction::Mute(true))
        .await
        .unwrap();
    assert!(
        demo.feed(&Feed::Home, None)
            .await
            .unwrap()
            .items
            .iter()
            .all(|p| p.account.id != post.account.id)
    );
    demo.account_action(&post.account.id, AccountAction::Mute(false))
        .await
        .unwrap();
    assert!(!demo.relationship(&post.account.id).await.unwrap().muting);
}

#[tokio::test]
async fn demo_publishing_editing_replies_idempotency_and_deletion_are_coherent() {
    let demo = DemoProvider::new();
    let mut compose = Compose {
        text: "Hello from the test".into(),
        reply_to: Some("1008".into()),
        idempotency_key: "stable-key".into(),
        ..Compose::default()
    };
    let published = demo.publish(&compose).await.unwrap();
    assert_eq!(demo.publish(&compose).await.unwrap().id, published.id);
    assert!(
        demo.context("1008")
            .await
            .unwrap()
            .descendants
            .iter()
            .any(|p| p.id == published.id)
    );
    compose = demo.source(&published.id).await.unwrap();
    compose.text = "A thoughtful edit".into();
    let edited = demo.publish(&compose).await.unwrap();
    assert!(edited.edited_at.is_some());
    assert_eq!(edited.id, published.id);
    assert_eq!(
        demo.post(&published.id).await.unwrap().text,
        "A thoughtful edit"
    );
    assert!(demo.post_action("1016", PostAction::Delete).await.is_err());
    demo.post_action(&published.id, PostAction::Delete)
        .await
        .unwrap();
    assert!(demo.post(&published.id).await.is_err());
}

#[tokio::test]
async fn demo_polls_and_list_membership_are_live() {
    let demo = DemoProvider::new();
    let voted = demo.vote("poll-home", &[1]).await.unwrap();
    assert!(voted.voted);
    assert_eq!(voted.options[1].votes_count, 37);
    assert!(demo.vote("poll-home", &[1]).await.is_err());
    let list = demo.create_list("My reading room").await.unwrap();
    demo.list_member(&list.id, "maya", true).await.unwrap();
    let feed = Feed::List(list.id.clone(), list.title);
    let posts = demo.feed(&feed, None).await.unwrap().items;
    assert!(!posts.is_empty());
    assert!(posts.iter().all(|p| p.account.id == "maya"));
    demo.list_member(&list.id, "maya", false).await.unwrap();
    assert!(demo.feed(&feed, None).await.unwrap().items.is_empty());
}
