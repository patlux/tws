use super::*;

impl App {
    /// Refresh the agent preview if an agent is selected and enough time has
    /// elapsed. Capture + ANSI parsing run on a background thread so a slow
    /// tmux server can't stall the UI.
    pub(super) fn refresh_preview(&mut self, selected: &SelectedItem) {
        if let SelectedItem::Agent(col_idx, thread_idx, sess_idx, agent_idx) = selected {
            if let Some(agent) =
                self.state
                    .resolve_agent(*col_idx, *thread_idx, *sess_idx, *agent_idx)
            {
                let session_name = agent.tmux_session_name.clone();
                let pane_id = agent.pane_id.clone();
                let pane_changed = self.preview_pane_id.as_deref() != Some(&pane_id);
                let needs_refresh =
                    pane_changed || self.last_preview_refresh.elapsed() >= PREVIEW_REFRESH_INTERVAL;
                if needs_refresh && !self.preview_in_flight {
                    self.preview_in_flight = true;
                    self.preview_pane_id = Some(pane_id.clone());
                    self.last_preview_refresh = Instant::now();
                    let tx = self.preview_tx.clone();
                    std::thread::spawn(move || {
                        if let Some(raw) = mux::capture_pane(&session_name, &pane_id)
                            && let Ok(mut text) = raw.as_bytes().into_text()
                        {
                                // Remap ANSI resets so the app
                                // background shows through, keeping the agent's real colors.
                                crate::core::ansi::clear_reset_backgrounds(&mut text);
                                let _ = tx.send(PreviewResult { pane_id, text });
                            }
                    });
                }
            }
        } else {
            if self.preview_content.is_some() {
                self.needs_redraw = true;
            }
            self.preview_content = None;
            self.preview_pane_id = None;
        }
    }

    pub(super) fn poll_preview_results(&mut self) {
        while let Ok(result) = self.preview_rx.try_recv() {
            self.preview_in_flight = false;
            // Drop captures for panes the user has already navigated away from.
            if self.preview_pane_id.as_deref() == Some(&result.pane_id) {
                self.preview_content = Some(result.text);
                self.needs_redraw = true;
            }
        }
    }

    pub(super) fn request_grid_refresh(&mut self) {
        if self.grid_refresh_in_flight || self.last_grid_refresh.elapsed() < GRID_REFRESH_INTERVAL {
            return;
        }
        let agents = self.state.all_agents_flat();
        if agents.is_empty() {
            if !self.grid_captures.is_empty() {
                self.grid_captures.clear();
                self.needs_redraw = true;
            }
            return;
        }

        self.grid_refresh_in_flight = true;
        self.last_grid_refresh = Instant::now();
        let panes: Vec<(String, String)> = agents
            .into_iter()
            .map(|agent| (agent.tmux_session_name, agent.pane_id))
            .collect();
        let tx = self.grid_preview_tx.clone();
        std::thread::spawn(move || {
            let mut captures = HashMap::new();
            for (session_name, pane_id) in panes {
                if let Some(raw) = mux::capture_pane(&session_name, &pane_id)
                    && let Ok(mut text) = raw.as_bytes().into_text()
                {
                    crate::core::ansi::clear_reset_backgrounds(&mut text);
                    captures.insert(pane_id, text);
                }
            }
            let _ = tx.send(GridPreviewResult { captures });
        });
    }

    pub(super) fn poll_grid_preview_results(&mut self) {
        while let Ok(result) = self.grid_preview_rx.try_recv() {
            self.grid_refresh_in_flight = false;
            self.grid_captures = result.captures;
            self.needs_redraw = true;
        }
    }

    /// Synchronous refresh — used right after user actions (kill, rename,
    /// attach) where the very next frame must reflect the new tmux state.
    /// Bumps the epoch so any in-flight background refresh gets discarded.
    pub(super) fn do_refresh_sessions(&mut self) {
        self.refresh_epoch += 1;
        self.refresh_worktree_sessions();
        let live = mux::list_managed_sessions_with_timestamps();
        self.state.refresh_sessions(&live);
        self.prune_marked_sessions();
        self.last_refresh = Instant::now();
        self.do_agent_scan();
        self.needs_redraw = true;
    }

    /// Kick off a full refresh (tmux sessions + git worktrees + agent scan +
    /// pi statuses) on a background thread. Results land in poll_refresh_results.
    pub(super) fn request_refresh(&mut self) {
        self.refresh_in_flight = true;
        self.last_refresh = Instant::now();
        let epoch = self.refresh_epoch;
        let tx = self.refresh_tx.clone();
        let worktree_jobs = self.expanded_worktree_jobs();
        let status_dir = persistence::config_dir().join("pi-status");
        let trigger_path = persistence::config_dir().join("agent.trigger");
        std::thread::spawn(move || {
            let live = mux::list_managed_sessions_with_timestamps();
            let discoveries: Vec<(
                uuid::Uuid,
                PathBuf,
                DiscoverOptions,
                Vec<worktrees::DiscoveredWorktree>,
            )> = worktree_jobs
                    .into_iter()
                    .map(|(thread_id, repo, options)| {
                        let wts = worktrees::discover(&repo, options).unwrap_or_default();
                        (thread_id, repo, options, wts)
                    })
                    .collect();
            // Scan all live tws sessions (superset of the matched ones);
            // apply_agent_scan filters down to active sessions.
            let session_names: Vec<String> = live.iter().map(|(n, _)| n.clone()).collect();
            let agents = mux::scan_agents(&session_names);
            let live_panes: HashSet<String> = agents.iter().map(|a| a.pane_id.clone()).collect();
            let pi_statuses = pi_status::load_and_prune(
                &status_dir,
                &live_panes,
                PI_STATUS_MAX_AGE,
                mux::backend(),
            );
            let trigger_mtime = std::fs::metadata(&trigger_path)
                .and_then(|m| m.modified())
                .ok();
            let _ = tx.send(RefreshResult {
                epoch,
                sessions: Some(SessionsPayload { live, discoveries }),
                agents,
                pi_statuses,
                trigger_mtime,
            });
        });
    }

    /// Kick off an agent-only scan on a background thread (hook-triggered).
    pub(super) fn request_agent_scan(&mut self) {
        self.scan_in_flight = true;
        self.last_trigger_scan = Instant::now();
        let epoch = self.refresh_epoch;
        let tx = self.refresh_tx.clone();
        let session_names: Vec<String> = self
            .state
            .active_sessions
            .iter()
            .map(|s| s.tmux_session_name.clone())
            .collect();
        let status_dir = persistence::config_dir().join("pi-status");
        let trigger_path = persistence::config_dir().join("agent.trigger");
        std::thread::spawn(move || {
            let agents = mux::scan_agents(&session_names);
            let live_panes: HashSet<String> = agents.iter().map(|a| a.pane_id.clone()).collect();
            let pi_statuses = pi_status::load_and_prune(
                &status_dir,
                &live_panes,
                PI_STATUS_MAX_AGE,
                mux::backend(),
            );
            let trigger_mtime = std::fs::metadata(&trigger_path)
                .and_then(|m| m.modified())
                .ok();
            let _ = tx.send(RefreshResult {
                epoch,
                sessions: None,
                agents,
                pi_statuses,
                trigger_mtime,
            });
        });
    }

    /// Apply background refresh/scan results on the main thread.
    pub(super) fn poll_refresh_results(&mut self) {
        while let Ok(result) = self.refresh_rx.try_recv() {
            if result.sessions.is_some() {
                self.refresh_in_flight = false;
            } else {
                self.scan_in_flight = false;
            }
            // Stale: a synchronous refresh ran after this was requested.
            if result.epoch != self.refresh_epoch {
                continue;
            }
            if let Some(payload) = result.sessions {
                self.apply_worktree_discoveries(payload.discoveries);
                self.state.refresh_sessions(&payload.live);
                self.prune_marked_sessions();
            }
            self.apply_agent_scan(result.agents, result.pi_statuses, result.trigger_mtime);
            // Re-apply the persisted selection once real session data exists.
            if let Some(sel) = self.pending_selection_restore.take() {
                self.tree_state.select(sel);
                self.ensure_visible_tree_selection();
            }
            self.needs_redraw = true;
        }
    }

    /// Worktree discovery jobs for all expanded, configured threads.
    ///
    /// Explicit `[[worktrees]]` entries come first. Then, for any expanded
    /// thread without an explicit entry whose auto start-dir subdirectory is a
    /// git repo, an auto job with default discovery options is appended.
    pub(super) fn expanded_worktree_jobs(&self) -> Vec<(uuid::Uuid, PathBuf, DiscoverOptions)> {
        let mut jobs: Vec<(uuid::Uuid, PathBuf, DiscoverOptions)> = Vec::new();
        let mut covered: HashSet<uuid::Uuid> = HashSet::new();

        for cfg in &self.worktree_configs {
            let Some((col_idx, thread_idx)) = self.find_thread_for_worktree_config(cfg) else {
                continue;
            };
            let thread_id = self.state.collections[col_idx].threads[thread_idx].id;
            covered.insert(thread_id);
            if !self.is_worktree_thread_expanded(col_idx, thread_idx) {
                continue;
            }
            let repo = expand_home(&cfg.repo);
            let options = DiscoverOptions {
                include_main: cfg.include_main(),
                include_detached: cfg.include_detached(),
                skip_prunable: cfg.skip_prunable(),
            };
            jobs.push((thread_id, repo, options));
        }

        for (col_idx, col) in self.state.collections.iter().enumerate() {
            for (thread_idx, thread) in col.threads.iter().enumerate() {
                if covered.contains(&thread.id)
                    || !self.is_worktree_thread_expanded(col_idx, thread_idx)
                {
                    continue;
                }
                if let Some(repo) = self.auto_worktree_repo(col_idx, thread_idx) {
                    jobs.push((thread.id, repo, DiscoverOptions::default()));
                }
            }
        }

        jobs
    }

    /// Resolve an auto worktree repo for a thread: a collection/root default
    /// `[[start_dirs]]` whose `<path>/<thread>` subdirectory exists and is a
    /// git repository. Returns `None` for thread-specific start dirs.
    pub(super) fn auto_worktree_repo(&self, col_idx: usize, thread_idx: usize) -> Option<PathBuf> {
        let col = self.state.collections.get(col_idx)?;
        let thread = col.threads.get(thread_idx)?;
        let collection = (!col.is_root).then_some(col.name.as_str());
        let spec =
            config::resolve_start_dir_match(&self.start_dir_configs, collection, &thread.name)?;
        let config::StartDirMatch::Default(path) = spec else {
            return None;
        };
        let dir = config::auto_thread_dir(&expand_home(path), &thread.name)?;
        dir.join(".git").exists().then_some(dir)
    }

    /// Synchronous worktree refresh — used when the user expands/collapses
    /// a thread and the next frame must show its worktrees. Serves cached
    /// discoveries when fresh enough so keypresses don't shell out to git.
    pub(super) fn refresh_worktree_sessions(&mut self) {
        let jobs = self.expanded_worktree_jobs();
        if jobs.is_empty() {
            self.state.refresh_worktree_sessions(Vec::new());
            return;
        }
        let discoveries = jobs
            .into_iter()
            .map(|(thread_id, repo, options)| {
                let cached = self
                    .worktree_cache
                    .get(&(repo.clone(), options))
                    .filter(|(at, _)| at.elapsed() < WORKTREE_CACHE_TTL)
                    .map(|(_, wts)| wts.clone());
                let wts = cached
                    .unwrap_or_else(|| worktrees::discover(&repo, options).unwrap_or_default());
                (thread_id, repo, options, wts)
            })
            .collect();
        self.apply_worktree_discoveries(discoveries);
    }

    pub(super) fn find_thread_indices_by_id(
        &self,
        thread_id: uuid::Uuid,
    ) -> Option<(usize, usize)> {
        self.state
            .collections
            .iter()
            .enumerate()
            .find_map(|(ci, col)| {
                col.threads
                    .iter()
                    .position(|t| t.id == thread_id)
                    .map(|ti| (ci, ti))
        })
    }

    pub(super) fn apply_worktree_discoveries(
        &mut self,
        discoveries: Vec<(
            uuid::Uuid,
            PathBuf,
            DiscoverOptions,
            Vec<worktrees::DiscoveredWorktree>,
        )>,
    ) {
        let mut sessions = Vec::new();
        let mut used_names = HashSet::new();

        for (thread_id, repo, options, worktrees) in discoveries {
            self.worktree_cache
                .insert((repo.clone(), options), (Instant::now(), worktrees.clone()));
            // Threads may have been renamed/deleted while discovery ran.
            let Some((col_idx, thread_idx)) = self.find_thread_indices_by_id(thread_id) else {
                continue;
            };
            for wt in worktrees {
                let mut label = slugify(&wt.label_source());
                if label.is_empty() {
                    label = wt
                        .head
                        .as_deref()
                        .map(short_head)
                        .unwrap_or("worktree")
                        .to_string();
                }
                let mut session_name =
                    match self.state.make_session_name(col_idx, thread_idx, &label) {
                    Some(name) => name,
                    None => continue,
                };
                if used_names.contains(&session_name)
                    && let Some(head) = wt.head.as_deref()
                {
                        label = format!("{}-{}", label, short_head(head));
                        if let Some(name) = self.state.make_session_name(col_idx, thread_idx, &label) {
                            session_name = name;
                        }
                    }
                if !used_names.insert(session_name.clone()) {
                    continue;
                }
                let path_exists = wt.path.is_dir();
                let launchable = path_exists && !wt.prunable;
                sessions.push(WorktreeSession {
                    tmux_session_name: session_name,
                    display_name: label,
                    thread_id,
                    repo: repo.clone(),
                    path: wt.path,
                    branch: wt.branch,
                    head: wt.head,
                    prunable: wt.prunable,
                    is_main: wt.is_main,
                    path_exists,
                    launchable,
                });
            }
        }

        self.state.refresh_worktree_sessions(sessions);
    }

    pub(super) fn is_worktree_thread_expanded(&self, col_idx: usize, thread_idx: usize) -> bool {
        let Some(col) = self.state.collections.get(col_idx) else {
            return false;
        };
        let Some(thread) = col.threads.get(thread_idx) else {
            return false;
        };
        let path = if col.is_root {
            vec![thread.id.to_string()]
        } else {
            vec![col.id.to_string(), thread.id.to_string()]
        };
        self.tree_state.opened().contains(&path)
    }

    pub(super) fn find_thread_for_worktree_config(
        &self,
        cfg: &WorktreeConfig,
    ) -> Option<(usize, usize)> {
        self.state
            .collections
            .iter()
            .enumerate()
            .find_map(|(col_idx, col)| {
                let collection_matches = match cfg.collection.as_deref() {
                    Some(name) => !col.is_root && col.name == name,
                    None => col.is_root,
                };
                if !collection_matches {
                    return None;
                }
                let thread_idx = col
                    .threads
                    .iter()
                    .position(|thread| thread.name == cfg.thread)?;
                Some((col_idx, thread_idx))
            })
    }

    pub(super) fn check_agent_trigger(&self) -> bool {
        let path = persistence::config_dir().join("agent.trigger");
        let mtime = match std::fs::metadata(&path).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => return false,
        };
        match self.last_agent_trigger_mtime {
            Some(last) => mtime > last,
            None => true,
        }
    }

    /// Synchronous agent scan (used by do_refresh_sessions after user actions).
    pub(super) fn do_agent_scan(&mut self) {
        if self.state.active_sessions.is_empty() {
            self.state.agent_sessions.clear();
            self.state.pi_statuses.clear();
            return;
        }

        let session_names: Vec<String> = self
            .state
            .active_sessions
            .iter()
            .map(|s| s.tmux_session_name.clone())
            .collect();
        let agents = mux::scan_agents(&session_names);

        let status_dir = persistence::config_dir().join("pi-status");
        let live_panes: HashSet<String> = agents.iter().map(|a| a.pane_id.clone()).collect();
        let pi_statuses =
            pi_status::load_and_prune(&status_dir, &live_panes, PI_STATUS_MAX_AGE, mux::backend());

        let trigger_path = persistence::config_dir().join("agent.trigger");
        let trigger_mtime = std::fs::metadata(&trigger_path)
            .and_then(|m| m.modified())
            .ok();

        self.apply_agent_scan(agents, pi_statuses, trigger_mtime);
    }

    /// Merge scanned agents into state, preserving renames/pins and applying
    /// the one-shot pin restore. Shared by the sync and async scan paths.
    pub(super) fn apply_agent_scan(
        &mut self,
        scanned: Vec<AgentSession>,
        pi_statuses: Vec<pi_status::PiStatus>,
        trigger_mtime: Option<SystemTime>,
    ) {
        if self.state.active_sessions.is_empty() {
            self.state.agent_sessions.clear();
            self.state.pi_statuses.clear();
            self.last_agent_trigger_mtime = trigger_mtime;
            return;
        }

        struct Preserved {
            custom_name: Option<String>,
            pin_slot: Option<u8>,
        }
        let snapshot: HashMap<String, Preserved> = self
            .state
            .agent_sessions
            .iter()
            .map(|a| {
                (
                    a.pane_id.clone(),
                    Preserved {
                        custom_name: if a.renamed {
                            Some(a.display_name.clone())
                        } else {
                            None
                        },
                        pin_slot: a.pin_slot,
                    },
                )
            })
            .collect();

        // Async scans may cover a superset of sessions; keep only agents in
        // sessions tws currently tracks.
        let active: HashSet<&str> = self
            .state
            .active_sessions
            .iter()
            .map(|s| s.tmux_session_name.as_str())
            .collect();
        self.state.agent_sessions = scanned
            .into_iter()
            .filter(|a| active.contains(a.tmux_session_name.as_str()))
            .collect();

        for agent in &mut self.state.agent_sessions {
            if let Some(prev) = snapshot.get(&agent.pane_id) {
                if let Some(name) = &prev.custom_name {
                    agent.display_name = name.clone();
                    agent.renamed = true;
                }
                agent.pin_slot = prev.pin_slot;
            }
        }

        // Reapply pins persisted from the previous session, one-shot. Drained on first match attempt.
        // Pins whose pane_id is no longer live get silently dropped (pin dies with the pane).
        if !self.pending_pin_restore.is_empty() {
            let restore: Vec<(String, u8)> = std::mem::take(&mut self.pending_pin_restore);
            for (pane_id, slot) in restore {
                if let Some(agent) = self
                    .state
                    .agent_sessions
                    .iter_mut()
                    .find(|a| a.pane_id == pane_id)
                {
                    agent.pin_slot = Some(slot);
                }
            }
        }

        self.state.pi_statuses = pi_statuses;
        self.last_agent_trigger_mtime = trigger_mtime;
    }
}
