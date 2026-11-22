use std::process::Command;
fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tootles"))
}
#[test]
fn help_and_version_work_without_config_or_terminal() {
    for arg in ["--help", "--version"] {
        let output = cli()
            .arg(arg)
            .env("TOOTLES_CONFIG_DIR", "/nonexistent/no-config-needed")
            .output()
            .unwrap();
        assert!(output.status.success());
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains("tootles"));
    }
}
#[test]
fn shell_completions_do_not_read_credentials() {
    let output = cli()
        .args(["--config-dir", "/root/not-readable", "completions", "fish"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("complete -c tootles"));
}
#[test]
fn snapshot_exports_real_demo_and_theme() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("preview.svg");
    let output = cli()
        .args(["--demo", "--theme", "tokyo-night", "--config-dir"])
        .arg(temp.path().join("private"))
        .arg("--snapshot")
        .arg(&path)
        .args(["--width", "100", "--height", "30"])
        .env_remove("NO_COLOR")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let svg = std::fs::read_to_string(path).unwrap();
    assert!(svg.starts_with("<svg"));
    assert!(svg.contains("#1a1b26"));
    assert!(svg.contains("font-family"));
}
#[test]
fn noninteractive_launch_explains_how_to_start() {
    let temp = tempfile::tempdir().unwrap();
    let output = cli()
        .arg("--demo")
        .arg("--config-dir")
        .arg(temp.path().join("private"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("interactive terminal"));
}
#[test]
fn invalid_snapshot_size_fails_cleanly() {
    let temp = tempfile::tempdir().unwrap();
    let output = cli()
        .arg("--config-dir")
        .arg(temp.path().join("private"))
        .arg("--snapshot")
        .arg(temp.path().join("tiny.svg"))
        .args(["--width", "1"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Snapshot dimensions"));
}

#[test]
fn snapshot_ignores_unreadable_saved_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let output = cli()
        .args(["--config-dir", "/root/not-readable", "--snapshot"])
        .arg(temp.path().join("clean.svg"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
