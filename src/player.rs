//! Optional mpv playback. All arguments are passed directly, never through a
//! shell; remote attachment URLs and explicitly selected local files stay distinct.
use std::{
    ffi::{OsStr, OsString},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
};

use anyhow::{Context, Result, anyhow, bail};

use crate::media_source::public_media_url;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlaybackMode {
    #[default]
    Window,
    Terminal,
    Browser,
}

impl PlaybackMode {
    pub fn next(self) -> Self {
        match self {
            Self::Window => Self::Terminal,
            Self::Terminal => Self::Browser,
            Self::Browser => Self::Window,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Window => "Window",
            Self::Terminal => "Terminal",
            Self::Browser => "Browser",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalOutput {
    Kitty,
    TrueColor,
}

/// A validated playback target. A remote attachment can never select a local
/// file or mpv protocol such as `fd://`, `memory://`, or `ytdl://`.
#[derive(Clone)]
pub struct Source(OsString);

impl Source {
    pub fn remote(value: &str) -> Result<Self> {
        let url = public_media_url(value).map_err(|_| {
            anyhow!("Playback requires a public HTTP or HTTPS URL without embedded credentials")
        })?;
        Ok(Self(OsString::from(url.as_str())))
    }

    pub fn local(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .context("The selected local file could not be found")?;
        if !path
            .metadata()
            .context("The selected local file could not be inspected")?
            .is_file()
        {
            bail!("Select a regular media file");
        }
        // An absolute filename cannot be mistaken for stdin (`-`) or a URL.
        Ok(Self(path.into_os_string()))
    }
}

fn arguments(source: &Source, output: Option<TerminalOutput>) -> Vec<OsString> {
    let mut args: Vec<_> = [
        "--no-config",
        "--no-ytdl",
        "--load-scripts=no",
        "--really-quiet",
        "--title=Tootles",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    match output {
        Some(output) => {
            args.push("--input-terminal=yes".into());
            args.push("--term-osd=no".into());
            match output {
                TerminalOutput::Kitty => args.push("--vo=kitty".into()),
                TerminalOutput::TrueColor => {
                    args.push("--vo=tct".into());
                    // Keep each frame together instead of issuing separate
                    // writes for every row of ANSI colors and half blocks.
                    args.push("--vo-tct-buffering=frame".into());
                }
            }
        }
        None => {
            args.push("--input-terminal=no".into());
            args.push("--force-window=yes".into());
        }
    }
    args.push("--".into());
    args.push(source.0.clone());
    args
}

fn spawn_error(error: std::io::Error) -> anyhow::Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        anyhow::anyhow!("mpv is unavailable. Install mpv or press v to choose Browser mode")
    } else {
        anyhow::anyhow!("Could not start mpv: {error}")
    }
}

fn playback_result(status: ExitStatus) -> Result<()> {
    if status.success() {
        Ok(())
    } else if let Some(code) = status.code() {
        bail!("mpv exited with status {code}. Try another playback mode with v");
    } else {
        bail!("mpv was interrupted. Press Enter to play again");
    }
}

/// A window player owned by its gallery. Closing the gallery stops playback;
/// normal completion and cancellation both reap the child process.
pub struct Player {
    child: Child,
    finished: bool,
}

impl Player {
    pub fn spawn(source: &Source) -> Result<Self> {
        Self::spawn_with(OsStr::new("mpv"), source)
    }

    /// Use a specific mpv executable, also allowing deterministic process tests.
    pub fn spawn_with(executable: &OsStr, source: &Source) -> Result<Self> {
        let child = Command::new(executable)
            .args(arguments(source, None))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(spawn_error)?;
        Ok(Self {
            child,
            finished: false,
        })
    }

    pub fn poll(&mut self) -> Option<Result<()>> {
        if self.finished {
            return None;
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.finished = true;
                Some(playback_result(status))
            }
            Ok(None) => None,
            Err(error) => Some(Err(error).context("Could not check mpv playback")),
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// The caller must stop drawing and reading input before awaiting this function.
/// Keep raw mode enabled and restore terminal modes afterward. mpv owns input
/// until it exits (q returns to Tootles).
pub async fn play_terminal(source: &Source, output: TerminalOutput) -> Result<()> {
    play_terminal_with(OsStr::new("mpv"), source, output).await
}

pub async fn play_terminal_with(
    executable: &OsStr,
    source: &Source,
    output: TerminalOutput,
) -> Result<()> {
    let status = tokio::process::Command::new(executable)
        .args(arguments(source, Some(output)))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        // Diagnostics must not interrupt the video driver's escape sequences.
        // Report process failures in the gallery after restoring the terminal.
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(spawn_error)?
        .wait()
        .await
        .context("Could not wait for mpv playback")?;
    playback_result(status)
}
