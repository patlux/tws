use std::collections::{HashMap, HashSet, VecDeque};
use std::process::Command;
use std::time::Duration;

use tws_core::model::{AgentSession, AgentType};

use crate::agent_detection::{clean_pane_title, identify_agent};
use crate::tmux::commands::output_with_timeout;

/// Deepest pane-descendant chain we follow looking for agents.
const MAX_AGENT_DEPTH: usize = 8;
const SCAN_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// Pane info parsed from tmux list-panes output.
struct PaneInfo {
    session_name: String,
    window_index: u32,
    pane_id: String,
    pane_pid: u32,
    pane_title: String,
}

/// Scan all tmux panes for known AI agents (Claude Code, Codex, Pi).
/// Only scans panes belonging to the given tws-managed session names.
///
/// Returns `Err` when discovery itself fails (tmux/ps errors, timeouts) so
/// callers can retain their previous agent list instead of treating the
/// failure as "no agents".
pub fn scan_agents(tws_sessions: &[String]) -> Result<Vec<AgentSession>, String> {
    if tws_sessions.is_empty() {
        return Ok(Vec::new());
    }

    let session_set: HashSet<&str> = tws_sessions.iter().map(|s| s.as_str()).collect();

    let panes = parse_panes(&list_all_panes()?);
    let panes: Vec<PaneInfo> = panes
        .into_iter()
        .filter(|p| session_set.contains(p.session_name.as_str()))
        .collect();

    if panes.is_empty() {
        return Ok(Vec::new());
    }

    // One full process-table read, indexed by PPID, so agents hiding behind
    // wrapper scripts / task runners / supervisors (any descendant of the
    // pane shell, not just direct children) are still found.
    let by_ppid = parse_processes(&list_all_processes()?);

    Ok(match_agents(&panes, &by_ppid))
}

fn list_all_panes() -> Result<String, String> {
    // Filter tws-managed sessions on the tmux side so panes of unrelated
    // sessions never cross the process boundary.
    let output = output_with_timeout(
        Command::new("tmux").args([
            "list-panes",
            "-a",
            "-f",
            "#{||:#{m:tws_*,#{session_name}},#{m:twsr_*,#{session_name}}}",
            "-F",
            "#{session_name}\t#{window_index}\t#{pane_id}\t#{pane_pid}\t#{pane_title}",
        ]),
        SCAN_COMMAND_TIMEOUT,
    )
    .map_err(|err| format!("failed to run tmux list-panes: {}", err))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            "tmux list-panes failed".to_string()
        } else {
            stderr
        })
    }
}

fn list_all_processes() -> Result<String, String> {
    let output = output_with_timeout(
        Command::new("ps").args(["-e", "-ww", "-o", "pid,ppid,command"]),
        SCAN_COMMAND_TIMEOUT,
    )
    .map_err(|err| format!("failed to run ps: {}", err))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err("ps -e failed".to_string())
    }
}

fn parse_panes(raw: &str) -> Vec<PaneInfo> {
    raw.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(5, '\t');
            let session_name = parts.next()?.to_string();
            let window_index = parts.next()?.parse::<u32>().ok()?;
            let pane_id = parts.next()?.to_string();
            let pane_pid = parts.next()?.parse::<u32>().ok()?;
            let pane_title = parts.next().unwrap_or("").to_string();
            Some(PaneInfo {
                session_name,
                window_index,
                pane_id,
                pane_pid,
                pane_title,
            })
        })
        .collect()
}

/// Build a map of parent_pid → Vec<(child_pid, command_name)>.
fn parse_processes(raw: &str) -> HashMap<u32, Vec<(u32, String)>> {
    let mut map: HashMap<u32, Vec<(u32, String)>> = HashMap::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        // Format: "  PID  PPID COMM" — use split_whitespace to collapse multiple spaces
        let mut parts = trimmed.split_whitespace();
        let pid = match parts.next().and_then(|s| s.parse::<u32>().ok()) {
            Some(p) => p,
            None => continue, // skips header line too (PID is not a u32)
        };
        let ppid = match parts.next().and_then(|s| s.parse::<u32>().ok()) {
            Some(p) => p,
            None => continue,
        };
        // Remaining tokens are the command (may contain spaces on macOS).
        // `ps -ww` keeps long Nix/Deno wrapper command lines from being truncated
        // before the actual agent script path appears.
        let comm: String = parts.collect::<Vec<&str>>().join(" ");
        if comm.is_empty() {
            continue;
        }
        map.entry(ppid).or_default().push((pid, comm));
    }
    map
}

fn make_display_name(pane: &PaneInfo, agent_type: AgentType) -> String {
    let cleaned = clean_pane_title(&pane.pane_title, agent_type);
    if cleaned.is_empty() {
        format!("{} (w:{})", agent_type.display_name(), pane.window_index)
    } else {
        cleaned
    }
}

/// Walk the full descendant tree of each pane shell (BFS, bounded depth,
/// cycle-safe) and associate every recognized agent process with the pane it
/// descends from. At most one entry per (pane, agent type).
fn match_agents(
    panes: &[PaneInfo],
    by_ppid: &HashMap<u32, Vec<(u32, String)>>,
) -> Vec<AgentSession> {
    let mut agents = Vec::new();
    for pane in panes {
        let mut visited: HashSet<u32> = HashSet::new();
        let mut queue: VecDeque<(u32, usize)> = VecDeque::from([(pane.pane_pid, 0)]);
        let mut found = None;
        while let Some((pid, depth)) = queue.pop_front() {
            if depth > MAX_AGENT_DEPTH || !visited.insert(pid) {
                continue;
            }
            let Some(kids) = by_ppid.get(&pid) else {
                continue;
            };
            for (child_pid, comm) in kids {
                if let Some(agent_type) = identify_agent(comm) {
                    found = Some(agent_type);
                    break;
                }
                queue.push_back((*child_pid, depth + 1));
            }
            if found.is_some() {
                break;
            }
        }
        if let Some(agent_type) = found {
            let display_name = make_display_name(pane, agent_type);
            agents.push(AgentSession {
                agent_type,
                tmux_session_name: pane.session_name.clone(),
                window_index: pane.window_index,
                pane_id: pane.pane_id.clone(),
                display_name,
                renamed: false,
                pin_slot: None,
            });
        }
    }
    agents
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_panes_basic() {
        let raw = "twsr_dev\t0\t%0\t12345\tsome title\ntwsr_dev\t1\t%1\t12346\t\n";
        let panes = parse_panes(raw);
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0].session_name, "twsr_dev");
        assert_eq!(panes[0].window_index, 0);
        assert_eq!(panes[0].pane_id, "%0");
        assert_eq!(panes[0].pane_pid, 12345);
        assert_eq!(panes[0].pane_title, "some title");
        assert_eq!(panes[1].window_index, 1);
        assert_eq!(panes[1].pane_title, "");
    }

    #[test]
    fn parse_processes_basic() {
        let raw = "  PID  PPID COMM\n  100     1 /bin/zsh\n  200   100 claude\n  300   100 vim\n";
        let map = parse_processes(raw);
        let kids = map.get(&100).unwrap();
        assert_eq!(kids.len(), 2);
        assert!(
            kids.iter()
                .any(|(pid, comm)| *pid == 200 && comm == "claude")
        );
    }

    #[test]
    fn match_agents_finds_claude() {
        let panes = vec![PaneInfo {
            session_name: "twsr_dev".into(),
            window_index: 0,
            pane_id: "%0".into(),
            pane_pid: 100,
            pane_title: "\u{2810} fix-bug".into(),
        }];
        let mut children = HashMap::new();
        children.insert(100, vec![(200, "claude".into())]);

        let agents = match_agents(&panes, &children);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].agent_type, AgentType::ClaudeCode);
        assert_eq!(agents[0].tmux_session_name, "twsr_dev");
        assert_eq!(agents[0].pane_id, "%0");
        assert_eq!(agents[0].display_name, "fix-bug");
        assert!(!agents[0].renamed);
    }

    #[test]
    fn match_agents_skips_non_agents() {
        let panes = vec![PaneInfo {
            session_name: "twsr_dev".into(),
            window_index: 0,
            pane_id: "%0".into(),
            pane_pid: 100,
            pane_title: "".into(),
        }];
        let mut children = HashMap::new();
        children.insert(100, vec![(200, "vim".into()), (201, "node".into())]);

        let agents = match_agents(&panes, &children);
        assert!(agents.is_empty());
    }

    #[test]
    fn match_agents_finds_agent_behind_wrapper() {
        // claude is a *grandchild* of the pane shell (e.g. via a non-exec
        // wrapper script) — must still be detected.
        let panes = vec![PaneInfo {
            session_name: "twsr_dev".into(),
            window_index: 0,
            pane_id: "%0".into(),
            pane_pid: 100,
            pane_title: "".into(),
        }];
        let mut by_ppid = HashMap::new();
        by_ppid.insert(100, vec![(200, "/bin/sh /opt/wrapper.sh".into())]);
        by_ppid.insert(200, vec![(300, "node /opt/wrapper.js".into())]);
        by_ppid.insert(300, vec![(400, "claude".into())]);

        let agents = match_agents(&panes, &by_ppid);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].agent_type, AgentType::ClaudeCode);
        assert_eq!(agents[0].pane_id, "%0");
    }

    #[test]
    fn match_agents_dedupes_same_type_per_pane() {
        let panes = vec![PaneInfo {
            session_name: "twsr_dev".into(),
            window_index: 0,
            pane_id: "%0".into(),
            pane_pid: 100,
            pane_title: "".into(),
        }];
        let mut by_ppid = HashMap::new();
        by_ppid.insert(100, vec![(200, "claude".into()), (201, "claude".into())]);

        let agents = match_agents(&panes, &by_ppid);
        assert_eq!(agents.len(), 1);
    }

    #[test]
    fn match_agents_survives_pid_cycles() {
        // A cyclic PPID map (PID reuse) must terminate, not loop forever.
        let panes = vec![PaneInfo {
            session_name: "twsr_dev".into(),
            window_index: 0,
            pane_id: "%0".into(),
            pane_pid: 100,
            pane_title: "".into(),
        }];
        let mut by_ppid = HashMap::new();
        by_ppid.insert(100, vec![(200, "vim".into())]);
        by_ppid.insert(200, vec![(100, "codex".into())]);

        let agents = match_agents(&panes, &by_ppid);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].agent_type, AgentType::Codex);
    }

    #[test]
    fn match_agents_multiple_agents_one_session() {
        let panes = vec![
            PaneInfo {
                session_name: "tws_work_proj".into(),
                window_index: 0,
                pane_id: "%0".into(),
                pane_pid: 100,
                pane_title: "\u{2810} task-a".into(),
            },
            PaneInfo {
                session_name: "tws_work_proj".into(),
                window_index: 1,
                pane_id: "%1".into(),
                pane_pid: 101,
                pane_title: "".into(),
            },
        ];
        let mut children = HashMap::new();
        children.insert(100, vec![(200, "claude".into())]);
        children.insert(101, vec![(300, "codex".into())]);

        let agents = match_agents(&panes, &children);
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].agent_type, AgentType::ClaudeCode);
        assert_eq!(agents[0].display_name, "task-a");
        assert_eq!(agents[1].agent_type, AgentType::Codex);
        assert_eq!(agents[1].display_name, "Codex (w:1)"); // fallback: empty title
    }
}
