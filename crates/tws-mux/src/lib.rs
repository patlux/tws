use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub use tws_core::BackendKind as Backend;
use tws_core::model::AgentSession;

mod agent_detection;
mod tmux;
mod zellij;

static BACKEND: OnceLock<Backend> = OnceLock::new();
static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn configure(value: &str) -> Result<(), String> {
    let backend = match value {
        "tmux" => Backend::Tmux,
        "zellij" => Backend::Zellij,
        other => {
            return Err(format!(
                "unknown backend {other:?}; expected tmux or zellij"
            ));
        }
    };
    BACKEND
        .set(backend)
        .map_err(|_| "multiplexer backend was already configured".to_string())
}

pub fn configure_config_dir(path: PathBuf) -> Result<(), String> {
    CONFIG_DIR
        .set(path)
        .map_err(|_| "multiplexer config directory was already configured".to_string())
}

fn config_dir() -> PathBuf {
    CONFIG_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| PathBuf::from(".config/tws"))
}

pub fn backend() -> Backend {
    *BACKEND.get_or_init(|| match std::env::var("TWS_BACKEND").as_deref() {
        Ok("zellij") => Backend::Zellij,
        _ => Backend::Tmux,
    })
}

pub fn name() -> &'static str {
    backend().name()
}

fn version_arg(backend: Backend) -> &'static str {
    match backend {
        Backend::Tmux => "-V",
        Backend::Zellij => "--version",
    }
}

const MIN_ZELLIJ_VERSION: (u64, u64, u64) = (0, 44, 3);

fn parse_version(output: &str) -> Option<(u64, u64, u64)> {
    let version = output
        .split_whitespace()
        .find(|token| token.chars().next().is_some_and(|c| c.is_ascii_digit()))?
        .split('-')
        .next()?;
    let mut parts = version.split('.').map(|part| {
        part.chars()
            .take_while(|character| character.is_ascii_digit())
            .collect::<String>()
            .parse::<u64>()
            .ok()
    });
    Some((
        parts.next()??,
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
    ))
}

fn validate_version(backend: Backend, output: &str) -> Result<(), String> {
    if backend != Backend::Zellij {
        return Ok(());
    }
    let version = parse_version(output)
        .ok_or_else(|| format!("could not parse zellij version from {output:?}"))?;
    if version < MIN_ZELLIJ_VERSION {
        return Err(format!(
            "zellij {}.{}.{} is unsupported; install zellij {}.{}.{} or newer",
            version.0,
            version.1,
            version.2,
            MIN_ZELLIJ_VERSION.0,
            MIN_ZELLIJ_VERSION.1,
            MIN_ZELLIJ_VERSION.2,
        ));
    }
    Ok(())
}

pub fn ensure_available() -> Result<(), String> {
    let backend = backend();
    let binary = backend.name();
    let version_arg = version_arg(backend);
    let output = std::process::Command::new(binary)
        .arg(version_arg)
        .output()
        .map_err(|_| format!("{binary} not found on PATH — install {binary} first"))?;
    if !output.status.success() {
        return Err(format!("failed to run {binary} {version_arg}"));
    }
    let version_output = if output.stdout.is_empty() {
        String::from_utf8_lossy(&output.stderr)
    } else {
        String::from_utf8_lossy(&output.stdout)
    };
    validate_version(backend, version_output.trim())
}

pub fn regular_prefix(collection_name: &str, thread_name: &str) -> String {
    backend().regular_prefix(collection_name, thread_name)
}

pub fn root_prefix(thread_name: &str) -> String {
    backend().root_prefix(thread_name)
}

pub fn regular_name(collection_name: &str, thread_name: &str, label: &str) -> String {
    backend().regular_name(collection_name, thread_name, label)
}

pub fn root_name(thread_name: &str, label: &str) -> String {
    backend().root_name(thread_name, label)
}

pub fn is_managed_name(name: &str) -> bool {
    backend().is_managed_name(name)
}

/// Names of all running sessions. `Err` means discovery itself failed —
/// callers should retain their last-known state rather than treat it as
/// "no sessions".
pub fn list_sessions() -> Result<Vec<String>, String> {
    match backend() {
        Backend::Tmux => tmux::commands::list_sessions(),
        Backend::Zellij => zellij::list_sessions(),
    }
}

/// Managed sessions with last-attached timestamps. Same error contract as
/// [`list_sessions`].
pub fn list_managed_sessions_with_timestamps() -> Result<Vec<(String, i64)>, String> {
    match backend() {
        Backend::Tmux => tmux::commands::list_tws_sessions_with_timestamps(),
        Backend::Zellij => zellij::list_managed_sessions_with_timestamps(),
    }
}

pub fn new_session(name: &str) -> Result<(), String> {
    match backend() {
        Backend::Tmux => tmux::commands::new_session(name),
        Backend::Zellij => zellij::new_session(name, None),
    }
}

pub fn new_session_in_dir(name: &str, cwd: &Path) -> Result<(), String> {
    match backend() {
        Backend::Tmux => tmux::commands::new_session_in_dir(name, cwd),
        Backend::Zellij => zellij::new_session(name, Some(cwd)),
    }
}

/// Creates a session in a directory and starts a command without shell parsing.
/// Zellij cannot currently accept an initial command through its background
/// session API, so command spawning is explicitly tmux-only.
pub fn new_session_in_dir_with_command(
    name: &str,
    cwd: &Path,
    command: &[OsString],
) -> Result<(), String> {
    match backend() {
        Backend::Tmux => tmux::commands::new_session_in_dir_with_command(name, cwd, command),
        Backend::Zellij => Err(
            "starting a command with `tws spawn` is not supported by the Zellij backend"
                .to_string(),
        ),
    }
}

pub fn kill_session(name: &str) -> Result<(), String> {
    match backend() {
        Backend::Tmux => tmux::commands::kill_session(name),
        Backend::Zellij => zellij::kill_session(name),
    }
}

pub fn rename_session(old_name: &str, new_name: &str) -> Result<(), String> {
    match backend() {
        Backend::Tmux => tmux::commands::rename_session(old_name, new_name),
        Backend::Zellij => zellij::rename_session(old_name, new_name),
    }
}

pub fn switch_client(name: &str) -> Result<(), String> {
    match backend() {
        Backend::Tmux => tmux::commands::switch_client(name),
        Backend::Zellij => zellij::switch_session(name, None),
    }
}

/// True when a failed `switch_client` means there is no live client to
/// switch (stale env) and the caller should retry with an external attach.
pub fn switch_error_indicates_no_client(err: &str) -> bool {
    match backend() {
        Backend::Tmux => tmux::commands::switch_error_indicates_no_client(err),
        Backend::Zellij => false,
    }
}

pub fn attach_session(name: &str) -> std::io::Result<bool> {
    let result = match backend() {
        Backend::Tmux => tmux::commands::attach_session(name),
        Backend::Zellij => zellij::attach_session(name),
    };
    if matches!(backend(), Backend::Zellij) && matches!(result, Ok(true)) {
        zellij::record_attach(name);
    }
    result
}

pub fn is_inside() -> bool {
    match backend() {
        Backend::Tmux => tmux::commands::is_inside_tmux(),
        Backend::Zellij => std::env::var("ZELLIJ").is_ok_and(|value| !value.is_empty()),
    }
}

pub fn focus_pane(session_name: &str, window_index: u32, pane_id: &str) -> Result<(), String> {
    match backend() {
        Backend::Tmux => {
            tmux::commands::select_window(session_name, window_index)?;
            tmux::commands::select_pane(pane_id)
        }
        Backend::Zellij => zellij::focus_pane(session_name, pane_id),
    }
}

pub fn capture_pane(session_name: &str, pane_id: &str) -> Option<String> {
    match backend() {
        Backend::Tmux => tmux::commands::capture_pane(pane_id),
        Backend::Zellij => zellij::capture_pane(session_name, pane_id),
    }
}

/// Detect agents in the given sessions. `Err` means the scan itself
/// failed — callers should retain the previous agent list.
pub fn scan_agents(session_names: &[String]) -> Result<Vec<AgentSession>, String> {
    match backend() {
        Backend::Tmux => tmux::agent_scan::scan_agents(session_names),
        Backend::Zellij => zellij::scan_agents(session_names),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backends_use_their_native_version_flags() {
        assert_eq!(version_arg(Backend::Tmux), "-V");
        assert_eq!(version_arg(Backend::Zellij), "--version");
    }

    #[test]
    fn parses_backend_version_output() {
        assert_eq!(parse_version("tmux 3.6a"), Some((3, 6, 0)));
        assert_eq!(parse_version("zellij 0.44.3"), Some((0, 44, 3)));
        assert_eq!(parse_version("zellij 0.45.0-dev"), Some((0, 45, 0)));
        assert_eq!(parse_version("unknown"), None);
    }

    #[test]
    fn rejects_unsupported_zellij_versions() {
        assert!(validate_version(Backend::Zellij, "zellij 0.43.1").is_err());
        assert!(validate_version(Backend::Zellij, "zellij 0.44.3").is_ok());
        assert!(validate_version(Backend::Zellij, "zellij 0.45.0").is_ok());
        assert!(validate_version(Backend::Tmux, "tmux 3.4").is_ok());
    }

    #[test]
    fn default_backend_keeps_tmux_names() {
        assert_eq!(regular_prefix("Work", "Task"), "tws_work_task");
        assert_eq!(root_prefix("Scratch"), "twsr_scratch");
        assert!(is_managed_name("tws_work_task_main"));
        assert!(!is_managed_name("twz_work_task_main"));
    }

    #[test]
    fn zellij_backend_uses_isolated_names() {
        let process = std::process::Command::new(std::env::current_exe().unwrap())
            .env("TWS_BACKEND", "zellij")
            .arg("--exact")
            .arg("tests::zellij_name_probe")
            .arg("--nocapture")
            .output()
            .unwrap();
        assert!(process.status.success());
        assert!(
            String::from_utf8_lossy(&process.stdout)
                .contains("twz_work_task|twzr_scratch|true|false")
        );
    }

    #[test]
    fn zellij_name_probe() {
        if std::env::var("TWS_BACKEND").as_deref() != Ok("zellij") {
            return;
        }
        println!(
            "{}|{}|{}|{}",
            regular_prefix("Work", "Task"),
            root_prefix("Scratch"),
            is_managed_name("twz_work_task_main"),
            is_managed_name("tws_work_task_main")
        );
    }
}
