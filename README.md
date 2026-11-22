# tootles

**Your corner of the open web.** A thoughtful, keyboard-first Mastodon client, built in Rust for the terminal. Tokyo Night by default. Mouse support throughout. At home in a Hyprland workspace, a laptop terminal, or a quiet SSH session.

![Tootles in Tokyo Night](docs/preview.svg)

## Install the CLI

Requires **Rust 1.90+** and a C linker. Linux is the primary platform; macOS is supported by the terminal and browser-launching code.

```sh
cd /vmstore/repos/tootles
cargo install --locked --path .

tootles --demo
```

Cargo installs a standalone `tootles` executable into `~/.cargo/bin`. Make sure that directory is on `PATH`. You do not need the repository or Cargo to run the installed binary. Alternatively, `./scripts/install.sh` installs under `~/.local` (override with `TOOTLES_INSTALL_ROOT`).

```sh
tootles login mastodon.social   # use your own Mastodon server
tootles                        # open your connected account
tootles accounts               # list accounts without exposing tokens
tootles switch 2               # choose your default account
tootles theme tokyo-night      # persist your preferred theme
tootles doctor                 # inspect terminal/configuration setup
```

`login` opens your server's authorization page; approve Tootles, then paste the displayed authorization code into the terminal. The client never asks for your password. For an existing access token, use `tootles login SERVER --token-stdin` and supply it on standard input, or set `TOOTLES_TOKEN` for that command. Tokens need `read write follow` scopes. Do not put tokens in command arguments.

Without a connected account, Tootles clearly labels and opens its offline demo. Demo posts, reactions, relationships, lists, and votes live in memory; drafts and preferences persist locally. The demo does not contact a social server.

## What is included

- **Timelines:** home, local, federated, favourites, bookmarks, lists, hashtags, and account profiles. Pagination loads older entries as you reach the bottom. Refresh preserves your selected post when possible.
- **Conversations:** parent and child posts, replies, content warnings, visibility, boost attribution, and edited-post markers.
- **Notifications:** mentions, follows, favourites, boosts, and other server notification types, with links to their author or conversation.
- **Search:** posts, people, and hashtags. Use the command palette's “Show people from search” to browse account results.
- **Compose:** multiline Unicode editing, replies, content warnings, four visibility levels, language, media with alt text, and single- or multiple-choice polls. Edit and delete your own posts. Drafts autosave locally and are separated by account.
- **Interactions:** favourite, boost, bookmark, follow/unfollow, mute/unmute, block/unblock, vote, open in browser, create lists, and add an author to a list.
- **Accounts:** browser OAuth, verified token login, account switching, local logout, and separate credentials/drafts.
- **Interface:** responsive navigation/timeline/inspector panes, clickable controls, mouse scrolling, command palette, contextual shortcuts, reduced motion, compact mode, and four themes: **Tokyo Night**, Midnight, Paper, and High Contrast.
- **Media gallery:** on-demand terminal image previews, attachment navigation, full alt text, and external video/audio playback.
- **Extensibility:** an asynchronous `SocialProvider` trait keeps Mastodon API details outside the UI. A second, stateful demo provider exercises the boundary. Other live networks can be added as Rust adapters.

Network requests run in background tasks. Account and request generations keep stale responses from replacing the current view. Read requests retry transient connection errors; writes are not automatically replayed. Publishing carries an idempotency key to make an intentional retry safe. Server hide/warn filters and sensitive content are respected by the adapter and interface.

## Keyboard and mouse

Press **?** at any time outside an input for help, or **Ctrl+K** / **:** for the searchable command palette.

| Key | Action |
| --- | --- |
| `j` / `k`, `↓` / `↑` | Next / previous post |
| `PgDown` / `PgUp`, `Space` | Move through the timeline |
| `g` / `G`, `Home` / `End` | First / last loaded post |
| `1` / `2` / `3` | Home / local / federated |
| `4` / `5` / `6` | Notifications / bookmarks / favourites |
| `Enter` | Open conversation |
| `Backspace` / `←` | Previous view |
| `n` / `r` | New post / reply |
| `f` / `b` / `s` | Favourite / boost / bookmark |
| `p` / `F` | Author profile / follow or unfollow |
| `/` / `#` | Search / browse hashtag |
| `x` | Reveal or hide content warning / sensitive media |
| `m` / `o` | Media gallery / post in browser |
| `v` / `e` | Vote / edit your post |
| `a` / `l` / `d` | Accounts / lists / drafts |
| `T` / `c` / `i` | Cycle themes / compact mode / inspector |
| `J` / `K` | Scroll the inspector |
| `R` | Refresh |
| `q` / `Ctrl+C` | Save the current draft and quit |

Click navigation items, post cards, action chips, fields, and modal controls. The wheel moves through posts or scrolls the inspector when hovered. Hold your terminal's text-selection modifier (often Shift) to select terminal text, or launch with `--no-mouse`.

### Writing a post

Use `Tab` / `Shift+Tab` to cycle body, content warning, attachment path, alt text, poll options, and language. Standard arrows, Home/End, and grapheme-aware Backspace/Delete work in inputs; bracketed paste supports multiline text. `Ctrl+U` clears text before the cursor and `Ctrl+W` deletes a word.

| Key | Composer action |
| --- | --- |
| `Ctrl+Enter` or `F4` | Publish (F4 works when a terminal cannot distinguish Ctrl+Enter) |
| `Ctrl+S` | Save a local draft |
| `Esc` | Save and close the draft |
| `Alt+V` or `F2` | Public → unlisted → followers → mentioned only |
| `Alt+A` or `F3` | Add the file and alt text entered in the attachment fields |
| `Alt+S` | Toggle sensitive media |
| `Alt+P` | Single / multiple choice poll |
| `Alt+D` | Cycle poll duration |

Separate poll options with semicolons, for example `Tea; Coffee; Water`. A post may contain media **or** a poll. Attachments upload only when publishing. Server limits and processing errors appear in the interface; the draft remains available if publishing fails. Mentioned-only posts are ordinary Mastodon direct messages, **not end-to-end encrypted**.

### Images, video and audio

Press `m` on a media post to open its gallery. Use `--image-protocol halfblocks` to skip terminal capability queries over unusual SSH or multiplexer setups. Images are downloaded only then, without attaching your account token to third-party media hosts. The gallery chooses the terminal's supported **Kitty**, **Sixel**, or **iTerm2** graphics protocol, with a Unicode half-block fallback. Images are bounded in download size and decoded dimensions; original URLs and alt text remain available if a preview cannot load. GIF previews show a static frame.

Use arrows or `h`/`l` to switch attachments and `Enter`/`o` to open the original through your desktop's URL handler (`xdg-open` under Linux). Video and audio playback use that external handler; Tootles does not decode moving video in the terminal. Sensitive attachments stay hidden until you reveal the post with `x`. The gallery uses a small memory cache; it does not save downloaded media to disk.

## Terminal setup and configuration

True-color UTF-8 terminals work best. No Nerd Font is required. The full layout appears in wider windows; side panes collapse on narrower splits. `--no-motion` disables transitions. `NO_COLOR` selects the High Contrast palette and disables motion. See [Hyprland setup](docs/hyprland.md) for terminal launch examples.

Preferences and tokens live in `$XDG_CONFIG_HOME/tootles/config.toml`, or `~/.config/tootles/config.toml`. `--config-dir PATH` or `TOOTLES_CONFIG_DIR` selects another location. On Unix, the directory is `0700` and files are `0600`. Tokens are stored as **plaintext protected by filesystem permissions**, not a system keyring. Draft files are private and scoped to each account. `tootles logout` removes the local token; revoke the application in your server settings to invalidate it remotely.

Example preferences (accounts are populated by `login`):

```toml
theme = "tokyo-night"
reduced_motion = false
compact = false
refresh_seconds = 60
```

Background polling refreshes active timelines when you are at the top with no modal open. Manual refresh is always available. This release uses REST polling rather than streaming. It does not yet include scheduled posts, quote-post composition, moderation reports, profile editing, follower management, push notifications, or every server-specific feature. Only Mastodon is connected to a live network in this release; the provider interface is a compiled extension point, not a runtime plugin system.

## Completions, desktop entry, and releases

```sh
tootles completions bash > ~/.local/share/bash-completion/completions/tootles
tootles completions fish > ~/.config/fish/completions/tootles.fish
tootles completions zsh  > /path/on/your/fpath/_tootles
```

Create the parent directory for your shell first. Optional desktop and man-page files are in `packaging/`. The CI workflow checks formatting, Clippy, tests, and release builds. Tagging a hosted repository with `v*` builds Linux x86_64, Linux ARM64, and macOS ARM64 archives, checksums, shell completions, and a GitHub release. No remote release has been published by this checkout.

## Development

```sh
cargo run -- --demo
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --release --locked
cargo run -- --snapshot docs/preview.svg --width 144 --height 44
cargo run --example render_previews -- docs
python3 scripts/smoke-terminal.py
```

Tests use a local mock server and offline providers. They do not require an account or publish anything. Terminal rendering tests exercise all themes, modals, compact layouts, content-warning privacy, and tiny window sizes. Live OAuth and actual graphics protocol/compositor behavior require an authenticated server and a supporting terminal to verify end-to-end.

Read the [architecture and adapter guide](docs/architecture.md) to add another network. Built with [Ratatui](https://ratatui.rs/), [TachyonFX](https://github.com/ratatui/tachyonfx), Crossterm, Tokio, and Reqwest. The reusable-component approach of [ratcn](https://github.com/kristoferlund/ratcn) informed the interface; its components are not a dependency. MIT licensed.
