//! Filesystem-only helpers for the attachment browser.
//!
//! Scanning is synchronous so callers can run it on `tokio::task::spawn_blocking`.
//! No files are opened for content, no recursion is performed, and display names
//! never contain terminal controls. Entry paths retain their exact OS spelling.
use std::{
    cmp::Ordering,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};

/// Bound both directory traversal and the number of retained rows. Large remote
/// mounts should not occupy a worker indefinitely or fill the process's memory.
const MAX_SCANNED_ENTRIES: usize = 20_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MediaFilter {
    #[default]
    All,
    Images,
    Video,
    Audio,
}

impl MediaFilter {
    pub fn next(self) -> Self {
        match self {
            Self::All => Self::Images,
            Self::Images => Self::Video,
            Self::Video => Self::Audio,
            Self::Audio => Self::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All media",
            Self::Images => "Images",
            Self::Video => "Video",
            Self::Audio => "Audio",
        }
    }

    fn accepts(self, kind: FileKind) -> bool {
        kind == FileKind::Directory
            || self == Self::All
            || matches!(
                (self, kind),
                (Self::Images, FileKind::Image)
                    | (Self::Video, FileKind::Video)
                    | (Self::Audio, FileKind::Audio)
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Directory,
    Image,
    Video,
    Audio,
}

impl FileKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Directory => "Folder",
            Self::Image => "Image",
            Self::Video => "Video",
            Self::Audio => "Audio",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    /// A readable filename for display. Use `path`, never `name`, to access it.
    pub name: String,
    pub kind: FileKind,
    /// The file size in bytes; directories report zero.
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct Listing {
    /// Absolute, resolved path of the folder that was actually scanned.
    pub directory: PathBuf,
    pub entries: Vec<Entry>,
    /// Entries hidden, filtered out, or unavailable. If the traversal limit is
    /// reached, this includes one additional entry and is a lower bound: callers
    /// should describe it as "at least N omitted" rather than an exact total.
    pub omitted: usize,
}

/// Read one directory, listing folders before matching media files. Symlinked
/// folders are navigable; symlinked media files use the target's file metadata.
/// Supported extensions describe candidates, not a promise that every server
/// accepts their format. The network adapter/server validates an actual upload.
pub fn scan(path: &Path, filter: MediaFilter, show_hidden: bool) -> Result<Listing> {
    scan_with_limit(path, filter, show_hidden, MAX_SCANNED_ENTRIES)
}

fn scan_with_limit(
    path: &Path,
    filter: MediaFilter,
    show_hidden: bool,
    maximum: usize,
) -> Result<Listing> {
    let directory = path
        .canonicalize()
        .with_context(|| format!("Cannot open attachment folder {}", path.display()))?;
    ensure!(directory.is_dir(), "Choose a folder, not a file");
    let iterator = fs::read_dir(&directory)
        .with_context(|| format!("Cannot read attachment folder {}", directory.display()))?;
    let mut entries = Vec::new();
    let mut omitted = 0;
    for (index, result) in iterator.enumerate() {
        if index == maximum {
            // Do not traverse an unbounded directory just to count excluded rows.
            omitted += 1;
            break;
        }
        let Ok(entry) = result else {
            omitted += 1;
            continue;
        };
        let raw_name = entry.file_name();
        if !show_hidden && raw_name.to_string_lossy().starts_with('.') {
            omitted += 1;
            continue;
        }
        let path = entry.path();
        // fs::metadata follows symlinks, unlike DirEntry::metadata. No target is
        // read; it only establishes whether the entry is a folder or a file.
        let metadata = match entry.file_type() {
            Ok(kind) if kind.is_symlink() => fs::metadata(&path),
            Ok(_) => entry.metadata(),
            Err(_) => {
                omitted += 1;
                continue;
            }
        };
        let Ok(metadata) = metadata else {
            omitted += 1;
            continue;
        };
        let kind = if metadata.is_dir() {
            Some(FileKind::Directory)
        } else if metadata.is_file() {
            media_kind(&path)
        } else {
            // Device nodes, sockets and FIFOs are never attachment candidates.
            None
        };
        let Some(kind) = kind.filter(|kind| filter.accepts(*kind)) else {
            omitted += 1;
            continue;
        };
        entries.push(Entry {
            path,
            name: display_name(&raw_name.to_string_lossy()),
            kind,
            bytes: if kind == FileKind::Directory {
                0
            } else {
                metadata.len()
            },
        });
    }
    // Cache Unicode lowercase names once. Numeric runs compare by magnitude, so
    // photo2 sorts before photo10 without ever parsing into a bounded integer.
    entries.sort_by_cached_key(|entry| {
        (
            entry.kind != FileKind::Directory,
            NaturalName(entry.name.to_lowercase()),
            entry.name.clone(),
            entry.path.clone(),
        )
    });
    Ok(Listing {
        directory,
        entries,
        omitted,
    })
}

/// Classify a candidate media filename by extension, without accessing it.
pub fn media_kind(path: &Path) -> Option<FileKind> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" | "jpe" | "png" | "gif" | "webp" | "avif" | "heic" | "heif" | "bmp"
        | "tif" | "tiff" => Some(FileKind::Image),
        "mp4" | "m4v" | "webm" | "mov" | "mkv" | "avi" => Some(FileKind::Video),
        "mp3" | "ogg" | "oga" | "opus" | "wav" | "flac" | "m4a" | "aac" => Some(FileKind::Audio),
        _ => None,
    }
}

fn display_name(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_control()
                || matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            {
                '\u{fffd}'
            } else {
                character
            }
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
struct NaturalName(String);
impl PartialOrd for NaturalName {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for NaturalName {
    fn cmp(&self, other: &Self) -> Ordering {
        let (mut a, mut b) = (self.0.as_bytes(), other.0.as_bytes());
        while !a.is_empty() && !b.is_empty() {
            if a[0].is_ascii_digit() && b[0].is_ascii_digit() {
                let a_end = a
                    .iter()
                    .position(|byte| !byte.is_ascii_digit())
                    .unwrap_or(a.len());
                let b_end = b
                    .iter()
                    .position(|byte| !byte.is_ascii_digit())
                    .unwrap_or(b.len());
                let a_run = &a[..a_end];
                let b_run = &b[..b_end];
                let a_zeroes = a_run.iter().take_while(|byte| **byte == b'0').count();
                let b_zeroes = b_run.iter().take_while(|byte| **byte == b'0').count();
                let a_number = &a_run[a_zeroes..];
                let b_number = &b_run[b_zeroes..];
                let order = a_number
                    .len()
                    .cmp(&b_number.len())
                    .then_with(|| a_number.cmp(b_number))
                    .then_with(|| a_run.len().cmp(&b_run.len()));
                if order != Ordering::Equal {
                    return order;
                }
                a = &a[a_end..];
                b = &b[b_end..];
            } else {
                let order = a[0].cmp(&b[0]);
                if order != Ordering::Equal {
                    return order;
                }
                a = &a[1..];
                b = &b[1..];
            }
        }
        a.len().cmp(&b.len())
    }
}

/// Start in the last navigated folder if possible, then ~/Pictures, the home
/// folder, and finally the working directory. Does not create any directories.
pub fn initial_directory(last: Option<&Path>) -> PathBuf {
    if let Some(last) = last.filter(|path| path.is_dir()) {
        return last.to_path_buf();
    }
    if let Some(home) = env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        let pictures = home.join("Pictures");
        if pictures.is_dir() {
            return pictures;
        }
        if home.is_dir() {
            return home;
        }
    }
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Resolve typed locations literally. Only `~` and `~/…` expand; shell syntax,
/// environment substitutions, globs and other users' tildes are never evaluated.
pub fn resolve_location(value: &str, current: &Path) -> PathBuf {
    if value.is_empty() {
        return current.to_path_buf();
    }
    if (value == "~" || value.starts_with("~/"))
        && let Some(home) = env::var_os("HOME")
    {
        let home = PathBuf::from(home);
        if home.is_absolute() {
            return if value == "~" {
                home
            } else {
                home.join(value[2..].trim_start_matches('/'))
            };
        }
    }
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        current.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(directory: &Path, name: &str) {
        fs::write(directory.join(name), b"media").unwrap();
    }

    #[test]
    fn folders_first_and_natural_filename_order() {
        let root = tempfile::tempdir().unwrap();
        for name in [
            "photo10.PNG",
            "Photo2.jpg",
            "photo02.webp",
            "audio.mp3",
            "ignored.txt",
        ] {
            write(root.path(), name);
        }
        fs::create_dir(root.path().join("Zoo")).unwrap();
        fs::create_dir(root.path().join("album")).unwrap();
        let listing = scan(root.path(), MediaFilter::All, false).unwrap();
        let names: Vec<_> = listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "album",
                "Zoo",
                "audio.mp3",
                "Photo2.jpg",
                "photo02.webp",
                "photo10.PNG"
            ]
        );
        assert_eq!(listing.omitted, 1);
        assert!(listing.directory.is_absolute());
        assert_eq!(listing.entries[0].bytes, 0);
        assert_eq!(listing.entries[2].bytes, 5);
    }

    #[test]
    fn filters_media_but_keeps_directories_and_can_show_hidden_files() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("folder")).unwrap();
        fs::create_dir(root.path().join(".hidden-folder")).unwrap();
        for name in [
            "picture.jpg",
            "video.mp4",
            "sound.opus",
            ".private.png",
            "document.pdf",
        ] {
            write(root.path(), name);
        }
        for (filter, expected) in [
            (MediaFilter::Images, "picture.jpg"),
            (MediaFilter::Video, "video.mp4"),
            (MediaFilter::Audio, "sound.opus"),
        ] {
            let listing = scan(root.path(), filter, false).unwrap();
            assert_eq!(listing.entries.len(), 2);
            assert_eq!(listing.entries[0].kind, FileKind::Directory);
            assert_eq!(listing.entries[1].name, expected);
        }
        let listing = scan(root.path(), MediaFilter::Images, true).unwrap();
        assert_eq!(listing.entries.len(), 4);
        assert!(
            listing
                .entries
                .iter()
                .any(|entry| entry.name == ".private.png")
        );
    }

    #[test]
    fn invalid_locations_report_errors_and_scan_limit_bounds_entries() {
        let root = tempfile::tempdir().unwrap();
        assert!(scan(&root.path().join("missing"), MediaFilter::All, false).is_err());
        write(root.path(), "not-a-folder.png");
        assert!(
            scan(
                &root.path().join("not-a-folder.png"),
                MediaFilter::All,
                false
            )
            .unwrap_err()
            .to_string()
            .contains("folder")
        );
        for name in ["1.jpg", "2.jpg", "3.jpg"] {
            write(root.path(), name);
        }
        let listing = scan_with_limit(root.path(), MediaFilter::All, false, 2).unwrap();
        assert_eq!(listing.entries.len(), 2);
        assert!(listing.omitted >= 1);
    }

    #[test]
    fn location_resolution_never_evaluates_shell_syntax() {
        let current = Path::new("/tmp/attachment-browser");
        assert_eq!(resolve_location("", current), current);
        assert_eq!(
            resolve_location("photos/image", current),
            current.join("photos/image")
        );
        assert_eq!(resolve_location("/var/tmp", current), Path::new("/var/tmp"));
        assert_eq!(
            resolve_location("../other", current),
            current.join("../other")
        );
        for value in [
            "$HOME",
            "$(touch nope)",
            "~another-user",
            "*.png",
            "a folder",
        ] {
            assert_eq!(resolve_location(value, current), current.join(value));
        }
        if let Some(home) = env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|home| home.is_absolute())
        {
            assert_eq!(resolve_location("~", current), home);
            assert_eq!(
                resolve_location("~/Pictures", current),
                home.join("Pictures")
            );
        }
    }

    #[test]
    fn initial_directory_prefers_existing_last_directory() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(initial_directory(Some(root.path())), root.path());
        assert!(initial_directory(Some(&root.path().join("missing"))).is_dir());
    }

    #[test]
    fn filter_cycle_and_long_numbers_are_stable() {
        let mut filter = MediaFilter::default();
        for _ in 0..4 {
            filter = filter.next();
        }
        assert_eq!(filter, MediaFilter::All);
        assert!(
            NaturalName("image99999999999999999999.png".into())
                < NaturalName("image100000000000000000000.png".into())
        );
        assert!(NaturalName("image0.png".into()) < NaturalName("image00.png".into()));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_folders_are_navigable_and_dangling_links_are_omitted() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        write(target.path(), "image.jpg");
        symlink(target.path(), root.path().join("linked-folder")).unwrap();
        symlink(
            target.path().join("image.jpg"),
            root.path().join("linked-image.jpg"),
        )
        .unwrap();
        symlink(
            target.path().join("missing"),
            root.path().join("broken.png"),
        )
        .unwrap();
        let listing = scan(root.path(), MediaFilter::All, false).unwrap();
        assert_eq!(listing.entries.len(), 2);
        assert_eq!(listing.entries[0].kind, FileKind::Directory);
        assert_eq!(listing.omitted, 1);
        let nested = scan(&listing.entries[0].path, MediaFilter::All, false).unwrap();
        assert_eq!(nested.directory, target.path().canonicalize().unwrap());
        assert_eq!(nested.entries[0].name, "image.jpg");
    }

    #[cfg(unix)]
    #[test]
    fn display_names_are_safe_while_entry_paths_remain_exact() {
        let root = tempfile::tempdir().unwrap();
        let name = "picture\u{1b}\n\u{202e}.png";
        write(root.path(), name);
        let listing = scan(root.path(), MediaFilter::All, false).unwrap();
        assert_eq!(listing.entries[0].path, root.path().join(name));
        assert_eq!(listing.entries[0].name, "picture���.png");
    }
}
