use tws_core::model::AgentType;

/// Identify a supported coding agent from a full process command line.
pub(crate) fn identify_agent(command: &str) -> Option<AgentType> {
    let mut tokens = command.split_whitespace();
    let executable = tokens.next()?;
    let basename = executable.rsplit('/').next().unwrap_or(executable);

    match basename {
        "claude" => Some(AgentType::ClaudeCode),
        "codex" => Some(AgentType::Codex),
        "pi" | "pi-coding-agent" => Some(AgentType::Pi),
        "node" | "deno" => identify_agent_script(tokens),
        _ => None,
    }
}

fn identify_agent_script<'a>(tokens: impl Iterator<Item = &'a str>) -> Option<AgentType> {
    for token in tokens {
        if !token.contains('/') {
            continue;
        }
        let components: Vec<&str> = token.split('/').collect();
        let is_package_component = |index: usize| {
            components.get(..index).is_some_and(|prefix| {
                prefix
                    .iter()
                    .rev()
                    .take(2)
                    .any(|&component| component == "node_modules")
            }) || components
                .get(index.wrapping_sub(1))
                .is_some_and(|&parent| parent == "bin" || parent.contains("-pi-coding-agent-"))
        };

        for (index, &component) in components.iter().enumerate() {
            let matched = match component {
                "codex" => Some(AgentType::Codex),
                "claude" | "claude-code" => Some(AgentType::ClaudeCode),
                "pi" | "pi-coding-agent" => Some(AgentType::Pi),
                _ if component.contains("-pi-coding-agent-") => Some(AgentType::Pi),
                _ => None,
            };
            if let Some(agent) = matched
                && (is_package_component(index) || component.contains("-pi-coding-agent-"))
            {
                return Some(agent);
            }
        }
    }
    None
}

pub(crate) fn clean_pane_title(title: &str, agent_type: AgentType) -> String {
    let title = title.trim();
    if agent_type != AgentType::ClaudeCode {
        return title.to_string();
    }
    let title = title.trim_start_matches(|character: char| {
        character.is_whitespace() || ('\u{2800}'..='\u{28ff}').contains(&character)
    });
    title
        .strip_prefix('\u{2733}')
        .unwrap_or(title)
        .trim_start()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_direct_agents() {
        assert_eq!(identify_agent("claude"), Some(AgentType::ClaudeCode));
        assert_eq!(
            identify_agent("/usr/local/bin/claude"),
            Some(AgentType::ClaudeCode)
        );
        assert_eq!(identify_agent("codex"), Some(AgentType::Codex));
        assert_eq!(
            identify_agent("/opt/homebrew/bin/codex"),
            Some(AgentType::Codex)
        );
        assert_eq!(identify_agent("pi"), Some(AgentType::Pi));
        assert_eq!(identify_agent("pi-coding-agent"), Some(AgentType::Pi));
        assert_eq!(identify_agent("nvim"), None);
    }

    #[test]
    fn identifies_packaged_agents_without_false_positives() {
        assert_eq!(
            identify_agent("node /opt/homebrew/lib/node_modules/@openai/codex/dist/cli.js"),
            Some(AgentType::Codex)
        );
        assert_eq!(
            identify_agent("node /opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js"),
            Some(AgentType::ClaudeCode)
        );
        assert_eq!(
            identify_agent(
                "deno run --allow-all /nix/store/hash-pi-coding-agent-0.78.0/lib/node_modules/@earendil-works/pi-coding-agent/dist/cli.js"
            ),
            Some(AgentType::Pi)
        );
        assert_eq!(
            identify_agent("node /path/to/codex-tutorial/index.js"),
            None
        );
        assert_eq!(identify_agent("node /projects/claude/index.js"), None);
        assert_eq!(identify_agent("node"), None);
    }

    #[test]
    fn cleans_agent_titles() {
        assert_eq!(
            clean_pane_title("\u{2810} fix-bug", AgentType::ClaudeCode),
            "fix-bug"
        );
        assert_eq!(
            clean_pane_title("\u{2733} task", AgentType::ClaudeCode),
            "task"
        );
        assert_eq!(
            clean_pane_title("codex-task", AgentType::Codex),
            "codex-task"
        );
        assert_eq!(clean_pane_title("pi-task", AgentType::Pi), "pi-task");
    }
}
