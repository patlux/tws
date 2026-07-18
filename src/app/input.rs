use super::*;

impl App {
    /// Top-level handler for normal mode.
    pub(super) fn handle_normal_mode(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        terminal: &mut Tui,
    ) -> std::io::Result<()> {
        let normal_action = self.keymap.resolve(KeyMode::Normal, code, modifiers);
        if normal_action == Some(Action::Help) {
            self.mode = Mode::Help { scroll: 0 };
            return Ok(());
        }
        if normal_action == Some(Action::ToggleGrid) {
            self.view_mode = if matches!(self.view_mode, ViewMode::AgentGrid) {
                ViewMode::Tree
            } else {
                ViewMode::AgentGrid
            };
            self.last_grid_refresh = Instant::now()
                .checked_sub(GRID_REFRESH_INTERVAL)
                .unwrap_or_else(Instant::now);
            return Ok(());
        }
        if normal_action == Some(Action::ToggleView) {
            self.view_mode = match self.view_mode {
                ViewMode::Tree => ViewMode::Agents,
                ViewMode::Agents | ViewMode::AgentGrid => ViewMode::Tree,
            };
            return Ok(());
        }
        if matches!(self.view_mode, ViewMode::Agents | ViewMode::AgentGrid) {
            return self.handle_agents_view_key(code, modifiers, terminal);
        }
        if self.handle_jump_key(code, modifiers) {
            return Ok(());
        }
        self.handle_normal_key(code, modifiers, terminal)
    }

    pub(super) fn handle_jump_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        match self.keymap.resolve(KeyMode::Normal, code, modifiers) {
            Some(Action::JumpTop) if self.jump_to_top_pending => {
                self.jump_to_top_pending = false;
                self.jump_to_tree_edge(false);
                true
            }
            Some(Action::JumpTop) => {
                self.jump_to_top_pending = true;
                true
            }
            Some(Action::JumpBottom) => {
                self.jump_to_top_pending = false;
                self.jump_to_tree_edge(true);
                true
            }
            _ => {
                self.jump_to_top_pending = false;
                false
            }
        }
    }

    pub(super) fn handle_agents_jump_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        agent_count: usize,
    ) -> bool {
        match self.keymap.resolve(KeyMode::Agents, code, modifiers) {
            Some(Action::JumpTop) if self.jump_to_top_pending => {
                self.jump_to_top_pending = false;
                self.agent_list_cursor = 0;
                true
            }
            Some(Action::JumpTop) => {
                self.jump_to_top_pending = true;
                true
            }
            Some(Action::JumpBottom) => {
                self.jump_to_top_pending = false;
                self.agent_list_cursor = agent_count.saturating_sub(1);
                true
            }
            _ => {
                self.jump_to_top_pending = false;
                false
            }
        }
    }

    pub(super) fn jump_to_tree_edge(&mut self, end: bool) {
        let paths = self.visible_tree_paths();
        if let Some(path) = if end { paths.last() } else { paths.first() } {
            self.tree_state.select(path.clone());
        }
    }

    pub(super) fn visible_tree_paths(&self) -> Vec<Vec<String>> {
        let mut paths = Vec::new();
        for (col_idx, col) in self.state.collections.iter().enumerate() {
            if col.is_root || self.state.collection_is_hidden(col_idx) {
                continue;
            }
            let col_path = vec![col.id.to_string()];
            paths.push(col_path.clone());
            if self.tree_state.opened().contains(&col_path) {
                for (thread_idx, thread) in col.threads.iter().enumerate() {
                    if self.state.thread_is_hidden(col_idx, thread_idx) {
                        continue;
                    }
                    let thread_path = vec![col.id.to_string(), thread.id.to_string()];
                    paths.push(thread_path.clone());
                    if self.tree_state.opened().contains(&thread_path) {
                        self.push_visible_thread_children(&mut paths, &thread_path, thread.id);
                    }
                }
            }
        }
        for (col_idx, col) in self.state.collections.iter().enumerate() {
            if !col.is_root {
                continue;
            }
            for (thread_idx, thread) in col.threads.iter().enumerate() {
                if self.state.thread_is_hidden(col_idx, thread_idx) {
                    continue;
                }
                let thread_path = vec![thread.id.to_string()];
                paths.push(thread_path.clone());
                if self.tree_state.opened().contains(&thread_path) {
                    self.push_visible_thread_children(&mut paths, &thread_path, thread.id);
                }
            }
        }
        paths
    }

    pub(super) fn push_visible_thread_children(
        &self,
        paths: &mut Vec<Vec<String>>,
        thread_path: &[String],
        thread_id: uuid::Uuid,
    ) {
        for session in self
            .state
            .active_sessions
            .iter()
            .filter(|s| s.thread_id == thread_id)
        {
            let mut session_path = thread_path.to_vec();
            session_path.push(session.tmux_session_name.clone());
            paths.push(session_path.clone());
            if self.tree_state.opened().contains(&session_path) {
                for agent in self.state.agents_for_session(&session.tmux_session_name) {
                    let mut agent_path = session_path.clone();
                    agent_path.push(agent.pane_id.clone());
                    paths.push(agent_path);
                }
            }
        }
        for worktree in self.state.worktrees_for_thread(thread_id) {
            let mut worktree_path = thread_path.to_vec();
            worktree_path.push(worktree.tmux_session_name.clone());
            paths.push(worktree_path);
        }
    }

    pub(super) fn next_visible_path(&self, selected: &[String]) -> Vec<String> {
        let paths = self.visible_tree_paths();
        if paths.is_empty() {
            return Vec::new();
        }
        if let Some(idx) = paths.iter().position(|path| path == selected) {
            let next_idx = if idx + 1 < paths.len() {
                idx + 1
            } else {
                idx.saturating_sub(1)
            };
            return paths[next_idx].clone();
        }
        paths.first().cloned().unwrap_or_default()
    }

    pub(super) fn next_visible_path_after_hiding(&self, selected: &[String]) -> Vec<String> {
        let paths = self.visible_tree_paths();
        let Some(idx) = paths.iter().position(|path| path == selected) else {
            return paths.first().cloned().unwrap_or_default();
        };
        for path in paths.iter().skip(idx + 1) {
            if !path.as_slice().starts_with(selected) {
                return path.clone();
            }
        }
        for path in paths.iter().take(idx).rev() {
            if !path.as_slice().starts_with(selected) {
                return path.clone();
            }
        }
        Vec::new()
    }

    /// Select a tree path and open all its ancestor nodes so the
    /// selection is actually visible (expanded collection/thread/session).
    pub(super) fn select_tree_path_expanded(&mut self, path: Vec<String>) {
        for i in 1..path.len() {
            self.tree_state.open(path[..i].to_vec());
        }
        self.tree_state.select(path);
    }

    pub(super) fn ensure_visible_tree_selection(&mut self) {
        if self.tree_state.selected().is_empty() {
            return;
        }
        if matches!(
            self.state.resolve_selection(self.tree_state.selected()),
            SelectedItem::None
        ) {
            let next = self.next_visible_path(self.tree_state.selected());
            self.tree_state.select(next);
        }
    }

    pub(super) fn handle_normal_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        terminal: &mut Tui,
    ) -> std::io::Result<()> {
        let action = match self.keymap.resolve(KeyMode::Normal, code, modifiers) {
            Some(a) => a,
            None => return Ok(()),
        };
        match action {
            Action::Quit => self.running = false,
            Action::MoveDown => {
                self.tree_state.key_down();
            }
            Action::MoveUp => {
                self.tree_state.key_up();
            }
            Action::MoveLeft => {
                if self.tree_state.key_left() {
                    self.refresh_worktree_sessions();
                }
            }
            Action::MoveRight => {
                if self.tree_state.key_right() {
                    self.refresh_worktree_sessions();
                }
            }
            Action::ToggleSelect => {
                if self.tree_state.toggle_selected() {
                    self.refresh_worktree_sessions();
                }
            }
            Action::Enter => self.start_enter(terminal)?,
            Action::Deselect => {
                self.tree_state.select(Vec::new());
            }
            Action::Add => self.start_add(),
            Action::AddCollection => {
                self.mode = Mode::Input {
                    purpose: InputPurpose::AddCollection,
                    buffer: InputBuffer::default(),
                };
            }
            Action::AddWorktree => self.start_add_worktree(),
            Action::Rename => self.start_rename(),
            Action::Delete => self.start_delete(),
            Action::KillSession => self.start_kill_session(),
            Action::Move => self.start_move_session(),
            Action::MarkSession => self.toggle_mark_current_session(),
            Action::ClearMarks => self.clear_marked_sessions(),
            Action::Hide => self.hide_selected(),
            Action::ShowHidden => self.show_all_hidden(),
            Action::ToggleActiveFilter => self.toggle_active_filter(),
            Action::Help => self.mode = Mode::Help { scroll: 0 },
            Action::Finder => {
                if self.state.active_sessions.is_empty() && self.state.worktree_sessions.is_empty()
                {
                    self.set_flash("No sessions or worktrees");
                    return Ok(());
                }
                self.start_finder();
            }
            Action::RecentSession1 => self.attach_recent(0, terminal)?,
            Action::RecentSession2 => self.attach_recent(1, terminal)?,
            Action::RecentSession3 => self.attach_recent(2, terminal)?,
            Action::RecentSession4 => self.attach_recent(3, terminal)?,
            Action::RecentSession5 => self.attach_recent(4, terminal)?,
            Action::ExpandAll => {
                self.toggle_expand_all();
                self.refresh_worktree_sessions();
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn attach_recent(
        &mut self,
        index: usize,
        terminal: &mut Tui,
    ) -> std::io::Result<()> {
        let recent = self.state.recent_sessions(5);
        if let Some(session) = recent.get(index) {
            let name = session.tmux_session_name.clone();
            self.attach_to_session(&name, terminal)?;
            if let Some(path) = self.state.session_tree_path(&name) {
                self.select_tree_path_expanded(path);
            }
        }
        Ok(())
    }

    pub(super) fn handle_agents_view_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        terminal: &mut Tui,
    ) -> std::io::Result<()> {
        let agents = self.state.all_agents_flat();

        if self.handle_agents_jump_key(code, modifiers, agents.len()) {
            return Ok(());
        }

        let current_pane_id = agents
            .get(self.agent_list_cursor)
            .map(|a| a.pane_id.clone());

        // If a P-triggered slot-assign is pending, the next keystroke either assigns
        // (digit), cancels silently (Esc), or cancels and falls through (any other key).
        if let Some(pending) = self.pin_assign_pending.take() {
            if let KeyCode::Char(c) = code
                && c.is_ascii_digit()
            {
                    let slot: u8 = c.to_digit(10).unwrap() as u8;
                    let snapshot = agents.iter().find(|a| a.pane_id == pending);
                    let already_in_slot = snapshot
                        .and_then(|a| a.pin_slot)
                        .map(|s| s == slot)
                        .unwrap_or(false);
                    let path = snapshot.map(|a| {
                    format!(
                        "{} / {} / {}",
                        a.thread_name, a.session_display_name, a.agent_display_name
                    )
                    });
                    self.state.pin_agent_to(&pending, slot);
                if !already_in_slot && let Some(p) = path {
                            self.set_flash(&format!("Pin {}: {}", slot, p));
                        }
                    self.reanchor_agent_cursor(Some(pending));
                    return Ok(());
                }
            if matches!(code, KeyCode::Esc) {
                return Ok(());
            }
            // Any other key cancels assignment and falls through to normal dispatch.
        }

        let action = self.keymap.resolve(KeyMode::Agents, code, modifiers);
        let grid_columns = usize::from((self.last_width / 42).max(1));
        match action {
            Some(Action::MoveDown) => {
                if !agents.is_empty() {
                    let step = if matches!(self.view_mode, ViewMode::AgentGrid) {
                        grid_columns
                    } else {
                        1
                    };
                    self.agent_list_cursor = (self.agent_list_cursor + step).min(agents.len() - 1);
                }
            }
            Some(Action::MoveUp) => {
                let step = if matches!(self.view_mode, ViewMode::AgentGrid) {
                    grid_columns
                } else {
                    1
                };
                self.agent_list_cursor = self.agent_list_cursor.saturating_sub(step);
            }
            Some(Action::MoveRight) if matches!(self.view_mode, ViewMode::AgentGrid) => {
                if !agents.is_empty() {
                    self.agent_list_cursor = (self.agent_list_cursor + 1).min(agents.len() - 1);
                }
            }
            Some(Action::MoveLeft) if matches!(self.view_mode, ViewMode::AgentGrid) => {
                self.agent_list_cursor = self.agent_list_cursor.saturating_sub(1);
            }
            Some(Action::Enter) => {
                if let Some(a) = agents.get(self.agent_list_cursor) {
                    let session_name = a.tmux_session_name.clone();
                    let _ = mux::focus_pane(&session_name, a.window_index, &a.pane_id);
                    self.attach_to_session(&session_name, terminal)?;
                }
            }
            Some(Action::Cancel) => {
                self.view_mode = ViewMode::Tree;
            }
            Some(Action::Quit) => {
                self.running = false;
            }
            Some(Action::PinAgent) => {
                if let Some(pane_id) = current_pane_id {
                    let snapshot = agents.get(self.agent_list_cursor);
                    let already_pinned = snapshot.map(|a| a.pin_slot.is_some()).unwrap_or(false);
                    let path = snapshot.map(|a| {
                        format!(
                            "{} / {} / {}",
                            a.thread_name, a.session_display_name, a.agent_display_name
                        )
                    });
                    if already_pinned {
                        self.state.unpin_agent(&pane_id);
                        if let Some(p) = &path {
                            self.set_flash(&format!("Unpinned: {}", p));
                        }
                    } else {
                        match self.state.pin_agent_auto(&pane_id) {
                            Some(slot) => {
                                if let Some(p) = &path {
                                    self.set_flash(&format!("Pin {}: {}", slot, p));
                                }
                            }
                            None => self.set_flash("Max 10 pins reached"),
                        }
                    }
                    self.reanchor_agent_cursor(Some(pane_id));
                }
            }
            Some(Action::PinAgentSlot) => {
                if current_pane_id.is_some() {
                    self.pin_assign_pending = current_pane_id;
                }
            }
            _ => {
                // Plain digit → jump cursor to that pinned slot (Enter still attaches)
                if let KeyCode::Char(c) = code
                    && c.is_ascii_digit()
                {
                        let slot: u8 = c.to_digit(10).unwrap() as u8;
                        if let Some(agent) = self.state.agent_by_pin_slot(slot) {
                            let target_id = agent.pane_id.clone();
                            if let Some(idx) = agents.iter().position(|a| a.pane_id == target_id) {
                                self.agent_list_cursor = idx;
                            }
                        }
                    }
            }
        }
        Ok(())
    }

    /// Re-anchor `agent_list_cursor` to the agent with the given `pane_id`,
    /// falling back to clamping in range if the agent is no longer in the flat list.
    pub(super) fn reanchor_agent_cursor(&mut self, anchor_pane_id: Option<String>) {
        let agents = self.state.all_agents_flat();
        if agents.is_empty() {
            self.agent_list_cursor = 0;
            return;
        }
        if let Some(id) = anchor_pane_id
            && let Some(idx) = agents.iter().position(|a| a.pane_id == id)
        {
                self.agent_list_cursor = idx;
                return;
            }
        self.agent_list_cursor = self.agent_list_cursor.min(agents.len() - 1);
    }

    /// Resolve the currently selected item accounting for the active view mode.
    pub(super) fn resolve_current_selected(&self) -> SelectedItem {
        match self.view_mode {
            ViewMode::Tree => self.state.resolve_selection(self.tree_state.selected()),
            ViewMode::Agents | ViewMode::AgentGrid => {
                let agents = self.state.all_agents_flat();
                agents
                    .get(self.agent_list_cursor)
                    .map(|a| SelectedItem::Agent(a.col_idx, a.thread_idx, a.sess_idx, a.agent_idx))
                    .unwrap_or(SelectedItem::None)
            }
        }
    }

    pub(super) fn handle_input_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        terminal: &mut Tui,
    ) -> std::io::Result<()> {
        let action = self.keymap.resolve(KeyMode::Input, code, modifiers);
        match action {
            Some(Action::Cancel) => {
                self.mode = Mode::Normal;
            }
            Some(Action::Confirm) => {
                self.confirm_input(terminal)?;
            }
            Some(Action::Backspace) => {
                if let Mode::Input { buffer, .. } = &mut self.mode {
                    buffer.backspace();
                }
            }
            _ => {
                if let Mode::Input { buffer, .. } = &mut self.mode {
                    match code {
                        KeyCode::Left => buffer.left(),
                        KeyCode::Right => buffer.right(),
                        KeyCode::Home => buffer.home(),
                        KeyCode::End => buffer.end(),
                        KeyCode::Char('a') if modifiers.contains(KeyModifiers::CONTROL) => {
                            buffer.home()
                        }
                        KeyCode::Char('e') if modifiers.contains(KeyModifiers::CONTROL) => {
                            buffer.end()
                        }
                        KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
                            buffer.kill_to_start()
                        }
                        KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
                            buffer.insert(c)
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn handle_confirm_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let action = self.keymap.resolve(KeyMode::ConfirmModal, code, modifiers);
        match action {
            Some(Action::Confirm) => {
                // Multi-item destructive confirms only accept an explicit `y`,
                // so a double-tapped Enter can't wipe a collection/thread.
                if code == KeyCode::Enter
                    && let Mode::Confirm { purpose } = &self.mode
                    && purpose.requires_explicit_yes()
                {
                            return;
                        }
                self.execute_confirm();
            }
            Some(Action::Cancel) => {
                self.mode = Mode::Normal;
            }
            _ => {}
        }
    }

    pub(super) fn handle_error_key(&mut self, code: KeyCode, _modifiers: KeyModifiers) {
        if matches!(code, KeyCode::Enter | KeyCode::Esc) {
            self.mode = Mode::Normal;
        }
    }

    pub(super) fn handle_help_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
                self.mode = Mode::Normal;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let Mode::Help { scroll } = &mut self.mode {
                    let max = help_modal::line_count(&self.theme, &self.keymap) as u16;
                    *scroll = (*scroll + 1).min(max);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Mode::Help { scroll } = &mut self.mode {
                    *scroll = scroll.saturating_sub(1);
                }
            }
            _ => {}
        }
    }

    pub(super) fn start_add(&mut self) {
        let selected = self.state.resolve_selection(self.tree_state.selected());
        let purpose = match selected {
            SelectedItem::Collection(idx)
            | SelectedItem::Thread(idx, _)
            | SelectedItem::Session(idx, _, _)
            | SelectedItem::Worktree(idx, _, _)
            | SelectedItem::Agent(idx, _, _, _) => InputPurpose::AddThread {
                    collection_idx: idx,
            },
            SelectedItem::None => {
                let col_idx = self.state.ensure_root_collection();
                InputPurpose::AddThread {
                    collection_idx: col_idx,
                }
            }
        };
        self.mode = Mode::Input {
            purpose,
            buffer: InputBuffer::default(),
        };
    }

    pub(super) fn start_add_worktree(&mut self) {
        let selected = self.state.resolve_selection(self.tree_state.selected());
        let (col_idx, thread_idx) = match selected {
            SelectedItem::Thread(col_idx, thread_idx)
            | SelectedItem::Session(col_idx, thread_idx, _)
            | SelectedItem::Worktree(col_idx, thread_idx, _)
            | SelectedItem::Agent(col_idx, thread_idx, _, _) => (col_idx, thread_idx),
            SelectedItem::Collection(..) | SelectedItem::None => {
                self.set_flash("Select a thread with a git repo");
                return;
            }
        };
        let Some((repo, worktree_dir)) = self.worktree_repo_for_thread(col_idx, thread_idx) else {
            self.set_flash("No git repo for this thread");
            return;
        };
        let thread_id = self.state.collections[col_idx].threads[thread_idx].id;
        self.mode = Mode::Input {
            purpose: InputPurpose::NewWorktree {
                thread_id,
                repo,
                worktree_dir,
            },
            buffer: InputBuffer::default(),
        };
    }

    /// Resolve the git repo (and optional worktree base dir) backing a thread,
    /// matching the discovery logic: explicit `[[worktrees]]` config first,
    /// then an auto-detected start-dir subdirectory that is a git repo.
    pub(super) fn worktree_repo_for_thread(
        &self,
        col_idx: usize,
        thread_idx: usize,
    ) -> Option<(PathBuf, Option<PathBuf>)> {
        let col = self.state.collections.get(col_idx)?;
        let thread = col.threads.get(thread_idx)?;
        let collection = (!col.is_root).then_some(col.name.as_str());
        for cfg in self.worktree_configs.iter().rev() {
            let cfg_collection = cfg.collection.as_deref();
            if cfg_collection == collection && cfg.thread == thread.name {
                let repo = expand_home(&cfg.repo);
                let worktree_dir = cfg.worktree_dir.as_deref().map(expand_home);
                return Some((repo, worktree_dir));
            }
        }
        self.auto_worktree_repo(col_idx, thread_idx)
            .map(|repo| (repo, None))
    }

    /// Select the freshly created worktree row for `thread_id` by its path,
    /// expanding the thread so it is visible.
    pub(super) fn select_worktree_by_path(
        &mut self,
        thread_id: uuid::Uuid,
        path: &std::path::Path,
    ) {
        let Some((col_idx, thread_idx)) = self.find_thread_indices_by_id(thread_id) else {
            return;
        };
        let col = &self.state.collections[col_idx];
        let thread = &col.threads[thread_idx];
        let thread_path = if col.is_root {
            vec![thread.id.to_string()]
        } else {
            vec![col.id.to_string(), thread.id.to_string()]
        };
        self.tree_state.open(thread_path.clone());
        if let Some(wt) = self
            .state
            .worktree_sessions
            .iter()
            .find(|w| w.thread_id == thread_id && w.path == path)
        {
            let mut sel = thread_path;
            sel.push(wt.tmux_session_name.clone());
            self.tree_state.select(sel);
        }
    }

    pub(super) fn start_rename(&mut self) {
        let selected = self.state.resolve_selection(self.tree_state.selected());
        let current_name = match self.state.selected_name(&selected) {
            Some(name) => name,
            None => return,
        };
        let purpose = match selected {
            SelectedItem::Collection(idx) => InputPurpose::RenameCollection { idx },
            SelectedItem::Thread(col_idx, thread_idx) => InputPurpose::RenameThread {
                col_idx,
                thread_idx,
            },
            SelectedItem::Session(col_idx, thread_idx, sess_idx) => {
                let thread_id = self.state.collections[col_idx].threads[thread_idx].id;
                let sessions = self.state.sessions_for_thread(thread_id);
                match sessions.get(sess_idx) {
                    Some(session) => InputPurpose::RenameSession {
                        col_idx,
                        thread_idx,
                        old_tmux_name: session.tmux_session_name.clone(),
                    },
                    None => return,
                }
            }
            SelectedItem::Agent(col_idx, thread_idx, sess_idx, agent_idx) => {
                match self
                    .state
                    .resolve_agent(col_idx, thread_idx, sess_idx, agent_idx)
                {
                    Some(agent) => InputPurpose::RenameAgent {
                        pane_id: agent.pane_id.clone(),
                    },
                    None => return,
                }
            }
            SelectedItem::Worktree(..) | SelectedItem::None => return,
        };
        self.mode = Mode::Input {
            purpose,
            buffer: InputBuffer::from(current_name),
        };
    }

    pub(super) fn worktree_for_active_session_selection(
        &self,
        col_idx: usize,
        thread_idx: usize,
        sess_idx: usize,
    ) -> Option<&WorktreeSession> {
        let thread_id = self
            .state
            .collections
            .get(col_idx)?
            .threads
            .get(thread_idx)?
            .id;
        let sessions = self.state.sessions_for_thread(thread_id);
        let session = sessions.get(sess_idx)?;
        self.state
            .find_worktree_by_tmux_name(&session.tmux_session_name)
    }

    pub(super) fn hide_selected(&mut self) {
        let selected_path = self.tree_state.selected().to_vec();
        let fallback_path = self.next_visible_path_after_hiding(&selected_path);
        let selected = self.state.resolve_selection(&selected_path);
        let hidden = match selected {
            SelectedItem::Collection(idx) => self.state.hide_collection(idx),
            SelectedItem::Thread(col_idx, thread_idx) => {
                self.state.hide_thread(col_idx, thread_idx)
            }
            SelectedItem::Session(..)
            | SelectedItem::Worktree(..)
            | SelectedItem::Agent(..)
            | SelectedItem::None => false,
        };
        if hidden {
            self.tree_state.select(fallback_path);
            self.save_state();
            self.set_flash("Hidden");
            }
    }

    pub(super) fn toggle_active_filter(&mut self) {
        self.state.active_filter = !self.state.active_filter;
        if self.state.active_filter {
            self.ensure_visible_tree_selection();
            self.set_flash("Active filter on");
        } else {
            self.set_flash("Active filter off");
        }
    }

    pub(super) fn show_all_hidden(&mut self) {
        let restored = self.state.show_all_hidden();
        if restored == 0 {
            self.set_flash("No hidden collections or threads");
            return;
        }
        self.save_state();
        self.set_flash(&format!("Restored {} hidden", restored));
    }

    pub(super) fn start_delete(&mut self) {
        let selected = self.state.resolve_selection(self.tree_state.selected());
        let purpose = match &selected {
            SelectedItem::Collection(idx) => {
                let name = self.state.collections[*idx].name.clone();
                ConfirmPurpose::DeleteCollection { idx: *idx, name }
            }
            SelectedItem::Thread(col_idx, thread_idx) => {
                let name = self.state.collections[*col_idx].threads[*thread_idx]
                    .name
                    .clone();
                ConfirmPurpose::DeleteThread {
                    col_idx: *col_idx,
                    thread_idx: *thread_idx,
                    name,
                }
            }
            SelectedItem::Worktree(col_idx, thread_idx, wt_idx) => {
                let thread_id = self.state.collections[*col_idx].threads[*thread_idx].id;
                let worktrees = self.state.worktrees_for_thread(thread_id);
                let Some(worktree) = worktrees.get(*wt_idx) else {
                    return;
                };
                if worktree.is_main {
                    self.set_flash("Cannot delete main worktree");
                    return;
                }
                if self
                    .pending_worktree_deletes
                    .contains_key(&worktree.tmux_session_name)
                {
                    self.set_flash("Worktree delete already running");
                    return;
                }
                ConfirmPurpose::DeleteWorktree {
                    repo: worktree.repo.clone(),
                    path: worktree.path.clone(),
                    name: worktree.display_name.clone(),
                    tmux_session_name: worktree.tmux_session_name.clone(),
                    kill_session: false,
                }
            }
            SelectedItem::Session(col_idx, thread_idx, sess_idx) => {
                let Some(worktree) =
                    self.worktree_for_active_session_selection(*col_idx, *thread_idx, *sess_idx)
                else {
                    let hint = self.keymap.key_hint(KeyMode::Normal, Action::KillSession);
                    self.set_flash(&format!("Use {} to kill sessions", hint));
                    return;
                };
                if worktree.is_main {
                    self.set_flash("Cannot delete main worktree");
                    return;
                }
                if self
                    .pending_worktree_deletes
                    .contains_key(&worktree.tmux_session_name)
                {
                    self.set_flash("Worktree delete already running");
                    return;
                }
                ConfirmPurpose::DeleteWorktree {
                    repo: worktree.repo.clone(),
                    path: worktree.path.clone(),
                    name: worktree.display_name.clone(),
                    tmux_session_name: worktree.tmux_session_name.clone(),
                    kill_session: true,
                }
            }
            SelectedItem::Agent(..) | SelectedItem::None => return,
        };
        self.mode = Mode::Confirm { purpose };
    }

    pub(super) fn start_kill_session(&mut self) {
        let marked = self.marked_session_names();
        if !marked.is_empty() {
            self.mode = Mode::Confirm {
                purpose: ConfirmPurpose::KillMarkedSessions {
                    session_names: marked,
                },
            };
            return;
        }

        let selected = self.state.resolve_selection(self.tree_state.selected());
        match selected {
            SelectedItem::Session(col_idx, thread_idx, sess_idx) => {
                if let Some(worktree) =
                    self.worktree_for_active_session_selection(col_idx, thread_idx, sess_idx)
                {
                    if worktree.is_main {
                        self.set_flash("Cannot delete main worktree");
                        return;
                    }
                    if self
                        .pending_worktree_deletes
                        .contains_key(&worktree.tmux_session_name)
                    {
                        self.set_flash("Worktree delete already running");
                        return;
                    }
                    self.mode = Mode::Confirm {
                        purpose: ConfirmPurpose::DeleteWorktree {
                            repo: worktree.repo.clone(),
                            path: worktree.path.clone(),
                            name: worktree.display_name.clone(),
                            tmux_session_name: worktree.tmux_session_name.clone(),
                            kill_session: true,
                        },
                    };
                    return;
                }

                let thread_id = self.state.collections[col_idx].threads[thread_idx].id;
                let sessions = self.state.sessions_for_thread(thread_id);
                if let Some(session) = sessions.get(sess_idx) {
                    let name = session.tmux_session_name.clone();
                    self.mode = Mode::Confirm {
                        purpose: ConfirmPurpose::KillSession { session_name: name },
                    };
                }
            }
            SelectedItem::Thread(col_idx, thread_idx) => {
                // If the thread has active sessions, offer to kill all of them
                if self.state.has_active_session(col_idx, thread_idx) {
                    let thread_name = self.state.collections[col_idx].threads[thread_idx]
                        .name
                        .clone();
                    self.mode = Mode::Confirm {
                        purpose: ConfirmPurpose::KillAllSessions {
                            col_idx,
                            thread_idx,
                            thread_name,
                        },
                    };
                }
            }
            _ => {}
        }
    }

    pub(super) fn start_move_session(&mut self) {
        let selected = self.state.resolve_selection(self.tree_state.selected());
        let (col_idx, thread_idx, sess_idx) = match selected {
            SelectedItem::Session(c, t, s) => (c, t, s),
            _ => return,
        };

        let thread_id = self.state.collections[col_idx].threads[thread_idx].id;
        let sessions = self.state.sessions_for_thread(thread_id);
        let session = match sessions.get(sess_idx) {
            Some(s) => s,
            None => return,
        };
        let session_name = session.tmux_session_name.clone();
        let session_label = session.display_name.clone();

        let entries: Vec<(String, String)> = self
            .state
            .all_threads_display()
            .into_iter()
            .filter(|(ci, ti, _)| !(*ci == col_idx && *ti == thread_idx))
            .map(|(ci, ti, path)| (format!("{}:{}", ci, ti), path))
            .collect();

        if entries.is_empty() {
            self.set_flash("No other threads to move to");
            return;
        }

        self.mode = Mode::ThreadPicker {
            state: FinderState::new(entries),
            session_name,
            session_label,
        };
    }

    pub(super) fn start_enter(&mut self, terminal: &mut Tui) -> std::io::Result<()> {
        let selected = self.state.resolve_selection(self.tree_state.selected());
        match selected {
            SelectedItem::Collection(..) => {}
            SelectedItem::Thread(col_idx, thread_idx) => {
                self.mode = Mode::Input {
                    purpose: InputPurpose::NewSession {
                        col_idx,
                        thread_idx,
                    },
                    buffer: InputBuffer::default(),
                };
            }
            SelectedItem::Session(col_idx, thread_idx, sess_idx) => {
                let sessions = self
                    .state
                    .sessions_for_thread(self.state.collections[col_idx].threads[thread_idx].id);
                if let Some(session) = sessions.get(sess_idx) {
                    let name = session.tmux_session_name.clone();
                    self.attach_to_session(&name, terminal)?;
                }
            }
            SelectedItem::Agent(col_idx, thread_idx, sess_idx, agent_idx) => {
                if let Some(agent) = self
                    .state
                    .resolve_agent(col_idx, thread_idx, sess_idx, agent_idx)
                {
                    let session_name = agent.tmux_session_name.clone();
                    let window_index = agent.window_index;
                    let pane_id = agent.pane_id.clone();
                    let _ = mux::focus_pane(&session_name, window_index, &pane_id);
                    self.attach_to_session(&session_name, terminal)?;
                }
            }
            SelectedItem::Worktree(col_idx, thread_idx, wt_idx) => {
                let thread = &self.state.collections[col_idx].threads[thread_idx];
                let worktrees = self.state.worktrees_for_thread(thread.id);
                if let Some(worktree) = worktrees.get(wt_idx) {
                    if !worktree.launchable {
                        self.set_flash("Worktree is prunable or missing");
                        return Ok(());
                    }
                    let session_name = worktree.tmux_session_name.clone();
                    let path = worktree.path.clone();
                    self.launch_session_in_dir(&session_name, path, terminal)?;
                    self.set_flash("Worktree launched");
                }
            }
            SelectedItem::None => {
                let (col_idx, thread_idx) = self.state.ensure_general_thread();
                self.mode = Mode::Input {
                    purpose: InputPurpose::NewSession {
                        col_idx,
                        thread_idx,
                    },
                    buffer: InputBuffer::default(),
                };
            }
        }
        Ok(())
    }

    pub(super) fn start_finder(&mut self) {
        let mut sessions: Vec<_> = self.state.active_sessions.iter().collect();
        sessions.sort_by(|a, b| b.last_attached.cmp(&a.last_attached));

        let mut entries: Vec<(String, String)> = sessions
            .iter()
            .filter_map(|s| {
                let path = self.state.session_display_path(s)?;
                Some((s.tmux_session_name.clone(), path))
            })
            .collect();

        // Worktree rows are inactive by definition — the active filter drops them.
        let mut worktrees: Vec<_> = if self.state.active_filter {
            Vec::new()
        } else {
            self.state.worktree_sessions.iter().collect()
        };
        worktrees.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        entries.extend(worktrees.into_iter().filter_map(|w| {
            if self
                .state
                .active_sessions
                .iter()
                .any(|s| s.tmux_session_name == w.tmux_session_name)
            {
                return None;
            }
            let path = self.state.worktree_display_path(w)?;
            Some((
                w.tmux_session_name.clone(),
                format!("{}  {}", path, w.path.display()),
            ))
        }));

        if entries.is_empty() {
            self.set_flash("No visible sessions or worktrees");
            return;
        }

        self.mode = Mode::Finder {
            state: FinderState::new(entries),
        };
    }

    pub(super) fn handle_finder_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        terminal: &mut Tui,
    ) -> std::io::Result<()> {
        let action = self.keymap.resolve(KeyMode::Finder, code, modifiers);

        match action {
            Some(Action::MoveDown) => {
                if let Mode::Finder { state } = &mut self.mode
                    && !state.filtered.is_empty()
                {
                        state.cursor = (state.cursor + 1).min(state.filtered.len() - 1);
                    }
            }
            Some(Action::MoveUp) => {
                if let Mode::Finder { state } = &mut self.mode {
                    state.cursor = state.cursor.saturating_sub(1);
                }
            }
            Some(Action::Cancel) => {
                self.mode = Mode::Normal;
            }
            Some(Action::Confirm) => {
                let old_mode = std::mem::replace(&mut self.mode, Mode::Normal);
                if let Mode::Finder { state } = old_mode
                    && let Some(&idx) = state.filtered.get(state.cursor)
                {
                        let name = state.all_entries[idx].0.clone();
                        self.open_session_or_worktree(&name, terminal)?;
                        if let Some(path) = self.state.session_tree_path(&name) {
                            self.select_tree_path_expanded(path);
                        }
                    }
            }
            Some(Action::Backspace) => {
                if let Mode::Finder { state } = &mut self.mode {
                    state.query.pop();
                    state.update_filter();
                }
            }
            _ => {
                // Character input for search query
                if let KeyCode::Char(c) = code
                    && let Mode::Finder { state } = &mut self.mode
                {
                        state.query.push(c);
                        state.update_filter();
                    }
            }
        }
        Ok(())
    }

    pub(super) fn handle_thread_picker_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> std::io::Result<()> {
        let ctrl = modifiers.contains(KeyModifiers::CONTROL);
        let nav_down = code == KeyCode::Down || (ctrl && code == KeyCode::Char('j'));
        let nav_up = code == KeyCode::Up || (ctrl && code == KeyCode::Char('k'));

        if nav_down {
            if let Mode::ThreadPicker { state, .. } = &mut self.mode
                && !state.filtered.is_empty()
            {
                    state.cursor = (state.cursor + 1).min(state.filtered.len() - 1);
                }
        } else if nav_up {
            if let Mode::ThreadPicker { state, .. } = &mut self.mode {
                state.cursor = state.cursor.saturating_sub(1);
            }
        } else {
            match code {
                KeyCode::Esc => {
                    self.mode = Mode::Normal;
                }
                KeyCode::Enter => {
                    self.execute_move_session();
                }
                KeyCode::Backspace => {
                    if let Mode::ThreadPicker { state, .. } = &mut self.mode {
                        state.query.pop();
                        state.update_filter();
                    }
                }
                KeyCode::Char(c) => {
                    if let Mode::ThreadPicker { state, .. } = &mut self.mode {
                        state.query.push(c);
                        state.update_filter();
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub(super) fn execute_move_session(&mut self) {
        let old_mode = std::mem::replace(&mut self.mode, Mode::Normal);
        if let Mode::ThreadPicker {
            state,
            session_name,
            session_label,
        } = old_mode
            && let Some(&idx) = state.filtered.get(state.cursor)
        {
                let key = &state.all_entries[idx].0;
                let dest_display = state.all_entries[idx].1.clone();

                let parts: Vec<&str> = key.split(':').collect();
            if parts.len() != 2 {
                return;
            }
            let dest_col: usize = match parts[0].parse() {
                Ok(v) => v,
                Err(_) => return,
            };
            let dest_thread: usize = match parts[1].parse() {
                Ok(v) => v,
                Err(_) => return,
            };

            if let Some(new_tmux_name) =
                self.state
                    .make_session_name(dest_col, dest_thread, &session_label)
            {
                    if let Err(err) = mux::rename_session(&session_name, &new_tmux_name) {
                        self.set_error(format!("Failed to move session: {}", err));
                        return;
                    }
                    if self.marked_sessions.remove(&session_name) {
                        self.marked_sessions.insert(new_tmux_name.clone());
                    }
                    self.do_refresh_sessions();

                    if let Some(path) = self.state.session_tree_path(&new_tmux_name) {
                        self.select_tree_path_expanded(path);
                    }
                        self.set_flash(&format!("Session moved to {}", dest_display));
                }
            }
    }

    pub(super) fn confirm_input(&mut self, terminal: &mut Tui) -> std::io::Result<()> {
        // Take ownership of the mode to extract buffer and purpose
        let old_mode = std::mem::replace(&mut self.mode, Mode::Normal);
        if let Mode::Input { purpose, buffer } = old_mode {
            let trimmed = buffer.content.trim().to_string();
            if trimmed.is_empty() {
                return Ok(());
            }
            match purpose {
                InputPurpose::AddCollection => {
                    self.state.add_collection(trimmed);
                    self.save_state();
                    self.set_flash("Collection created");
                }
                InputPurpose::AddThread { collection_idx } => {
                    self.state.add_thread(collection_idx, trimmed);
                    // Auto-expand the collection so the new thread is visible
                    let col_id = self.state.collections[collection_idx].id.to_string();
                    self.tree_state.open(vec![col_id]);
                    self.save_state();
                    self.set_flash("Thread added");
                }
                InputPurpose::RenameCollection { idx } => {
                    // Collect old tmux session names before the rename changes the prefix.
                    let old_sessions: Vec<(String, String, usize)> = self.state.collections[idx]
                        .threads
                        .iter()
                        .enumerate()
                        .flat_map(|(pi, thread)| {
                            self.state
                                .sessions_for_thread(thread.id)
                                .into_iter()
                                .map(move |s| {
                                    (s.tmux_session_name.clone(), s.display_name.clone(), pi)
                                })
                        })
                        .collect();
                    self.state.rename_collection(idx, trimmed);
                    let mut rename_errors = Vec::new();
                    for (old_name, label, thread_idx) in &old_sessions {
                        if let Some(new_name) =
                            self.state.make_session_name(idx, *thread_idx, label)
                        {
                            match mux::rename_session(old_name, &new_name) {
                                Ok(()) => {
                                    if self.marked_sessions.remove(old_name) {
                                        self.marked_sessions.insert(new_name);
                                    }
                                }
                                Err(err) => rename_errors.push(format!("{}: {}", old_name, err)),
                            }
                        }
                    }
                    self.do_refresh_sessions();
                    self.save_state();
                    if rename_errors.is_empty() {
                        self.set_flash("Collection renamed");
                    } else {
                        self.set_error(format!(
                            "Collection renamed, but tmux rename failed:\n{}",
                            rename_errors.join("\n")
                        ));
                    }
                }
                InputPurpose::RenameThread {
                    col_idx,
                    thread_idx,
                } => {
                    // Collect old tmux session names before the rename changes the prefix.
                    let old_sessions: Vec<(String, String)> = self.state.collections[col_idx]
                        .threads
                        .get(thread_idx)
                        .map(|thread| {
                            self.state
                                .sessions_for_thread(thread.id)
                                .into_iter()
                                .map(|s| (s.tmux_session_name.clone(), s.display_name.clone()))
                                .collect()
                        })
                        .unwrap_or_default();
                    self.state.rename_thread(col_idx, thread_idx, trimmed);
                    let mut rename_errors = Vec::new();
                    for (old_name, label) in &old_sessions {
                        if let Some(new_name) =
                            self.state.make_session_name(col_idx, thread_idx, label)
                        {
                            match mux::rename_session(old_name, &new_name) {
                                Ok(()) => {
                                    if self.marked_sessions.remove(old_name) {
                                        self.marked_sessions.insert(new_name);
                                    }
                                }
                                Err(err) => rename_errors.push(format!("{}: {}", old_name, err)),
                            }
                        }
                    }
                    self.do_refresh_sessions();
                    self.save_state();
                    if rename_errors.is_empty() {
                        self.set_flash("Thread renamed");
                    } else {
                        self.set_error(format!(
                            "Thread renamed, but tmux rename failed:\n{}",
                            rename_errors.join("\n")
                        ));
                    }
                }
                InputPurpose::NewSession {
                    col_idx,
                    thread_idx,
                } => {
                    if let Some(session_name) =
                        self.state.make_session_name(col_idx, thread_idx, &trimmed)
                    {
                        let start_dir = match self.resolve_start_dir(col_idx, thread_idx) {
                            Ok(dir) => dir,
                            Err(msg) => {
                                self.set_error(msg);
                                return Ok(());
                            }
                        };
                        self.save_state();
                        if let Some(cwd) = start_dir {
                            self.launch_session_in_dir(&session_name, cwd, terminal)?;
                        } else {
                            self.launch_session(&session_name, terminal)?;
                        }
                        self.set_flash("Session launched");
                    }
                }
                InputPurpose::NewWorktree {
                    thread_id,
                    repo,
                    worktree_dir,
                } => {
                    let branch = trimmed;
                    let path = worktrees::worktree_path(&repo, worktree_dir.as_deref(), &branch);
                    if path.exists() {
                        self.set_error(format!("Path already exists: {}", path.display()));
                        return Ok(());
                    }
                    if self.pending_worktree_creates.contains_key(&path) {
                        self.set_flash("Worktree create already running");
                        return Ok(());
                    }
                    self.pending_worktree_creates
                        .insert(path.clone(), branch.clone());
                    self.set_flash("Creating worktree…");
                    let tx = self.worktree_create_tx.clone();
                    std::thread::spawn(move || {
                        let result = worktrees::add(&repo, &path, &branch);
                        let _ = tx.send(WorktreeCreateResult {
                            thread_id,
                            branch,
                            path,
                            result,
                        });
                    });
                }
                InputPurpose::RenameSession {
                    col_idx,
                    thread_idx,
                    old_tmux_name,
                } => {
                    if let Some(new_tmux_name) =
                        self.state.make_session_name(col_idx, thread_idx, &trimmed)
                    {
                        match mux::rename_session(&old_tmux_name, &new_tmux_name) {
                            Ok(()) => {
                                if self.marked_sessions.remove(&old_tmux_name) {
                                    self.marked_sessions.insert(new_tmux_name.clone());
                                }
                                self.do_refresh_sessions();
                                self.set_flash("Session renamed");
                            }
                            Err(err) => {
                                self.set_error(format!("Failed to rename session: {}", err));
                            }
                        }
                    }
                }
                InputPurpose::RenameAgent { pane_id } => {
                    if let Some(agent) = self
                        .state
                        .agent_sessions
                        .iter_mut()
                        .find(|a| a.pane_id == pane_id)
                    {
                        agent.display_name = trimmed;
                        agent.renamed = true;
                        self.set_flash("Agent renamed");
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn execute_confirm(&mut self) {
        let old_mode = std::mem::replace(&mut self.mode, Mode::Normal);
        if let Mode::Confirm { purpose } = old_mode {
            match purpose {
                ConfirmPurpose::DeleteCollection { idx, .. } => {
                    // Refresh first so active_sessions reflects any sessions created
                    // since the last 2-second tick.
                    self.do_refresh_sessions();
                    // Collect session names before deletion
                    let col = &self.state.collections[idx];
                    let mut session_names: Vec<String> = Vec::new();
                    for thread in &col.threads {
                        for s in self.state.sessions_for_thread(thread.id) {
                            session_names.push(s.tmux_session_name.clone());
                        }
                    }
                    let mut kill_errors = Vec::new();
                    for name in &session_names {
                        if let Err(err) = mux::kill_session(name) {
                            kill_errors.push(format!("{}: {}", name, err));
                        }
                        self.marked_sessions.remove(name);
                    }
                    self.state.delete_collection(idx);
                    // Select the item that slid into this position, or the one before
                    // it, rather than always jumping to the first collection.
                    let new_sel = self
                        .state
                        .collections
                        .get(idx)
                        .or_else(|| self.state.collections.last())
                        .map(|c| vec![c.id.to_string()])
                        .unwrap_or_default();
                    self.tree_state.select(new_sel);
                    self.save_state();
                    self.do_refresh_sessions();
                    if kill_errors.is_empty() {
                        self.set_flash("Collection deleted");
                    } else {
                        self.set_error(format!(
                            "Collection deleted, but killing sessions failed:\n{}",
                            kill_errors.join("\n")
                        ));
                    }
                    }
                ConfirmPurpose::DeleteThread {
                    col_idx,
                    thread_idx,
                    ..
                } => {
                    // Refresh first so active_sessions is current.
                    self.do_refresh_sessions();
                    let thread_id = self.state.collections[col_idx].threads[thread_idx].id;
                    let session_names: Vec<String> = self
                        .state
                        .sessions_for_thread(thread_id)
                        .iter()
                        .map(|s| s.tmux_session_name.clone())
                        .collect();
                    let mut kill_errors = Vec::new();
                    for name in session_names {
                        if let Err(err) = mux::kill_session(&name) {
                            kill_errors.push(format!("{}: {}", name, err));
                        }
                        self.marked_sessions.remove(&name);
                    }
                    self.state.delete_thread(col_idx, thread_idx);
                    // Select the thread that slid into this position, or the one
                    // before it, falling back to the collection itself.
                    let col = &self.state.collections[col_idx];
                    let new_sel = col
                        .threads
                        .get(thread_idx)
                        .or_else(|| col.threads.last())
                        .map(|p| vec![col.id.to_string(), p.id.to_string()])
                        .unwrap_or_else(|| vec![col.id.to_string()]);
                    self.tree_state.select(new_sel);
                    self.save_state();
                    self.do_refresh_sessions();
                    if kill_errors.is_empty() {
                        self.set_flash("Thread deleted");
                    } else {
                        self.set_error(format!(
                            "Thread deleted, but killing sessions failed:\n{}",
                            kill_errors.join("\n")
                        ));
                    }
                    }
                ConfirmPurpose::KillSession { session_name } => {
                    if let Err(err) = mux::kill_session(&session_name) {
                        self.set_error(format!("Failed to kill session: {}", err));
                        return;
                    }
                    self.marked_sessions.remove(&session_name);
                    self.do_refresh_sessions();
                    // Move selection up to the parent thread.
                    let parent: Vec<String> = self
                        .tree_state
                        .selected()
                        .iter()
                        .rev()
                        .skip(1)
                        .rev()
                        .cloned()
                        .collect();
                    self.tree_state.select(parent);
                    self.set_flash("Session killed");
                    }
                ConfirmPurpose::KillAllSessions {
                    col_idx,
                    thread_idx,
                    ..
                } => {
                    let thread_id = self.state.collections[col_idx].threads[thread_idx].id;
                    let names: Vec<String> = self
                        .state
                        .sessions_for_thread(thread_id)
                        .iter()
                        .map(|s| s.tmux_session_name.clone())
                        .collect();
                    let mut kill_errors = Vec::new();
                    for name in &names {
                        if let Err(err) = mux::kill_session(name) {
                            kill_errors.push(format!("{}: {}", name, err));
                        }
                        self.marked_sessions.remove(name);
                    }
                    self.do_refresh_sessions();
                    // Select the parent thread.
                    let col = &self.state.collections[col_idx];
                    let thread = &col.threads[thread_idx];
                    let thread_path = if col.is_root {
                        vec![thread.id.to_string()]
                    } else {
                        vec![col.id.to_string(), thread.id.to_string()]
                    };
                    self.tree_state.select(thread_path);
                    if kill_errors.is_empty() {
                        self.set_flash("All sessions killed");
                    } else {
                        self.set_error(format!(
                            "Killing sessions failed:\n{}",
                            kill_errors.join("\n")
                        ));
                    }
                    }
                ConfirmPurpose::KillMarkedSessions { session_names } => {
                    let count = session_names.len();
                    let mut kill_errors = Vec::new();
                    for name in &session_names {
                        if let Err(err) = mux::kill_session(name) {
                            kill_errors.push(format!("{}: {}", name, err));
                        }
                    }
                    self.marked_sessions.clear();
                    self.do_refresh_sessions();
                    if kill_errors.is_empty() {
                        self.set_flash(&format!("Killed {} sessions", count));
                    } else {
                        self.set_error(format!(
                            "Killing sessions failed:\n{}",
                            kill_errors.join("\n")
                        ));
                    }
                    }
                ConfirmPurpose::DeleteWorktree {
                    repo,
                    path,
                    name,
                    tmux_session_name,
                    kill_session,
                } => {
                    if self
                        .pending_worktree_deletes
                        .contains_key(&tmux_session_name)
                    {
                        self.set_flash("Worktree delete already running");
                        return;
                    }
                    let parent_selection = parent_selection_path(self.tree_state.selected());
                    self.pending_worktree_deletes.insert(
                        tmux_session_name.clone(),
                        PendingWorktreeDelete { parent_selection },
                    );
                    self.set_flash("Deleting worktree…");

                    let tx = self.worktree_delete_tx.clone();
                    std::thread::spawn(move || {
                        let result = worktrees::remove(&repo, &path);
                        if result.is_ok() && kill_session {
                            let _ = mux::kill_session(&tmux_session_name);
                        }
                        let _ = tx.send(WorktreeDeleteResult {
                            tmux_session_name,
                            name,
                            result,
                        });
                    });
                }
            }
        }
    }

    /// Resolve the configured start directory for a new ordinary session.
    pub(super) fn resolve_start_dir(
        &self,
        col_idx: usize,
        thread_idx: usize,
    ) -> Result<Option<PathBuf>, String> {
        let Some(col) = self.state.collections.get(col_idx) else {
            return Ok(None);
        };
        let Some(thread) = col.threads.get(thread_idx) else {
            return Ok(None);
        };
        let collection = if col.is_root {
            None
        } else {
            Some(col.name.as_str())
        };
        let Some(spec) =
            config::resolve_start_dir_match(&self.start_dir_configs, collection, &thread.name)
        else {
            return Ok(None);
        };

        let base = expand_home(spec.path());
        if !base.is_absolute() {
            return Err(format!("Start dir must be absolute: {}", spec.path()));
        }
        // For a collection/root default, prefer a `<base>/<thread>` subdir when present.
        let path = match spec {
            config::StartDirMatch::Default(_) => {
                config::auto_thread_dir(&base, &thread.name).unwrap_or(base)
            }
            config::StartDirMatch::Thread(_) => base,
        };
        if !path.is_dir() {
            return Err(format!("Start dir missing: {}", path.display()));
        }
        Ok(Some(path))
    }

    /// Launch a new tmux session with the given name and attach to it.
    pub(super) fn launch_session(
        &mut self,
        session_name: &str,
        terminal: &mut Tui,
    ) -> std::io::Result<()> {
        if let Err(err) = mux::new_session(session_name) {
            self.set_error(format!("Failed to create session: {}", err));
            return Ok(());
        }
        self.attach_to_session(session_name, terminal)
    }

    /// Launch a new tmux session in a worktree directory and attach to it.
    pub(super) fn launch_session_in_dir(
        &mut self,
        session_name: &str,
        cwd: PathBuf,
        terminal: &mut Tui,
    ) -> std::io::Result<()> {
        if let Err(err) = mux::new_session_in_dir(session_name, &cwd) {
            self.set_error(format!("Failed to create session: {}", err));
            return Ok(());
        }
        self.attach_to_session(session_name, terminal)
    }

    pub(super) fn open_session_or_worktree(
        &mut self,
        session_name: &str,
        terminal: &mut Tui,
    ) -> std::io::Result<()> {
        if self
            .state
            .active_sessions
            .iter()
            .any(|s| s.tmux_session_name == session_name)
        {
            return self.attach_to_session(session_name, terminal);
        }

        if let Some(worktree) = self.state.find_worktree_by_tmux_name(session_name) {
            if !worktree.launchable {
                self.set_flash("Worktree is prunable or missing");
                return Ok(());
            }
            return self.launch_session_in_dir(session_name, worktree.path.clone(), terminal);
        }

        Ok(())
    }

    /// Attach or switch to a multiplexer session by name.
    ///
    /// Latency invariant: after an external client exits, the retained TWS
    /// frame is rendered before backend discovery, worktree lookup, agent scan,
    /// or status reconciliation starts. Never call `do_refresh_sessions` here.
    pub(super) fn attach_to_session(
        &mut self,
        session_name: &str,
        terminal: &mut Tui,
    ) -> std::io::Result<()> {
        if mux::is_inside() {
            match mux::switch_client(session_name) {
                Ok(()) => self.running = false,
                Err(err) => {
                    self.set_error(format!("Failed to switch to session: {}", err));
                }
            }
            return Ok(());
        }

        // Outside the multiplexer: suspend TUI, attach (blocks), then resume.
        // Clear the main screen on both transitions so output left by prior
        // clients never flashes between alternate screens.
        tui::restore()?;
        tui::clear_main_screen()?;
        let attach_result = mux::attach_session(session_name);

        // Recover terminal ownership before interpreting the child outcome so
        // attach failures are shown inside a usable TWS screen.
        tui::clear_main_screen()?;
        *terminal = tui::init()?;

        match attach_result {
            Ok(true) => {}
            Ok(false) => self.set_error(format!(
                "Failed to attach to {} session: client exited unsuccessfully",
                mux::name()
            )),
            Err(err) => self.set_error(format!(
                "Failed to attach to {} session: {}",
                mux::name(), err
            )),
        }

        // First frame first: no subprocess or filesystem discovery may move
        // above this lifecycle boundary.
        self.needs_redraw = true;
        self.draw(terminal)?;
        self.needs_redraw = false;

        // Reconcile retained state only after the interface is visible again.
        self.request_refresh();
        Ok(())
    }

    pub(super) fn toggle_expand_all(&mut self) {
        let mut all_paths: Vec<Vec<String>> = Vec::new();

        for (col_idx, col) in self.state.collections.iter().enumerate() {
            if col.is_root {
                for (thread_idx, thread) in col.threads.iter().enumerate() {
                    if self.state.thread_is_hidden(col_idx, thread_idx) {
                        continue;
                    }
                    if self
                        .state
                        .active_sessions
                        .iter()
                        .any(|s| s.thread_id == thread.id)
                    {
                        all_paths.push(vec![thread.id.to_string()]);
                        // Also expand sessions that have agents
                        for session in &self.state.active_sessions {
                            if session.thread_id == thread.id
                                && !self
                                    .state
                                    .agents_for_session(&session.tmux_session_name)
                                    .is_empty()
                            {
                                all_paths.push(vec![
                                    thread.id.to_string(),
                                    session.tmux_session_name.clone(),
                                ]);
                            }
                        }
                    }
                }
            } else {
                if self.state.collection_is_hidden(col_idx) {
                    continue;
                }
                all_paths.push(vec![col.id.to_string()]);
                for (thread_idx, thread) in col.threads.iter().enumerate() {
                    if self.state.thread_is_hidden(col_idx, thread_idx) {
                        continue;
                    }
                    if self
                        .state
                        .active_sessions
                        .iter()
                        .any(|s| s.thread_id == thread.id)
                    {
                        all_paths.push(vec![col.id.to_string(), thread.id.to_string()]);
                        // Also expand sessions that have agents
                        for session in &self.state.active_sessions {
                            if session.thread_id == thread.id
                                && !self
                                    .state
                                    .agents_for_session(&session.tmux_session_name)
                                    .is_empty()
                            {
                                all_paths.push(vec![
                                    col.id.to_string(),
                                    thread.id.to_string(),
                                    session.tmux_session_name.clone(),
                                ]);
                            }
                        }
                    }
                }
            }
        }

        let all_open = !all_paths.is_empty()
            && all_paths
                .iter()
                .all(|p| self.tree_state.opened().contains(p));

        if all_open {
            self.tree_state.close_all();
        } else {
            for path in all_paths {
                self.tree_state.open(path);
            }
        }
    }
}
