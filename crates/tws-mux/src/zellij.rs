use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use tws_core::model::{AgentSession, AgentType};

fn run(args: &[&str]) -> Result<(), String> {
    let output = Command::new("zellij")
        .args(args)
        .output()
        .map_err(|err| format!("failed to run zellij: {err}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            "zellij command failed".to_string()
        } else {
            stderr
        })
    }
}

fn output(args: &[&str]) -> Option<String> {
    let result = Command::new("zellij").args(args).output().ok()?;
    result
        .status
        .success()
        .then(|| String::from_utf8_lossy(&result.stdout).into_owned())
}

pub fn list_sessions() -> Vec<String> {
    output(&["list-sessions", "--short"])
        .map(|stdout| {
            stdout
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn recency_file() -> PathBuf {
    crate::config_dir().join("zellij-recency.json")
}

fn load_recency() -> HashMap<String, i64> {
    fs::read_to_string(recency_file())
        .ok()
        .and_then(|data| serde_json::from_str(&data).ok())
        .unwrap_or_default()
}

fn save_recency(recency: &HashMap<String, i64>) {
    let path = recency_file();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(data) = serde_json::to_string(recency) {
        let temp = path.with_extension("tmp");
        if fs::write(&temp, data).is_ok() {
            let _ = fs::rename(temp, path);
        }
    }
}

pub fn record_attach(name: &str) {
    let mut recency = load_recency();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    recency.insert(name.to_string(), timestamp);
    save_recency(&recency);
}

pub fn list_managed_sessions_with_timestamps() -> Vec<(String, i64)> {
    let recency = load_recency();
    list_sessions()
        .into_iter()
        .filter(|name| crate::is_managed_name(name))
        .map(|name| {
            let timestamp = recency.get(&name).copied().unwrap_or(0);
            (name, timestamp)
        })
        .collect()
}

pub fn new_session(name: &str, cwd: Option<&Path>) -> Result<(), String> {
    let mut command = Command::new("zellij");
    command.args(["attach", "--create-background", name]);
    let layout = std::env::var("TWS_ZELLIJ_LAYOUT").ok();
    if cwd.is_some() || layout.is_some() {
        command.arg("options");
    }
    if let Some(cwd) = cwd {
        command.arg("--default-cwd").arg(cwd);
    }
    if let Some(layout) = layout {
        command.arg("--default-layout").arg(layout);
    }
    let output = command
        .output()
        .map_err(|err| format!("failed to run zellij: {err}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            "zellij session creation failed".to_string()
        } else {
            stderr
        })
    }
}

pub fn kill_session(name: &str) -> Result<(), String> {
    run(&["kill-session", name])?;
    let mut recency = load_recency();
    if recency.remove(name).is_some() {
        save_recency(&recency);
    }
    Ok(())
}

pub fn rename_session(old_name: &str, new_name: &str) -> Result<(), String> {
    run(&["--session", old_name, "action", "rename-session", new_name])?;
    let mut recency = load_recency();
    if let Some(timestamp) = recency.remove(old_name) {
        recency.insert(new_name.to_string(), timestamp);
        save_recency(&recency);
    }
    Ok(())
}

pub fn switch_session(name: &str, pane_id: Option<&str>) -> Result<(), String> {
    let mut args = vec!["action", "switch-session", name];
    if let Some(pane_id) = pane_id {
        args.extend(["--pane-id", pane_id]);
    }
    run(&args)?;
    record_attach(name);
    Ok(())
}

pub fn attach_session(name: &str) -> std::io::Result<bool> {
    Command::new("zellij")
        .args(["attach", name])
        .status()
        .map(|status| status.success())
}

pub fn focus_pane(session_name: &str, pane_id: &str) -> Result<(), String> {
    run(&[
        "--session",
        session_name,
        "action",
        "focus-pane-id",
        pane_id,
    ])
}

pub fn capture_pane(session_name: &str, pane_id: &str) -> Option<String> {
    output(&[
        "--session",
        session_name,
        "action",
        "dump-screen",
        "--pane-id",
        pane_id,
        "--ansi",
    ])
}

#[derive(Debug, Deserialize)]
struct PaneInfo {
    id: u32,
    #[serde(default)]
    is_plugin: bool,
    #[serde(default)]
    title: String,
    #[serde(default)]
    tab_position: u32,
    #[serde(default)]
    exited: bool,
    #[serde(default, alias = "pane_command")]
    terminal_command: Option<String>,
}

fn parse_panes(data: &str) -> Vec<PaneInfo> {
    serde_json::from_str(data).unwrap_or_default()
}

fn list_panes(session_name: &str) -> Vec<PaneInfo> {
    output(&[
        "--session",
        session_name,
        "action",
        "list-panes",
        "--json",
        "--all",
    ])
    .map(|data| parse_panes(&data))
    .unwrap_or_default()
}

fn identify_agent(command: &str) -> Option<AgentType> {
    let mut tokens = command.split_whitespace();
    let executable = tokens.next()?;
    let basename = executable.rsplit('/').next().unwrap_or(executable);
    match basename {
        "claude" => Some(AgentType::ClaudeCode),
        "codex" => Some(AgentType::Codex),
        "pi" | "pi-coding-agent" => Some(AgentType::Pi),
        "node" | "deno" => tokens.find_map(|token| {
            if token.contains("pi-coding-agent") {
                Some(AgentType::Pi)
            } else if token.contains("claude-code") || token.contains("node_modules/claude/") {
                Some(AgentType::ClaudeCode)
            } else if token.contains("node_modules/codex/") {
                Some(AgentType::Codex)
            } else {
                None
            }
        }),
        _ => None,
    }
}

fn clean_title(title: &str, agent_type: AgentType) -> String {
    let title = title.trim();
    if agent_type != AgentType::ClaudeCode {
        return title.to_string();
    }
    let title = title.trim_start_matches(|character: char| {
        character.is_whitespace() || ('\u{2800}'..='\u{28ff}').contains(&character)
    });
    title.strip_prefix('\u{2733}').unwrap_or(title).trim_start().to_string()
}

pub fn scan_agents(session_names: &[String]) -> Vec<AgentSession> {
    let sessions: HashSet<&str> = session_names.iter().map(String::as_str).collect();
    let mut result = Vec::new();
    for session_name in list_sessions() {
        if !sessions.contains(session_name.as_str()) {
            continue;
        }
        for pane in list_panes(&session_name) {
            if pane.is_plugin || pane.exited {
                continue;
            }
            let command = pane.terminal_command.as_deref().unwrap_or_default();
            let Some(agent_type) = identify_agent(command) else {
                continue;
            };
            let display_name = clean_title(&pane.title, agent_type);
            result.push(AgentSession {
                agent_type,
                tmux_session_name: session_name.clone(),
                window_index: pane.tab_position,
                pane_id: format!("terminal_{}", pane.id),
                display_name: if display_name.is_empty() {
                    format!("{} (tab:{})", agent_type.display_name(), pane.tab_position + 1)
                } else {
                    display_name
                },
                renamed: false,
                pin_slot: None,
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_direct_agents() {
        assert_eq!(identify_agent("claude"), Some(AgentType::ClaudeCode));
        assert_eq!(identify_agent("/usr/bin/codex"), Some(AgentType::Codex));
        assert_eq!(identify_agent("pi"), Some(AgentType::Pi));
        assert_eq!(identify_agent("nvim"), None);
    }

    #[test]
    fn identifies_wrapped_agents() {
        assert_eq!(
            identify_agent("node /tmp/node_modules/@earendil-works/pi-coding-agent/dist/cli.js"),
            Some(AgentType::Pi)
        );
        assert_eq!(
            identify_agent("node /tmp/node_modules/@anthropic-ai/claude-code/cli.js"),
            Some(AgentType::ClaudeCode)
        );
    }

    #[test]
    fn parses_real_zellij_pane_command_fields() {
        let panes = parse_panes(
            r#"[
                {"id":0,"is_plugin":false,"title":"layout","tab_position":0,"exited":false,"terminal_command":"pi"},
                {"id":1,"is_plugin":false,"title":"shell","tab_position":1,"exited":false,"pane_command":"claude"}
            ]"#,
        );
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0].terminal_command.as_deref(), Some("pi"));
        assert_eq!(panes[1].terminal_command.as_deref(), Some("claude"));
    }
}
