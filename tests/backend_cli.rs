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

fn write_fake_tmux(dir: &Path) {
    write_executable(
        &dir.join("tmux"),
        r#"#!/bin/sh
set -eu
if [ "$1" = "-V" ]; then
  echo 'tmux 3.6a'
  exit 0
fi
sessions="$TWS_FAKE_TMUX_SESSIONS"
case "$1" in
  list-sessions)
    [ -f "$sessions" ] && cat "$sessions"
    exit 0
    ;;
  new-session)
    name=''
    cwd=''
    shift
    while [ "$#" -gt 0 ]; do
      case "$1" in
        -d) shift ;;
        -s) name="$2"; shift 2 ;;
        -c) cwd="$2"; shift 2 ;;
        --) shift; break ;;
        *) break ;;
      esac
    done
    printf '%s\n' "$name" >> "$sessions"
    if [ -n "${TWS_FAKE_TMUX_LOG:-}" ]; then
      printf 'name=%s\ncwd=%s\nargc=%s\n' "$name" "$cwd" "$#" > "$TWS_FAKE_TMUX_LOG"
      for arg in "$@"; do printf 'arg=%s\n' "$arg" >> "$TWS_FAKE_TMUX_LOG"; done
    fi
    exit 0
    ;;
esac
exit 1
"#,
    );
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
fn hierarchy_commands_do_not_require_a_mux_binary() {
    let dir = TestDir::new("ensure-without-mux");
    let config = dir.0.join("config");
    let output = Command::new(env!("CARGO_BIN_EXE_tws"))
        .args(["collection", "ensure", "Personal", "--json"])
        .env("PATH", &dir.0)
        .env("TWS_CONFIG_DIR", &config)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["created"], true);
}

#[test]
fn hierarchy_list_is_read_only_json_and_works_while_locked_without_a_mux_binary() {
    let dir = TestDir::new("hierarchy-list");
    let config = dir.0.join("config");
    let sessions = dir.0.join("sessions");
    write_fake_tmux(&dir.0);

    for args in [
        vec!["collection", "ensure", "init"],
        vec!["thread", "ensure", "--collection", "init", "etb"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_tws"))
            .args(args)
            .env("PATH", test_path(&dir.0))
            .env("TWS_CONFIG_DIR", &config)
            .env("TWS_FAKE_TMUX_SESSIONS", &sessions)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::write(config.join("tws.lock"), std::process::id().to_string()).unwrap();
    fs::remove_file(dir.0.join("tmux")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_tws"))
        .args(["hierarchy", "list", "--json"])
        .env("PATH", &dir.0)
        .env("TWS_CONFIG_DIR", &config)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["collections"][0]["name"], "init");
    assert_eq!(json["collections"][0]["hidden"], false);
    assert_eq!(json["collections"][0]["isRoot"], false);
    assert_eq!(json["collections"][0]["threads"][0]["name"], "etb");
    assert_eq!(json["collections"][0]["threads"][0]["hidden"], false);
}

#[test]
fn collection_and_thread_ensure_are_idempotent_json_commands() {
    let dir = TestDir::new("ensure");
    write_fake_tmux(&dir.0);
    let config = dir.0.join("config");
    let sessions = dir.0.join("sessions");

    let collection = Command::new(env!("CARGO_BIN_EXE_tws"))
        .args(["collection", "ensure", "Personal", "--json"])
        .env("PATH", test_path(&dir.0))
        .env("TWS_CONFIG_DIR", &config)
        .env("TWS_FAKE_TMUX_SESSIONS", &sessions)
        .output()
        .unwrap();
    assert!(
        collection.status.success(),
        "{}",
        String::from_utf8_lossy(&collection.stderr)
    );
    let collection_json: serde_json::Value = serde_json::from_slice(&collection.stdout).unwrap();
    assert_eq!(collection_json["created"], true);
    assert_eq!(collection_json["name"], "Personal");

    let thread = Command::new(env!("CARGO_BIN_EXE_tws"))
        .args([
            "thread",
            "ensure",
            "--collection",
            "Personal",
            "TWS agents",
            "--json",
        ])
        .env("PATH", test_path(&dir.0))
        .env("TWS_CONFIG_DIR", &config)
        .env("TWS_FAKE_TMUX_SESSIONS", &sessions)
        .output()
        .unwrap();
    assert!(
        thread.status.success(),
        "{}",
        String::from_utf8_lossy(&thread.stderr)
    );
    let thread_json: serde_json::Value = serde_json::from_slice(&thread.stdout).unwrap();
    assert_eq!(thread_json["created"], true);
    assert_eq!(thread_json["collection"], "Personal");
    assert_eq!(thread_json["name"], "TWS agents");

    let repeated = Command::new(env!("CARGO_BIN_EXE_tws"))
        .args([
            "thread",
            "ensure",
            "--collection",
            "Personal",
            "TWS agents",
            "--json",
        ])
        .env("PATH", test_path(&dir.0))
        .env("TWS_CONFIG_DIR", &config)
        .env("TWS_FAKE_TMUX_SESSIONS", &sessions)
        .output()
        .unwrap();
    assert!(repeated.status.success());
    let repeated_json: serde_json::Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert_eq!(repeated_json["created"], false);

    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(config.join("state.json")).unwrap()).unwrap();
    assert_eq!(state.as_array().unwrap().len(), 1);
    assert_eq!(state[0]["threads"].as_array().unwrap().len(), 1);
}

#[test]
fn thread_ensure_requires_an_existing_collection() {
    let dir = TestDir::new("thread-missing-collection");
    write_fake_tmux(&dir.0);
    let output = Command::new(env!("CARGO_BIN_EXE_tws"))
        .args(["thread", "ensure", "--collection", "Missing", "Worker"])
        .env("PATH", test_path(&dir.0))
        .env("TWS_CONFIG_DIR", dir.0.join("config"))
        .env("TWS_FAKE_TMUX_SESSIONS", dir.0.join("sessions"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("collection \"Missing\" does not exist")
    );
}

#[test]
fn hierarchy_mutation_respects_a_live_tws_lock() {
    let dir = TestDir::new("lock");
    write_fake_tmux(&dir.0);
    let config = dir.0.join("config");
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("tws.lock"), std::process::id().to_string()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_tws"))
        .args(["collection", "ensure", "Personal"])
        .env("PATH", test_path(&dir.0))
        .env("TWS_CONFIG_DIR", &config)
        .env("TWS_FAKE_TMUX_SESSIONS", dir.0.join("sessions"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("state is locked by a running tws instance")
    );
    assert!(!config.join("state.json").exists());
}

#[test]
fn second_tui_instance_is_refused_without_an_unsafe_override() {
    let dir = TestDir::new("tui-lock");
    write_fake_tmux(&dir.0);
    let config = dir.0.join("config");
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("tws.lock"), std::process::id().to_string()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_tws"))
        .env("PATH", test_path(&dir.0))
        .env("TWS_CONFIG_DIR", &config)
        .env("TWS_FAKE_TMUX_SESSIONS", dir.0.join("sessions"))
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("another TUI instance is running"));
    assert!(!stderr.contains("continue anyway"));
}

#[test]
fn spawn_creates_hierarchy_and_passes_command_as_literal_argv() {
    let dir = TestDir::new("spawn");
    write_fake_tmux(&dir.0);
    let config = dir.0.join("config");
    let sessions = dir.0.join("sessions");
    let log = dir.0.join("tmux.log");

    let output = Command::new(env!("CARGO_BIN_EXE_tws"))
        .args([
            "spawn",
            "--collection",
            "Personal",
            "--thread",
            "TWS agents",
            "--label",
            "implementation",
            "--cwd",
            dir.0.to_str().unwrap(),
            "--ensure-hierarchy",
            "--json",
            "--",
            "pi",
            "--name",
            "Parallel worker",
            "Do work; touch /tmp/not-executed",
        ])
        .env("PATH", test_path(&dir.0))
        .env("TWS_CONFIG_DIR", &config)
        .env("TWS_FAKE_TMUX_SESSIONS", &sessions)
        .env("TWS_FAKE_TMUX_LOG", &log)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["createdCollection"], true);
    assert_eq!(json["createdThread"], true);
    assert_eq!(json["createdSession"], true);
    assert_eq!(json["commandStarted"], true);
    assert_eq!(
        json["sessionName"],
        "tws_personal_tws-agents_implementation"
    );

    let calls = fs::read_to_string(log).unwrap();
    assert!(calls.contains("argc=4"));
    assert!(calls.contains("arg=Parallel worker"));
    assert!(calls.contains("arg=Do work; touch /tmp/not-executed"));
    assert!(!Path::new("/tmp/not-executed").exists());
}

#[test]
fn spawn_existing_session_requires_reuse_and_does_not_restart_command() {
    let dir = TestDir::new("reuse");
    write_fake_tmux(&dir.0);
    let config = dir.0.join("config");
    let sessions = dir.0.join("sessions");
    let log = dir.0.join("tmux.log");

    let base = [
        "spawn",
        "--collection",
        "Personal",
        "--thread",
        "Workers",
        "--label",
        "review",
        "--cwd",
        dir.0.to_str().unwrap(),
        "--ensure-hierarchy",
        "--json",
        "--",
        "pi",
        "first",
    ];
    let first = Command::new(env!("CARGO_BIN_EXE_tws"))
        .args(base)
        .env("PATH", test_path(&dir.0))
        .env("TWS_CONFIG_DIR", &config)
        .env("TWS_FAKE_TMUX_SESSIONS", &sessions)
        .env("TWS_FAKE_TMUX_LOG", &log)
        .output()
        .unwrap();
    assert!(first.status.success());

    let duplicate = Command::new(env!("CARGO_BIN_EXE_tws"))
        .args(base)
        .env("PATH", test_path(&dir.0))
        .env("TWS_CONFIG_DIR", &config)
        .env("TWS_FAKE_TMUX_SESSIONS", &sessions)
        .env("TWS_FAKE_TMUX_LOG", &log)
        .output()
        .unwrap();
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("already exists"));

    fs::write(&log, "sentinel\n").unwrap();
    let reused = Command::new(env!("CARGO_BIN_EXE_tws"))
        .args([
            "spawn",
            "--collection",
            "Personal",
            "--thread",
            "Workers",
            "--label",
            "review",
            "--cwd",
            dir.0.to_str().unwrap(),
            "--reuse",
            "--json",
            "--",
            "pi",
            "second",
        ])
        .env("PATH", test_path(&dir.0))
        .env("TWS_CONFIG_DIR", &config)
        .env("TWS_FAKE_TMUX_SESSIONS", &sessions)
        .env("TWS_FAKE_TMUX_LOG", &log)
        .output()
        .unwrap();
    assert!(reused.status.success());
    let json: serde_json::Value = serde_json::from_slice(&reused.stdout).unwrap();
    assert_eq!(json["createdSession"], false);
    assert_eq!(json["commandStarted"], false);
    assert_eq!(fs::read_to_string(log).unwrap(), "sentinel\n");
}

#[test]
fn spawn_suggests_exact_existing_hierarchy_names_before_a_lock_error() {
    let dir = TestDir::new("spawn-name-suggestions");
    write_fake_tmux(&dir.0);
    let config = dir.0.join("config");
    let sessions = dir.0.join("sessions");

    for args in [
        vec!["collection", "ensure", "init"],
        vec!["thread", "ensure", "--collection", "init", "etb"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_tws"))
            .args(args)
            .env("PATH", test_path(&dir.0))
            .env("TWS_CONFIG_DIR", &config)
            .env("TWS_FAKE_TMUX_SESSIONS", &sessions)
            .output()
            .unwrap();
        assert!(output.status.success());
    }
    fs::write(config.join("tws.lock"), std::process::id().to_string()).unwrap();

    let collection_case = Command::new(env!("CARGO_BIN_EXE_tws"))
        .args([
            "spawn",
            "--collection",
            "INIT",
            "--thread",
            "etb",
            "--label",
            "review",
            "--cwd",
            dir.0.to_str().unwrap(),
            "--ensure-hierarchy",
        ])
        .env("PATH", test_path(&dir.0))
        .env("TWS_CONFIG_DIR", &config)
        .env("TWS_FAKE_TMUX_SESSIONS", &sessions)
        .output()
        .unwrap();
    assert!(!collection_case.status.success());
    let collection_error = String::from_utf8_lossy(&collection_case.stderr);
    assert!(collection_error.contains("Did you mean \"init\"?"));
    assert!(!collection_error.contains("state is locked"));

    let thread_case = Command::new(env!("CARGO_BIN_EXE_tws"))
        .args([
            "spawn",
            "--collection",
            "init",
            "--thread",
            "ETB",
            "--label",
            "review",
            "--cwd",
            dir.0.to_str().unwrap(),
            "--ensure-hierarchy",
        ])
        .env("PATH", test_path(&dir.0))
        .env("TWS_CONFIG_DIR", &config)
        .env("TWS_FAKE_TMUX_SESSIONS", &sessions)
        .output()
        .unwrap();
    assert!(!thread_case.status.success());
    let thread_error = String::from_utf8_lossy(&thread_case.stderr);
    assert!(thread_error.contains("Did you mean \"etb\"?"));
    assert!(!thread_error.contains("state is locked"));
}

#[test]
fn spawn_into_existing_hierarchy_works_while_state_is_locked() {
    let dir = TestDir::new("spawn-locked-existing");
    write_fake_tmux(&dir.0);
    let config = dir.0.join("config");
    let sessions = dir.0.join("sessions");

    for args in [
        vec!["collection", "ensure", "Personal"],
        vec!["thread", "ensure", "--collection", "Personal", "Workers"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_tws"))
            .args(args)
            .env("PATH", test_path(&dir.0))
            .env("TWS_CONFIG_DIR", &config)
            .env("TWS_FAKE_TMUX_SESSIONS", &sessions)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::write(config.join("tws.lock"), std::process::id().to_string()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_tws"))
        .args([
            "spawn",
            "--collection",
            "Personal",
            "--thread",
            "Workers",
            "--label",
            "live",
            "--cwd",
            dir.0.to_str().unwrap(),
            "--json",
        ])
        .env("PATH", test_path(&dir.0))
        .env("TWS_CONFIG_DIR", &config)
        .env("TWS_FAKE_TMUX_SESSIONS", &sessions)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["createdSession"], true);
    assert_eq!(json["sessionName"], "tws_personal_workers_live");
}

#[test]
fn slug_collisions_are_rejected() {
    let dir = TestDir::new("slug-collision");
    write_fake_tmux(&dir.0);
    let config = dir.0.join("config");
    let sessions = dir.0.join("sessions");
    let first = Command::new(env!("CARGO_BIN_EXE_tws"))
        .args(["collection", "ensure", "Work!"])
        .env("PATH", test_path(&dir.0))
        .env("TWS_CONFIG_DIR", &config)
        .env("TWS_FAKE_TMUX_SESSIONS", &sessions)
        .output()
        .unwrap();
    assert!(first.status.success());
    let collision = Command::new(env!("CARGO_BIN_EXE_tws"))
        .args(["collection", "ensure", "Work?"])
        .env("PATH", test_path(&dir.0))
        .env("TWS_CONFIG_DIR", &config)
        .env("TWS_FAKE_TMUX_SESSIONS", &sessions)
        .output()
        .unwrap();
    assert!(!collision.status.success());
    assert!(
        String::from_utf8_lossy(&collision.stderr).contains("collides with existing collection")
    );
}

#[test]
fn zellij_spawn_with_command_fails_before_mutating_hierarchy() {
    let dir = TestDir::new("zellij-command-spawn");
    write_executable(
        &dir.0.join("zellij"),
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'zellij 0.44.3'; exit 0; fi\nexit 1\n",
    );
    let config = dir.0.join("config");
    let output = Command::new(env!("CARGO_BIN_EXE_tws"))
        .args([
            "--backend",
            "zellij",
            "spawn",
            "--collection",
            "Personal",
            "--thread",
            "Workers",
            "--label",
            "pi",
            "--cwd",
            dir.0.to_str().unwrap(),
            "--ensure-hierarchy",
            "--",
            "pi",
        ])
        .env("PATH", test_path(&dir.0))
        .env("TWS_CONFIG_DIR", &config)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("starting a command with `tws spawn` is not supported by the Zellij backend")
    );
    assert!(!config.join("state.json").exists());
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
