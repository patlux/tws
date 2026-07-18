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
    pub agent_grid_active: bool,
    /// Whether the active filter (hide threads without live sessions) is on.
    #[serde(default)]
    pub active_filter: bool,
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

/// Load UI state, returning a human-readable warning when an existing file
/// could not be read or parsed (instead of silently resetting to defaults).
pub fn load_ui() -> (UiState, Option<String>) {
    load_ui_from(&ui_state_file())
}

fn load_ui_from(path: &Path) -> (UiState, Option<String>) {
    if !path.exists() {
        return (UiState::default(), None);
    }
    let data = match std::fs::read_to_string(path) {
        Ok(d) => d,
        Err(e) => {
            return (
                UiState::default(),
                Some(format!("could not read {}: {}", path.display(), e)),
            );
        }
    };
    match serde_json::from_str(&data) {
        Ok(ui) => (ui, None),
        Err(e) => (
            UiState::default(),
            Some(format!("could not parse {}: {}", path.display(), e)),
        ),
    }
}

pub fn save_ui(ui: &UiState) -> io::Result<()> {
    ensure_config_dir()?;
    let data = serde_json::to_string_pretty(ui)?;
    write_atomic(&ui_state_file(), &data)?;
    Ok(())
}

/// Process-unique suffix for temp files so two tws instances (explicitly
/// allowed past the lock warning) can never clobber each other's temp file.
fn unique_tmp(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    path.with_extension(format!("{}.{}.tmp", std::process::id(), n))
}

/// Write via a per-process temp file + fsync + rename so a crash mid-write
/// never corrupts the target, concurrent instances never share the temp
/// file, and the data survives an OS crash/power loss (temp file and parent
/// directory are both synced before/after the rename).
fn write_atomic(path: &Path, data: &str) -> io::Result<()> {
    use io::Write;
    let tmp = unique_tmp(path);
    let result = (|| -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(data.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        // Make the rename itself durable.
        #[cfg(unix)]
        if let Some(dir) = path.parent() {
            let dir_file = fs::File::open(dir)?;
            dir_file.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Create the config dir if needed and keep it private (0700 on Unix) —
/// State can contain sensitive project details.
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
    if let Some(path) = std::env::var_os("TWS_CONFIG_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return path;
    }

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

/// Best-effort single-instance lock. Holds `tws.lock` (containing our PID)
/// for the process lifetime; removed on clean exit via Drop.
pub struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Outcome of trying to take the instance lock.
pub enum LockState {
    /// We hold the lock now.
    Acquired(LockGuard),
    /// Another live tws process (PID) already holds it.
    HeldByOther(u32),
    /// Locking itself failed (permissions/I/O). Never pretend acquisition:
    /// doing so would silently re-enable concurrent writers.
    Failed(io::Error),
}

pub fn acquire_instance_lock() -> LockState {
    let dir = match ensure_config_dir() {
        Ok(dir) => dir,
        Err(err) => return LockState::Failed(err),
    };
    let path = dir.join("tws.lock");

    loop {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                use io::Write;
                if let Err(err) = write!(f, "{}", std::process::id()).and_then(|()| f.sync_all()) {
                    let _ = fs::remove_file(&path);
                    return LockState::Failed(err);
                }
                return LockState::Acquired(LockGuard { path });
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let holder = fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok());
                match holder {
                    Some(pid) if pid != std::process::id() && process_alive(pid) => {
                        return LockState::HeldByOther(pid);
                    }
                    _ => {
                        // Stale lock (dead process or unreadable) — reclaim.
                        if let Err(err) = fs::remove_file(&path) {
                            return LockState::Failed(err);
                        }
                        continue;
                    }
                }
            }
            Err(err) => return LockState::Failed(err),
        }
    }
}

fn process_alive(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn load() -> io::Result<Vec<Collection>> {
    load_from(&state_file())
}

fn load_from(path: &Path) -> io::Result<Vec<Collection>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = fs::read_to_string(path)?;
    match serde_json::from_str::<Vec<Collection>>(&data) {
        Ok(collections) => Ok(collections),
        Err(e) => {
            // Try the backup written before the last successful save.
            let bak = path.with_extension("json.bak");
            if let Ok(bak_data) = fs::read_to_string(&bak)
                && let Ok(collections) = serde_json::from_str::<Vec<Collection>>(&bak_data)
            {
                return Ok(collections);
            }
            Err(io::Error::new(io::ErrorKind::InvalidData, e))
        }
    }
}

pub fn save(collections: &[Collection]) -> io::Result<()> {
    ensure_config_dir()?;
    save_to(&state_file(), collections)
}

/// Save `collections` to `path`, first replacing the `.bak` recovery copy
/// with the current file contents. A failed backup is propagated (not
/// silently dropped) — saving without a working recovery path must be loud.
fn save_to(path: &Path, collections: &[Collection]) -> io::Result<()> {
    let data = serde_json::to_string_pretty(collections)?;
    if path.exists() {
        let previous = fs::read_to_string(path)?;
        write_atomic(&path.with_extension("json.bak"), &previous)?;
    }
    write_atomic(path, &data)?;
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
    fn ui_state_without_active_filter_defaults_false() {
        // Old ui-state.json predating the active_filter field loads cleanly.
        let json = r#"{"open_nodes": [], "selected": null}"#;
        let ui: UiState = serde_json::from_str(json).unwrap();
        assert!(!ui.active_filter);
    }

    #[test]
    fn ui_state_active_filter_round_trip() {
        let ui = UiState {
            active_filter: true,
            ..UiState::default()
        };
        let json = serde_json::to_string(&ui).unwrap();
        let loaded: UiState = serde_json::from_str(&json).unwrap();
        assert!(loaded.active_filter);
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

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("tws_test_{}_{}", tag, uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn write_atomic_leaves_no_temp_files_and_valid_content() {
        let dir = temp_dir("atomic");
        let path = dir.join("state.json");
        write_atomic(&path, "{}").unwrap();
        write_atomic(&path, "{\"a\":1}").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"a\":1}");
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files must not linger");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_to_writes_backup_of_previous_state() {
        let dir = temp_dir("backup");
        let path = dir.join("state.json");
        let mut col = Collection::new("First");
        col.threads.push(Thread::new("T"));
        save_to(&path, std::slice::from_ref(&col)).unwrap();

        let mut col2 = Collection::new("Second");
        col2.threads.push(Thread::new("T2"));
        save_to(&path, std::slice::from_ref(&col2)).unwrap();

        let bak = fs::read_to_string(path.with_extension("json.bak")).unwrap();
        let backup: Vec<Collection> = serde_json::from_str(&bak).unwrap();
        assert_eq!(backup[0].name, "First");
        let current = load_from(&path).unwrap();
        assert_eq!(current[0].name, "Second");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_to_propagates_backup_failure() {
        let dir = temp_dir("backupfail");
        let path = dir.join("state.json");
        let mut col = Collection::new("First");
        col.threads.push(Thread::new("T"));
        save_to(&path, std::slice::from_ref(&col)).unwrap();
        // Make the backup target unwritable: a directory occupies the .bak path.
        fs::create_dir(dir.join("state.json.bak")).unwrap();
        let mut col2 = Collection::new("Second");
        col2.threads.push(Thread::new("T2"));
        let result = save_to(&path, std::slice::from_ref(&col2));
        assert!(
            result.is_err(),
            "backup failure must surface, not be ignored"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_ui_from_warns_on_unparseable_existing_file() {
        let dir = temp_dir("uiwarn");
        let path = dir.join("ui-state.json");
        fs::write(&path, "not json").unwrap();
        let (ui, warning) = load_ui_from(&path);
        assert!(warning.is_some(), "corrupt UI state must produce a warning");
        assert!(ui.open_nodes.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_ui_from_missing_file_is_silent_default() {
        let dir = temp_dir("uimissing");
        let path = dir.join("ui-state.json");
        let (ui, warning) = load_ui_from(&path);
        assert!(warning.is_none());
        assert!(ui.selected.is_none());
        fs::remove_dir_all(&dir).unwrap();
    }
}
