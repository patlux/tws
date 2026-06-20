use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredWorktree {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub prunable: bool,
    pub is_main: bool,
}

impl DiscoveredWorktree {
    pub fn label_source(&self) -> String {
        self.branch
            .as_deref()
            .map(short_branch_name)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                self.path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty())
            })
            .or_else(|| self.head.as_ref().map(|h| short_head(h)))
            .unwrap_or_else(|| "worktree".to_string())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DiscoverOptions {
    pub include_main: bool,
    pub include_detached: bool,
    pub skip_prunable: bool,
}

impl Default for DiscoverOptions {
    fn default() -> Self {
        Self {
            include_main: true,
            include_detached: true,
            skip_prunable: true,
        }
    }
}

pub fn discover(repo: &Path, options: DiscoverOptions) -> std::io::Result<Vec<DiscoveredWorktree>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["worktree", "list", "--porcelain"])
        .output()?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    Ok(parse_porcelain(&String::from_utf8_lossy(&output.stdout), options))
}

pub fn remove(path: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["worktree", "remove"])
        .arg(path)
        .output()
        .map_err(|err| format!("Failed to run git: {}", err))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            Err("Failed to delete worktree".to_string())
        } else {
            Err(stderr)
        }
    }
}

fn parse_porcelain(input: &str, options: DiscoverOptions) -> Vec<DiscoveredWorktree> {
    let mut entries = Vec::new();
    let mut current: Option<DiscoveredWorktree> = None;
    let mut first_path: Option<PathBuf> = None;

    for line in input.lines() {
        if line.trim().is_empty() {
            continue;
        }

        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            let path = PathBuf::from(path);
            let is_main = first_path.is_none();
            if first_path.is_none() {
                first_path = Some(path.clone());
            }
            current = Some(DiscoveredWorktree {
                path,
                head: None,
                branch: None,
                detached: false,
                prunable: false,
                is_main,
            });
        } else if let Some(entry) = current.as_mut() {
            if let Some(head) = line.strip_prefix("HEAD ") {
                entry.head = Some(head.to_string());
            } else if let Some(branch) = line.strip_prefix("branch ") {
                entry.branch = Some(branch.to_string());
            } else if line == "detached" || line.starts_with("detached ") {
                entry.detached = true;
            } else if line == "prunable" || line.starts_with("prunable ") {
                entry.prunable = true;
            }
        }
    }

    if let Some(entry) = current.take() {
        entries.push(entry);
    }

    let mut seen_paths = HashSet::new();
    entries
        .into_iter()
        .filter(|entry| options.include_main || !entry.is_main)
        .filter(|entry| options.include_detached || !entry.detached)
        .filter(|entry| !options.skip_prunable || !entry.prunable)
        .filter(|entry| entry.path.is_dir() || !options.skip_prunable)
        .filter(|entry| seen_paths.insert(entry.path.clone()))
        .collect()
}

fn short_branch_name(branch: &str) -> String {
    branch
        .strip_prefix("refs/heads/")
        .or_else(|| branch.strip_prefix("refs/remotes/"))
        .unwrap_or(branch)
        .to_string()
}

fn short_head(head: &str) -> String {
    head.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("tws_worktree_test_{}_{}", name, uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn parses_and_filters_porcelain() {
        let main = temp_dir("main");
        let feature = temp_dir("feature");
        let stale = std::env::temp_dir().join(format!("tws_worktree_test_stale_{}", uuid::Uuid::new_v4()));
        let input = format!(
            "worktree {}\nHEAD abcdef123456\nbranch refs/heads/main\n\nworktree {}\nHEAD 123456789abc\nbranch refs/heads/feature/foo\n\nworktree {}\nHEAD deadbeef\ndetached\nprunable gitdir file points to non-existent location\n",
            main.display(),
            feature.display(),
            stale.display()
        );

        let entries = parse_porcelain(&input, DiscoverOptions::default());
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].label_source(), "main");
        assert_eq!(entries[1].label_source(), "feature/foo");

        fs::remove_dir_all(main).unwrap();
        fs::remove_dir_all(feature).unwrap();
    }

    #[test]
    fn includes_prunable_when_not_skipping() {
        let main = temp_dir("main");
        let stale = std::env::temp_dir().join(format!("tws_worktree_test_stale_{}", uuid::Uuid::new_v4()));
        let input = format!(
            "worktree {}\nHEAD abc\nbranch refs/heads/main\n\nworktree {}\nHEAD deadbeef\nbranch refs/heads/stale\nprunable gitdir file points to non-existent location\n",
            main.display(),
            stale.display()
        );

        let entries = parse_porcelain(
            &input,
            DiscoverOptions {
                skip_prunable: false,
                ..DiscoverOptions::default()
            },
        );
        assert_eq!(entries.len(), 2);
        assert!(entries[1].prunable);
        assert!(!entries[1].path.is_dir());

        fs::remove_dir_all(main).unwrap();
    }

    #[test]
    fn can_exclude_main() {
        let main = temp_dir("main");
        let wt = temp_dir("wt");
        let input = format!(
            "worktree {}\nHEAD abc\nbranch refs/heads/main\n\nworktree {}\nHEAD def\nbranch refs/heads/topic\n",
            main.display(),
            wt.display()
        );

        let entries = parse_porcelain(
            &input,
            DiscoverOptions {
                include_main: false,
                ..DiscoverOptions::default()
            },
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].label_source(), "topic");

        fs::remove_dir_all(main).unwrap();
        fs::remove_dir_all(wt).unwrap();
    }
}
