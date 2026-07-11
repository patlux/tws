#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let sequence = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("tws-cli-{name}-{}-{sequence}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn test_path(bin_dir: &Path) -> String {
    format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

fn run_import(
    bin_dir: &Path,
    config_dir: &Path,
    backend: &str,
    input: &str,
) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tws"))
        .args(["--backend", backend, "import"])
        .env("PATH", test_path(bin_dir))
        .env("TWS_CONFIG_DIR", config_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if !input.is_empty() {
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    drop(child.stdin.take());
    child.wait_with_output().unwrap()
}

#[test]
fn tmux_backend_uses_dash_v_and_reaches_import() {
    let dir = TestDir::new("tmux");
    let log = dir.0.join("tmux.log");
    write_executable(
        &dir.0.join("tmux"),
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$TWS_TEST_LOG"
if [ "$1" = "-V" ]; then
  echo 'tmux 3.6a'
  exit 0
fi
if [ "$1" = "list-sessions" ]; then
  echo 'unmanaged-session'
  exit 0
fi
exit 1
"#,
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_tws"))
        .args(["--backend", "tmux", "import"])
        .env("PATH", test_path(&dir.0))
        .env("TWS_CONFIG_DIR", dir.0.join("config"))
        .env("TWS_TEST_LOG", &log)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"s\n").unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Import complete."));
    let calls = fs::read_to_string(log).unwrap();
    assert!(calls.lines().any(|call| call == "-V"));
    assert!(calls.lines().any(|call| call.starts_with("list-sessions")));
}

#[test]
fn import_stops_cleanly_at_end_of_input() {
    let dir = TestDir::new("import-eof");
    write_executable(
        &dir.0.join("tmux"),
        r#"#!/bin/sh
if [ "$1" = "-V" ]; then
  echo 'tmux 3.6a'
  exit 0
fi
if [ "$1" = "list-sessions" ]; then
  echo 'unmanaged-session'
  exit 0
fi
exit 1
"#,
    );

    let output = run_import(&dir.0, &dir.0.join("config"), "tmux", "");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Skipping \"unmanaged-session\"."));
    assert!(stdout.contains("Import complete."));
}

#[test]
fn zellij_backend_rejects_versions_before_0_44_3() {
    let dir = TestDir::new("old-zellij");
    write_executable(&dir.0.join("zellij"), "#!/bin/sh\necho 'zellij 0.43.1'\n");

    let output = run_import(&dir.0, &dir.0.join("config"), "zellij", "");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("zellij 0.43.1 is unsupported; install zellij 0.44.3 or newer")
    );
}

#[test]
fn zellij_backend_accepts_minimum_version() {
    let dir = TestDir::new("zellij");
    write_executable(
        &dir.0.join("zellij"),
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'zellij 0.44.3'
  exit 0
fi
if [ "$1" = "list-sessions" ]; then
  echo 'unmanaged-session'
  exit 0
fi
exit 1
"#,
    );

    let output = run_import(&dir.0, &dir.0.join("config"), "zellij", "s\n");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Import complete."));
}
