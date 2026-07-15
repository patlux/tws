use super::*;

impl App {
    pub(super) fn draw(&mut self, terminal: &mut Tui) -> std::io::Result<()> {
        // Compute flash outside the closure — we need &mut self to expire it,
        // but the closure also borrows self mutably via render_stateful_widget.
        let flash_msg: Option<String> = match &self.flash {
            Some((msg, t)) if t.elapsed() < Duration::from_secs(2) => Some(msg.clone()),
            Some(_) => {
                self.flash = None;
                None
            }
            None => None,
        };
        let notification_msg: Option<(String, bool)> = match &self.notification {
            Some(notification) if notification.created_at.elapsed() < NOTIFICATION_DURATION => {
                Some((notification.message.clone(), notification.is_error))
            }
            Some(_) => {
                self.notification = None;
                None
            }
            None => None,
        };

        // Pre-compute recent sessions data outside the closure for readability.
        // (Only flash_msg *must* be outside — it mutates self.flash on expiry.)
        // Only show the bar in Normal mode when there are recent sessions.
        let is_normal = matches!(self.mode, Mode::Normal);
        let recent_data: Vec<(String, String)> =
            if is_normal && matches!(self.view_mode, ViewMode::Tree) {
            self.state
                .recent_sessions(5)
                .iter()
                .filter_map(|s| {
                    let path = self.state.session_display_path(s)?;
                    Some((s.tmux_session_name.clone(), path))
                })
                .collect()
        } else {
            Vec::new()
        };
        let recent_count = recent_data.len() as u16;
        let show_recent = !recent_data.is_empty();
        let hidden_count = self.state.hidden_count();
        let empty_hint = if self.state.active_filter {
            format!(
                "Active filter on — nothing is active. Press {} to show all.",
                self.keymap
                    .key_hint(KeyMode::Normal, Action::ToggleActiveFilter),
            )
        } else if hidden_count > 0 {
            format!(
                "{} hidden. Press {} to show all hidden collections and threads.",
                hidden_count,
                self.keymap.key_hint(KeyMode::Normal, Action::ShowHidden),
            )
        } else {
            "Press Enter for a quick session, or a to add a thread.".to_string()
        };

        // Pre-compute flat agents list for agents view mode.
        let flat_agents: Vec<FlatAgent> =
            if matches!(self.view_mode, ViewMode::Agents | ViewMode::AgentGrid) {
            self.state.all_agents_flat()
        } else {
            Vec::new()
        };

        // Pre-compute agent preview data.
        let selected_item = match self.view_mode {
            ViewMode::Tree => self.state.resolve_selection(self.tree_state.selected()),
            ViewMode::Agents | ViewMode::AgentGrid => flat_agents
                .get(self.agent_list_cursor)
                .map(|a| SelectedItem::Agent(a.col_idx, a.thread_idx, a.sess_idx, a.agent_idx))
                .unwrap_or(SelectedItem::None),
        };
        let show_preview =
            self.preview_content.is_some() && matches!(selected_item, SelectedItem::Agent(..));
        let sidebar_title = match &selected_item {
            SelectedItem::Agent(col_idx, thread_idx, sess_idx, agent_idx) => self
                .state
                .resolve_agent(*col_idx, *thread_idx, *sess_idx, *agent_idx)
                    .map(|a| format!("{} {}", a.agent_type.icon(), a.display_name))
                .unwrap_or_default(),
            _ => String::new(),
        };
        let show_sidebar =
            show_preview && is_normal && !matches!(self.view_mode, ViewMode::AgentGrid);

        terminal.draw(|frame| {
            let area = frame.area();

            // Paint the theme background before any widgets
            frame.render_widget(Block::default().style(self.theme.background), area);

            // Build layout: tree, [separator, recent bar], separator, status bar
            let constraints = if show_recent {
                vec![
                    Constraint::Min(0),
                    Constraint::Length(1),
                    Constraint::Length(recent_count),
                    Constraint::Length(1),
                    Constraint::Length(1),
                ]
            } else {
                vec![
                    Constraint::Min(0),
                    Constraint::Length(1),
                    Constraint::Length(1),
                ]
            };
            let chunks = Layout::vertical(constraints).split(area);

            // Index variables for separator and status bar positions
            let (recent_sep_idx, recent_idx, sep_idx, status_idx) = if show_recent {
                (Some(1), Some(2), 3, 4)
            } else {
                (None, None, 1, 2)
            };

            // Split content area horizontally if sidebar should show
            let content_area = chunks[0];
            let (tree_area, sidebar_area) = if show_sidebar {
                let horiz =
                    Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                .split(content_area);
                (horiz[0], Some(horiz[1]))
            } else {
                (content_area, None)
            };

            // Tree area or agents flat-list view
            if matches!(self.view_mode, ViewMode::AgentGrid) {
                agent_grid::render(
                    frame,
                    &flat_agents,
                    &self.grid_captures,
                    self.agent_list_cursor,
                    tree_area,
                    &self.theme,
                );
            } else if matches!(self.view_mode, ViewMode::Agents) {
                agents_view::render(
                    frame,
                    &flat_agents,
                    self.agent_list_cursor,
                    tree_area,
                    &self.theme,
                );
            } else {
                let block = Block::default();
                let deleting_labels = self.worktree_delete_progress_labels();
                let deleting_label =
                    |session_name: &str| deleting_labels.get(session_name).cloned();
                let deleting_icon = self.worktree_spinner_frame();
                let marked_sessions = &self.marked_sessions;
                let is_marked = |session_name: &str| marked_sessions.contains(session_name);
                let items = tree_view::build_tree_items(
                    &self.state,
                    &self.tree_state,
                    &self.theme,
                    &deleting_label,
                );
                if items.is_empty() {
                    let available_height = tree_area.height.saturating_sub(2);
                    let content_height = 4u16;
                    let top_padding = (available_height.saturating_sub(content_height)) / 2;

                    let mut lines: Vec<Line> = vec![Line::from(""); top_padding as usize];
                    lines.push(Line::from(""));
                    lines.push(Line::from(vec![
                        Span::raw("Welcome to "),
                        Span::styled("tws", self.theme.empty_title),
                    ]));
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        empty_hint.as_str(),
                        self.theme.empty_hint,
                    )));

                    let paragraph = Paragraph::new(lines)
                        .block(block)
                        .alignment(Alignment::Center);
                    frame.render_widget(paragraph, tree_area);
                } else {
                    let tree_highlight = self.theme.highlight;

                    let Ok(tree) = Tree::new(&items) else {
                        // Duplicate identifiers (should never happen) — skip the
                        // tree this frame instead of panicking mid-render.
                        return;
                    };
                    let tree = tree
                        .block(block)
                        .highlight_style(tree_highlight)
                        .highlight_symbol("  ")
                        .node_closed_symbol("\u{203A} ")
                        .node_open_symbol("\u{2304} ")
                        .node_no_children_symbol("  ");

                    frame.render_stateful_widget(tree, tree_area, &mut self.tree_state);
                    tree_view::render_worktree_icons(
                        frame,
                        &self.state,
                        &self.tree_state,
                        &items,
                        tree_area,
                        &self.theme,
                        tree_highlight,
                        &deleting_label,
                        deleting_icon,
                        &is_marked,
                    );
                }
            }

            // Agent preview sidebar
            if let Some(sb_area) = sidebar_area
                && show_preview
            {
                let title = format!("Preview: {}", sidebar_title);
                // Pin to bottom: scroll so the last screenful is visible.
                // Inner height = area minus 2 for top/bottom border.
                let visible = sb_area.height.saturating_sub(2) as usize;
                let scroll = self
                    .preview_content
                    .as_ref()
                    .map_or(0, |t| t.lines.len().saturating_sub(visible));
                agent_preview::render(
                    frame,
                    &agent_preview::PreviewState {
                        content: self.preview_content.as_ref(),
                        scroll_offset: scroll,
                        title: &title,
                    },
                    sb_area,
                    &self.theme,
                );
            }

            // Separator between tree and recent bar
            if let Some(idx) = recent_sep_idx {
                let sep = "\u{2500}".repeat(chunks[idx].width as usize);
                frame.render_widget(
                    Paragraph::new(Line::styled(sep, self.theme.separator)),
                    chunks[idx],
                );
            }

            // Recent sessions bar (only in Normal mode with active sessions)
            if let Some(idx) = recent_idx {
                recent_bar::render(frame, &recent_data, chunks[idx], &self.theme);
            }

            // Separator line
            let separator = "\u{2500}".repeat(chunks[sep_idx].width as usize);
            frame.render_widget(
                Paragraph::new(Line::styled(separator, self.theme.separator)),
                chunks[sep_idx],
            );

            // Status bar
            let active_count = self.state.active_sessions.len();
            let status_ctx = self.status_context(&selected_item);
            status_bar::render(
                frame,
                status_ctx,
                chunks[status_idx],
                status_bar::StatusState {
                    active_session_count: active_count,
                    filter_active: self.state.active_filter,
                    flash: flash_msg.as_deref(),
                },
                &self.theme,
                &self.keymap,
            );

            // Draw modal overlay if active (over full area so it centers properly)
            match &self.mode {
                Mode::Normal => {}
                Mode::Help { scroll } => {
                    help_modal::render(frame, area, &self.theme, &self.keymap, *scroll);
                }
                Mode::Input { purpose, buffer } => {
                    let title = match purpose {
                        InputPurpose::AddCollection => "New Collection",
                        InputPurpose::AddThread { .. } => "New Thread",
                        InputPurpose::RenameCollection { .. } => "Rename Collection",
                        InputPurpose::RenameThread { .. } => "Rename Thread",
                        InputPurpose::NewSession { .. } => "Session Name",
                        InputPurpose::NewWorktree { .. } => "New Worktree (branch)",
                        InputPurpose::RenameSession { .. } => "Rename Session",
                        InputPurpose::RenameAgent { .. } => "Rename Agent",
                    };
                    input_modal::render(
                        frame,
                        title,
                        &buffer.content,
                        buffer.cursor,
                        area,
                        &self.theme,
                    );
                }
                Mode::Confirm { purpose } => {
                    let message = match purpose {
                        ConfirmPurpose::DeleteCollection { idx, name } => {
                            let sessions: usize = self
                                .state
                                .collections
                                .get(*idx)
                                .map(|col| {
                                    col.threads
                                        .iter()
                                    .map(|t| self.state.sessions_for_thread(t.id).len())
                                        .sum()
                                })
                                .unwrap_or(0);
                            if sessions > 0 {
                                format!(
                                    "Delete collection \"{}\" and kill {} session(s)?",
                                    name, sessions
                                )
                            } else {
                                format!("Delete collection \"{}\"?", name)
                            }
                        }
                        ConfirmPurpose::DeleteThread {
                            col_idx,
                            thread_idx,
                            name,
                        } => {
                            let sessions = self
                                .state
                                .collections
                                .get(*col_idx)
                                .and_then(|col| col.threads.get(*thread_idx))
                                .map(|t| self.state.sessions_for_thread(t.id).len())
                                .unwrap_or(0);
                            if sessions > 0 {
                                format!(
                                    "Delete thread \"{}\" and kill {} session(s)?",
                                    name, sessions
                                )
                            } else {
                                format!("Delete thread \"{}\"?", name)
                            }
                        }
                        ConfirmPurpose::KillSession { session_name } => {
                            format!("Kill session \"{}\"?", session_name)
                        }
                        ConfirmPurpose::KillAllSessions { thread_name, .. } => {
                            format!("Kill all sessions for \"{}\"?", thread_name)
                        }
                        ConfirmPurpose::KillMarkedSessions { session_names } => {
                            format!("Kill {} selected sessions?", session_names.len())
                        }
                        ConfirmPurpose::DeleteWorktree {
                            name, kill_session, ..
                        } => {
                            if *kill_session {
                                format!("Kill session and delete worktree \"{}\"?", name)
                            } else {
                                format!("Delete worktree \"{}\"?", name)
                            }
                        }
                    };
                    confirm_modal::render(
                        frame,
                        &message,
                        !purpose.requires_explicit_yes(),
                        area,
                        &self.theme,
                    );
                }
                Mode::Error { message } => {
                    error_modal::render(frame, message, area, &self.theme);
                }
                Mode::Finder { state } => {
                    finder_modal::render(
                        frame,
                        " Find Session ",
                        &state.query,
                        &state.all_entries,
                        &state.filtered,
                        state.cursor,
                        area,
                        &self.theme,
                    );
                }
                Mode::ThreadPicker { state, .. } => {
                    finder_modal::render(
                        frame,
                        " Move to Thread ",
                        &state.query,
                        &state.all_entries,
                        &state.filtered,
                        state.cursor,
                        area,
                        &self.theme,
                    );
                }
            }

            if let Some((message, is_error)) = &notification_msg {
                render_notification(frame, area, message, *is_error, &self.theme);
            }
        })?;
        Ok(())
    }

    /// Build a `StatusContext` from the current mode and already-resolved selection.
    pub(super) fn status_context(&self, selected: &SelectedItem) -> StatusContext {
        match &self.mode {
            Mode::Input { .. } => StatusContext::Input,
            Mode::Confirm { .. } => StatusContext::Confirm,
            Mode::Error { .. } => StatusContext::Error,
            Mode::Help { .. } => StatusContext::Help,
            Mode::Finder { .. } => StatusContext::Finder,
            Mode::ThreadPicker { .. } => StatusContext::ThreadPicker,
            Mode::Normal => {
                if matches!(self.view_mode, ViewMode::Agents | ViewMode::AgentGrid) {
                    if let Some(pending) = &self.pin_assign_pending {
                        let target_path = self
                            .state
                            .all_agents_flat()
                            .into_iter()
                            .find(|a| &a.pane_id == pending)
                            .map(|a| {
                                format!(
                                    "{} / {} / {}",
                                    a.thread_name, a.session_display_name, a.agent_display_name
                                )
                            })
                            .unwrap_or_else(|| "agent".to_string());
                        return StatusContext::AgentsViewSlotAssign { target_path };
                    }
                    return if matches!(self.view_mode, ViewMode::AgentGrid) {
                        StatusContext::AgentGrid
                    } else {
                        StatusContext::AgentsView
                    };
                }
                let marked_count = self.marked_session_names().len();
                if marked_count > 0 {
                    return StatusContext::NormalMarkedSessions {
                        count: marked_count,
                    };
                }
                match selected {
                    SelectedItem::None => StatusContext::NormalNone,
                    SelectedItem::Collection(_) => StatusContext::NormalCollection,
                    SelectedItem::Thread(_, _) => StatusContext::NormalThread,
                    SelectedItem::Session(col_idx, thread_idx, sess_idx) => {
                        if self
                            .worktree_for_active_session_selection(*col_idx, *thread_idx, *sess_idx)
                            .is_some()
                        {
                            StatusContext::NormalWorktreeSession
                        } else {
                            StatusContext::NormalSession
                        }
                    }
                    SelectedItem::Worktree(_, _, _) => StatusContext::NormalWorktree,
                    SelectedItem::Agent(..) => StatusContext::NormalAgent,
                }
            }
        }
    }
}
