use std::path::Path;

use tootles::player::{PlaybackMode, Source};

#[test]
fn playback_modes_cycle_and_remote_sources_cannot_select_files_or_options() {
    let mode = PlaybackMode::default();
    assert_eq!(mode, PlaybackMode::Window);
    assert_eq!(mode.next(), PlaybackMode::Terminal);
    assert_eq!(mode.next().next(), PlaybackMode::Browser);
    assert_eq!(mode.next().next().next(), PlaybackMode::Window);
    for bad in [
        "file:///etc/passwd",
        "fd://0",
        "ytdl://https://example.com/",
        "--script=evil.lua",
        "https://name:secret@example.com/media",
        "https://example.com/media\n--script=evil.lua",
        "http://127.0.0.1/media",
        "http://169.254.169.254/latest/meta-data/",
        "http://[::1]/media",
    ] {
        assert!(Source::remote(bad).is_err(), "accepted {bad}");
    }
    assert!(Source::remote("https://example.com/video.mp4").is_ok());
    let directory = tempfile::tempdir().unwrap();
    assert!(Source::local(directory.path()).is_err());
    assert!(Source::local(Path::new("missing-tootles-video.mp4")).is_err());
}

#[cfg(unix)]
mod processes {
    use std::{
        ffi::OsStr,
        fs,
        io::Write,
        path::{Path, PathBuf},
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    use tootles::player::{Player, Source, TerminalOutput, play_terminal_with};

    fn fake_mpv(directory: &Path, body: &str) -> PathBuf {
        let path = directory.join("fake mpv");
        let script = format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\n{body}\n");
        // Keep the executable's writable descriptor out of this multithreaded
        // test process. A concurrent fork can otherwise inherit it briefly and
        // fail to exec the script with ETXTBSY on Linux container filesystems.
        let mut writer = Command::new("/bin/sh")
            .args([
                "-c",
                "cat > \"$1\" && chmod 700 \"$1\"",
                "tootles-test-writer",
            ])
            .arg(&path)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        writer
            .stdin
            .take()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        assert!(writer.wait().unwrap().success());
        path
    }

    fn args(path: &Path) -> Vec<String> {
        fs::read_to_string(format!("{}.args", path.display()))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn finish(player: &mut Player) -> anyhow::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(result) = player.poll() {
                return result;
            }
            assert!(Instant::now() < deadline, "fake mpv did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn safe_options(args: &[String]) {
        for option in ["--no-config", "--no-ytdl", "--load-scripts=no"] {
            assert!(args.iter().any(|arg| arg == option), "missing {option}");
        }
        assert_eq!(args[args.len() - 2], "--");
    }

    #[test]
    fn window_playback_passes_literal_arguments_and_has_no_terminal_input() {
        let directory = tempfile::tempdir().unwrap();
        let executable = fake_mpv(directory.path(), "if read -r line; then exit 9; fi\nexit 0");
        let target = "https://example.com/movie.mp4?title=$(touch${IFS}not-executed)&x=1";
        let source = Source::remote(target).unwrap();
        let mut player = Player::spawn_with(executable.as_os_str(), &source).unwrap();
        finish(&mut player).unwrap();
        assert!(
            player.poll().is_none(),
            "completion should be reported once"
        );
        let args = args(&executable);
        safe_options(&args);
        assert!(args.iter().any(|a| a == "--input-terminal=no"));
        assert!(args.iter().any(|a| a == "--force-window=yes"));
        assert_eq!(args.last().unwrap(), target);
        assert!(!directory.path().join("not-executed").exists());
    }

    #[test]
    fn missing_player_and_failed_process_return_actionable_errors() {
        let directory = tempfile::tempdir().unwrap();
        let source = Source::remote("https://example.com/movie.mp4").unwrap();
        let error = match Player::spawn_with(directory.path().join("missing").as_os_str(), &source)
        {
            Ok(_) => panic!("missing mpv unexpectedly started"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("Install mpv"));
        let executable = fake_mpv(directory.path(), "echo private-server-text >&2\nexit 7");
        let mut player = Player::spawn_with(executable.as_os_str(), &source).unwrap();
        let error = finish(&mut player).unwrap_err().to_string();
        assert!(error.contains("status 7"));
        assert!(!error.contains("private-server-text"));
    }

    #[test]
    fn closing_the_gallery_owner_kills_and_reaps_window_playback() {
        let directory = tempfile::tempdir().unwrap();
        let executable = fake_mpv(
            directory.path(),
            "printf '%s' \"$$\" > \"$0.pid\"\nwhile :; do :; done",
        );
        let source = Source::remote("https://example.com/movie.mp4").unwrap();
        let player = Player::spawn_with(executable.as_os_str(), &source).unwrap();
        let pid_file = directory.path().join("fake mpv.pid");
        let deadline = Instant::now() + Duration::from_secs(3);
        let pid = loop {
            if let Ok(pid) = fs::read_to_string(&pid_file)
                && !pid.is_empty()
            {
                break pid;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        };
        drop(player);
        assert!(
            !Command::new("kill")
                .args(["-0", &pid])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
    }

    #[tokio::test]
    async fn terminal_outputs_select_mpv_driver_and_preserve_local_filename() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("--script=$(bad) a clip.mp4");
        fs::write(&path, b"fake clip").unwrap();
        let source = Source::local(&path).unwrap();
        for (output, expected) in [
            (TerminalOutput::Kitty, "--vo=kitty"),
            (TerminalOutput::TrueColor, "--vo=tct"),
        ] {
            let executable = fake_mpv(directory.path(), "exit 0");
            play_terminal_with(executable.as_os_str(), &source, output)
                .await
                .unwrap();
            let args = args(&executable);
            safe_options(&args);
            assert!(args.iter().any(|a| a == expected));
            assert!(args.iter().any(|a| a == "--input-terminal=yes"));
            assert!(args.iter().any(|a| a == "--term-osd=no"));
            assert_eq!(
                args.iter().any(|a| a == "--vo-tct-buffering=frame"),
                output == TerminalOutput::TrueColor
            );
            assert_eq!(
                OsStr::new(args.last().unwrap()),
                path.canonicalize().unwrap().as_os_str()
            );
        }
        let error = play_terminal_with(
            directory.path().join("missing").as_os_str(),
            &source,
            TerminalOutput::TrueColor,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Install mpv"));
    }
}
