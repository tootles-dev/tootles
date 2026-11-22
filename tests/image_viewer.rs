use std::{
    fs::{self, File, FileTimes},
    io::Cursor,
    path::Path,
    time::{Duration, SystemTime},
};

use image::{DynamicImage, ImageFormat};
use tootles::{
    image_viewer::{self, MAX_CACHED_IMAGES, MIN_RETENTION, RETENTION},
    media_source::{OriginalImage, decode_original},
};

fn original(format: ImageFormat) -> OriginalImage {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::new_rgb8(8, 5)
        .write_to(&mut bytes, format)
        .unwrap();
    decode_original(bytes.into_inner()).unwrap()
}

fn age(path: &Path, elapsed: Duration) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(SystemTime::now() - elapsed))
        .unwrap();
}

#[test]
fn storage_keeps_original_bytes_and_uses_verified_extension() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory
        .path()
        .canonicalize()
        .unwrap()
        .join("image previews");
    for (format, extension) in [
        (ImageFormat::Png, "png"),
        (ImageFormat::Jpeg, "jpg"),
        (ImageFormat::Gif, "gif"),
        (ImageFormat::WebP, "webp"),
    ] {
        let image = original(format);
        let path = image_viewer::store(&image, &root).unwrap();
        assert!(path.is_absolute());
        assert_eq!(path.parent().unwrap(), root);
        assert_eq!(path.extension().unwrap(), extension);
        assert_eq!(fs::read(&path).unwrap(), image.bytes());
        drop(image);
        assert!(path.is_file(), "viewers may load after the gallery closes");
    }
}

#[test]
fn recent_previews_are_kept_when_cache_is_full_and_oldest_can_then_be_evicted() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap().join("previews");
    let image = original(ImageFormat::Png);
    let paths: Vec<_> = (0..MAX_CACHED_IMAGES)
        .map(|_| image_viewer::store(&image, &root).unwrap())
        .collect();
    let error = image_viewer::store(&image, &root).unwrap_err().to_string();
    assert!(error.contains("cache is full"));
    assert!(paths.iter().all(|path| path.exists()));
    age(&paths[0], MIN_RETENTION + Duration::from_secs(10));
    let replacement = image_viewer::store(&image, &root).unwrap();
    assert!(!paths[0].exists());
    assert!(paths[1..].iter().all(|path| path.exists()));
    assert!(replacement.exists());
    assert_eq!(fs::read_dir(&root).unwrap().count(), MAX_CACHED_IMAGES + 1); // Includes lock.
}

#[test]
fn expiration_removes_only_managed_images_and_preserves_unrelated_files() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap().join("previews");
    let image = original(ImageFormat::Png);
    let expired = image_viewer::store(&image, &root).unwrap();
    let unrelated = root.join("holiday.png");
    fs::write(&unrelated, b"unrelated user data").unwrap();
    age(&expired, RETENTION + Duration::from_secs(10));
    age(&unrelated, RETENTION + Duration::from_secs(10));
    let fresh = image_viewer::store(&image, &root).unwrap();
    assert!(!expired.exists());
    assert!(fresh.exists());
    assert_eq!(fs::read(unrelated).unwrap(), b"unrelated user data");
}

#[test]
fn storage_does_not_exceed_capacity_with_concurrent_writers() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap().join("previews");
    let image = original(ImageFormat::Png);
    let successes = std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..MAX_CACHED_IMAGES + 4)
            .map(|_| {
                let image = &image;
                let root = &root;
                scope.spawn(move || image_viewer::store(image, root).is_ok())
            })
            .collect();
        jobs.into_iter()
            .map(|job| job.join().unwrap())
            .filter(|success| *success)
            .count()
    });
    assert_eq!(successes, MAX_CACHED_IMAGES);
    assert_eq!(fs::read_dir(root).unwrap().count(), MAX_CACHED_IMAGES + 1);
}

#[test]
fn opener_rejects_remote_urls_and_arbitrary_paths_before_running_any_program() {
    for value in [
        "https://example.com/picture.png",
        "file:///etc/passwd",
        "--help",
        "/etc/passwd",
    ] {
        let error =
            image_viewer::open_with(Path::new(value), std::ffi::OsStr::new("must-never-run"))
                .unwrap_err();
        assert!(error.to_string().contains("stored local image preview"));
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::{
        os::unix::fs::{PermissionsExt, symlink},
        time::Instant,
    };

    #[test]
    fn cache_files_and_directory_are_private_and_symlinks_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap().join("previews");
        let image = original(ImageFormat::Png);
        let path = image_viewer::store(&image, &root).unwrap();
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(image_viewer::store(&image, &root).is_err());
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let link = directory.path().join("linked-cache");
        symlink(&root, &link).unwrap();
        assert!(image_viewer::store(&image, &link).is_err());
        fs::remove_file(&path).unwrap();
        let original = directory.path().join("original.png");
        fs::write(&original, image.bytes()).unwrap();
        symlink(&original, &path).unwrap();
        assert!(image_viewer::store(&image, &root).is_err());
        assert!(image_viewer::open_with(&path, std::ffi::OsStr::new("must-never-run")).is_err());
        assert!(original.exists());
    }

    #[test]
    fn native_opener_gets_one_local_argument_and_file_outlives_launch() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory
            .path()
            .canonicalize()
            .unwrap()
            .join("cache with spaces");
        let image = original(ImageFormat::Png);
        let expected = image.bytes().to_owned();
        let path = image_viewer::store(&image, &root).unwrap();
        let executable = directory.path().join("fake image viewer");
        fs::write(&executable, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\nsleep 0.05\ncat \"$1\" > \"$0.loaded\"\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        image_viewer::open_with(&path, executable.as_os_str()).unwrap();
        drop(image);
        let loaded = directory.path().join("fake image viewer.loaded");
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if fs::read(&loaded).is_ok_and(|bytes| bytes == expected) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "fake viewer could not read retained image"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            fs::read_to_string(directory.path().join("fake image viewer.args")).unwrap(),
            format!("{}\n", path.display())
        );
        assert!(path.exists());
        let error =
            image_viewer::open_with(&path, directory.path().join("missing-opener").as_os_str())
                .unwrap_err();
        assert!(error.to_string().contains("default image viewer"));
        assert!(
            path.exists(),
            "failed launch must still allow retrying a stored preview"
        );
    }

    #[test]
    fn failed_desktop_association_is_reported_without_leaking_launcher_output() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap().join("previews");
        let path = image_viewer::store(&original(ImageFormat::Png), &root).unwrap();
        let executable = directory.path().join("failed-opener");
        fs::write(
            &executable,
            "#!/bin/sh\necho private-image-name >&2\nexit 3\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let mut launcher = image_viewer::start_with(&path, executable.as_os_str()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let error = loop {
            if let Some(result) = launcher.poll() {
                break result.unwrap_err().to_string();
            }
            assert!(Instant::now() < deadline, "fake opener did not exit");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(error.contains("file associations"));
        assert!(!error.contains("private-image-name"));
        assert!(
            launcher.poll().is_none(),
            "completion must be delivered only once"
        );
        assert!(path.exists());
    }

    #[test]
    fn reopening_an_old_preview_protects_it_from_immediate_eviction() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap().join("previews");
        let image = original(ImageFormat::Png);
        let paths: Vec<_> = (0..MAX_CACHED_IMAGES)
            .map(|_| image_viewer::store(&image, &root).unwrap())
            .collect();
        age(&paths[0], MIN_RETENTION + Duration::from_secs(10));
        let executable = directory.path().join("successful-opener");
        fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        image_viewer::open_with(&paths[0], executable.as_os_str()).unwrap();
        assert!(
            image_viewer::store(&image, &root)
                .unwrap_err()
                .to_string()
                .contains("cache is full")
        );
        assert!(paths.iter().all(|path| path.exists()));
    }
}
