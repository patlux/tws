use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

/// Run a tmux command with `.output()` (never inheriting stdio, so tmux
/// errors can't corrupt the raw-mode screen) and surface stderr on failure.
fn run_tmux(args: &[&str]) -> Result<(), String> {
    let output = Command::new("tmux")
        .args(args)
        .output()
        .map_err(|err| format!("failed to run tmux: {}", err))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            Err("tmux command failed".to_string())
        } else {
            Err(stderr)
        }
    }
}

/// Exact-match session target (`=name`) so tmux doesn't prefix-match
/// another session with a similar name.
fn exact(name: &str) -> String {
    format!("={}", name)
}

/// Returns the names of all running tmux sessions.
/// Returns an empty Vec if the tmux server isn't running.
pub fn list_sessions() -> Vec<String> {
    let output = Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}"])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout
                .lines()
                .filter(|l| !l.is_empty())
                .map(|l| l.to_string())
                .collect()
        }
        // tmux returns error when no server is running — that's fine
        _ => Vec::new(),
    }
}

/// Returns tws-prefixed sessions with their `last_attached` Unix timestamps.
/// Each entry is `(session_name, last_attached_timestamp)`.
pub fn list_tws_sessions_with_timestamps() -> Vec<(String, i64)> {
    let output = Command::new("tmux")
        .args([
            "list-sessions",
            "-F",
            "#{session_name}\t#{session_last_attached}",
        ])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout
                .lines()
                .filter_map(|line| {
                    let (name, ts_str) = line.split_once('\t')?;
                    if !name.starts_with("tws_") && !name.starts_with("twsr_") {
                        return None;
                    }
                    let ts = ts_str.parse::<i64>().unwrap_or(0);
                    Some((name.to_string(), ts))
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

/// Creates a new detached tmux session with the given name.
pub fn new_session(name: &str) -> Result<(), String> {
    run_tmux(&["new-session", "-d", "-s", name])
}

/// Creates a new detached tmux session with the given name and working directory.
pub fn new_session_in_dir(name: &str, cwd: &Path) -> Result<(), String> {
    new_session_in_dir_with_command(name, cwd, &[])
}

/// Creates a new detached tmux session and runs a command without shell parsing.
pub fn new_session_in_dir_with_command(
    name: &str,
    cwd: &Path,
    command: &[OsString],
) -> Result<(), String> {
    let mut tmux = Command::new("tmux");
    tmux.args(["new-session", "-d", "-s", name, "-c"]).arg(cwd);
    if !command.is_empty() {
        tmux.arg("--").args(command);
    }
    let output = tmux
        .output()
        .map_err(|err| format!("failed to run tmux: {}", err))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            Err("tmux new-session failed".to_string())
        } else {
            Err(stderr)
        }
    }
}

/// Kills the tmux session with the given name.
pub fn kill_session(name: &str) -> Result<(), String> {
    run_tmux(&["kill-session", "-t", &exact(name)])
}

/// Renames a tmux session.
pub fn rename_session(old_name: &str, new_name: &str) -> Result<(), String> {
    run_tmux(&["rename-session", "-t", &exact(old_name), new_name])
}

/// Switches the current tmux client to the given session.
/// Non-blocking — only works when already inside tmux.
pub fn switch_client(name: &str) -> Result<(), String> {
    run_tmux(&["switch-client", "-t", &exact(name)])
}

/// Attaches to the given tmux session, inheriting stdio.
/// **Blocks** until the user detaches. Only use outside tmux.
pub fn attach_session(name: &str) -> std::io::Result<bool> {
    let status = Command::new("tmux")
        .args(["attach-session", "-t", &exact(name)])
        .status()?;
    Ok(status.success())
}

/// Selects the given window in the target session.
/// Works across sessions — doesn't require being attached to that session.
pub fn select_window(session_name: &str, window_index: u32) -> Result<(), String> {
    let target = format!("={}:{}", session_name, window_index);
    run_tmux(&["select-window", "-t", &target])
}

/// Selects the given pane (by global pane ID like "%5").
/// Works across sessions — doesn't require being attached to that session.
pub fn select_pane(pane_id: &str) -> Result<(), String> {
    run_tmux(&["select-pane", "-t", pane_id])
}

/// Captures the visible content of a tmux pane, including ANSI escape sequences.
/// Returns `None` if the pane doesn't exist or the command fails.
pub fn capture_pane(pane_id: &str) -> Option<String> {
    let output = Command::new("tmux")
        .args(["capture-pane", "-t", pane_id, "-e", "-p"])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        None
    }
}

/// Returns true if we're currently running inside a tmux session.
pub fn is_inside_tmux() -> bool {
    std::env::var("TMUX").is_ok_and(|v| !v.is_empty())
}
