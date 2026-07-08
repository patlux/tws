//! Reader for Pi work-status sidecar files.
//!
//! A companion Pi extension (`extensions/pi/tws-status.ts`) writes one JSON file
//! per tmux pane to `~/.config/tws/pi-status/` on Pi lifecycle events
//! (`agent_start`, `agent_end`, ...) and touches `~/.config/tws/agent.trigger`
//! so tws rescans within one poll tick (250ms).
//!
//! tws only reads these files. Invalid files are skipped, never fatal.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Work state reported by the Pi extension for a single pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiWorkState {
    /// Pi is currently streaming/working on a prompt.
    Working,
    /// Pi started but has not worked yet (fresh session).
    Idle,
    /// Pi finished its last prompt.
    Done,
    /// Pi exited cleanly.
    Shutdown,
    /// Unrecognized state string (forward compatibility).
    Unknown,
}

impl PiWorkState {
    fn parse(s: &str) -> Self {
        match s {
            "working" => PiWorkState::Working,
            "idle" => PiWorkState::Idle,
            "done" => PiWorkState::Done,
            "shutdown" => PiWorkState::Shutdown,
            _ => PiWorkState::Unknown,
        }
    }
}

/// What the UI should show for a Pi agent or its session row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiIndicator {
    /// Show a spinner — Pi is working right now.
    Working,
    /// Show a checkmark — Pi finished.
    Done,
}

/// One parsed status file.
#[derive(Debug, Clone)]
pub struct PiStatus {
    pub pane_id: String,
    pub tmux_session_name: String,
    pub session_name: Option<String>,
    pub work_state: PiWorkState,
    pub updated_at_ms: u64,
    pub finished_at_ms: Option<u64>,
}

/// Wire format written by the Pi extension. Unknown fields are ignored.
#[derive(serde::Deserialize)]
struct RawStatus {
    #[serde(default)]
    pane_id: String,
    #[serde(default)]
    tmux_session_name: String,
    #[serde(default)]
    session_name: Option<String>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    updated_at_ms: u64,
    #[serde(default)]
    finished_at_ms: Option<u64>,
}

/// Parse a single status file's contents. Returns `None` for invalid JSON
/// or files without a pane id.
pub fn parse_status(data: &str) -> Option<PiStatus> {
    let raw: RawStatus = serde_json::from_str(data).ok()?;
    if raw.pane_id.is_empty() {
        return None;
    }
    Some(PiStatus {
        pane_id: raw.pane_id,
        tmux_session_name: raw.tmux_session_name,
        session_name: raw.session_name,
        work_state: PiWorkState::parse(&raw.state),
        updated_at_ms: raw.updated_at_ms,
        finished_at_ms: raw.finished_at_ms,
    })
}

/// Load all status files from `dir`. Missing directory or unreadable/invalid
/// files simply yield fewer results.
#[cfg(test)]
pub fn load_all(dir: &Path) -> Vec<PiStatus> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut result = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Some(status) = parse_status(&data) {
                result.push(status);
            }
        }
    }
    result
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Load all statuses and prune stale files in a single directory pass.
///
/// Files whose pane is no longer live and whose last update is older than
/// `max_age` are deleted and excluded from the result. Recent "done" markers
/// stay visible even after the Pi process exited.
pub fn load_and_prune(dir: &Path, live_pane_ids: &HashSet<String>, max_age: Duration) -> Vec<PiStatus> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let now = now_ms();
    let max_age_ms = max_age.as_millis() as u64;
    let mut result = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(data) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(status) = parse_status(&data) else {
            continue;
        };
        if !live_pane_ids.contains(&status.pane_id)
            && now.saturating_sub(status.updated_at_ms) > max_age_ms
        {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        result.push(status);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn status_json(pane_id: &str, session: &str, state: &str, updated_at_ms: u64) -> String {
        format!(
            r#"{{"schema":1,"agent":"pi","pane_id":"{}","tmux_session_name":"{}","state":"{}","updated_at_ms":{}}}"#,
            pane_id, session, state, updated_at_ms
        )
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tws-pi-status-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parse_valid_status() {
        let s = parse_status(&status_json("%12", "tws_work_proj", "working", 1000)).unwrap();
        assert_eq!(s.pane_id, "%12");
        assert_eq!(s.tmux_session_name, "tws_work_proj");
        assert_eq!(s.work_state, PiWorkState::Working);
        assert_eq!(s.updated_at_ms, 1000);
        assert_eq!(s.finished_at_ms, None);
    }

    #[test]
    fn parse_done_with_finished_at() {
        let json = r#"{"pane_id":"%3","tmux_session_name":"tws_a_b","state":"done","updated_at_ms":5,"finished_at_ms":5,"session_name":"Fix tests"}"#;
        let s = parse_status(json).unwrap();
        assert_eq!(s.work_state, PiWorkState::Done);
        assert_eq!(s.finished_at_ms, Some(5));
        assert_eq!(s.session_name.as_deref(), Some("Fix tests"));
    }

    #[test]
    fn parse_unknown_state_is_forward_compatible() {
        let s = parse_status(&status_json("%1", "tws_x", "compacting", 1)).unwrap();
        assert_eq!(s.work_state, PiWorkState::Unknown);
    }

    #[test]
    fn parse_rejects_invalid_json_and_missing_pane() {
        assert!(parse_status("not json").is_none());
        assert!(parse_status(r#"{"state":"working"}"#).is_none());
    }

    #[test]
    fn load_all_skips_invalid_files() {
        let dir = temp_dir("load");
        std::fs::write(dir.join("a.json"), status_json("%1", "tws_x", "working", 1)).unwrap();
        std::fs::write(dir.join("b.json"), "garbage").unwrap();
        std::fs::write(dir.join("c.txt"), status_json("%2", "tws_x", "done", 1)).unwrap();

        let all = load_all(&dir);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].pane_id, "%1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_all_missing_dir_is_empty() {
        assert!(load_all(Path::new("/nonexistent/tws-pi-status")).is_empty());
    }

    #[test]
    fn prune_removes_only_stale_dead_panes() {
        let dir = temp_dir("prune");
        let now = now_ms();
        // Live pane, ancient status → kept.
        std::fs::write(dir.join("live.json"), status_json("%1", "tws_x", "working", 0)).unwrap();
        // Dead pane, fresh status → kept (recent "done" marker).
        std::fs::write(dir.join("fresh.json"), status_json("%2", "tws_x", "done", now)).unwrap();
        // Dead pane, ancient status → removed.
        std::fs::write(dir.join("stale.json"), status_json("%3", "tws_x", "done", 0)).unwrap();

        let live: HashSet<String> = ["%1".to_string()].into_iter().collect();
        let loaded = load_and_prune(&dir, &live, Duration::from_secs(60 * 60));

        let panes: Vec<&str> = loaded.iter().map(|s| s.pane_id.as_str()).collect();
        assert!(panes.contains(&"%1"));
        assert!(panes.contains(&"%2"));
        assert!(!panes.contains(&"%3"));
        // The stale file is gone from disk too.
        assert!(!dir.join("stale.json").exists());
        assert!(dir.join("fresh.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
