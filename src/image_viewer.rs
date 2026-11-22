//! Native image viewing without passing remote URLs to the desktop opener.
//!
//! Files remain in a private, bounded cache after launching: desktop openers
//! often return before the viewer has read the file. Cleanup happens on the next
//! use, never from a launch handle's destructor or a short-lived gallery.
use std::{
    ffi::OsStr,
    fs::{self, File, FileTimes, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, bail};
use uuid::Uuid;

use crate::media_source::OriginalImage;

pub const MAX_CACHED_IMAGES: usize = 16;
pub const RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
pub const MIN_RETENTION: Duration = Duration::from_secs(10 * 60);
const MAX_CACHE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 512;

/// Ask the OS default application to open a file returned by `store`.
/// Call this only after confirming that the gallery is still active: storing
/// on a worker must not automatically open a canceled or superseded request.
/// A successful return
/// means the desktop opener started, not that its separate GUI finished loading.
pub fn open(path: &Path) -> Result<()> {
    drop(start(path)?);
    Ok(())
}

/// Launch and retain a completion handle so the gallery can report a desktop
/// association failure asynchronously. Dropping this handle never kills the GUI.
pub fn start(path: &Path) -> Result<Launcher> {
    start_with(path, native_program())
}

pub struct Launcher {
    child: Option<Child>,
}

impl Launcher {
    pub fn poll(&mut self) -> Option<Result<()>> {
        let child = self.child.as_mut()?;
        match child.try_wait() {
            Ok(None) => None,
            Ok(Some(status)) => {
                self.child = None;
                Some(if status.success() {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!(
                        "The default image viewer could not open this image ({status}); check your desktop file associations or use Browser mode"
                    ))
                })
            }
            Err(error) => Some(Err(error).context("Could not check the default image viewer")),
        }
    }
}

impl Drop for Launcher {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Launchers may remain alive while their GUI is open. Reap without
            // blocking a closed gallery or terminating a viewer the user wants.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

/// Override the desktop opener executable for process tests or packaging.
/// Only absolute, private cached image paths are accepted, never remote URLs.
#[doc(hidden)]
pub fn open_with(path: &Path, executable: &OsStr) -> Result<()> {
    drop(start_with(path, executable)?);
    Ok(())
}

#[doc(hidden)]
pub fn start_with(path: &Path, executable: &OsStr) -> Result<Launcher> {
    if !path.is_absolute() || !managed_name(path) {
        bail!("The image viewer can only open a stored local image preview");
    }
    let parent = path
        .parent()
        .context("Image preview has no cache directory")?;
    // Verify the private directory and file again immediately before launching.
    // No remote URL or source filename is ever passed through to the opener.
    private_directory(parent, false)?;
    let lock = lock_file(parent)?;
    // Launching runs in the event loop; never block it behind a cache writer.
    lock.try_lock()
        .context("The image preview cache is busy; try opening the image again")?;
    private_file(&fs::symlink_metadata(path).context("The image preview is no longer available")?)?;
    let file = OpenOptions::new().write(true).open(path)?;
    private_file(&file.metadata()?)?;
    file.set_times(FileTimes::new().set_modified(SystemTime::now()))?;
    let mut command = Command::new(executable);
    #[cfg(windows)]
    command.arg("url.dll,FileProtocolHandler");
    let child = command
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context(
            "Could not start the default image viewer; check your desktop file associations",
        )?;
    Ok(Launcher { child: Some(child) })
}

fn native_program() -> &'static OsStr {
    #[cfg(target_os = "macos")]
    {
        OsStr::new("open")
    }
    #[cfg(windows)]
    {
        OsStr::new("rundll32.exe")
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        OsStr::new("xdg-open")
    }
}

/// Materialize original, validated bytes using their verified image extension.
/// Does not launch a viewer. The shared file lock keeps simultaneous Tootles
/// processes inside the same cache bounds.
pub fn store(image: &OriginalImage, cache_root: &Path) -> Result<PathBuf> {
    let root = private_directory(cache_root, true)?;
    let lock = lock_file(&root)?;
    lock.lock()
        .context("Could not lock the image preview cache")?;
    make_room(&root, image.bytes().len() as u64)?;
    let path = root.join(format!("image-{}.{}", Uuid::new_v4(), image.extension()));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        private_mode(&mut options);
        let mut file = options
            .open(&path)
            .context("Could not create a private image preview")?;
        file.write_all(image.bytes())
            .context("Could not write the image preview")?;
        file.sync_all()
            .context("Could not finish writing the image preview")?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    // Dropping the file releases its advisory lock on all supported platforms.
    drop(lock);
    Ok(path)
}

fn lock_file(root: &Path) -> Result<File> {
    let lock_path = root.join(".cache.lock");
    if lock_path.exists() || fs::symlink_metadata(&lock_path).is_ok() {
        private_file(&fs::symlink_metadata(&lock_path)?)?;
    }
    let mut lock_options = OpenOptions::new();
    lock_options
        .read(true)
        .write(true)
        .create(true)
        .truncate(false);
    private_mode(&mut lock_options);
    let lock = lock_options
        .open(&lock_path)
        .context("Could not open the image cache lock")?;
    private_file(&lock.metadata()?)?;
    Ok(lock)
}

fn private_mode(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = options;
}

fn private_file(metadata: &fs::Metadata) -> Result<()> {
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!("Image cache entries must be regular files, not symbolic links");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o077 != 0 || metadata.nlink() != 1 {
            bail!("Image cache files must be private (0600) and must not be hard-linked");
        }
    }
    Ok(())
}

fn private_directory(path: &Path, create: bool) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut current = PathBuf::new();
    for component in path.components() {
        if component == Component::ParentDir {
            bail!("Image cache path must not contain '..'");
        }
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!("Image cache path must not contain symbolic links")
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Could not inspect the image cache path"),
        }
    }
    if create {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .context("Could not create the image preview cache")?;
    }
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!("Image cache must be a private directory");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o077 != 0 {
            bail!("Image cache directory must be private (0700)");
        }
    }
    Ok(path)
}

fn managed_name(path: &Path) -> bool {
    let Some(stem) = path.file_stem().and_then(OsStr::to_str) else {
        return false;
    };
    let Some(uuid) = stem.strip_prefix("image-") else {
        return false;
    };
    Uuid::parse_str(uuid).is_ok()
        && matches!(
            path.extension().and_then(OsStr::to_str),
            Some("png" | "jpg" | "gif" | "webp")
        )
}

fn make_room(root: &Path, incoming_bytes: u64) -> Result<()> {
    let now = SystemTime::now();
    let mut retained = Vec::new();
    let mut bytes = 0_u64;
    for (index, entry) in fs::read_dir(root)?.enumerate() {
        if index >= MAX_DIRECTORY_ENTRIES {
            bail!("Image cache contains too many entries; clear its previews before trying again");
        }
        let entry = entry?;
        let path = entry.path();
        if !managed_name(&path) {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        private_file(&metadata)?;
        let modified = metadata
            .modified()
            .context("Could not inspect an image preview timestamp")?;
        let age = now.duration_since(modified).unwrap_or_default();
        if age >= RETENTION {
            fs::remove_file(path).context("Could not remove an expired image preview")?;
        } else {
            bytes = bytes.saturating_add(metadata.len());
            retained.push((modified, path, metadata.len(), age));
        }
    }
    retained.sort_by_key(|entry| entry.0);
    let mut count = retained.len();
    for (_, path, size, age) in retained {
        if count < MAX_CACHED_IMAGES && bytes.saturating_add(incoming_bytes) <= MAX_CACHE_BYTES {
            return Ok(());
        }
        if age < MIN_RETENTION {
            break;
        }
        fs::remove_file(path).context("Could not make room for an image preview")?;
        bytes = bytes.saturating_sub(size);
        count -= 1;
    }
    if count >= MAX_CACHED_IMAGES || bytes.saturating_add(incoming_bytes) > MAX_CACHE_BYTES {
        bail!(
            "Image preview cache is full. Recent files are kept for 10 minutes so viewers can load them; try later or use Terminal or Browser mode"
        );
    }
    Ok(())
}
