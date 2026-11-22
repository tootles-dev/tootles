# Architecture

Tootles separates the social network from the terminal interface. Mastodon is the first live adapter; the included demo adapter runs the same interface locally with seeded data. Adding another network means translating its API into the common models rather than rebuilding the UI.

```text
CLI / browser OAuth
        │
        ▼
app.rs ───────────── config.rs
  │                    accounts, preferences, drafts
  ├── View state ──── ui.rs ──── Ratatui / Crossterm / TachyonFX
  │
  └── SocialProvider
          ├── Mastodon ─── HTTPS REST API
          ├── Demo ─────── local sample data
          └── Your network adapter
```

## Module boundaries

| Module | Responsibility |
| --- | --- |
| `model` | Network-neutral accounts, posts, threads, polls, media, lists, and compose requests |
| `provider::SocialProvider` | Async reads and mutations; optional operations return an explicit unsupported error |
| `provider::mastodon` | Mastodon endpoints, pagination, HTML conversion, media uploads, and API errors |
| `provider::demo` | Local data and mutations for exploration and development without credentials |
| `view` | Renderable application state, modals, commands, and mouse hit targets |
| `ui` | Responsive terminal layout, color themes, widget rendering, and animation |
| `app` | Input handling, background requests, navigation, and state updates |
| `editor` | Unicode-aware text editing |
| `media` | Explicit attachment gallery, bounded public image downloads, and background image decoding |
| `config` | Atomic private file persistence and redacted account diagnostics |
| `auth` | Server validation, browser OAuth, credential verification, and safe browser launching |

The application works with `SocialProvider` through shared models. API response objects stay in adapters. A provider's capabilities declare posting, media, polls, lists, its display name, and the supported post length. Account IDs and pagination cursors are opaque strings: adapters must not assume other networks use Mastodon's numeric IDs.

## Adding a network

1. Add `src/provider/your_network.rs` and export it from `src/provider/mod.rs`.
2. Implement `SocialProvider` with `#[async_trait]`. Map accounts, posts, visibility, media, and notifications to `model`; preserve remote IDs and URLs.
3. Return accurate `Capabilities`. Implement the required operations and return useful errors for unavailable features; override optional operations when supported.
4. Add authentication for that network, save its provider name in `SavedAccount`, and extend the account-to-provider construction in `app` and the CLI login command.
5. Preserve account isolation for drafts and state when switching providers. Use opaque cursors and the returned pagination links where supported.
6. Add mocked HTTP tests for authorization, paging, mutations, and API errors. Exercise the adapter in the existing UI and check every capability-dependent action.

The provider boundary is a compiled Rust extension point, not a runtime plugin loader. No additional live networks ship in this release. A network with fundamentally different primitives may require extending the common models and capability flags.

## Attachment gallery

Press `m` to open a post's attachments after revealing any content warning. The gallery loads images on demand using the terminal protocol selected by `ratatui-image` (Kitty, Sixel, iTerm2, or Unicode half blocks). `h` / `l`, arrow keys, or the mouse wheel move between attachments. Enter opens the attachment in the default browser; video and audio use browser playback.

Media requests carry no account token. The gallery limits downloads to 16 MiB, decoding to 40 megapixels with a 256 MiB decoder allocation limit, and retained previews to 2048 pixels per dimension. At most four attachments are cached in memory. A single pending image job prevents rapid navigation from launching parallel decoders; the returned attachment index keeps asynchronous results associated with the correct image. Decoding, resizing, and terminal protocol encoding run outside the UI thread. No media cache is written to disk. Demo previews are bundled in the executable and load without a network connection.

## Credentials and drafts

The default directory is `$XDG_CONFIG_HOME/tootles`, falling back to `~/.config/tootles`. `--config-dir` selects a separate directory. Preferences and verified tokens are stored in `config.toml`; draft files are separated by account ID. Tokens are plaintext on disk and are protected by filesystem permissions rather than a system keyring. Keep the directory out of synced public folders and version control.

On Unix, Tootles creates directories with mode `0700` and files with mode `0600`. Existing paths with broader permissions are rejected with a repair command. Symlink paths and hard-linked private files are rejected. Saves write a fresh private temporary file, sync it, and rename it atomically. Account debug output and TOML parse failures do not expose tokens. Windows does not provide these Unix permission guarantees; use a private user profile directory.

Preference saves reread the current configuration before merging, preserving accounts added or removed by CLI commands since the TUI opened. There is no cross-process lock, so exactly simultaneous writes can still overwrite one another.

Browser login registers an application with `read write follow`, sends the user to the instance's authorization page, and exchanges the pasted out-of-band authorization code for a token. The client verifies the account before saving it. HTTPS is required except for loopback development servers. OAuth requests disable redirects; web links are validated as HTTP(S) URLs and opened without a shell. The flow follows the official [application registration](https://docs.joinmastodon.org/methods/apps/) and [OAuth API](https://docs.joinmastodon.org/methods/oauth/) documentation.

## Validation

`cargo test --locked --all-targets` runs local tests and mocked API contracts. `cargo clippy --locked --all-targets -- -D warnings` checks the code, and `cargo fmt --all -- --check` checks formatting. CI runs those checks plus a release build. Live OAuth, real server policy differences, and compositor behavior still require testing with an actual account and terminal session.
