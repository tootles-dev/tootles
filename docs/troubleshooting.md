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

## Login reports an unexpected response format

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

See [the media guide](media.md) for attachment browsing, inline image previews,
and optional video playback, and [the Hyprland guide](hyprland.md) for terminal
launch examples.
