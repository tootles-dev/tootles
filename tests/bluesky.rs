//! Offline wire-level coverage: no Bluesky credentials or live mutations.
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tootles::{
    bluesky_auth::Session,
    config::SavedAccount,
    model::*,
    provider::{SocialProvider, bluesky::Bluesky},
};
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{body_partial_json, header, method, path, query_param},
};

const ME: &str = "did:plc:abcdefghijklmnopqrstuvwx";
const OTHER: &str = "did:plc:zyxwvutsrqponmlkjihgfedcb";
const CID: &str = "bafyreiaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
fn uri(who: &str, key: &str) -> String {
    format!("at://{who}/app.bsky.feed.post/{key}")
}
fn profile(who: &str) -> Value {
    json!({"did":who,"handle":if who==ME{"alice.bsky.social"}else{"bob.example"},"displayName":"Alice 🌱","description":"Open social web","followersCount":18,"followsCount":9,"postsCount":40,"viewer":{}})
}
fn view(who: &str, key: &str, text: &str) -> Value {
    json!({"uri":uri(who,key),"cid":CID,"author":profile(who),"record":{"$type":"app.bsky.feed.post","text":text,"createdAt":"2026-11-22T12:00:00Z"},"indexedAt":"2026-11-22T12:00:01Z","likeCount":3,"repostCount":2,"replyCount":1,"viewer":{}})
}
async fn setup() -> (MockServer, Bluesky) {
    let server = MockServer::start().await;
    let session = Session {
        did: ME.into(),
        handle: "alice.bsky.social".into(),
        access_jwt: "test-access-secret".into(),
        refresh_jwt: "test-refresh-secret".into(),
    };
    let saved = SavedAccount {
        id: format!("bluesky:{ME}"),
        label: "Alice · Bluesky".into(),
        provider: "bluesky".into(),
        instance: server.uri(),
        token: serde_json::to_string(&session).unwrap(),
    };
    (server, Bluesky::from_saved(&saved, None).unwrap())
}
async fn get(server: &MockServer, endpoint: &str, response: Value) {
    Mock::given(method("GET"))
        .and(path(format!("/xrpc/{endpoint}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .mount(server)
        .await;
}
async fn own_profile(server: &MockServer) {
    get(server, "app.bsky.actor.getProfile", profile(ME)).await;
}

#[tokio::test]
async fn timeline_preserves_reposts_media_warnings_facets_and_opaque_cursor() {
    let (server, client) = setup().await;
    let mut p = view(OTHER, "post1", "🌱 linked story");
    p["record"]["facets"] = json!([
        {"index":{"byteStart":5,"byteEnd":11},"features":[{"$type":"app.bsky.richtext.facet#link","uri":"https://example.org/article?x=1"}]},
        {"index":{"byteStart":1,"byteEnd":3},"features":[{"$type":"app.bsky.richtext.facet#link","uri":"https://evil.example/invalid-byte-range"}]},
        {"index":{"byteStart":5,"byteEnd":11},"features":[{"$type":"app.bsky.richtext.facet#link","uri":"javascript:alert(1)"}]}
    ]);
    p["labels"] = json!([{"val":"graphic-media"},{"val":"porn","neg":true}]);
    p["embed"] = json!({"$type":"app.bsky.embed.recordWithMedia#view","media":{"$type":"app.bsky.embed.images#view","images":[{"fullsize":"https://cdn.example/image.png","thumb":"https://cdn.example/thumb.png","alt":"A garden\u{1b}[31m"}]},"record":{"$type":"app.bsky.embed.record#view","record":{"$type":"app.bsky.embed.record#viewRecord","uri":uri(ME,"quote1"),"cid":CID,"author":profile(ME),"value":{"text":"Quoted text"},"labels":[{"val":"sexual"}]}}});
    let repost = format!("at://{ME}/app.bsky.feed.repost/repost1");
    let mut hidden = view(OTHER, "hidden", "do not display");
    hidden["labels"] = json!([{"val":"!hide"}]);
    Mock::given(method("GET")).and(path("/xrpc/app.bsky.feed.getTimeline")).and(query_param("cursor","opaque /?x=1")).and(query_param("limit","30")).and(header("authorization","Bearer test-access-secret")).and(header("atproto-proxy","did:web:api.bsky.app#bsky_appview"))
      .respond_with(ResponseTemplate::new(200).set_body_json(json!({"cursor":"next/opaque", "feed":[{"post":p,"reason":{"$type":"app.bsky.feed.defs#reasonRepost","by":profile(ME),"uri":repost}},{"post":hidden}]}))).expect(1).mount(&server).await;
    let page = client
        .feed(&Feed::Home, Some("opaque /?x=1"))
        .await
        .unwrap();
    assert_eq!(page.next.as_deref(), Some("next/opaque"));
    assert_eq!(page.items.len(), 1);
    let p = &page.items[0];
    assert_eq!(p.id, uri(OTHER, "post1"));
    assert_eq!(p.timeline_id, repost);
    assert_eq!(p.boosted_by.as_deref(), Some("Alice 🌱"));
    assert!(p.text.contains("https://example.org/article?x=1"));
    assert!(p.text.contains("Quoted text"));
    assert!(!p.text.contains("evil.example"));
    assert!(!p.text.contains("javascript"));
    assert!(p.sensitive);
    assert!(p.spoiler_text.contains("graphic-media"));
    assert!(p.spoiler_text.contains("sexual"));
    assert!(!p.spoiler_text.contains("porn"));
    assert_eq!(p.media.len(), 1);
    assert!(!p.media[0].description.contains('\u{1b}'));
}

#[tokio::test]
async fn profiles_search_tag_likes_and_bookmarks_use_distinct_real_endpoints() {
    let (server, client) = setup().await;
    own_profile(&server).await;
    assert_eq!(client.me().await.unwrap().followers_count, Some(18));
    Mock::given(path("/xrpc/app.bsky.feed.getAuthorFeed"))
        .and(query_param("actor", OTHER))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"feed":[{"post":view(OTHER,"a","Profile")}]})),
        )
        .mount(&server)
        .await;
    Mock::given(path("/xrpc/app.bsky.feed.getActorLikes"))
        .and(query_param("actor", ME))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"feed":[{"post":view(OTHER,"b","Liked")}]})),
        )
        .mount(&server)
        .await;
    Mock::given(path("/xrpc/app.bsky.feed.searchPosts"))
        .and(query_param("q", "#Rust"))
        .and(query_param("tag", "Rust"))
        .and(query_param("cursor", "tagcursor"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"posts":[view(OTHER,"c","#Rust")],"cursor":"nexttag"})),
        )
        .mount(&server)
        .await;
    get(&server,"app.bsky.bookmark.getBookmarks",json!({"bookmarks":[{"subject":{"uri":uri(OTHER,"d"),"cid":CID},"item":view(OTHER,"d","Saved")},{"item":{"$type":"app.bsky.feed.defs#notFoundPost","uri":uri(OTHER,"gone"),"notFound":true}}],"cursor":"savednext"})).await;
    assert_eq!(
        client
            .feed(&Feed::Profile(OTHER.into()), None)
            .await
            .unwrap()
            .items[0]
            .text,
        "Profile"
    );
    assert_eq!(
        client.feed(&Feed::Favourites, None).await.unwrap().items[0].text,
        "Liked"
    );
    assert_eq!(
        client
            .feed(&Feed::Tag("Rust".into()), Some("tagcursor"))
            .await
            .unwrap()
            .next
            .as_deref(),
        Some("nexttag")
    );
    let saved = client.feed(&Feed::Bookmarks, None).await.unwrap();
    assert_eq!(saved.items.len(), 1);
    assert!(saved.items[0].bookmarked);
    Mock::given(path("/xrpc/app.bsky.feed.searchPosts"))
        .and(query_param("q", "open web"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"posts":[view(OTHER,"s","Open web")]})),
        )
        .mount(&server)
        .await;
    get(
        &server,
        "app.bsky.actor.searchActors",
        json!({"actors":[profile(OTHER)]}),
    )
    .await;
    let results = client.search("open web").await.unwrap();
    assert_eq!(results.accounts[0].id, OTHER);
    assert_eq!(results.posts[0].id, uri(OTHER, "s"));
}

#[tokio::test]
async fn notification_hydration_and_thread_order_preserve_actual_post_ids() {
    let (server, client) = setup().await;
    get(&server,"app.bsky.notification.listNotifications",json!({"notifications":[
        {"uri":format!("at://{OTHER}/app.bsky.feed.like/like1"),"cid":CID,"author":profile(OTHER),"reason":"like","reasonSubject":uri(ME,"root"),"record":{},"isRead":false,"indexedAt":"2026-11-22T12:00:00Z"},
        {"uri":uri(OTHER,"reply"),"cid":CID,"author":profile(OTHER),"reason":"reply","reasonSubject":uri(ME,"root"),"record":{},"isRead":false,"indexedAt":"2026-11-22T12:00:00Z"},
        {"uri":format!("at://{OTHER}/app.bsky.graph.follow/new"),"cid":CID,"author":profile(OTHER),"reason":"follow","record":{},"isRead":false,"indexedAt":"2026-11-22T12:00:00Z"}
    ],"cursor":"notify-next"})).await;
    get(
        &server,
        "app.bsky.feed.getPosts",
        json!({"posts":[view(ME,"root","Root"),view(OTHER,"reply","Reply")]}),
    )
    .await;
    let page = client.notifications(None).await.unwrap();
    assert_eq!(page.next.as_deref(), Some("notify-next"));
    assert_eq!(page.items[0].kind, "favourite");
    assert_eq!(page.items[0].post.as_ref().unwrap().id, uri(ME, "root"));
    assert_eq!(page.items[1].post.as_ref().unwrap().id, uri(OTHER, "reply"));
    assert!(page.items[2].post.is_none());
    get(&server,"app.bsky.feed.getPostThread",json!({"thread":{"post":view(OTHER,"reply","Reply"),"parent":{"post":view(ME,"root","Root"),"parent":{"post":view(ME,"older","Older")}},"replies":[{"post":view(ME,"child","Child"),"replies":[{"post":view(OTHER,"grandchild","Grandchild")}]},{"$type":"app.bsky.feed.defs#blockedPost","blocked":true}]}})).await;
    let thread = client.context(&uri(OTHER, "reply")).await.unwrap();
    assert_eq!(
        thread
            .ancestors
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>(),
        ["Older", "Root"]
    );
    assert_eq!(
        thread
            .descendants
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>(),
        ["Child", "Grandchild"]
    );
}

#[tokio::test]
async fn actions_write_strong_refs_and_only_delete_own_matching_records() {
    let (server, client) = setup().await;
    let id = uri(OTHER, "p");
    get(
        &server,
        "app.bsky.feed.getPosts",
        json!({"posts":[view(OTHER,"p","Post")]}),
    )
    .await;
    for (collection, action) in [
        ("app.bsky.feed.like", PostAction::Favourite(true)),
        ("app.bsky.feed.repost", PostAction::Boost(true)),
    ] {
        Mock::given(method("POST")).and(path("/xrpc/com.atproto.repo.createRecord")).and(body_partial_json(json!({"repo":ME,"collection":collection,"record":{"$type":collection,"subject":{"uri":id,"cid":CID}}}))).respond_with(ResponseTemplate::new(200).set_body_json(json!({"uri":format!("at://{ME}/{collection}/created"),"cid":CID}))).expect(1).mount(&server).await;
        let result = client.post_action(&id, action).await.unwrap().unwrap();
        if collection.ends_with("like") {
            assert!(result.favourited);
            assert_eq!(result.favourites_count, 4)
        } else {
            assert!(result.reblogged);
            assert_eq!(result.reblogs_count, 3)
        }
    }
    Mock::given(method("POST"))
        .and(path("/xrpc/app.bsky.bookmark.createBookmark"))
        .and(body_partial_json(json!({"uri":id,"cid":CID})))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    assert!(
        client
            .post_action(&id, PostAction::Bookmark(true))
            .await
            .unwrap()
            .unwrap()
            .bookmarked
    );
    Mock::given(method("POST"))
        .and(path("/xrpc/app.bsky.bookmark.deleteBookmark"))
        .and(body_partial_json(json!({"uri":id})))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    assert!(
        !client
            .post_action(&id, PostAction::Bookmark(false))
            .await
            .unwrap()
            .unwrap()
            .bookmarked
    );
    assert!(
        client
            .post_action(&id, PostAction::Delete)
            .await
            .unwrap_err()
            .to_string()
            .contains("your own")
    );
    for bad in [
        "https://evil.example/p",
        &format!("at://{ME}/app.bsky.graph.follow/x"),
        &format!("at://{ME}/app.bsky.feed.post/../x"),
        &format!("at://{ME}/app.bsky.feed.post/a?token=secret"),
    ] {
        assert!(client.post_action(bad, PostAction::Delete).await.is_err());
    }
    Mock::given(path("/xrpc/com.atproto.repo.deleteRecord"))
        .and(body_partial_json(
            json!({"repo":ME,"collection":"app.bsky.feed.post","rkey":"own"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    assert!(
        client
            .post_action(&uri(ME, "own"), PostAction::Delete)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn corrupt_viewer_record_cannot_delete_another_account_or_collection() {
    for reference in [
        format!("at://{OTHER}/app.bsky.feed.like/x"),
        format!("at://{ME}/app.bsky.feed.post/x"),
    ] {
        let (server, client) = setup().await;
        let mut p = view(OTHER, "p", "Post");
        p["viewer"]["like"] = reference.into();
        get(&server, "app.bsky.feed.getPosts", json!({"posts":[p]})).await;
        assert!(
            client
                .post_action(&uri(OTHER, "p"), PostAction::Favourite(false))
                .await
                .is_err()
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn following_muting_blocking_and_removal_use_authenticated_identity() {
    let (server, client) = setup().await;
    get(&server, "app.bsky.actor.getProfile", profile(OTHER)).await;
    for (collection, action) in [
        ("app.bsky.graph.follow", AccountAction::Follow(true)),
        ("app.bsky.graph.block", AccountAction::Block(true)),
    ] {
        Mock::given(method("POST"))
            .and(path("/xrpc/com.atproto.repo.createRecord"))
            .and(body_partial_json(
                json!({"repo":ME,"collection":collection,"record":{"subject":OTHER}}),
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"uri":format!("at://{ME}/{collection}/new"),"cid":CID})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let relation = client.account_action(OTHER, action).await.unwrap();
        if collection.ends_with("follow") {
            assert!(relation.following)
        } else {
            assert!(relation.blocking)
        }
    }
    for (endpoint, want) in [("muteActor", true), ("unmuteActor", false)] {
        Mock::given(path(format!("/xrpc/app.bsky.graph.{endpoint}")))
            .and(body_partial_json(json!({"actor":OTHER})))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            client
                .account_action(OTHER, AccountAction::Mute(want))
                .await
                .unwrap()
                .muting,
            want
        );
    }
    server.reset().await;
    let mut p = profile(OTHER);
    p["viewer"] = json!({"following":format!("at://{ME}/app.bsky.graph.follow/new"),"followedBy":format!("at://{OTHER}/app.bsky.graph.follow/f2"),"muted":true});
    get(&server, "app.bsky.actor.getProfile", p).await;
    let relation = client.relationship(OTHER).await.unwrap();
    assert!(relation.following && relation.followed_by && relation.muting);
    Mock::given(path("/xrpc/com.atproto.repo.deleteRecord"))
        .and(body_partial_json(
            json!({"repo":ME,"collection":"app.bsky.graph.follow","rkey":"new"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    assert!(
        !client
            .account_action(OTHER, AccountAction::Follow(false))
            .await
            .unwrap()
            .following
    );
}

// A tiny in-memory PDS emulates create-only records and subsequent lookup. This
// verifies stable rkeys across provider instances, not just an in-memory cache.
async fn publication_server(
    server: &MockServer,
    fail_after_commit: bool,
) -> Arc<Mutex<HashMap<String, Value>>> {
    own_profile(server).await;
    let records = Arc::new(Mutex::new(HashMap::<String, Value>::new()));
    let copy = records.clone();
    Mock::given(path("/xrpc/com.atproto.repo.getRecord"))
        .respond_with(move |request: &Request| {
            let key = request
                .url
                .query_pairs()
                .find(|(k, _)| k == "rkey")
                .unwrap()
                .1
                .to_string();
            match copy.lock().unwrap().get(&key) {
                Some(value) => ResponseTemplate::new(200).set_body_json(value),
                None => ResponseTemplate::new(400)
                    .set_body_json(json!({"error":"RecordNotFound","message":"Missing record"})),
            }
        })
        .mount(server)
        .await;
    let copy = records.clone();
    Mock::given(path("/xrpc/com.atproto.repo.createRecord"))
        .respond_with(move |request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let key = body["rkey"].as_str().unwrap();
            let result = json!({"uri":uri(ME,key),"cid":CID,"value":body["record"]});
            let mut records = copy.lock().unwrap();
            assert!(
                !records.contains_key(key),
                "No duplicate create request on deliberate retry"
            );
            records.insert(key.into(), result.clone());
            if fail_after_commit {
                ResponseTemplate::new(500).set_body_json(json!({"error":"InternalError"}))
            } else {
                ResponseTemplate::new(200).set_body_json(result)
            }
        })
        .expect(1)
        .mount(server)
        .await;
    // New posts are deliberately not indexed; a committed publication must still
    // succeed rather than tell the user to retry an already completed mutation.
    get(server, "app.bsky.feed.getPosts", json!({"posts":[]})).await;
    records
}
fn compose(text: &str) -> Compose {
    Compose {
        text: text.into(),
        idempotency_key: "1795348800000000:827bd9cb-a132-48f5-9ffd-4be9bfef487d".into(),
        ..Compose::default()
    }
}

#[tokio::test]
async fn publication_has_utf8_facets_and_stable_idempotency_across_client_restart() {
    let (server, client) = setup().await;
    let records = publication_server(&server, false).await;
    get(
        &server,
        "com.atproto.identity.resolveHandle",
        json!({"did":OTHER}),
    )
    .await;
    let draft = compose("🌱 Hi @bob.example — #Rust https://example.org/docs.");
    let posted = tokio::time::timeout(Duration::from_secs(5), client.publish(&draft))
        .await
        .expect("publication must not deadlock upgrading cached profile")
        .unwrap();
    assert_eq!(posted.text, draft.text);
    assert_eq!(posted.account.id, ME);
    let stored = records.lock().unwrap().values().next().unwrap().clone();
    let facets = stored["value"]["facets"].as_array().unwrap();
    assert_eq!(facets.len(), 3);
    for facet in facets {
        let start = facet["index"]["byteStart"].as_u64().unwrap() as usize;
        let end = facet["index"]["byteEnd"].as_u64().unwrap() as usize;
        let substring = &draft.text[start..end];
        match facet["features"][0]["$type"].as_str().unwrap() {
            "app.bsky.richtext.facet#mention" => {
                assert_eq!(substring, "@bob.example");
                assert_eq!(facet["features"][0]["did"], OTHER)
            }
            "app.bsky.richtext.facet#tag" => assert_eq!(substring, "#Rust"),
            _ => assert_eq!(substring, "https://example.org/docs"),
        }
    }
    let saved = SavedAccount {
        id: format!("bluesky:{ME}"),
        label: "Alice".into(),
        provider: "bluesky".into(),
        instance: server.uri(),
        token: serde_json::to_string(&Session {
            did: ME.into(),
            handle: "alice.bsky.social".into(),
            access_jwt: "test-access-secret".into(),
            refresh_jwt: "test-refresh-secret".into(),
        })
        .unwrap(),
    };
    let again = Bluesky::from_saved(&saved, None)
        .unwrap()
        .publish(&draft)
        .await
        .unwrap();
    assert_eq!(posted.id, again.id);
    let mut changed = draft.clone();
    changed.text = "New content using the same key".into();
    assert!(
        client
            .publish(&changed)
            .await
            .unwrap_err()
            .to_string()
            .contains("different content")
    );
    let key = posted.id.rsplit('/').next().unwrap();
    assert_eq!(key.len(), 13);
    assert!(
        key.bytes()
            .all(|b| b"234567abcdefghijklmnopqrstuvwxyz".contains(&b))
    );
}

#[tokio::test]
async fn ambiguous_write_result_recovers_exact_committed_record_without_reposting() {
    let (server, client) = setup().await;
    publication_server(&server, true).await;
    let post = client
        .publish(&compose("Committed despite response failure"))
        .await
        .unwrap();
    assert_eq!(post.text, "Committed despite response failure");
}

#[tokio::test]
async fn reply_preserves_thread_root_and_parent_strong_refs() {
    let (server, client) = setup().await;
    let records = publication_server(&server, false).await;
    let mut parent = view(OTHER, "parent", "Reply parent");
    parent["record"]["reply"] = json!({"root":{"uri":uri(ME,"root"),"cid":"bafyroot"},"parent":{"uri":uri(ME,"root"),"cid":"bafyroot"}});
    Mock::given(path("/xrpc/app.bsky.feed.getPosts"))
        .and(query_param("uris", uri(OTHER, "parent")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"posts":[parent]})))
        .with_priority(1)
        .mount(&server)
        .await;
    let mut draft = compose("A public reply");
    draft.reply_to = Some(uri(OTHER, "parent"));
    client.publish(&draft).await.unwrap();
    let saved = records.lock().unwrap().values().next().unwrap().clone();
    assert_eq!(
        saved["value"]["reply"],
        json!({"root":{"uri":uri(ME,"root"),"cid":"bafyroot"},"parent":{"uri":uri(OTHER,"parent"),"cid":CID}})
    );
}

#[tokio::test]
async fn image_upload_preserves_alt_text_bytes_and_validated_dimensions() {
    let (server, client) = setup().await;
    let records = publication_server(&server, false).await;
    let dir = tempfile::tempdir().unwrap();
    let filename = dir.path().join("garden with spaces.png");
    image::RgbImage::new(20, 12).save(&filename).unwrap();
    let bytes = std::fs::read(&filename).unwrap();
    let expected = bytes.clone();
    let cid = blob_cid(&bytes);
    Mock::given(method("POST")).and(path("/xrpc/com.atproto.repo.uploadBlob")).and(header("content-type","image/png")).respond_with(move|request:&Request|{assert_eq!(request.body,expected);assert!(request.headers.get("atproto-proxy").is_none());ResponseTemplate::new(200).set_body_json(json!({"blob":{"$type":"blob","ref":{"$link":cid},"mimeType":"image/png","size":request.body.len()}}))}).expect(1).mount(&server).await;
    let mut draft = compose("A garden");
    draft.attachments.push(Attachment {
        path: filename.to_str().unwrap().into(),
        description: "🌱 Green leaves in morning light".into(),
    });
    client.publish(&draft).await.unwrap();
    let stored = records.lock().unwrap().values().next().unwrap().clone();
    let embed = &stored["value"]["embed"];
    assert_eq!(embed["$type"], "app.bsky.embed.images");
    assert_eq!(embed["images"][0]["alt"], draft.attachments[0].description);
    assert_eq!(
        embed["images"][0]["aspectRatio"],
        json!({"width":20,"height":12})
    );
    client.publish(&draft).await.unwrap(); // No second upload on idempotent retry.
}

#[tokio::test]
async fn unsupported_inputs_and_all_attachments_are_rejected_before_any_upload() {
    let (server, client) = setup().await;
    for mutate in 0..7 {
        let mut c = compose("text");
        match mutate {
            0 => c.visibility = Visibility::Private,
            1 => c.poll_options = vec!["a".into(), "b".into()],
            2 => c.edit_id = Some(uri(ME, "edit")),
            3 => c.spoiler_text = "Warning".into(),
            4 => c.sensitive = true,
            5 => c.text = "x".repeat(301),
            _ => c.idempotency_key.clear(),
        };
        assert!(client.publish(&c).await.is_err());
    }
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("good.png");
    image::RgbImage::new(1, 1).save(&good).unwrap();
    let bad = dir.path().join("video.mp4");
    std::fs::write(&bad, b"not an image").unwrap();
    let mut c = compose("Images");
    c.attachments = vec![
        Attachment {
            path: good.to_str().unwrap().into(),
            description: String::new(),
        },
        Attachment {
            path: bad.to_str().unwrap().into(),
            description: String::new(),
        },
    ];
    assert!(
        client
            .publish(&c)
            .await
            .unwrap_err()
            .to_string()
            .contains("video and audio")
    );
    let big = dir.path().join("big.png");
    std::fs::File::create(&big)
        .unwrap()
        .set_len(2_000_001)
        .unwrap();
    c.attachments = vec![Attachment {
        path: big.to_str().unwrap().into(),
        description: String::new(),
    }];
    assert!(
        client
            .publish(&c)
            .await
            .unwrap_err()
            .to_string()
            .contains("2,000,000")
    );
    for feed in [
        Feed::Local,
        Feed::Federated,
        Feed::List("id".into(), "title".into()),
    ] {
        assert!(client.feed(&feed, None).await.is_err());
    }
    assert!(client.source(&uri(ME, "x")).await.is_err());
    assert!(client.vote("poll", &[0]).await.is_err());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn malformed_identity_missing_fields_and_server_errors_remain_actionable_and_secret_free() {
    let (server, client) = setup().await;
    get(
        &server,
        "app.bsky.feed.getTimeline",
        json!({"unexpected":[]}),
    )
    .await;
    assert!(
        client
            .feed(&Feed::Home, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("missing feed")
    );
    server.reset().await;
    let mut malformed = view(OTHER, "post", "Text");
    malformed["author"]["did"] = ME.into();
    get(
        &server,
        "app.bsky.feed.getPosts",
        json!({"posts":[malformed]}),
    )
    .await;
    assert!(
        client
            .post(&uri(OTHER, "post"))
            .await
            .unwrap_err()
            .to_string()
            .contains("identity")
    );
    server.reset().await;
    Mock::given(path("/xrpc/app.bsky.feed.getTimeline"))
        .respond_with(
            ResponseTemplate::new(429)
                .set_body_json(
                    json!({"error":"RateLimitExceeded","message":"test-access-secret\u{1b}[31m"}),
                )
                .insert_header("retry-after", "120"),
        )
        .mount(&server)
        .await;
    let error = client
        .feed(&Feed::Home, None)
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains("test-access-secret"));
    assert!(!error.contains('\u{1b}'));
    assert!(error.to_lowercase().contains("rate"));
}

fn blob_cid(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut raw = vec![1, 0x55, 0x12, 0x20];
    raw.extend_from_slice(&Sha256::digest(bytes));
    let bits: String = raw.iter().map(|byte| format!("{byte:08b}")).collect();
    let mut result = String::from("b");
    for group in bits.as_bytes().chunks(5) {
        let mut value = 0;
        for b in group {
            value = value * 2 + usize::from(*b == b'1');
        }
        value <<= 5 - group.len();
        result.push(b"abcdefghijklmnopqrstuvwxyz234567"[value] as char);
    }
    result
}

#[tokio::test]
async fn immediate_undo_uses_confirmed_record_even_when_appview_is_stale() {
    let (server, client) = setup().await;
    get(
        &server,
        "app.bsky.feed.getPosts",
        json!({"posts":[view(OTHER,"p","Post")]}),
    )
    .await;
    get(&server, "app.bsky.actor.getProfile", profile(OTHER)).await;
    for collection in [
        "app.bsky.feed.like",
        "app.bsky.feed.repost",
        "app.bsky.graph.follow",
        "app.bsky.graph.block",
    ] {
        Mock::given(path("/xrpc/com.atproto.repo.createRecord"))
            .and(body_partial_json(json!({"collection":collection})))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"uri":format!("at://{ME}/{collection}/fresh"),"cid":CID}),
                ),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/xrpc/com.atproto.repo.deleteRecord"))
            .and(body_partial_json(
                json!({"repo":ME,"collection":collection,"rkey":"fresh"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
    }
    for (on, off) in [
        (PostAction::Favourite(true), PostAction::Favourite(false)),
        (PostAction::Boost(true), PostAction::Boost(false)),
    ] {
        client.post_action(&uri(OTHER, "p"), on).await.unwrap();
        client.post_action(&uri(OTHER, "p"), on).await.unwrap();
        client.post_action(&uri(OTHER, "p"), off).await.unwrap();
    }
    for (on, off) in [
        (AccountAction::Follow(true), AccountAction::Follow(false)),
        (AccountAction::Block(true), AccountAction::Block(false)),
    ] {
        client.account_action(OTHER, on).await.unwrap();
        client.account_action(OTHER, on).await.unwrap();
        client.account_action(OTHER, off).await.unwrap();
    }
}

#[tokio::test]
async fn gallery_video_and_quoted_account_moderation_are_preserved() {
    let (server, client) = setup().await;
    let mut gallery = view(OTHER, "gallery", "Twenty images");
    gallery["embed"] = json!({"$type":"app.bsky.embed.gallery#view","items":(0..20).map(|i|json!({"$type":"app.bsky.embed.gallery#viewImage","fullsize":format!("https://cdn.example/{i}.jpg"),"alt":format!("Photo {i}")})).collect::<Vec<_>>()});
    let mut video = view(OTHER, "video", "Video");
    video["embed"] = json!({"$type":"app.bsky.embed.video#view","playlist":"https://video.example/playlist.m3u8","cid":CID,"alt":"A trail"});
    let mut quote = view(OTHER, "quote", "Quoted account warning");
    let mut quoted_author = profile(ME);
    quoted_author["labels"] = json!([{"val":"sexual"}]);
    quote["embed"] = json!({"$type":"app.bsky.embed.record#view","record":{"$type":"app.bsky.embed.record#viewRecord","author":quoted_author,"value":{"text":"Sensitive quotation"}}});
    let mut blocked = quote.clone();
    blocked["uri"] = uri(OTHER, "blockedquote").into();
    blocked["embed"]["record"]["author"]["viewer"]["blockingByList"] = json!({"uri":"at://list"});
    get(
        &server,
        "app.bsky.feed.getTimeline",
        json!({"feed":[{"post":gallery},{"post":video},{"post":quote},{"post":blocked}]}),
    )
    .await;
    let posts = client.feed(&Feed::Home, None).await.unwrap().items;
    assert_eq!(posts[0].media.len(), 20);
    assert_eq!(posts[1].media[0].kind, "video");
    assert!(posts[2].sensitive);
    assert!(posts[2].spoiler_text.contains("sexual"));
    assert!(!posts[3].text.contains("Sensitive quotation"));
    assert!(posts[3].text.contains("unavailable"));
}

#[tokio::test]
async fn same_size_changed_image_is_not_mistaken_for_an_idempotent_retry() {
    let (server, client) = setup().await;
    publication_server(&server, false).await;
    let mut lengths = HashMap::<usize, Vec<u8>>::new();
    let mut pair = None;
    for color in 0..=255u8 {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            2,
            2,
            image::Rgb([color, 0, 0]),
        ));
        let mut encoded = std::io::Cursor::new(Vec::new());
        img.write_to(&mut encoded, image::ImageFormat::Png).unwrap();
        let bytes = encoded.into_inner();
        if let Some(previous) = lengths.get(&bytes.len()) {
            pair = Some((previous.clone(), bytes));
            break;
        }
        lengths.insert(bytes.len(), bytes);
    }
    let (original, changed) = pair.expect("PNG fixtures of equal size");
    assert_ne!(original, changed);
    assert_eq!(original.len(), changed.len());
    let cid = blob_cid(&original);
    let length = original.len();
    Mock::given(path("/xrpc/com.atproto.repo.uploadBlob")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"blob":{"$type":"blob","ref":{"$link":cid},"mimeType":"image/png","size":length}}))).expect(1).mount(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let filename = dir.path().join("changed.png");
    std::fs::write(&filename, original).unwrap();
    let mut draft = compose("Same draft, replaced image");
    draft.attachments.push(Attachment {
        path: filename.to_str().unwrap().into(),
        description: "Same alt text".into(),
    });
    client.publish(&draft).await.unwrap();
    std::fs::write(&filename, changed).unwrap();
    assert!(
        client
            .publish(&draft)
            .await
            .unwrap_err()
            .to_string()
            .contains("different content")
    );
}
