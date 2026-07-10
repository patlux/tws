use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tws_core::model::AgentSession;
pub use tws_core::BackendKind as Backend;

mod tmux;
mod zellij;

static BACKEND: OnceLock<Backend> = OnceLock::new();
static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn configure(value: &str) -> Result<(), String> {
    let backend = match value {
        "tmux" => Backend::Tmux,
        "zellij" => Backend::Zellij,
        other => return Err(format!("unknown backend {other:?}; expected tmux or zellij")),
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

pub fn ensure_available() -> Result<(), String> {
    let binary = name();
    std::process::Command::new(binary)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|_| format!("{binary} not found on PATH — install {binary} first"))
        .and_then(|status| {
            status
                .success()
                .then_some(())
                .ok_or_else(|| format!("failed to run {binary} --version"))
        })
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

pub fn list_sessions() -> Vec<String> {
    match backend() {
        Backend::Tmux => tmux::commands::list_sessions(),
        Backend::Zellij => zellij::list_sessions(),
    }
}

pub fn list_managed_sessions_with_timestamps() -> Vec<(String, i64)> {
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

pub fn attach_session(name: &str) -> std::io::Result<bool> {
    let result = match backend() {
        Backend::Tmux => tmux::commands::attach_session(name),
        Backend::Zellij => zellij::attach_session(name),
    };
    if matches!(backend(), Backend::Zellij) {
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

pub fn scan_agents(session_names: &[String]) -> Vec<AgentSession> {
    match backend() {
        Backend::Tmux => tmux::agent_scan::scan_agents(session_names),
        Backend::Zellij => zellij::scan_agents(session_names),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
