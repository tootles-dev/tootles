# Images, video, and local previews

Press **m** on a post to open its attachments inside Tootles. Images appear directly in the terminal; there is no browser step. Content warnings must be revealed first. Use **h/l**, the arrow keys, the mouse wheel, or the navigation buttons to move between attachments. **Esc** returns to the post. **R** retries an image, and **o** explicitly opens an online attachment in your browser.

PNG, JPEG, GIF, and WebP previews are supported. Animated images show their first frame. Tootles detects terminal graphics support and uses Kitty, Sixel, or iTerm graphics when available, with colored half blocks as the fallback. If capability detection is unreliable over SSH or a multiplexer, launch `tootles --image-protocol halfblocks`.

## Preview a file before posting

Open the composer with **n**, then **F3** or **Browse files**. Select an image, video, or audio file and press **p** or click **Preview**. **Ctrl+L** edits the folder location; **Enter** visits the folder, **j/k** or the arrows select a file, and **Backspace** goes up. Closing the preview returns to the same browser selection. The composer's **Preview** button also opens the pending file or the most recently attached file.

Local images use the same inline viewer as post attachments. Previewing does not upload the file. A file is uploaded only when you publish its post. File formats recognized by the browser are candidates: actual decoding or playback may still fail for a damaged or unsupported file.

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

Images are read only when their gallery opens. Both local and remote previews have a 16 MiB input limit, a 40-megapixel limit, a maximum source dimension of 16,384 pixels, and a bounded decoder allocation. Large accepted images are scaled to at most 2,048 pixels per side. Up to four attachments are retained in a gallery. Decoding and terminal image encoding run on workers, and closing the gallery cancels its pending asynchronous task. Demo images are bundled and work offline.

Remote image requests and mpv playback receive no account access token. Remote playback accepts HTTP(S) URLs without embedded credentials. Local paths come from an explicit file selection; a remote `file://` attachment cannot open a local file. mpv receives arguments directly, with no shell, no configuration, no scripts, no YouTube extraction, and an option delimiter before the media target. Window players have no access to Tootles' standard input or output, and their processes are reaped on completion or cancellation. Terminal playback keeps the original shell settings saved while mpv runs. On return, Tootles resets display modes, restores input and mouse capture, and fully redraws the gallery. Text video frames are buffered together and player diagnostics are suppressed so they cannot interrupt the display; process failures appear in the gallery. Tootles uses nonblocking input polling so no background input reader competes with mpv.

The media flow was independently implemented using the interaction ideas shown on the [public TubeTUI product page](https://polblancoo.github.io/tubetui/): visible playback modes, local previews, and context-specific controls. No TubeTUI source code was used.
