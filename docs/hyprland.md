# Tootles on Hyprland

Tootles runs inside your terminal, so Hyprland manages it like any other terminal window. It uses true-color text and Unicode; a Nerd Font or terminal image protocol is not required. A wide window gives the timeline and detail pane room to breathe, while narrower windows use a smaller layout.

After installing Tootles, launch it with Kitty:

```sh
kitty --class tootles --title Tootles tootles
```

Explore without signing in:

```sh
kitty --class tootles --title Tootles tootles --demo
```

Kitty exposes `--class` and `--title` for application identity and window naming in its [command-line documentation](https://sw.kovidgoyal.net/kitty/invocation/). Other terminal emulators can launch the same `tootles` command.

For a Hyprland configuration using hyprlang, add an available key combination to `~/.config/hypr/hyprland.conf`:

```ini
bind = SUPER SHIFT, M, exec, kitty --class tootles --title Tootles tootles
```

This follows the [Hyprland bind syntax](https://wiki.hypr.land/0.54.0/configuring/binds/). Use a combination that does not already appear in your configuration. `tootles` must be in the compositor's `PATH`; otherwise replace it with the absolute path to your installed executable.

## Everyday navigation

| Key | Action |
| --- | --- |
| `j` / `k`, arrow keys | Move between posts |
| `1` … `6` | Home, local, federated, notifications, bookmarks, favorites |
| `Enter` | Open the conversation |
| `n` / `r` | New post / reply |
| `f` / `b` / `s` | Favorite / boost / bookmark |
| `/` | Search |
| `a` | Switch account |
| `m` | Open the attachment gallery |
| `L` / `o` | Choose a post's linked website / open the post itself |
| `T` | Cycle themes, including Tokyo Night |
| `?` | Show current shortcuts |
| `q` | Quit |

Click navigation items, posts, and visible action targets; use the mouse wheel to scroll. Terminal text selection may require holding your terminal's mouse override modifier, commonly Shift, while dragging. Paste normally through your terminal's paste shortcut.

The command palette exposes less frequent actions and a reduced-motion toggle. Choose the high-contrast theme if your terminal's background or compositor transparency reduces readability. Tootles paints its own background; terminal transparency is controlled by the terminal and compositor.

Browser login and opening posts or media externally use `xdg-open` on Linux. Make sure a default browser is configured and `xdg-open` is installed. If launching the browser fails during login, Tootles prints the authorization address for manual opening.

Press `m` to view attachments in the terminal. The gallery detects supported image protocols and falls back to colored Unicode half blocks when necessary. Kitty is a good fit for detailed inline images. Terminal previews are fitted and capped at 2,048 pixels per side; for original-resolution inspection, choose Default image viewer with `v`, then press Enter. Images load only when the gallery is opened; content warnings must be revealed first. `h` / `l` or arrow keys move between attachments and Escape returns to the timeline. For video or audio, `v` cycles Window, Terminal, and Browser modes; Enter starts playback. Install `mpv` separately for the first two modes. Window mode opens a regular player window managed by Hyprland. Terminal mode temporarily gives mpv the terminal, using Kitty graphics when detected or true-color text otherwise; `q` returns to Tootles. The attachment browser also previews selected local files with `p`. See [the media guide](media.md) for details.

No global shortcuts, compositor rules, or Hyprland configuration are modified by Tootles.
