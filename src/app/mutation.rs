use super::*;

/// Main-thread state update that follows a successful transactional rename.
pub(super) enum RenameFollowUp {
    Collection {
        idx: usize,
        new_name: String,
    },
    Thread {
        col_idx: usize,
        thread_idx: usize,
        new_name: String,
    },
    Session,
    Move {
        destination: String,
        dest_col: usize,
        dest_thread: usize,
        new_session_name: String,
    },
}

/// Main-thread state/selection update that follows a kill batch.
pub(super) enum KillFollowUp {
    DeleteCollection { idx: usize },
    DeleteThread { col_idx: usize, thread_idx: usize },
    SelectThread { col_idx: usize, thread_idx: usize },
    SelectParent { path: Vec<String> },
    Marked,
}

/// Backend-only work. All variants run on a worker thread; the UI thread only
/// applies the returned state/selection follow-up.
pub(super) enum MutationJob {
    /// Rename every pair as one logical transaction. On the first failure,
    /// already-applied pairs are renamed back before the result is delivered.
    Rename {
        pairs: Vec<(String, String)>,
        follow_up: RenameFollowUp,
    },
    /// Kill an explicit set of session names.
    Kill {
        names: Vec<String>,
        follow_up: KillFollowUp,
    },
    /// Refresh the backend session list in the worker, select names matching
    /// any managed prefix, then kill them. Used for collection/thread deletes
    /// so sessions created since the last UI refresh are included.
    KillByPrefix {
        prefixes: Vec<String>,
        follow_up: KillFollowUp,
    },
}

pub(super) struct MutationOutcome {
    job: MutationJob,
    renamed: Vec<(String, String)>,
    killed: Vec<String>,
    errors: Vec<String>,
    rolled_back: bool,
    /// Prefix discovery failed; destructive hierarchy deletion must not run.
    aborted: bool,
}

fn session_matches_prefix(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('_'))
        .is_some_and(|label| !label.is_empty())
}

fn kill_names(names: &[String]) -> (Vec<String>, Vec<String>) {
    let mut killed = Vec::new();
    let mut errors = Vec::new();
    for name in names {
        match mux::kill_session(name) {
            Ok(()) => killed.push(name.clone()),
            Err(err) => errors.push(format!("{}: {}", name, err)),
        }
    }
    (killed, errors)
}

fn rename_transaction<F>(
    pairs: &[(String, String)],
    mut rename: F,
) -> (Vec<(String, String)>, Vec<String>, bool)
where
    F: FnMut(&str, &str) -> Result<(), String>,
{
    let mut renamed: Vec<(String, String)> = Vec::new();
    let mut errors = Vec::new();
    let mut rolled_back = false;

    for (old_name, new_name) in pairs {
        match rename(old_name, new_name) {
            Ok(()) => renamed.push((old_name.clone(), new_name.clone())),
            Err(err) => {
                errors.push(format!("{}: {}", old_name, err));
                rolled_back = true;
                // Reverse order is safest if names overlap in a future naming
                // scheme. Hierarchy state is still unchanged.
                let mut rollback_failed = false;
                for (done_old, done_new) in renamed.iter().rev() {
                    if let Err(rollback_err) = rename(done_new, done_old) {
                        rollback_failed = true;
                        errors.push(format!(
                            "rollback {} → {} failed: {}",
                            done_new, done_old, rollback_err
                        ));
                    }
                }
                if rollback_failed {
                    errors.push(
                        "Backend rollback was incomplete; inspect live session names before retrying"
                            .to_string(),
                    );
                }
                break;
            }
        }
    }

    if rolled_back {
        // Do not migrate UI paths: hierarchy names were not committed.
        renamed.clear();
    }
    (renamed, errors, rolled_back)
}

fn run_mutation(job: MutationJob) -> MutationOutcome {
    match &job {
        MutationJob::Rename { pairs, .. } => {
            let (renamed, errors, rolled_back) = rename_transaction(pairs, mux::rename_session);
            MutationOutcome {
                job,
                renamed,
                killed: Vec::new(),
                errors,
                rolled_back,
                aborted: false,
            }
        }
        MutationJob::Kill { names, .. } => {
            let (killed, errors) = kill_names(names);
            MutationOutcome {
                job,
                renamed: Vec::new(),
                killed,
                errors,
                rolled_back: false,
                aborted: false,
            }
        }
        MutationJob::KillByPrefix { prefixes, .. } => {
            let live = match mux::list_managed_sessions_with_timestamps() {
                Ok(live) => live,
                Err(err) => {
                    return MutationOutcome {
                        job,
                        renamed: Vec::new(),
                        killed: Vec::new(),
                        errors: vec![format!("could not list live sessions: {}", err)],
                        rolled_back: false,
                        aborted: true,
                    };
                }
            };
            let names: Vec<String> = live
                .into_iter()
                .map(|(name, _)| name)
                .filter(|name| {
                    prefixes
                        .iter()
                        .any(|prefix| session_matches_prefix(name, prefix))
                })
                .collect();
            let (killed, errors) = kill_names(&names);
            MutationOutcome {
                job,
                renamed: Vec::new(),
                killed,
                errors,
                rolled_back: false,
                aborted: false,
            }
        }
    }
}

fn migrate_path(path: &mut [String], renames: &HashMap<&str, &str>) {
    for segment in path {
        if let Some(new_name) = renames.get(segment.as_str()) {
            *segment = (*new_name).to_string();
        }
    }
}

impl App {
    pub(super) fn start_mutation(&mut self, label: &str, job: MutationJob) {
        if self.pending_mutation.is_some() {
            self.set_flash("Another backend operation is still running");
            return;
        }
        self.pending_mutation = Some(label.to_string());
        self.set_flash(label);
        self.needs_redraw = true;
        let tx = self.mutation_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(run_mutation(job));
        });
    }

    pub(super) fn poll_mutation_results(&mut self) {
        while let Ok(outcome) = self.mutation_rx.try_recv() {
            self.pending_mutation = None;
            self.needs_redraw = true;
            self.tree_cache_dirty = true;
            self.flat_agents_cache_dirty = true;

            let MutationOutcome {
                job,
                renamed,
                killed,
                errors,
                rolled_back,
                aborted,
            } = outcome;

            for (old_name, new_name) in &renamed {
                if self.marked_sessions.remove(old_name) {
                    self.marked_sessions.insert(new_name.clone());
                }
            }
            for name in &killed {
                self.marked_sessions.remove(name);
            }
            if !renamed.is_empty() {
                self.migrate_session_paths(&renamed);
            }

            match job {
                MutationJob::Rename { follow_up, .. } => {
                    self.finish_rename(follow_up, &errors, rolled_back)
                }
                MutationJob::Kill { follow_up, .. }
                | MutationJob::KillByPrefix { follow_up, .. } => {
                    self.finish_kill(follow_up, &killed, &errors, aborted)
                }
            }

            // Latest-wins refresh supersedes any result collected before the
            // backend mutation and reconciles the completed/partially failed
            // operation entirely off-thread.
            self.request_refresh();
        }
    }

    fn finish_rename(&mut self, follow_up: RenameFollowUp, errors: &[String], rolled_back: bool) {
        if rolled_back {
            let prefix = match follow_up {
                RenameFollowUp::Session => "Failed to rename session",
                RenameFollowUp::Move { .. } => "Failed to move session",
                RenameFollowUp::Collection { .. } | RenameFollowUp::Thread { .. } => {
                    "Rename failed; hierarchy was not changed and successful backend renames were rolled back"
                }
            };
            self.set_error(format!("{}:\n{}", prefix, errors.join("\n")));
            return;
        }

        match follow_up {
            RenameFollowUp::Collection { idx, new_name } => {
                self.state.rename_collection(idx, new_name);
                self.save_state();
                self.set_flash("Collection renamed");
            }
            RenameFollowUp::Thread {
                col_idx,
                thread_idx,
                new_name,
            } => {
                self.state.rename_thread(col_idx, thread_idx, new_name);
                self.save_state();
                self.set_flash("Thread renamed");
            }
            RenameFollowUp::Session => self.set_flash("Session renamed"),
            RenameFollowUp::Move {
                destination,
                dest_col,
                dest_thread,
                new_session_name,
            } => {
                self.relocate_moved_session_path(&new_session_name, dest_col, dest_thread);
                self.set_flash(&format!("Session moved to {}", destination));
            }
        }
    }

    fn finish_kill(
        &mut self,
        follow_up: KillFollowUp,
        killed: &[String],
        errors: &[String],
        aborted: bool,
    ) {
        if aborted {
            self.set_error(format!(
                "Backend discovery failed; nothing was deleted:\n{}",
                errors.join("\n")
            ));
            return;
        }
        if !errors.is_empty()
            && matches!(
                follow_up,
                KillFollowUp::DeleteCollection { .. } | KillFollowUp::DeleteThread { .. }
            )
        {
            // Keep the hierarchy entry so any sessions that failed to die
            // remain managed/visible rather than becoming hidden orphans.
            self.set_error(format!(
                "Some sessions could not be killed; hierarchy was not deleted:\n{}",
                errors.join("\n")
            ));
            return;
        }

        match follow_up {
            KillFollowUp::DeleteCollection { idx } => {
                if idx >= self.state.collections.len() {
                    self.set_error("Collection disappeared while deletion was running");
                    return;
                }
                self.state.delete_collection(idx);
                let new_selection = self
                    .state
                    .collections
                    .get(idx)
                    .or_else(|| self.state.collections.last())
                    .map(|collection| vec![collection.id.to_string()])
                    .unwrap_or_default();
                self.tree_state.select(new_selection);
                self.ui_dirty = true;
                self.save_state();
                if errors.is_empty() {
                    self.set_flash("Collection deleted");
                } else {
                    self.set_error(format!(
                        "Collection deleted, but killing sessions failed:\n{}",
                        errors.join("\n")
                    ));
                }
            }
            KillFollowUp::DeleteThread {
                col_idx,
                thread_idx,
            } => {
                let valid = self
                    .state
                    .collections
                    .get(col_idx)
                    .is_some_and(|collection| thread_idx < collection.threads.len());
                if !valid {
                    self.set_error("Thread disappeared while deletion was running");
                    return;
                }
                self.state.delete_thread(col_idx, thread_idx);
                let collection = &self.state.collections[col_idx];
                let new_selection = collection
                    .threads
                    .get(thread_idx)
                    .or_else(|| collection.threads.last())
                    .map(|thread| {
                        if collection.is_root {
                            vec![thread.id.to_string()]
                        } else {
                            vec![collection.id.to_string(), thread.id.to_string()]
                        }
                    })
                    .unwrap_or_else(|| {
                        if collection.is_root {
                            Vec::new()
                        } else {
                            vec![collection.id.to_string()]
                        }
                    });
                self.tree_state.select(new_selection);
                self.ui_dirty = true;
                self.save_state();
                if errors.is_empty() {
                    self.set_flash("Thread deleted");
                } else {
                    self.set_error(format!(
                        "Thread deleted, but killing sessions failed:\n{}",
                        errors.join("\n")
                    ));
                }
            }
            KillFollowUp::SelectThread {
                col_idx,
                thread_idx,
            } => {
                if let Some(collection) = self.state.collections.get(col_idx)
                    && let Some(thread) = collection.threads.get(thread_idx)
                {
                    let path = if collection.is_root {
                        vec![thread.id.to_string()]
                    } else {
                        vec![collection.id.to_string(), thread.id.to_string()]
                    };
                    self.tree_state.select(path);
                    self.ui_dirty = true;
                }
                if errors.is_empty() {
                    self.set_flash("All sessions killed");
                } else {
                    self.set_error(format!("Killing sessions failed:\n{}", errors.join("\n")));
                }
            }
            KillFollowUp::SelectParent { path } => {
                if errors.is_empty() {
                    self.tree_state.select(path);
                    self.ui_dirty = true;
                    self.set_flash("Session killed");
                } else {
                    // Keep the visual selection on the still-live session.
                    self.set_error(format!("Failed to kill session:\n{}", errors.join("\n")));
                }
            }
            KillFollowUp::Marked => {
                if errors.is_empty() {
                    self.set_flash(&format!("Killed {} sessions", killed.len()));
                } else {
                    self.set_error(format!(
                        "Killed {}, but some sessions failed:\n{}",
                        killed.len(),
                        errors.join("\n")
                    ));
                }
            }
        }
    }

    /// After a move, the session-name segment changed *and* its UUID parent
    /// path changed. Relocate stale opened/selected paths only if they still
    /// point at that session; never yank a user who navigated elsewhere while
    /// the async operation was running.
    pub(super) fn relocate_moved_session_path(
        &mut self,
        session_name: &str,
        dest_col: usize,
        dest_thread: usize,
    ) {
        let Some(collection) = self.state.collections.get(dest_col) else {
            return;
        };
        let Some(thread) = collection.threads.get(dest_thread) else {
            return;
        };
        let mut destination = if collection.is_root {
            vec![thread.id.to_string()]
        } else {
            vec![collection.id.to_string(), thread.id.to_string()]
        };
        destination.push(session_name.to_string());

        let selected_follows_session = self
            .tree_state
            .selected()
            .iter()
            .any(|segment| segment == session_name);
        let opened: Vec<Vec<String>> = self.tree_state.opened().iter().cloned().collect();
        let session_was_open = opened
            .iter()
            .any(|path| path.iter().any(|segment| segment == session_name));
        self.tree_state.close_all();
        for path in opened
            .into_iter()
            .filter(|path| !path.iter().any(|segment| segment == session_name))
        {
            self.tree_state.open(path);
        }
        if session_was_open {
            self.tree_state.open(destination.clone());
        }
        if selected_follows_session {
            self.select_tree_path_expanded(destination);
        }
        self.ui_dirty = true;
    }

    /// Rewrite selected and opened tree paths after successful backend session
    /// renames. Collection/thread UUID segments remain unchanged; only exact
    /// session-name segments are replaced.
    pub(super) fn migrate_session_paths(&mut self, renames: &[(String, String)]) {
        let map: HashMap<&str, &str> = renames
            .iter()
            .map(|(old_name, new_name)| (old_name.as_str(), new_name.as_str()))
            .collect();

        let mut opened: Vec<Vec<String>> = self.tree_state.opened().iter().cloned().collect();
        for path in &mut opened {
            migrate_path(path, &map);
        }
        self.tree_state.close_all();
        for path in opened {
            self.tree_state.open(path);
        }

        let mut selected = self.tree_state.selected().to_vec();
        if !selected.is_empty() {
            migrate_path(&mut selected, &map);
            self.tree_state.select(selected);
        }
        self.ui_dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_prefix_matching_requires_nonempty_label() {
        assert!(session_matches_prefix(
            "tws_work_thread_main",
            "tws_work_thread"
        ));
        assert!(!session_matches_prefix(
            "tws_work_thread",
            "tws_work_thread"
        ));
        assert!(!session_matches_prefix(
            "tws_work_threadish_main",
            "tws_work_thread"
        ));
    }

    #[test]
    fn rename_transaction_rolls_back_completed_pairs_on_failure() {
        let pairs = vec![
            ("old-a".to_string(), "new-a".to_string()),
            ("old-b".to_string(), "new-b".to_string()),
        ];
        let mut calls = Vec::new();
        let (renamed, errors, rolled_back) = rename_transaction(&pairs, |old, new| {
            calls.push((old.to_string(), new.to_string()));
            if old == "old-b" {
                Err("backend failure".to_string())
            } else {
                Ok(())
            }
        });

        assert!(rolled_back);
        assert!(renamed.is_empty());
        assert_eq!(errors.len(), 1);
        assert_eq!(
            calls,
            vec![
                ("old-a".into(), "new-a".into()),
                ("old-b".into(), "new-b".into()),
                ("new-a".into(), "old-a".into()),
            ]
        );
    }

    #[test]
    fn migrate_path_rewrites_only_exact_session_segments() {
        let map = HashMap::from([("tws_old", "tws_new")]);
        let mut path = vec![
            "collection-id".to_string(),
            "thread-id".to_string(),
            "tws_old".to_string(),
            "%1".to_string(),
        ];
        migrate_path(&mut path, &map);
        assert_eq!(path[2], "tws_new");
        assert_eq!(path[0], "collection-id");
        assert_eq!(path[3], "%1");
    }
}
