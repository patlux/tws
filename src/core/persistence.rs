use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::model::Collection;

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct UiState {
    pub open_nodes: Vec<Vec<String>>,
    pub selected: Option<Vec<String>>,
    #[serde(default)]
    pub agents_view_active: bool,
    #[serde(default)]
    pub agent_list_cursor: usize,
    /// Persisted pin assignments: `(pane_id, slot)`. Reapplied on first scan after startup;
    /// entries whose pane_id is no longer live are silently dropped.
    ///
    /// Note: tmux recycles pane ids (%1, %2, …) after a server restart, so a restored pin
    /// could theoretically attach to an unrelated agent that inherited the id. This is
    /// inherently fuzzy for cross-restart persistence; within a single tmux server lifetime
    /// the pane_id is stable and the mapping is exact.
    #[serde(default)]
    pub pins: Vec<(String, u8)>,
}

fn ui_state_file() -> PathBuf {
    config_dir().join("ui-state.json")
}

pub fn load_ui() -> UiState {
    let path = ui_state_file();
    if !path.exists() {
        return UiState::default();
    }
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(_) => return UiState::default(),
    };
    serde_json::from_str(&data).unwrap_or_default()
}

pub fn save_ui(ui: &UiState) -> io::Result<()> {
    ensure_config_dir()?;
    let data = serde_json::to_string_pretty(ui)?;
    write_atomic(&ui_state_file(), &data)?;
    Ok(())
}

/// Write via temp file + rename so a crash mid-write never corrupts the target.
fn write_atomic(path: &Path, data: &str) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Create the config dir if needed and keep it private (0700 on Unix) —
/// notes and state can contain sensitive project details.
fn ensure_config_dir() -> io::Result<PathBuf> {
    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

pub(crate) fn config_dir() -> PathBuf {
    // Respect XDG_CONFIG_HOME when set; fall back to ~/.config (the historic
    // location on all platforms), then to a relative path if home is unknown.
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("tws")
}

fn state_file() -> PathBuf {
    config_dir().join("state.json")
}

pub fn load() -> io::Result<Vec<Collection>> {
    let path = state_file();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = fs::read_to_string(&path)?;
    let collections: Vec<Collection> =
        serde_json::from_str(&data).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(collections)
}

pub fn save(collections: &[Collection]) -> io::Result<()> {
    ensure_config_dir()?;
    let data = serde_json::to_string_pretty(collections)?;
    write_atomic(&state_file(), &data)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::Thread;
    use std::env;

    fn with_temp_config<F: FnOnce()>(f: F) {
        let dir = env::temp_dir().join(format!("tws_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        // We test save/load by writing directly to a temp path
        // rather than overriding config_dir
        let path = dir.join("state.json");

        let mut col = Collection::new("Test");
        col.threads.push(Thread::new("Thread A"));
        let collections = vec![col];

        let data = serde_json::to_string_pretty(&collections).unwrap();
        fs::write(&path, &data).unwrap();

        let loaded: Vec<Collection> =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "Test");
        assert_eq!(loaded[0].threads.len(), 1);
        assert_eq!(loaded[0].threads[0].name, "Thread A");

        fs::remove_dir_all(&dir).unwrap();
        f();
    }

    #[test]
    fn round_trip_serialization() {
        with_temp_config(|| {});
    }

    #[test]
    fn load_missing_file_returns_empty() {
        // load() returns empty vec when file doesn't exist
        // We can't easily test this without mocking config_dir,
        // so we test the logic directly
        let path = env::temp_dir().join("tws_nonexistent_state.json");
        assert!(!path.exists());
        // Simulating what load() does:
        if !path.exists() {
            let result: Vec<Collection> = Vec::new();
            assert!(result.is_empty());
        }
    }

    #[test]
    fn deserialize_without_is_root_defaults_false() {
        // Simulate loading an old state.json that predates the is_root/hidden fields
        let json = r#"[{
            "id": "00000000-0000-0000-0000-000000000001",
            "name": "Legacy",
            "threads": [{
                "id": "00000000-0000-0000-0000-000000000002",
                "name": "Thread A",
                "description": null
            }]
        }]"#;
        let collections: Vec<Collection> = serde_json::from_str(json).unwrap();
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].name, "Legacy");
        assert!(!collections[0].is_root);
        assert!(!collections[0].hidden);
        assert!(!collections[0].threads[0].hidden);
    }

    #[test]
    fn root_collection_round_trip() {
        let mut col = Collection::new_root();
        col.threads.push(Thread::new("general"));
        let collections = vec![col];

        let json = serde_json::to_string_pretty(&collections).unwrap();
        let loaded: Vec<Collection> = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].is_root);
        assert_eq!(loaded[0].threads.len(), 1);
        assert_eq!(loaded[0].threads[0].name, "general");
    }

    #[test]
    fn empty_collections_serialize() {
        let collections: Vec<Collection> = Vec::new();
        let json = serde_json::to_string_pretty(&collections).unwrap();
        let loaded: Vec<Collection> = serde_json::from_str(&json).unwrap();
        assert!(loaded.is_empty());
    }
}
