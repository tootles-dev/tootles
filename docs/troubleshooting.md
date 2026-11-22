# Troubleshooting

## Updating an installed CLI

Pull the current repository and rebuild the installed executable. Running a
previously installed `tootles` does not automatically use changes in the source
directory.

```sh
cd /vmstore/repos/tootles
git pull --ff-only
cargo install --locked --path . --force
tootles --version
```

If the version still looks old, `command -v tootles` shows which executable your
shell runs. Cargo's default install location is `~/.cargo/bin/tootles`.

## Bluesky login and account switching

Use `tootles login --provider bluesky YOUR_HANDLE` with a dedicated app password
from Bluesky settings. The prompt hides your input. An account on a different
host also needs `--pds https://YOUR_HOST`. See [Bluesky setup](bluesky.md) for
supported features and session troubleshooting.

Press **a** in the TUI to switch accounts. This refreshes the saved account list,
so an account connected from another terminal is available without restarting.
Drafts belong to the account that created them. If a revoked or expired session
cannot refresh, log in again; do not include app passwords or session tokens in
bug reports.

## Mastodon login reports an unexpected response format

Version 0.1.0 combined several unrelated failures into the message “The server
returned an unexpected response format.” Version 0.1.1 and later support nullable
account profile URLs, standard HTTP response compression, and more precise errors.

Version 0.1.2 also accepts unavailable or nonstandard account statistics. The
reported `response.followers_count` error no longer blocks login; an unavailable
count displays as an em dash instead of zero. Account identity fields are still
validated.

After updating, retry login using the server hostname:

```sh
tootles login infosec.exchange
```

The new error distinguishes these cases:

- **HTML instead of API JSON:** a server, proxy, or protection page intercepted
  the API request. Check that the hostname is your Mastodon server and that your
  connection does not require signing in through a web page.
- **Unexpected JSON value at a named field:** the response contains a field type
  the client does not support. Report the field name and Tootles version.
- **Empty, incomplete, or invalid JSON:** retry once; the server or a proxy may
  have returned a broken response.
- **Timeout or interrupted response:** check your connection and retry.
- **Incomplete account identity:** the server returned no usable account. The
  failed login does not save the token.

For a bug report, share the server hostname, `tootles --version`, and the error
message. The diagnostic does not print response contents or credentials. Do not
include access tokens or authorization codes.

## Media and terminal setup

### Open a website linked inside a post

Click an underlined URL, or select the post and press **L** to open its link
picker. Choose a destination with the arrows and press **Enter**, or click its
row. **o** opens the post itself. The picker shows HTTP and HTTPS links;
reveal a content warning with **x** before opening links hidden behind it.

With `--no-mouse`, use **L** and the keyboard. If a link does not launch, check
that your system has a default browser and, on Linux, `xdg-open` from `xdg-utils`.

### Images look pixelated in Alacritty

Alacritty does not support the image graphics protocols used by Tootles. The
gallery therefore shows **Block preview**, a picture made from colored text
cells. This is a terminal limitation; downloading a larger source image cannot
remove the blocks.

Press **v** until the image mode says **Default image viewer**, then **Enter**. Tootles
opens the original image in your operating system's associated application.
For an online image, **o** opens the original in your browser instead. Neither
action changes the post or uploads a local file.

For detailed images inside the terminal, run Tootles in Kitty, Ghostty, or foot.
Omit `--image-protocol halfblocks`, which deliberately forces text rendering.
The gallery should show **Kitty**, **Sixel**, or **iTerm2** when graphics are
available. These terminal previews still fit the available area, are capped at
2,048 pixels per side, and do not have zoom or pan; use Default image viewer for
original-resolution inspection.

### Default viewer does not open an image

On Linux, install `xdg-utils` if `xdg-open` is unavailable and configure a default
application for the image's file type. On macOS, Tootles uses the system `open`
command. Tootles does not require or install a particular image application.
Online images need a successful download before the application opens; the
gallery reports download or launcher failures. Browser mode remains available
for an online original.

See [the media guide](media.md) for attachment browsing, inline image previews,
and optional video playback, and [the Hyprland guide](hyprland.md) for terminal
launch examples.
