# Images, video, and local previews

Press **m** on a post to open its attachments inside Tootles. Images initially appear in the terminal. Content warnings must be revealed first. Use **h/l**, the arrow keys, the mouse wheel, or the navigation buttons to move between attachments. **Esc** returns to the post. **R** retries an image. Opening the gallery does not launch another application.

PNG, JPEG, GIF, and WebP previews are supported. Animated images show their first frame. The gallery labels its renderer as **Kitty**, **Sixel**, **iTerm2**, or **Block preview**. The last uses colored Unicode characters, with two color blocks per terminal cell. Alacritty has no native image graphics support, so its block previews look pixelated. A larger terminal or smaller font gives the preview more cells, but it cannot match real image pixels. Kitty, Ghostty, and foot support detailed inline images; see the [renderer's compatibility matrix](https://github.com/ratatui/ratatui-image#compatibility-matrix).

Tootles detects graphics automatically. `tootles --image-protocol halfblocks` deliberately uses the block preview and skips capability queries, which can help over SSH or a multiplexer. Omit that flag when trying a terminal with graphics support. A block preview can also indicate that capability or font-size detection failed.

## Choose an image viewer

For an online image, press **v** to cycle **Terminal → Default image viewer → Browser**, then **Enter** to activate the mode. Local images cycle between Terminal and Default image viewer. The mode and open buttons also work with the mouse; the compact button reads **Default viewer**.

| Mode | Behavior |
| --- | --- |
| **Terminal** (default) | Shows a fitted preview inside the gallery. Enter reloads the inline preview. The renderer depends on your terminal. |
| **Default image viewer** | Opens the original image in the operating system's associated application. Both online and local images use a private, validated copy of the original bytes. |
| **Browser** | Opens an online image's original URL through your default browser. |

**o** is a shortcut to open an online original in your browser or a local file in its default application. No external viewer starts until you explicitly choose to open an image. You can keep using Tootles while the image application is open.

Default image viewer mode needs no additional image-viewer library or mpv. Your desktop must have an image application associated with the file type. Linux uses **xdg-open**, supplied by **xdg-utils**; macOS uses **open**. The operating system chooses the application, so configure the file association if you prefer a different viewer.

Terminal previews are scaled to fit and capped at 2,048 pixels per side. They have no zoom or pan controls. Default image viewer mode opens the original bytes without that preview scaling; zoom and animation support then depend on the selected application.

## Preview a file before posting

Open the composer with **n**, then **F3** or **Browse files**. Select an image, video, or audio file and press **p** or click **Preview**. **Ctrl+L** edits the folder location; **Enter** visits the folder, **j/k** or the arrows select a file, and **Backspace** goes up. Closing the preview returns to the same browser selection. The composer's **Preview** button also opens the pending file or the most recently attached file.

Local images use the same gallery, with Terminal and Default image viewer modes. Previewing or opening a file in its default application does not upload it. A file is uploaded only when you publish its post. File formats recognized by the browser are candidates: actual decoding or playback may still fail for a damaged or unsupported file.

## Choose video and audio playback

Install the optional **mpv** executable using your operating system's package manager. Tootles itself does not install a player. In a video or audio preview, **v** cycles playback modes and **Enter** plays:

| Mode | Behavior |
| --- | --- |
| **Window** (default) | Opens a separate mpv window. Tootles retains its keyboard and mouse input. Closing the gallery or changing attachments stops that player. |
| **Terminal** | Temporarily gives this terminal to mpv. Kitty graphics are used when detected; otherwise mpv's true-color text output is used. Press **q** in mpv to return to the same gallery. |
| **Browser** | Opens an online attachment in the default browser. Local files use Window or Terminal mode. |

Inside mpv, **Space** pauses, the **arrow keys** seek, **9/0** adjusts volume, and **q** exits playback. Window mode can also use mpv's mouse controls. These are mpv's default controls; Tootles starts it without user configuration or scripts. See the [mpv manual](https://mpv.io/manual/stable/) for supported outputs and controls.

Terminal playback occupies the whole terminal while it runs. The timeline does not remain visible alongside the video. Alacritty uses the true-color text output. Both normal completion and **q** return to the gallery with its keyboard and mouse controls restored. Support for Kitty video depends on your mpv build and terminal; `mpv --vo=help` lists available outputs. If playback fails, the gallery shows an error and stays open. Try Window mode, or use **o** for an online attachment. Installing mpv is unnecessary for inline images or browser playback.

## Resource limits and process behavior

Inline images are read only when their gallery opens. Both local and remote terminal previews have a 16 MiB input limit, a 40-megapixel limit, a maximum source dimension of 16,384 pixels, and a bounded decoder allocation. Large accepted images are scaled to at most 2,048 pixels per side. Up to four attachments are retained in a gallery. Decoding and terminal image encoding run on workers, and closing the gallery cancels its pending asynchronous task. Inline previews stay in memory. Demo images are bundled and work offline.

Default image viewer mode reads an original only after you request it. Both online and local PNG, JPEG, GIF, and WebP originals are validated with the same 16 MiB, 40-megapixel, and 16,384-pixel limits, then copied without resizing into a generated cache file with a verified image extension. The cache is the `image-previews` directory inside your Tootles configuration directory, including a custom `--config-dir`. On Unix that directory has `0700` permissions and its files have `0600` permissions. Files remain after Tootles exits so a separately launched application can finish opening them.

The image cache holds at most 16 files and 256 MiB. When another original is stored, files unused for more than 24 hours are removed. When the cache is full, the oldest files can be removed only after 10 minutes have passed since storage or their most recent launch; otherwise the gallery reports that the cache is full. Opening a cached image refreshes its retention time. Cleanup runs only while storing an original, without a background service, so unused cached files can remain longer than 24 hours. Cached copies of local images follow the same limits and cleanup policy. Your original local files are never modified or deleted.

Remote image requests and mpv playback receive no account access token. Image previews require a publicly routable server address, recheck every redirect, and connect directly without HTTP proxy settings. Remote playback accepts HTTP(S) URLs without embedded credentials; hostname resolution and redirects after handoff are controlled by mpv or the browser. Local paths come from an explicit file selection; a remote `file://` attachment cannot open a local file. mpv receives arguments directly, with no shell, no configuration, no scripts, no YouTube extraction, and an option delimiter before the media target. Window players have no access to Tootles' standard input or output, and their processes are reaped on completion or cancellation. Terminal playback keeps the original shell settings saved while mpv runs. On return, Tootles resets display modes, restores input and mouse capture, and fully redraws the gallery. Text video frames are buffered together and player diagnostics are suppressed so they cannot interrupt the display; process failures appear in the gallery. Tootles uses nonblocking input polling so no background input reader competes with mpv.

The media flow was independently implemented using the interaction ideas shown on the [public TubeTUI product page](https://polblancoo.github.io/tubetui/): visible playback modes, local previews, and context-specific controls. No TubeTUI source code was used.
