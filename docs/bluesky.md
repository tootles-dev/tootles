# Bluesky accounts

Tootles can keep Mastodon and Bluesky accounts together. Press **a** to choose an account; the active network appears in the interface. Timelines, pending requests, and drafts stay separate when you switch.

## Connect

Create an app password in [Bluesky's account settings](https://bsky.app/settings/app-passwords), then run:

```sh
tootles login --provider bluesky your-handle.bsky.social
```

Type the app password at the hidden prompt and press Enter. Use an app password created for Tootles, not your main account password. The app password is sent only to the login service you selected and is not saved. This CLI uses password sessions rather than browser OAuth, consistent with the [AT Protocol guidance for command-line tools](https://atproto.com/guides/sdk-auth).

The default login service is `https://bsky.social`. Use `--pds https://your-pds.example` for a different account host. The returned DID document identifies the PDS used for subsequent requests; the client validates the identity and origin before using it. See Bluesky's [entryway explanation](https://bsky.network/docs/entryway/).

For automation, add `--app-password-stdin` and provide the password through standard input from your secret manager. Passwords are not accepted as command arguments. `--label NAME` gives the account a friendly name in the switcher.

```sh
tootles accounts
tootles switch 2
tootles
```

Mastodon login remains `tootles login YOUR_SERVER`; existing accounts need no migration.

## Supported features

| Feature | Bluesky behavior |
| --- | --- |
| Home, profiles, search, hashtags | Browse posts and people; follow pagination cursors for supported feeds. |
| Conversations and notifications | Open replies and parent posts with Enter; Backspace or ← returns to the previous view. |
| Likes and reposts | **f** likes/unlikes; **b** reposts/removes your repost. |
| Bookmarks | **s** adds/removes a bookmark; **5** opens your saved posts. |
| Account actions | Follow, unfollow, mute, unmute, block, and unblock. |
| Publishing | Public posts and replies, with links, mentions, hashtags, and a language tag. |
| Images | Up to four images with alt text, at most 2,000,000 bytes each; the adapter validates file type and size before upload. |
| Media viewing | The existing terminal gallery, default image viewer, browser, and optional mpv video playback. |
| Drafts and deletion | Private local drafts per account; delete your own posts. |

Bluesky posts are limited to 300 grapheme clusters and 3,000 UTF-8 bytes. Combined emoji and combining characters count as graphemes. The composer shows the applicable limit. Unsupported features are hidden or rejected before publishing: Mastodon-style private/unlisted/direct post visibility, polls, editing existing posts, free-form content warnings, and video/audio uploads. Bluesky chat, quote-post composition, list management, custom feed subscriptions, and labeler settings are not included in this adapter. Bluesky accounts use Home and Search instead of Mastodon's local and federated timelines.

Existing image and video attachments can be viewed even when this adapter cannot upload that media type. Images use the full-size URL provided by Bluesky's AppView; that CDN representation may differ from the uploaded source. The default image viewer receives those bytes without additional scaling by Tootles. Returned moderation labels and unavailable/blocked posts are handled conservatively; this is not a complete implementation of Bluesky's customizable moderation preferences.

## Sessions and troubleshooting

Session tokens are stored in the same private configuration directory as Mastodon tokens. Access tokens refresh automatically using the saved refresh token. Tootles saves rotated tokens while preserving your other accounts and preferences. Tokens are plaintext protected by file permissions, not a system keyring.

If a session expires or its app password is revoked, repeat the login command. A refresh failure is shown in the interface; the client does not ask for or save your main account password. Avoid editing saved token JSON by hand.

`tootles logout` removes the local session. Revoke Tootles' app password in Bluesky settings to invalidate it remotely. Your local drafts remain available.

API errors omit response bodies and credential values. If a request fails, report the command or action and error message, without tokens, app passwords, or the contents of `config.toml`.

The adapter is checked against mocked API responses and real terminal interactions with a local server. Connecting a real account is still needed to verify server-specific behavior end to end.
