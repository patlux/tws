use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use tui_tree_widget::{TreeItem, TreeState};

use crate::core::model::Thread;
use crate::core::pi_status::PiIndicator;
use crate::core::state::{AppState, SelectedItem};
use crate::theme::Theme;

/// Converts the app state into TreeItems for rendering.
/// Collections -> Threads -> Sessions (3-level hierarchy).
/// Root threads (from the root collection) render at root level, not nested under a collection node.
pub fn build_tree_items<'a>(
    state: &'a AppState,
    theme: &Theme,
    deleting_label: &dyn Fn(&str) -> Option<String>,
    pi_spinner: &str,
) -> Vec<TreeItem<'a, String>> {
    let mut items: Vec<TreeItem<'a, String>> = Vec::new();

    // Regular collections first
    for (col_idx, col) in state.collections.iter().enumerate() {
        if col.is_root || state.collection_is_hidden(col_idx) {
            continue;
        }
        let children: Vec<TreeItem<'a, String>> = col
            .threads
            .iter()
            .enumerate()
            .filter(|(thread_idx, _)| !state.thread_is_hidden(col_idx, *thread_idx))
            .map(|(_, thread)| build_thread_item(state, thread, theme, deleting_label, pi_spinner))
            .collect();

        items.push(
            TreeItem::new(
                col.id.to_string(),
                collection_text(state, col_idx, col.name.as_str(), theme, pi_spinner),
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
                items.push(build_thread_item(state, thread, theme, deleting_label, pi_spinner));
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
                let highlight_width = if tree_state.selected().is_empty() { 0 } else { 2 };
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
/// Spans appended to a collection/thread/session row for Pi activity: `⠋ pi`
/// while working, `✓ pi` once finished. Empty when there is nothing to report.
fn pi_indicator_suffix(
    indicator: Option<PiIndicator>,
    theme: &Theme,
    pi_spinner: &str,
) -> Vec<Span<'static>> {
    match indicator {
        Some(PiIndicator::Working) => vec![
            Span::raw("  "),
            Span::styled(format!("{} pi", pi_spinner), theme.pi_working),
        ],
        Some(PiIndicator::Done) => vec![
            Span::raw("  "),
            Span::styled("✓ pi".to_string(), theme.pi_done),
        ],
        None => Vec::new(),
    }
}

fn collection_text(
    state: &AppState,
    col_idx: usize,
    name: &str,
    theme: &Theme,
    pi_spinner: &str,
) -> Text<'static> {
    let mut spans = vec![Span::styled(name.to_string(), theme.collection)];
    spans.extend(pi_indicator_suffix(
        state.pi_indicator_for_collection(col_idx),
        theme,
        pi_spinner,
    ));
    Text::from(Line::from(spans))
}

fn build_thread_item<'a>(
    state: &'a AppState,
    thread: &'a Thread,
    theme: &Theme,
    deleting_label: &dyn Fn(&str) -> Option<String>,
    pi_spinner: &str,
) -> TreeItem<'a, String> {
    let mut session_children: Vec<TreeItem<'a, String>> = state
        .active_sessions
        .iter()
        .filter(|s| s.thread_id == thread.id)
        .map(|s| {
            let agents = state.agents_for_session(&s.tmux_session_name);
            let deleting = deleting_label(&s.tmux_session_name);
            let is_deleting = deleting.is_some();
            let display_name = deleting.unwrap_or_else(|| s.display_name.clone());
            let style = if is_deleting { theme.worktree_meta } else { theme.session };
            let mut spans = vec![Span::styled(display_name, style)];
            if !is_deleting {
                spans.extend(pi_indicator_suffix(
                    state.pi_indicator_for_session(&s.tmux_session_name),
                    theme,
                    pi_spinner,
                ));
            }
            let session_label = Text::from(Line::from(spans));
            if agents.is_empty() {
                TreeItem::new_leaf(s.tmux_session_name.clone(), session_label)
            } else {
                let agent_children: Vec<TreeItem<'a, String>> = agents
                    .iter()
                    .map(|a| {
                        let mut spans = vec![Span::styled("╰─ ", theme.agent_connector)];
                        match state.pi_indicator_for_agent(a) {
                            Some(PiIndicator::Working) => {
                                spans.push(Span::styled(format!("{} ", pi_spinner), theme.pi_working));
                            }
                            Some(PiIndicator::Done) => {
                                spans.push(Span::styled("✓ ".to_string(), theme.pi_done));
                            }
                            None => {}
                        }
                        spans.push(Span::styled(a.agent_type.icon(), theme.agent.add_modifier(Modifier::BOLD)));
                        spans.push(Span::styled(format!(" {}", a.display_name), theme.agent));
                        TreeItem::new_leaf(a.pane_id.clone(), Line::from(spans))
                    })
                    .collect();
                TreeItem::new(
                    s.tmux_session_name.clone(),
                    session_label,
                    agent_children,
                )
                .expect("pane IDs are unique within a session")
            }
        })
        .collect();

    for worktree in state.worktrees_for_thread(thread.id) {
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
            spans.push(Span::styled(branch_display_name(branch), theme.worktree_meta));
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
            Span::styled(thread.name.as_str(), theme.thread),
            Span::styled(" \u{25CF} ", theme.badge_dot),
            Span::styled(session_count.to_string(), theme.badge_count),
        ];
        spans.extend(pi_indicator_suffix(
            state.pi_indicator_for_thread(thread.id),
            theme,
            pi_spinner,
        ));
        Text::from(Line::from(spans))
    } else {
        // No sessions means no Pi status to report on this thread row.
        Text::styled(thread.name.as_str(), theme.thread_dim)
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
