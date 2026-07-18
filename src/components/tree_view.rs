use std::collections::{HashMap, HashSet};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use tui_tree_widget::{TreeItem, TreeState};

use crate::core::model::{AgentSession, Session, Thread, WorktreeSession};
use crate::core::pi_status::{PiIndicator, WORKING_INDICATOR};
use crate::core::state::{AppState, SelectedItem};
use crate::theme::Theme;

/// Per-build indexes turn per-thread/per-session global scans into keyed
/// lookups. The resulting tree is fully owned (`'static`) so App can cache it
/// across redraws caused only by overlays/animation.
struct RenderIndex<'a> {
    sessions_by_thread: HashMap<uuid::Uuid, Vec<&'a Session>>,
    agents_by_session: HashMap<&'a str, Vec<&'a AgentSession>>,
    worktrees_by_thread: HashMap<uuid::Uuid, Vec<&'a WorktreeSession>>,
    session_indicators: HashMap<String, PiIndicator>,
    pane_indicators: HashMap<String, PiIndicator>,
    thread_indicators: HashMap<uuid::Uuid, PiIndicator>,
}

fn indicator_priority(indicator: PiIndicator) -> u8 {
    match indicator {
        PiIndicator::Working => 5,
        PiIndicator::Retrying => 4,
        PiIndicator::Failed => 3,
        PiIndicator::Incomplete => 2,
        PiIndicator::Cancelled => 1,
        PiIndicator::Done => 0,
    }
}

fn merge_indicator(current: Option<PiIndicator>, next: Option<PiIndicator>) -> Option<PiIndicator> {
    match (current, next) {
        (None, next) => next,
        (current, None) => current,
        (Some(current), Some(next)) => {
            Some(if indicator_priority(next) > indicator_priority(current) {
                next
            } else {
                current
            })
        }
    }
}

impl<'a> RenderIndex<'a> {
    fn new(state: &'a AppState) -> Self {
        let mut sessions_by_thread: HashMap<uuid::Uuid, Vec<&Session>> = HashMap::new();
        let active_names: HashSet<&str> = state
            .active_sessions
            .iter()
            .map(|session| session.tmux_session_name.as_str())
            .collect();
        for session in &state.active_sessions {
            sessions_by_thread
                .entry(session.thread_id)
                .or_default()
                .push(session);
        }

        let mut agents_by_session: HashMap<&str, Vec<&AgentSession>> = HashMap::new();
        for agent in &state.agent_sessions {
            let session_agents = agents_by_session
                .entry(agent.tmux_session_name.as_str())
                .or_default();
            if !session_agents
                .iter()
                .any(|existing| existing.pane_id == agent.pane_id)
            {
                session_agents.push(agent);
            }
        }

        let mut worktrees_by_thread: HashMap<uuid::Uuid, Vec<&WorktreeSession>> = HashMap::new();
        if !state.active_filter {
            for worktree in &state.worktree_sessions {
                if !active_names.contains(worktree.tmux_session_name.as_str()) {
                    worktrees_by_thread
                        .entry(worktree.thread_id)
                        .or_default()
                        .push(worktree);
                }
            }
        }

        let session_indicators = state.pi_indicators_by_session();
        let pane_indicators = state.pi_indicators_by_pane();
        let mut thread_indicators = HashMap::new();
        for session in &state.active_sessions {
            let next = session_indicators.get(&session.tmux_session_name).copied();
            let current = thread_indicators.get(&session.thread_id).copied();
            if let Some(indicator) = merge_indicator(current, next) {
                thread_indicators.insert(session.thread_id, indicator);
            }
        }

        Self {
            sessions_by_thread,
            agents_by_session,
            worktrees_by_thread,
            session_indicators,
            pane_indicators,
            thread_indicators,
        }
    }

    fn collection_indicator(&self, state: &AppState, col_idx: usize) -> Option<PiIndicator> {
        let collection = state.collections.get(col_idx)?;
        collection
            .threads
            .iter()
            .enumerate()
            .filter(|(thread_idx, _)| !state.thread_is_hidden(col_idx, *thread_idx))
            .fold(None, |current, (_, thread)| {
                merge_indicator(current, self.thread_indicators.get(&thread.id).copied())
            })
    }
}

/// Converts the app state into TreeItems for rendering.
/// Collections -> Threads -> Sessions (3-level hierarchy).
/// Root threads (from the root collection) render at root level, not nested under a collection node.
pub fn build_tree_items(
    state: &AppState,
    tree_state: &TreeState<String>,
    theme: &Theme,
    deleting_label: &dyn Fn(&str) -> Option<String>,
) -> Vec<TreeItem<'static, String>> {
    let index = RenderIndex::new(state);
    let mut items: Vec<TreeItem<'static, String>> = Vec::new();

    // Regular collections first
    for (col_idx, col) in state.collections.iter().enumerate() {
        if col.is_root || state.collection_is_hidden(col_idx) {
            continue;
        }
        let collection_path = vec![col.id.to_string()];
        let children: Vec<TreeItem<'static, String>> = col
            .threads
            .iter()
            .enumerate()
            .filter(|(thread_idx, _)| !state.thread_is_hidden(col_idx, *thread_idx))
            .map(|(_, thread)| {
                let mut thread_path = collection_path.clone();
                thread_path.push(thread.id.to_string());
                build_thread_item(
                    &index,
                    tree_state,
                    thread,
                    &thread_path,
                    theme,
                    deleting_label,
                )
            })
            .collect();

        items.push(
            TreeItem::new(
                col.id.to_string(),
                collection_text(
                    &index,
                    state,
                    col_idx,
                    col.name.as_str(),
                    !should_show_parent_indicator(tree_state, &collection_path),
                    theme,
                ),
                children,
            )
            .expect("thread IDs are unique within a collection"),
        );
    }

    // Root threads at the bottom, rendered as root-level items
    for (col_idx, col) in state.collections.iter().enumerate() {
        if !col.is_root {
            continue;
        }
        for (thread_idx, thread) in col.threads.iter().enumerate() {
            if !state.thread_is_hidden(col_idx, thread_idx) {
                let thread_path = vec![thread.id.to_string()];
                items.push(build_thread_item(
                    &index,
                    tree_state,
                    thread,
                    &thread_path,
                    theme,
                    deleting_label,
                ));
            }
        }
    }

    items
}

/// Render worktree status icons in the tree's symbol column.
///
/// `tui-tree-widget` only has one global no-children symbol, so worktree icons are
/// overlaid after the tree is rendered. This keeps worktree labels aligned with
/// regular session labels while drawing `◌`/`✕` where `›`/`⌄` would appear.
#[allow(clippy::too_many_arguments)]
pub fn render_worktree_icons(
    frame: &mut Frame<'_>,
    app_state: &AppState,
    tree_state: &TreeState<String>,
    items: &[TreeItem<'_, String>],
    area: Rect,
    theme: &Theme,
    highlight_style: Style,
    deleting_label: &dyn Fn(&str) -> Option<String>,
    deleting_icon: &str,
    is_marked: &dyn Fn(&str) -> bool,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let visible = tree_state.flatten(items);
    let mut y_offset = 0u16;

    for flattened in visible.iter().skip(tree_state.get_offset()) {
        let height = flattened.item.height() as u16;
        if y_offset.saturating_add(height) > area.height {
            break;
        }

        if let Some(item_name) = flattened.identifier.last() {
            let deleting = deleting_label(item_name).is_some();
            let worktree = app_state.find_worktree_by_tmux_name(item_name);
            let is_worktree_row = matches!(
                app_state.resolve_selection(&flattened.identifier),
                SelectedItem::Worktree(..)
            );

            let icon = if deleting {
                Some((deleting_icon, theme.worktree_meta))
            } else if is_marked(item_name) {
                Some(("✓ ", theme.flash))
            } else if is_worktree_row {
                worktree.map(|w| {
                    if w.launchable {
                        ("◌ ", theme.worktree_meta)
                    } else {
                        ("✕ ", theme.worktree_prunable)
                    }
                })
            } else {
                None
            };

            if let Some((icon, unselected_style)) = icon {
                let style = if tree_state.selected() == flattened.identifier.as_slice() {
                    highlight_style
                } else {
                    unselected_style
                };
                let highlight_width = if tree_state.selected().is_empty() {
                    0
                } else {
                    2
                };
                let x_offset = highlight_width + (flattened.depth() as u16 * 2);
                if x_offset < area.width {
                    let icon_area = Rect {
                        x: area.x + x_offset,
                        y: area.y + y_offset,
                        width: area.width.saturating_sub(x_offset).min(2),
                        height: 1,
                    };
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(icon, style))),
                        icon_area,
                    );
                }
            }
        }

        y_offset = y_offset.saturating_add(height);
        if y_offset >= area.height {
            break;
        }
    }
}

/// Build a TreeItem for a single thread (shared between regular and root threads).
/// Spans appended to a collection/thread/session row for Pi activity.
fn pi_indicator_suffix(indicator: Option<PiIndicator>, theme: &Theme) -> Vec<Span<'static>> {
    match indicator {
        Some(PiIndicator::Working) => vec![
            Span::raw("  "),
            Span::styled(format!("{} pi", WORKING_INDICATOR), theme.pi_working),
        ],
        Some(PiIndicator::Retrying) => vec![
            Span::raw("  "),
            Span::styled("↻ pi".to_string(), theme.pi_warning),
        ],
        Some(PiIndicator::Done) => vec![
            Span::raw("  "),
            Span::styled("✓ pi".to_string(), theme.pi_done),
        ],
        Some(PiIndicator::Cancelled) => vec![
            Span::raw("  "),
            Span::styled("■ pi".to_string(), theme.pi_warning),
        ],
        Some(PiIndicator::Incomplete) => vec![
            Span::raw("  "),
            Span::styled("… pi".to_string(), theme.pi_warning),
        ],
        Some(PiIndicator::Failed) => vec![
            Span::raw("  "),
            Span::styled("! pi".to_string(), theme.pi_failed),
        ],
        None => Vec::new(),
    }
}

fn should_show_parent_indicator(tree_state: &TreeState<String>, path: &[String]) -> bool {
    !tree_state.opened().contains(path)
}

fn collection_text(
    index: &RenderIndex<'_>,
    state: &AppState,
    col_idx: usize,
    name: &str,
    is_expanded: bool,
    theme: &Theme,
) -> Text<'static> {
    let mut spans = vec![Span::styled(name.to_string(), theme.collection)];
    if !is_expanded {
        spans.extend(pi_indicator_suffix(
            index.collection_indicator(state, col_idx),
            theme,
        ));
    }
    Text::from(Line::from(spans))
}

fn build_thread_item(
    index: &RenderIndex<'_>,
    tree_state: &TreeState<String>,
    thread: &Thread,
    thread_path: &[String],
    theme: &Theme,
    deleting_label: &dyn Fn(&str) -> Option<String>,
) -> TreeItem<'static, String> {
    let mut session_children: Vec<TreeItem<'static, String>> = index
        .sessions_by_thread
        .get(&thread.id)
        .into_iter()
        .flatten()
        .map(|s| {
            let agents = index
                .agents_by_session
                .get(s.tmux_session_name.as_str())
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let deleting = deleting_label(&s.tmux_session_name);
            let is_deleting = deleting.is_some();
            let display_name = deleting.unwrap_or_else(|| s.display_name.clone());
            let style = if is_deleting {
                theme.worktree_meta
            } else {
                theme.session
            };
            let mut spans = vec![Span::styled(display_name, style)];
            let mut session_path = thread_path.to_vec();
            session_path.push(s.tmux_session_name.clone());
            if !is_deleting && should_show_parent_indicator(tree_state, &session_path) {
                spans.extend(pi_indicator_suffix(
                    index.session_indicators.get(&s.tmux_session_name).copied(),
                    theme,
                ));
            }
            let session_label = Text::from(Line::from(spans));
            if agents.is_empty() {
                TreeItem::new_leaf(s.tmux_session_name.clone(), session_label)
            } else {
                let agent_children: Vec<TreeItem<'static, String>> = agents
                    .iter()
                    .map(|a| {
                        let mut spans = vec![Span::styled("╰─ ", theme.agent_connector)];
                        let indicator = (a.agent_type == crate::core::model::AgentType::Pi)
                            .then(|| index.pane_indicators.get(&a.pane_id).copied())
                            .flatten();
                        match indicator {
                            Some(PiIndicator::Working) => {
                                spans.push(Span::styled(
                                    format!("{} ", WORKING_INDICATOR),
                                    theme.pi_working,
                                ));
                            }
                            Some(PiIndicator::Retrying) => {
                                spans.push(Span::styled("↻ ".to_string(), theme.pi_warning));
                            }
                            Some(PiIndicator::Done) => {
                                spans.push(Span::styled("✓ ".to_string(), theme.pi_done));
                            }
                            Some(PiIndicator::Cancelled) => {
                                spans.push(Span::styled("■ ".to_string(), theme.pi_warning));
                            }
                            Some(PiIndicator::Incomplete) => {
                                spans.push(Span::styled("… ".to_string(), theme.pi_warning));
                            }
                            Some(PiIndicator::Failed) => {
                                spans.push(Span::styled("! ".to_string(), theme.pi_failed));
                            }
                            None => {}
                        }
                        spans.push(Span::styled(
                            a.agent_type.icon(),
                            theme.agent.add_modifier(Modifier::BOLD),
                        ));
                        spans.push(Span::styled(format!(" {}", a.display_name), theme.agent));
                        TreeItem::new_leaf(a.pane_id.clone(), Line::from(spans))
                    })
                    .collect();
                TreeItem::new(s.tmux_session_name.clone(), session_label, agent_children)
                    .expect("pane IDs are unique within a session")
            }
        })
        .collect();

    for worktree in index
        .worktrees_by_thread
        .get(&thread.id)
        .into_iter()
        .flatten()
    {
        let deleting = deleting_label(&worktree.tmux_session_name);
        let name_style = if deleting.is_some() {
            theme.worktree_meta
        } else if worktree.launchable {
            theme.worktree
        } else {
            theme.worktree_prunable
        };
        let display_name = deleting.as_deref().unwrap_or(&worktree.display_name);
        let mut spans = vec![Span::styled(display_name.to_string(), name_style)];
        if let Some(branch) = &worktree.branch {
            spans.push(Span::styled("  ", theme.worktree_meta));
            spans.push(Span::styled(
                branch_display_name(branch),
                theme.worktree_meta,
            ));
        } else if let Some(head) = &worktree.head {
            spans.push(Span::styled("  ", theme.worktree_meta));
            spans.push(Span::styled(short_head(head), theme.worktree_meta));
        }
        if worktree.prunable {
            spans.push(Span::styled("  prunable", theme.worktree_prunable));
        } else if !worktree.path_exists {
            spans.push(Span::styled("  missing", theme.worktree_prunable));
        }
        session_children.push(TreeItem::new_leaf(
            worktree.tmux_session_name.clone(),
            Line::from(spans),
        ));
    }

    let session_count = session_children.len();

    let thread_text = if session_count > 0 {
        let mut spans = vec![
            Span::styled(thread.name.clone(), theme.thread),
            Span::styled(" \u{25CF} ", theme.badge_dot),
            Span::styled(session_count.to_string(), theme.badge_count),
        ];
        if should_show_parent_indicator(tree_state, thread_path) {
            spans.extend(pi_indicator_suffix(
                index.thread_indicators.get(&thread.id).copied(),
                theme,
            ));
        }
        Text::from(Line::from(spans))
    } else {
        // No sessions means no Pi status to report on this thread row.
        Text::styled(thread.name.clone(), theme.thread_dim)
    };

    if session_children.is_empty() {
        TreeItem::new_leaf(thread.id.to_string(), thread_text)
    } else {
        TreeItem::new(thread.id.to_string(), thread_text, session_children)
            .expect("session names are unique within a thread")
    }
}

fn branch_display_name(branch: &str) -> String {
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
    use super::should_show_parent_indicator;
    use tui_tree_widget::TreeState;

    #[test]
    fn pi_indicator_stays_on_collapsed_parent() {
        let tree_state = TreeState::default();
        let path = vec!["collection".to_string()];

        assert!(should_show_parent_indicator(&tree_state, &path));
    }

    #[test]
    fn pi_indicator_moves_below_expanded_parent() {
        let mut tree_state = TreeState::default();
        let path = vec!["collection".to_string(), "thread".to_string()];
        tree_state.open(path.clone());

        assert!(!should_show_parent_indicator(&tree_state, &path));
    }
}
