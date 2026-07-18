use std::ffi::OsString;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Upper bound for a single non-interactive tmux invocation. A hung tmux
/// server must never freeze the UI thread (or a background worker)
/// indefinitely — every call except interactive `attach` goes through this.
const TMUX_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// Like `Command::output()` but kills the child once `timeout` elapses, so a
/// stuck tmux server surfaces as an error instead of an indefinite block.
pub(crate) fn output_with_timeout(command: &mut Command, timeout: Duration) -> io::Result<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;

    // Drain both pipes concurrently while the process runs. Polling a child
    // with unread pipes can deadlock when verbose stderr/stdout fills the OS
    // pipe buffer before the process exits.
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("tmux stdout pipe was not created"))?;
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("tmux stderr pipe was not created"))?;
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });

    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("timed out after {}s", timeout.as_secs()),
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    let stdout = stdout_reader
        .join()
        .map_err(|_| io::Error::other("stdout reader thread panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| io::Error::other("stderr reader thread panicked"))??;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn output_of(command: &mut Command) -> Result<Output, String> {
    output_with_timeout(command, TMUX_COMMAND_TIMEOUT)
        .map_err(|err| format!("failed to run tmux: {}", err))
}

/// True when tmux stderr means "no server is running" (fresh machine or the
/// server exited) as opposed to a real failure (socket permissions, broken
/// binary, …). Only the former may be read as an empty session list.
pub(crate) fn is_no_server_error(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("no server running")
        || lower.contains("error connecting to")
        || lower.contains("can't find socket")
        || lower.contains("no such file or directory")
}

/// True when a `switch-client` failure means there is no live client to
/// switch (stale `TMUX` env) and the caller should retry with an external
/// attach. Deliberately narrow: session/server target errors must not fall
/// back.
pub fn switch_error_indicates_no_client(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("no current client")
        || lower.contains("can't find client")
        || is_no_server_error(&lower)
}

/// Run a tmux command with piped stdio (never inheriting, so tmux errors
/// can't corrupt the raw-mode screen) and surface stderr on failure.
fn run_tmux(args: &[&str]) -> Result<(), String> {
    let output = output_of(Command::new("tmux").args(args))?;
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
/// A missing tmux server is `Ok(vec![])`; real failures (spawn, socket
/// permissions, timeout, unexpected stderr) are `Err` so callers can retain
/// their last-known state instead of wiping it.
pub fn list_sessions() -> Result<Vec<String>, String> {
    let output = output_of(Command::new("tmux").args(["list-sessions", "-F", "#{session_name}"]))?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Ok(stdout
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if is_no_server_error(&stderr) {
        Ok(Vec::new())
    } else {
        Err(stderr)
    }
}

/// Returns tws-prefixed sessions with their `last_attached` Unix timestamps.
/// Each entry is `(session_name, last_attached_timestamp)`.
/// Same error contract as [`list_sessions`].
pub fn list_tws_sessions_with_timestamps() -> Result<Vec<(String, i64)>, String> {
    let output = output_of(Command::new("tmux").args([
        "list-sessions",
        "-F",
        "#{session_name}\t#{session_last_attached}",
    ]))?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Ok(stdout
            .lines()
            .filter_map(|line| {
                let (name, ts_str) = line.split_once('\t')?;
                if !name.starts_with("tws_") && !name.starts_with("twsr_") {
                    return None;
                }
                let ts = ts_str.parse::<i64>().unwrap_or(0);
                Some((name.to_string(), ts))
            })
            .collect());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if is_no_server_error(&stderr) {
        Ok(Vec::new())
    } else {
        Err(stderr)
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
    let output = output_of(&mut tmux)?;
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
/// `TMUX` is scrubbed from the child environment so a stale inherited value
/// can't make tmux refuse the attach ("sessions should be nested with care").
pub fn attach_session(name: &str) -> std::io::Result<bool> {
    let status = Command::new("tmux")
        .args(["attach-session", "-t", &exact(name)])
        .env_remove("TMUX")
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
    let output = output_with_timeout(
        Command::new("tmux").args(["capture-pane", "-t", pane_id, "-e", "-p"]),
        TMUX_COMMAND_TIMEOUT,
    )
    .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        None
    }
}

/// Returns true if we're currently running inside a tmux session.
/// A stale inherited `TMUX` (dead server, missing socket) counts as *outside*
/// so callers route to `attach-session` instead of a doomed `switch-client`.
pub fn is_inside_tmux() -> bool {
    let Some(value) = std::env::var("TMUX").ok().filter(|v| !v.is_empty()) else {
        return false;
    };
    // Format: "<socket_path>,<server_pid>,<session_id>".
    let socket = value.split(',').next().unwrap_or("");
    !socket.is_empty() && Path::new(socket).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn timeout_wrapper_drains_large_output_without_pipe_deadlock() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "dd if=/dev/zero bs=1024 count=256 2>/dev/null; dd if=/dev/zero bs=1024 count=256 1>&2 2>/dev/null",
        ]);
        let output = output_with_timeout(&mut command, Duration::from_secs(2)).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 256 * 1024);
        assert_eq!(output.stderr.len(), 256 * 1024);
    }

    #[test]
    fn no_server_detection_covers_tmux_messages() {
        assert!(is_no_server_error(
            "no server running on /tmp/tmux-1000/default"
        ));
        assert!(is_no_server_error(
            "error connecting to /tmp/tmux-1000/default (No such file or directory)"
        ));
        assert!(is_no_server_error(
            "can't find socket: /tmp/tmux-1000/default"
        ));
        assert!(!is_no_server_error("can't find session: tws_x"));
        assert!(!is_no_server_error("permission denied"));
    }

    #[test]
    fn switch_error_fallback_is_narrow() {
        assert!(switch_error_indicates_no_client("no current client"));
        assert!(switch_error_indicates_no_client("can't find client"));
        assert!(switch_error_indicates_no_client(
            "no server running on /tmp/tmux-1000/default"
        ));
        assert!(!switch_error_indicates_no_client(
            "can't find session: tws_a_b_c"
        ));
        assert!(!switch_error_indicates_no_client(
            "duplicate session: tws_a_b_c"
        ));
    }

    #[test]
    fn stale_tmux_env_counts_as_outside() {
        // A TMUX value pointing at a nonexistent socket must be treated as outside.
        // (Env mutation is process-wide; this test only sets a bogus value and
        // never relies on a real one.)
        unsafe {
            std::env::set_var("TMUX", "/nonexistent/tws-test-socket,123,0");
        }
        assert!(!is_inside_tmux());
        unsafe {
            std::env::remove_var("TMUX");
        }
        assert!(!is_inside_tmux());
    }
}
