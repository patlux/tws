use uuid::Uuid;

use super::model::{AgentSession, AgentType, Collection, Session, Thread, WorktreeSession};
use super::pi_status::{PiIndicator, PiStatus, PiWorkState};
use crate::naming::BackendKind;

pub struct AppState {
    pub collections: Vec<Collection>,
    pub backend: BackendKind,
    /// Runtime-only: live tmux sessions managed by tws. Never persisted.
    pub active_sessions: Vec<Session>,
    /// Runtime-only: Git worktrees that can be launched as tws sessions. Never persisted.
    pub worktree_sessions: Vec<WorktreeSession>,
    /// Runtime-only: AI agents detected in tmux panes. Never persisted.
    pub agent_sessions: Vec<AgentSession>,
    /// Runtime-only: Pi work statuses read from `~/.config/tws/pi-status/`. Never persisted.
    pub pi_statuses: Vec<PiStatus>,
    /// When true, threads without live sessions (and their worktrees, and
    /// collections left empty) are treated as hidden. Persisted in UiState.
    pub active_filter: bool,
}

/// A single agent flattened out of the collection/thread/session hierarchy.
/// Carries both display strings and the index tuple needed to produce `SelectedItem::Agent`.
pub struct FlatAgent {
    pub col_idx: usize,
    pub thread_idx: usize,
    pub thread_name: String,
    pub sess_idx: usize,
    pub session_display_name: String,
    pub agent_idx: usize,
    pub agent_type: super::model::AgentType,
    pub agent_display_name: String,
    pub tmux_session_name: String,
    pub window_index: u32,
    pub pane_id: String,
    pub pin_slot: Option<u8>,
    /// Pi work indicator (working/retrying/outcome), `None` for non-Pi agents or unknown state.
    pub pi_indicator: Option<PiIndicator>,
}

/// What the current tree selection points to.
pub enum SelectedItem {
    /// Nothing is selected.
    None,
    /// A collection is selected (index into collections vec).
    Collection(usize),
    /// A thread is selected (collection index, thread index).
    Thread(usize, usize),
    /// A session is selected (collection index, thread index, session index within active_sessions for that thread).
    Session(usize, usize, usize),
    /// A launchable Git worktree is selected (collection index, thread index, worktree index within worktree_sessions for that thread).
    Worktree(usize, usize, usize),
    /// An agent is selected (collection index, thread index, session index, agent index within agents_for_session).
    Agent(usize, usize, usize, usize),
}

impl AppState {
    pub fn empty(backend: BackendKind) -> Self {
        Self {
            collections: Vec::new(),
            backend,
            active_sessions: Vec::new(),
            worktree_sessions: Vec::new(),
            agent_sessions: Vec::new(),
            pi_statuses: Vec::new(),
            active_filter: false,
        }
    }

    /// Resolve a tree selection path (from TreeState::selected()) to a SelectedItem.
    ///
    /// Path lengths:
    /// - 0 → None
    /// - 1 → collection UUID, or root thread UUID
    /// - 2 → (col_uuid, thread_uuid) for regular threads, or (thread_uuid, session_name) for root sessions
    /// - 3 → (col_uuid, thread_uuid, session_name) for regular sessions,
    ///   or (thread_uuid, session_name, pane_id) for root agents
    /// - 4 → (col_uuid, thread_uuid, session_name, pane_id) for regular agents
    pub fn resolve_selection(&self, selected: &[String]) -> SelectedItem {
        match selected.len() {
            0 => SelectedItem::None,
            1 => {
                let id = &selected[0];
                // Try collection first, then root thread
                if let Some(idx) = self.find_collection_idx(id) {
                    if self.collection_is_hidden(idx) {
                        SelectedItem::None
                    } else {
                        SelectedItem::Collection(idx)
                    }
                } else if let Some((col_idx, thread_idx)) = self.find_root_thread_by_uuid(id) {
                    if self.thread_is_hidden(col_idx, thread_idx) {
                        SelectedItem::None
                    } else {
                        SelectedItem::Thread(col_idx, thread_idx)
                    }
                } else {
                    SelectedItem::None
                }
            }
            2 => {
                let first = &selected[0];
                let second = &selected[1];
                // Try regular thread first (col_uuid + thread_uuid)
                if let Some(col_idx) = self.find_collection_idx(first)
                    && let Some(thread_idx) = self.find_thread_idx(col_idx, second)
                {
                    if self.thread_is_hidden(col_idx, thread_idx) {
                        return SelectedItem::None;
                    }
                    return SelectedItem::Thread(col_idx, thread_idx);
                }
                // Try root session (thread_uuid + session_name)
                if let Some((col_idx, thread_idx)) = self.find_root_thread_by_uuid(first) {
                    if self.thread_is_hidden(col_idx, thread_idx) {
                        return SelectedItem::None;
                    }
                    let thread = &self.collections[col_idx].threads[thread_idx];
                    let sessions = self.sessions_for_thread(thread.id);
                    if let Some(sess_idx) =
                        sessions.iter().position(|s| s.tmux_session_name == *second)
                    {
                        return SelectedItem::Session(col_idx, thread_idx, sess_idx);
                    }
                    let worktrees = self.worktrees_for_thread(thread.id);
                    if let Some(wt_idx) = worktrees
                        .iter()
                        .position(|w| w.tmux_session_name == *second)
                    {
                        return SelectedItem::Worktree(col_idx, thread_idx, wt_idx);
                    }
                }
                SelectedItem::None
            }
            3 => {
                // Try regular session: col / thread / session
                if let Some(col_idx) = self.find_collection_idx(&selected[0])
                    && let Some(thread_idx) = self.find_thread_idx(col_idx, &selected[1])
                {
                    if self.thread_is_hidden(col_idx, thread_idx) {
                        return SelectedItem::None;
                    }
                    let thread = &self.collections[col_idx].threads[thread_idx];
                    let sessions = self.sessions_for_thread(thread.id);
                    if let Some(sess_idx) = sessions
                        .iter()
                        .position(|s| s.tmux_session_name == selected[2])
                    {
                        return SelectedItem::Session(col_idx, thread_idx, sess_idx);
                    }
                    let worktrees = self.worktrees_for_thread(thread.id);
                    if let Some(wt_idx) = worktrees
                        .iter()
                        .position(|w| w.tmux_session_name == selected[2])
                    {
                        return SelectedItem::Worktree(col_idx, thread_idx, wt_idx);
                    }
                    // The session/worktree this path pointed at is gone.
                    // Never silently retarget to the parent thread —
                    // Rename/Delete/Enter would act on the wrong item.
                    return SelectedItem::None;
                }
                // Try root agent: thread / session / pane_id
                if let Some((col_idx, thread_idx)) = self.find_root_thread_by_uuid(&selected[0]) {
                    if self.thread_is_hidden(col_idx, thread_idx) {
                        return SelectedItem::None;
                    }
                    let thread = &self.collections[col_idx].threads[thread_idx];
                    let sessions = self.sessions_for_thread(thread.id);
                    if let Some(sess_idx) = sessions
                        .iter()
                        .position(|s| s.tmux_session_name == selected[1])
                    {
                        let agents = self.agents_for_session(&selected[1]);
                        if let Some(agent_idx) =
                            agents.iter().position(|a| a.pane_id == selected[2])
                        {
                            return SelectedItem::Agent(col_idx, thread_idx, sess_idx, agent_idx);
                        }
                        // Stale agent path (pane gone): resolving to the
                        // parent session would retarget Enter/Rename to the
                        // wrong item.
                        return SelectedItem::None;
                    }
                    let worktrees = self.worktrees_for_thread(thread.id);
                    if let Some(wt_idx) = worktrees
                        .iter()
                        .position(|w| w.tmux_session_name == selected[1])
                    {
                        return SelectedItem::Worktree(col_idx, thread_idx, wt_idx);
                    }
                }
                SelectedItem::None
            }
            4 => {
                // Regular agent: col / thread / session / pane_id
                if let Some(col_idx) = self.find_collection_idx(&selected[0])
                    && let Some(thread_idx) = self.find_thread_idx(col_idx, &selected[1])
                {
                    if self.thread_is_hidden(col_idx, thread_idx) {
                        return SelectedItem::None;
                    }
                    let thread = &self.collections[col_idx].threads[thread_idx];
                    let sessions = self.sessions_for_thread(thread.id);
                    if let Some(sess_idx) = sessions
                        .iter()
                        .position(|s| s.tmux_session_name == selected[2])
                    {
                        let agents = self.agents_for_session(&selected[2]);
                        if let Some(agent_idx) =
                            agents.iter().position(|a| a.pane_id == selected[3])
                        {
                            return SelectedItem::Agent(col_idx, thread_idx, sess_idx, agent_idx);
                        }
                        // Stale agent path — see the root-agent branch above.
                        return SelectedItem::None;
                    }
                }
                SelectedItem::None
            }
            _ => SelectedItem::None,
        }
    }

    pub fn add_collection(&mut self, name: String) {
        self.collections.push(Collection::new(name));
    }

    pub fn add_thread(&mut self, collection_idx: usize, name: String) {
        if let Some(col) = self.collections.get_mut(collection_idx) {
            col.threads.push(Thread::new(name));
        }
    }

    pub fn rename_collection(&mut self, idx: usize, new_name: String) {
        if let Some(col) = self.collections.get_mut(idx) {
            col.name = new_name;
        }
    }

    pub fn rename_thread(&mut self, col_idx: usize, thread_idx: usize, new_name: String) {
        if let Some(col) = self.collections.get_mut(col_idx)
            && let Some(thread) = col.threads.get_mut(thread_idx)
        {
            thread.name = new_name;
        }
    }

    pub fn hide_collection(&mut self, idx: usize) -> bool {
        let Some(col) = self.collections.get_mut(idx) else {
            return false;
        };
        if col.is_root || col.hidden {
            return false;
        }
        col.hidden = true;
        true
    }

    pub fn hide_thread(&mut self, col_idx: usize, thread_idx: usize) -> bool {
        let Some(thread) = self
            .collections
            .get_mut(col_idx)
            .and_then(|col| col.threads.get_mut(thread_idx))
        else {
            return false;
        };
        if thread.hidden {
            return false;
        }
        thread.hidden = true;
        true
    }

    pub fn show_all_hidden(&mut self) -> usize {
        let mut count = 0;
        for col in &mut self.collections {
            if col.hidden {
                count += 1;
                col.hidden = false;
            }
            for thread in &mut col.threads {
                if thread.hidden {
                    count += 1;
                    thread.hidden = false;
                }
            }
        }
        count
    }

    pub fn hidden_count(&self) -> usize {
        self.collections
            .iter()
            .map(|col| {
                let collection_count = if col.hidden { 1 } else { 0 };
                collection_count + col.threads.iter().filter(|thread| thread.hidden).count()
            })
            .sum()
    }

    pub fn collection_is_hidden(&self, idx: usize) -> bool {
        let Some(col) = self.collections.get(idx) else {
            return false;
        };
        if col.is_root {
            return false;
        }
        if col.hidden {
            return true;
        }
        // Active filter: a collection with no live session in any visible thread is hidden.
        self.active_filter
            && !col
                .threads
                .iter()
                .any(|t| !t.hidden && self.thread_has_live_session(t.id))
    }

    pub fn thread_is_hidden(&self, col_idx: usize, thread_idx: usize) -> bool {
        let Some(col) = self.collections.get(col_idx) else {
            return false;
        };
        let Some(thread) = col.threads.get(thread_idx) else {
            return false;
        };
        if self.collection_is_hidden(col_idx) || thread.hidden {
            return true;
        }
        self.active_filter && !self.thread_has_live_session(thread.id)
    }

    fn thread_has_live_session(&self, thread_id: Uuid) -> bool {
        self.active_sessions
            .iter()
            .any(|s| s.thread_id == thread_id)
    }

    pub fn thread_is_visible(&self, col_idx: usize, thread_idx: usize) -> bool {
        !self.thread_is_hidden(col_idx, thread_idx)
    }

    pub fn delete_collection(&mut self, idx: usize) {
        if idx < self.collections.len() {
            self.collections.remove(idx);
        }
    }

    pub fn delete_thread(&mut self, col_idx: usize, thread_idx: usize) {
        if let Some(col) = self.collections.get_mut(col_idx)
            && thread_idx < col.threads.len()
        {
            col.threads.remove(thread_idx);
        }
    }

    /// Get the name of a selected item (for pre-filling rename input).
    pub fn selected_name(&self, selected: &SelectedItem) -> Option<String> {
        match selected {
            SelectedItem::None => None,
            SelectedItem::Session(col_idx, thread_idx, sess_idx) => {
                let thread_id = self.collections.get(*col_idx)?.threads.get(*thread_idx)?.id;
                let sessions = self.sessions_for_thread(thread_id);
                sessions.get(*sess_idx).map(|s| s.display_name.clone())
            }
            SelectedItem::Worktree(col_idx, thread_idx, wt_idx) => {
                let thread_id = self.collections.get(*col_idx)?.threads.get(*thread_idx)?.id;
                let worktrees = self.worktrees_for_thread(thread_id);
                worktrees.get(*wt_idx).map(|w| w.display_name.clone())
            }
            SelectedItem::Collection(idx) => self.collections.get(*idx).map(|c| c.name.clone()),
            SelectedItem::Thread(col_idx, thread_idx) => self
                .collections
                .get(*col_idx)
                .and_then(|c| c.threads.get(*thread_idx))
                .map(|p| p.name.clone()),
            SelectedItem::Agent(col_idx, thread_idx, sess_idx, agent_idx) => self
                .resolve_agent(*col_idx, *thread_idx, *sess_idx, *agent_idx)
                .map(|a| a.display_name.clone()),
        }
    }

    /// Generate a labeled session name for a thread using the user-provided label.
    pub fn make_session_name(
        &self,
        col_idx: usize,
        thread_idx: usize,
        label: &str,
    ) -> Option<String> {
        let col = self.collections.get(col_idx)?;
        let thread = col.threads.get(thread_idx)?;
        if col.is_root {
            Some(self.backend.root_name(&thread.name, label))
        } else {
            Some(self.backend.regular_name(&col.name, &thread.name, label))
        }
    }

    /// Pin the agent identified by pane_id to the lowest free slot 0..=9.
    /// Returns the assigned slot, or None if all 10 slots are taken.
    /// If the agent is already pinned, returns its existing slot.
    pub fn pin_agent_auto(&mut self, pane_id: &str) -> Option<u8> {
        if let Some(slot) = self
            .agent_sessions
            .iter()
            .find(|a| a.pane_id == pane_id)
            .and_then(|a| a.pin_slot)
        {
            return Some(slot);
        }
        let used: std::collections::HashSet<u8> = self
            .agent_sessions
            .iter()
            .filter_map(|a| a.pin_slot)
            .collect();
        let slot = (0u8..=9).find(|s| !used.contains(s))?;
        if let Some(agent) = self
            .agent_sessions
            .iter_mut()
            .find(|a| a.pane_id == pane_id)
        {
            agent.pin_slot = Some(slot);
            Some(slot)
        } else {
            None
        }
    }

    /// Pin (or repin) the agent identified by pane_id to the given slot (0..=9).
    ///
    /// - If the agent already holds this slot, no-op.
    /// - If the slot is currently held by another agent X:
    ///     - If the moving agent was already pinned (slot Y), the two agents swap slots.
    ///     - If the moving agent was unpinned, X is re-auto-pinned to the lowest free slot.
    ///
    /// Slot values outside 0..=9 are clamped to 9.
    /// If `pane_id` does not match any current agent, this is a no-op.
    pub fn pin_agent_to(&mut self, pane_id: &str, slot: u8) {
        let slot = slot.min(9);

        let moving_existing = self
            .agent_sessions
            .iter()
            .find(|a| a.pane_id == pane_id)
            .and_then(|a| a.pin_slot);

        if moving_existing == Some(slot) {
            return;
        }

        let occupant_pane_id: Option<String> = self
            .agent_sessions
            .iter()
            .find(|a| a.pin_slot == Some(slot))
            .map(|a| a.pane_id.clone());

        if let Some(agent) = self
            .agent_sessions
            .iter_mut()
            .find(|a| a.pane_id == pane_id)
        {
            agent.pin_slot = Some(slot);
        } else {
            return;
        }

        if let Some(occupant_id) = occupant_pane_id {
            if let Some(prev_slot) = moving_existing {
                if let Some(agent) = self
                    .agent_sessions
                    .iter_mut()
                    .find(|a| a.pane_id == occupant_id)
                {
                    agent.pin_slot = Some(prev_slot);
                }
            } else {
                // Clear the occupant's slot *before* calling pin_agent_auto so that the
                // freed slot is counted as available when it scans for the lowest free.
                // pin_agent_auto cannot pick `slot` (the moving agent already owns it).
                if let Some(agent) = self
                    .agent_sessions
                    .iter_mut()
                    .find(|a| a.pane_id == occupant_id)
                {
                    agent.pin_slot = None;
                }
                self.pin_agent_auto(&occupant_id);
            }
        }
    }

    /// Unpin the agent identified by pane_id. No-op if not pinned or not found.
    pub fn unpin_agent(&mut self, pane_id: &str) {
        if let Some(agent) = self
            .agent_sessions
            .iter_mut()
            .find(|a| a.pane_id == pane_id)
        {
            agent.pin_slot = None;
        }
    }

    /// Return the agent occupying the given pin slot, if any.
    pub fn agent_by_pin_slot(&self, slot: u8) -> Option<&AgentSession> {
        self.agent_sessions
            .iter()
            .find(|a| a.pin_slot == Some(slot))
    }

    /// Get all agents detected in a given tmux session.
    pub fn agents_for_session(&self, tmux_session_name: &str) -> Vec<&AgentSession> {
        self.agent_sessions
            .iter()
            .filter(|a| a.tmux_session_name == tmux_session_name)
            .collect()
    }

    fn pi_indicator_priority(indicator: PiIndicator) -> u8 {
        match indicator {
            PiIndicator::Working => 5,
            PiIndicator::Retrying => 4,
            PiIndicator::Failed => 3,
            PiIndicator::Incomplete => 2,
            PiIndicator::Cancelled => 1,
            PiIndicator::Done => 0,
        }
    }

    fn merge_pi_indicator(
        current: Option<PiIndicator>,
        next: Option<PiIndicator>,
    ) -> Option<PiIndicator> {
        match (current, next) {
            (None, next) => next,
            (current, None) => current,
            (Some(current), Some(next)) => Some(
                if Self::pi_indicator_priority(next) > Self::pi_indicator_priority(current) {
                    next
                } else {
                    current
                },
            ),
        }
    }

    /// Reduce sidecars once to the latest status per pane. Rendering and flat
    /// agent construction should reuse this map instead of repeatedly scanning
    /// the full status vector for every session/agent.
    fn latest_pi_status_map(&self) -> std::collections::HashMap<&str, &PiStatus> {
        let mut latest = std::collections::HashMap::new();
        for status in &self.pi_statuses {
            let replace = latest
                .get(status.pane_id.as_str())
                .is_none_or(|current: &&PiStatus| status.updated_at_ms > current.updated_at_ms);
            if replace {
                latest.insert(status.pane_id.as_str(), status);
            }
        }
        latest
    }

    fn indicator_for_status(
        &self,
        status: &PiStatus,
        live_pi_panes: &std::collections::HashSet<&str>,
    ) -> Option<PiIndicator> {
        match status.work_state {
            PiWorkState::Working | PiWorkState::Retrying => {
                Some(if live_pi_panes.contains(status.pane_id.as_str()) {
                    if status.work_state == PiWorkState::Working {
                        PiIndicator::Working
                    } else {
                        PiIndicator::Retrying
                    }
                } else {
                    PiIndicator::Failed
                })
            }
            PiWorkState::Failed => Some(PiIndicator::Failed),
            PiWorkState::Incomplete => Some(PiIndicator::Incomplete),
            PiWorkState::Cancelled => Some(PiIndicator::Cancelled),
            PiWorkState::Done => Some(PiIndicator::Done),
            _ => None,
        }
    }

    /// Aggregate session indicators in O(statuses + agents), keyed by session.
    pub fn pi_indicators_by_session(&self) -> std::collections::HashMap<String, PiIndicator> {
        let latest = self.latest_pi_status_map();
        let live_pi_panes: std::collections::HashSet<&str> = self
            .agent_sessions
            .iter()
            .filter(|agent| agent.agent_type == AgentType::Pi)
            .map(|agent| agent.pane_id.as_str())
            .collect();
        let mut result = std::collections::HashMap::new();
        for status in latest.values() {
            let next = self.indicator_for_status(status, &live_pi_panes);
            let current = result.get(status.tmux_session_name.as_str()).copied();
            if let Some(indicator) = Self::merge_pi_indicator(current, next) {
                result.insert(status.tmux_session_name.clone(), indicator);
            }
        }
        result
    }

    /// Latest Pi indicator per pane, computed in one pass. This map is used
    /// only for rows representing a known live Pi agent, so active sidecar
    /// states remain Working/Retrying. The stale-active → Failed conversion is
    /// intentionally session-only, where no live process may exist.
    pub fn pi_indicators_by_pane(&self) -> std::collections::HashMap<String, PiIndicator> {
        self.latest_pi_status_map()
            .into_iter()
            .filter_map(|(pane_id, status)| {
                let indicator = match status.work_state {
                    PiWorkState::Working => Some(PiIndicator::Working),
                    PiWorkState::Retrying => Some(PiIndicator::Retrying),
                    PiWorkState::Failed => Some(PiIndicator::Failed),
                    PiWorkState::Incomplete => Some(PiIndicator::Incomplete),
                    PiWorkState::Cancelled => Some(PiIndicator::Cancelled),
                    PiWorkState::Done => Some(PiIndicator::Done),
                    _ => None,
                }?;
                Some((pane_id.to_string(), indicator))
            })
            .collect()
    }

    /// What indicator (if any) the session row should show for Pi activity.
    ///
    /// Active states win over terminal outcomes. Technical failures win over
    /// incomplete and cancelled outcomes, which in turn win over success.
    /// A `working`/`retrying` sidecar without a live Pi process is treated as
    /// `Failed`, covering crashes and forced termination.
    pub fn pi_indicator_for_session(&self, tmux_session_name: &str) -> Option<PiIndicator> {
        self.pi_indicators_by_session()
            .get(tmux_session_name)
            .copied()
    }

    /// Aggregate thread indicators once from a precomputed session map.
    pub fn pi_indicators_by_thread(&self) -> std::collections::HashMap<Uuid, PiIndicator> {
        let sessions = self.pi_indicators_by_session();
        let mut result = std::collections::HashMap::new();
        for session in &self.active_sessions {
            let next = sessions.get(&session.tmux_session_name).copied();
            let current = result.get(&session.thread_id).copied();
            if let Some(indicator) = Self::merge_pi_indicator(current, next) {
                result.insert(session.thread_id, indicator);
            }
        }
        result
    }

    /// What indicator (if any) a thread row should show for Pi activity in any
    /// of its active sessions, using the same priority as session rows.
    pub fn pi_indicator_for_thread(&self, thread_id: Uuid) -> Option<PiIndicator> {
        self.pi_indicators_by_thread().get(&thread_id).copied()
    }

    /// What indicator (if any) a collection row should show for Pi activity in
    /// any visible thread, using the same priority as session rows.
    pub fn pi_indicator_for_collection(&self, col_idx: usize) -> Option<PiIndicator> {
        let col = self.collections.get(col_idx)?;
        let by_thread = self.pi_indicators_by_thread();
        col.threads
            .iter()
            .enumerate()
            .filter(|(thread_idx, _)| !self.thread_is_hidden(col_idx, *thread_idx))
            .fold(None, |acc, (_, thread)| {
                Self::merge_pi_indicator(acc, by_thread.get(&thread.id).copied())
            })
    }

    /// What indicator (if any) a specific live Pi agent should show.
    pub fn pi_indicator_for_agent(&self, agent: &AgentSession) -> Option<PiIndicator> {
        if agent.agent_type != AgentType::Pi {
            return None;
        }
        self.pi_indicators_by_pane().get(&agent.pane_id).copied()
    }

    /// Resolve a tree selection to the specific agent it points at.
    pub fn resolve_agent(
        &self,
        col_idx: usize,
        thread_idx: usize,
        sess_idx: usize,
        agent_idx: usize,
    ) -> Option<&AgentSession> {
        let thread_id = self.collections.get(col_idx)?.threads.get(thread_idx)?.id;
        let sessions = self.sessions_for_thread(thread_id);
        let session = sessions.get(sess_idx)?;
        let agents = self.agents_for_session(&session.tmux_session_name);
        agents.get(agent_idx).copied()
    }

    /// Get all active sessions belonging to a given thread.
    pub fn sessions_for_thread(&self, thread_id: Uuid) -> Vec<&Session> {
        self.active_sessions
            .iter()
            .filter(|s| s.thread_id == thread_id)
            .collect()
    }

    /// Get all launchable worktree sessions belonging to a given thread, excluding already-active tmux sessions.
    /// Mainline branch names are listed first (main, master, dev, develop), followed by the rest alphabetically.
    pub fn worktrees_for_thread(&self, thread_id: Uuid) -> Vec<&WorktreeSession> {
        // Worktree rows are inactive by definition (they only exist while no
        // session runs for them), so the active filter hides them all.
        if self.active_filter {
            return Vec::new();
        }
        // worktree_sessions is pre-sorted at refresh time; filtering keeps order.
        self.worktree_sessions
            .iter()
            .filter(|w| w.thread_id == thread_id)
            .filter(|w| {
                !self
                    .active_sessions
                    .iter()
                    .any(|s| s.tmux_session_name == w.tmux_session_name)
            })
            .collect()
    }

    pub fn find_worktree_by_tmux_name(&self, session_name: &str) -> Option<&WorktreeSession> {
        self.worktree_sessions
            .iter()
            .find(|w| w.tmux_session_name == session_name)
    }

    /// Check whether a thread has any active sessions.
    pub fn has_active_session(&self, col_idx: usize, thread_idx: usize) -> bool {
        if let Some(col) = self.collections.get(col_idx)
            && let Some(thread) = col.threads.get(thread_idx)
        {
            return self
                .active_sessions
                .iter()
                .any(|s| s.thread_id == thread.id);
        }
        false
    }

    /// Refresh active_sessions by matching live tmux session names against
    /// our collection/thread hierarchy. Matches by prefix to support
    /// multiple labeled sessions per thread (e.g. `tws_work_pipeline_bugfix`).
    ///
    /// Each entry is `(session_name, last_attached_timestamp)`.
    pub fn refresh_sessions(&mut self, live_tmux_sessions: &[(String, i64)]) {
        self.active_sessions.clear();

        // Threads whose names slugify identically ("Work" vs "work!") share a
        // prefix; without dedup the same tmux session would appear under every
        // matching thread and kill/notes would act on it multiple times.
        let mut claimed: std::collections::HashSet<&str> = std::collections::HashSet::new();

        for col in &self.collections {
            for thread in &col.threads {
                let prefix = if col.is_root {
                    self.backend.root_prefix(&thread.name)
                } else {
                    self.backend.regular_prefix(&col.name, &thread.name)
                };
                for (session_name, last_attached) in live_tmux_sessions {
                    // Match "prefix_label" where label is any non-empty suffix
                    if let Some(rest) = session_name.strip_prefix(&prefix)
                        && let Some(label) = rest.strip_prefix('_')
                        && !label.is_empty()
                        && claimed.insert(session_name.as_str())
                    {
                        self.active_sessions.push(Session {
                            tmux_session_name: session_name.clone(),
                            display_name: label.to_string(),
                            thread_id: thread.id,
                            last_attached: *last_attached,
                        });
                    }
                }
            }
        }
    }

    pub fn refresh_worktree_sessions(&mut self, mut worktree_sessions: Vec<WorktreeSession>) {
        // Sort once here instead of on every worktrees_for_thread call
        // (which runs several times per frame).
        worktree_sessions.sort_by_key(worktree_sort_key);
        self.worktree_sessions = worktree_sessions;
    }

    /// Format a session's display path: `Collection/Thread/label` or `Thread/label` for root threads.
    pub fn session_display_path(&self, session: &Session) -> Option<String> {
        let (col_name, thread_name) = self.resolve_thread_path(session.thread_id)?;
        Some(match col_name {
            Some(c) => format!("{}/{}/{}", c, thread_name, session.display_name),
            None => format!("{}/{}", thread_name, session.display_name),
        })
    }

    /// Format a worktree's display path: `Collection/Thread/label` or `Thread/label` for root threads.
    pub fn worktree_display_path(&self, worktree: &WorktreeSession) -> Option<String> {
        let (col_name, thread_name) = self.resolve_thread_path(worktree.thread_id)?;
        Some(match col_name {
            Some(c) => format!("{}/{}/{}", c, thread_name, worktree.display_name),
            None => format!("{}/{}", thread_name, worktree.display_name),
        })
    }

    /// List all threads as `(col_idx, thread_idx, display_path)` for the thread picker.
    /// Display path is `"Collection/Thread"` or just `"Thread"` for root threads.
    pub fn all_threads_display(&self) -> Vec<(usize, usize, String)> {
        let mut result = Vec::new();
        for (col_idx, col) in self.collections.iter().enumerate() {
            for (thread_idx, thread) in col.threads.iter().enumerate() {
                if self.thread_is_hidden(col_idx, thread_idx) {
                    continue;
                }
                let path = if col.is_root {
                    thread.name.clone()
                } else {
                    format!("{}/{}", col.name, thread.name)
                };
                result.push((col_idx, thread_idx, path));
            }
        }
        result
    }

    /// Given a thread ID, find its collection and thread names.
    /// Returns `(Option<collection_name>, thread_name)`. Collection name is `None` for root threads.
    pub fn resolve_thread_path(&self, thread_id: Uuid) -> Option<(Option<String>, String)> {
        for (col_idx, col) in self.collections.iter().enumerate() {
            for (thread_idx, thread) in col.threads.iter().enumerate() {
                if thread.id == thread_id && self.thread_is_visible(col_idx, thread_idx) {
                    let col_name = if col.is_root {
                        None
                    } else {
                        Some(col.name.clone())
                    };
                    return Some((col_name, thread.name.clone()));
                }
            }
        }
        None
    }

    /// Returns the `n` most recently attached sessions, sorted by
    /// recency (most recent first). Sessions with `last_attached == 0` are excluded.
    pub fn recent_sessions(&self, n: usize) -> Vec<&Session> {
        let mut recent: Vec<&Session> = self
            .active_sessions
            .iter()
            .filter(|s| s.last_attached > 0)
            .filter(|s| self.resolve_thread_path(s.thread_id).is_some())
            .collect();
        recent.sort_by(|a, b| b.last_attached.cmp(&a.last_attached));
        recent.truncate(n);
        recent
    }

    /// Returns the tree widget selection path for a session by its tmux name.
    /// Path: `[collection_id, thread_id, session_name]` or `[thread_id, session_name]` for root threads.
    pub fn session_tree_path(&self, session_name: &str) -> Option<Vec<String>> {
        let session = self
            .active_sessions
            .iter()
            .find(|s| s.tmux_session_name == session_name)?;
        for (col_idx, col) in self.collections.iter().enumerate() {
            for (thread_idx, thread) in col.threads.iter().enumerate() {
                if thread.id == session.thread_id && self.thread_is_visible(col_idx, thread_idx) {
                    return if col.is_root {
                        Some(vec![thread.id.to_string(), session_name.to_string()])
                    } else {
                        Some(vec![
                            col.id.to_string(),
                            thread.id.to_string(),
                            session_name.to_string(),
                        ])
                    };
                }
            }
        }
        None
    }

    /// Find the index of the root collection (where `is_root == true`).
    pub fn find_root_collection_idx(&self) -> Option<usize> {
        self.collections.iter().position(|c| c.is_root)
    }

    /// Find a thread within the root collection by UUID string.
    /// Returns `(col_idx, thread_idx)`.
    pub fn find_root_thread_by_uuid(&self, uuid_str: &str) -> Option<(usize, usize)> {
        let id: Uuid = uuid_str.parse().ok()?;
        let col_idx = self.find_root_collection_idx()?;
        let thread_idx = self.collections[col_idx]
            .threads
            .iter()
            .position(|t| t.id == id)?;
        Some((col_idx, thread_idx))
    }

    /// Lazy-init: returns the root collection index, creating it on first call if absent.
    pub fn ensure_root_collection(&mut self) -> usize {
        if let Some(idx) = self.find_root_collection_idx() {
            idx
        } else {
            self.collections.push(Collection::new_root());
            self.collections.len() - 1
        }
    }

    /// Lazy-init: ensures the root collection has a "general" thread, creating it if absent.
    /// Returns `(col_idx, thread_idx)`.
    pub fn ensure_general_thread(&mut self) -> (usize, usize) {
        let col_idx = self.ensure_root_collection();
        if let Some(thread_idx) = self.collections[col_idx]
            .threads
            .iter()
            .position(|t| t.name == "general")
        {
            (col_idx, thread_idx)
        } else {
            self.collections[col_idx]
                .threads
                .push(Thread::new("general"));
            (col_idx, self.collections[col_idx].threads.len() - 1)
        }
    }

    fn find_collection_idx(&self, uuid_str: &str) -> Option<usize> {
        let id: Uuid = uuid_str.parse().ok()?;
        self.collections.iter().position(|c| c.id == id)
    }

    fn find_thread_idx(&self, col_idx: usize, uuid_str: &str) -> Option<usize> {
        let id: Uuid = uuid_str.parse().ok()?;
        self.collections
            .get(col_idx)?
            .threads
            .iter()
            .position(|p| p.id == id)
    }

    /// Flatten every active agent across all collections/threads/sessions into a single list.
    /// Each entry carries the display strings and the index tuple needed to produce `SelectedItem::Agent`.
    pub fn all_agents_flat(&self) -> Vec<FlatAgent> {
        let mut result = Vec::new();
        let pane_indicators = self.pi_indicators_by_pane();
        let mut sessions_by_thread: std::collections::HashMap<Uuid, Vec<&Session>> =
            std::collections::HashMap::new();
        for session in &self.active_sessions {
            sessions_by_thread
                .entry(session.thread_id)
                .or_default()
                .push(session);
        }
        let mut agents_by_session: std::collections::HashMap<&str, Vec<&AgentSession>> =
            std::collections::HashMap::new();
        for agent in &self.agent_sessions {
            agents_by_session
                .entry(agent.tmux_session_name.as_str())
                .or_default()
                .push(agent);
        }

        for (col_idx, col) in self.collections.iter().enumerate() {
            for (thread_idx, thread) in col.threads.iter().enumerate() {
                if self.thread_is_hidden(col_idx, thread_idx) {
                    continue;
                }
                let sessions = sessions_by_thread
                    .get(&thread.id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                for (sess_idx, session) in sessions.iter().enumerate() {
                    let agents = agents_by_session
                        .get(session.tmux_session_name.as_str())
                        .map(Vec::as_slice)
                        .unwrap_or(&[]);
                    for (agent_idx, agent) in agents.iter().enumerate() {
                        result.push(FlatAgent {
                            col_idx,
                            thread_idx,
                            sess_idx,
                            agent_idx,
                            thread_name: thread.name.clone(),
                            session_display_name: session.display_name.clone(),
                            agent_type: agent.agent_type,
                            agent_display_name: agent.display_name.clone(),
                            tmux_session_name: agent.tmux_session_name.clone(),
                            window_index: agent.window_index,
                            pane_id: agent.pane_id.clone(),
                            pin_slot: agent.pin_slot,
                            pi_indicator: if agent.agent_type == AgentType::Pi {
                                pane_indicators.get(&agent.pane_id).copied()
                            } else {
                                None
                            },
                        });
                    }
                }
            }
        }
        // Pinned agents first, by slot ascending; unpinned keep original tree order.
        result.sort_by_key(|a| match a.pin_slot {
            Some(slot) => (0u8, slot),
            None => (1u8, 0u8),
        });
        result
    }
}

fn worktree_sort_key(worktree: &WorktreeSession) -> (u8, String, String) {
    let name = worktree_sort_name(worktree);
    let rank = match name.as_str() {
        "main" => 0,
        "master" => 1,
        "dev" => 2,
        "develop" => 3,
        _ => 4,
    };
    (rank, name, worktree.path.to_string_lossy().into_owned())
}

fn worktree_sort_name(worktree: &WorktreeSession) -> String {
    worktree
        .branch
        .as_deref()
        .map(short_branch_name)
        .unwrap_or(&worktree.display_name)
        .to_ascii_lowercase()
}

fn short_branch_name(branch: &str) -> &str {
    branch
        .strip_prefix("refs/heads/")
        .or_else(|| branch.strip_prefix("refs/remotes/"))
        .unwrap_or(branch)
}

impl Default for AppState {
    fn default() -> Self {
        Self::empty(BackendKind::Tmux)
    }
}

#[cfg(test)]
impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates sample data for development/testing.
    pub fn with_sample_data() -> Self {
        let mut work = Collection::new("Work");
        work.threads.push(Thread::new("Edge Device Pipeline"));
        work.threads.push(Thread::new("Model Training Infra"));
        work.threads.push(Thread::new("CI/CD Overhaul"));

        let mut learning = Collection::new("Learning");
        learning.threads.push(Thread::new("Rust Book"));
        learning.threads.push(Thread::new("Ratatui Experiments"));

        let mut podcast = Collection::new("Derin Notlar Podcast");
        podcast.threads.push(Thread::new("Episode 12"));
        podcast.threads.push(Thread::new("Episode 13 - Planning"));

        let personal = Collection::new("Personal");

        Self {
            collections: vec![work, learning, podcast, personal],
            backend: BackendKind::Tmux,
            active_sessions: Vec::new(),
            worktree_sessions: Vec::new(),
            agent_sessions: Vec::new(),
            pi_statuses: Vec::new(),
            active_filter: false,
        }
    }

    /// Generate the session prefix for a given collection/thread index pair.
    pub fn session_prefix_for(&self, col_idx: usize, thread_idx: usize) -> Option<String> {
        let col = self.collections.get(col_idx)?;
        let thread = col.threads.get(thread_idx)?;
        if col.is_root {
            Some(self.backend.root_prefix(&thread.name))
        } else {
            Some(self.backend.regular_prefix(&col.name, &thread.name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_find_collection() {
        let mut state = AppState::new();
        state.add_collection("Work".into());
        assert_eq!(state.collections.len(), 1);
        assert_eq!(state.collections[0].name, "Work");
    }

    #[test]
    fn add_thread_to_collection() {
        let mut state = AppState::new();
        state.add_collection("Work".into());
        state.add_thread(0, "Pipeline".into());
        assert_eq!(state.collections[0].threads.len(), 1);
        assert_eq!(state.collections[0].threads[0].name, "Pipeline");
    }

    #[test]
    fn rename_collection() {
        let mut state = AppState::new();
        state.add_collection("Work".into());
        state.rename_collection(0, "Job".into());
        assert_eq!(state.collections[0].name, "Job");
    }

    #[test]
    fn rename_thread() {
        let mut state = AppState::new();
        state.add_collection("Work".into());
        state.add_thread(0, "Old".into());
        state.rename_thread(0, 0, "New".into());
        assert_eq!(state.collections[0].threads[0].name, "New");
    }

    #[test]
    fn delete_collection() {
        let mut state = AppState::new();
        state.add_collection("A".into());
        state.add_collection("B".into());
        state.delete_collection(0);
        assert_eq!(state.collections.len(), 1);
        assert_eq!(state.collections[0].name, "B");
    }

    #[test]
    fn delete_thread() {
        let mut state = AppState::new();
        state.add_collection("Work".into());
        state.add_thread(0, "A".into());
        state.add_thread(0, "B".into());
        state.delete_thread(0, 0);
        assert_eq!(state.collections[0].threads.len(), 1);
        assert_eq!(state.collections[0].threads[0].name, "B");
    }

    #[test]
    fn hide_collection_marks_visible_collection_hidden() {
        let mut state = AppState::new();
        state.add_collection("Work".into());
        assert!(state.hide_collection(0));
        assert!(state.collections[0].hidden);
        assert_eq!(state.hidden_count(), 1);
    }

    #[test]
    fn hide_collection_ignores_root_collection() {
        let mut state = AppState::new();
        state.ensure_general_thread();
        assert!(!state.hide_collection(0));
        assert!(!state.collections[0].hidden);
    }

    #[test]
    fn hide_thread_marks_thread_hidden() {
        let mut state = AppState::new();
        state.add_collection("Work".into());
        state.add_thread(0, "A".into());
        assert!(state.hide_thread(0, 0));
        assert!(state.collections[0].threads[0].hidden);
    }

    #[test]
    fn show_all_hidden_restores_everything() {
        let mut state = AppState::new();
        state.add_collection("Work".into());
        state.add_thread(0, "A".into());
        state.hide_collection(0);
        state.hide_thread(0, 0);
        assert_eq!(state.show_all_hidden(), 2);
        assert!(!state.collections[0].hidden);
        assert!(!state.collections[0].threads[0].hidden);
    }

    #[test]
    fn active_filter_hides_threads_without_sessions() {
        let mut state = AppState::with_sample_data();
        state.active_filter = true;
        // No sessions at all: every thread and collection is hidden.
        assert!(state.thread_is_hidden(0, 0));
        assert!(state.collection_is_hidden(0));

        // A live session for Work/Edge Device Pipeline reveals thread + collection.
        state.refresh_sessions(&[("tws_work_edge-device-pipeline_main".into(), 0)]);
        assert!(!state.thread_is_hidden(0, 0));
        assert!(!state.collection_is_hidden(0));
        // Sibling thread without a session stays hidden.
        assert!(state.thread_is_hidden(0, 1));
        // Other collection stays hidden.
        assert!(state.collection_is_hidden(1));

        // Toggling off restores everything.
        state.active_filter = false;
        assert!(!state.thread_is_hidden(0, 1));
        assert!(!state.collection_is_hidden(1));
    }

    #[test]
    fn zellij_backend_discovers_zellij_names_only() {
        let mut state = AppState::with_sample_data();
        state.backend = BackendKind::Zellij;
        state.refresh_sessions(&[
            ("twz_work_edge-device-pipeline_main".into(), 42),
            ("tws_work_edge-device-pipeline_tmux".into(), 99),
        ]);

        assert_eq!(state.active_sessions.len(), 1);
        assert_eq!(
            state.active_sessions[0].tmux_session_name,
            "twz_work_edge-device-pipeline_main"
        );
        assert_eq!(
            state.make_session_name(0, 0, "feature"),
            Some("twz_work_edge-device-pipeline_feature".into())
        );
    }

    #[test]
    fn active_filter_ignores_sessions_in_manually_hidden_threads() {
        let mut state = AppState::with_sample_data();
        state.refresh_sessions(&[("tws_work_edge-device-pipeline_main".into(), 0)]);
        state.hide_thread(0, 0);
        state.active_filter = true;
        // The only live session sits in a manually hidden thread, so the
        // collection has no visible activity and is filtered too.
        assert!(state.collection_is_hidden(0));
    }

    #[test]
    fn active_filter_hides_all_worktrees() {
        let mut state = AppState::with_sample_data();
        let thread_id = state.collections[0].threads[0].id;
        state.refresh_worktree_sessions(vec![WorktreeSession {
            tmux_session_name: "tws_work_edge-device-pipeline_feature-x".into(),
            display_name: "feature-x".into(),
            thread_id,
            repo: std::path::PathBuf::from("/tmp/repo"),
            path: std::path::PathBuf::from("/tmp/feature-x"),
            branch: None,
            head: None,
            prunable: false,
            is_main: false,
            path_exists: true,
            launchable: true,
        }]);
        assert_eq!(state.worktrees_for_thread(thread_id).len(), 1);
        state.active_filter = true;
        assert!(state.worktrees_for_thread(thread_id).is_empty());
    }

    #[test]
    fn active_filter_never_hides_root_collection() {
        let mut state = AppState::new();
        state.ensure_general_thread();
        state.active_filter = true;
        let root_idx = state.find_root_collection_idx().unwrap();
        assert!(!state.collection_is_hidden(root_idx));
        // But the root thread itself is filtered without a session.
        assert!(state.thread_is_hidden(root_idx, 0));
    }

    #[test]
    fn hidden_selection_resolves_to_none() {
        let mut state = AppState::with_sample_data();
        let col_id = state.collections[0].id.to_string();
        state.hide_collection(0);
        match state.resolve_selection(&[col_id]) {
            SelectedItem::None => {}
            _ => panic!("expected None"),
        }

        let thread_id = state.collections[1].threads[0].id.to_string();
        state.hide_thread(1, 0);
        match state.resolve_selection(&[state.collections[1].id.to_string(), thread_id]) {
            SelectedItem::None => {}
            _ => panic!("expected None"),
        }
    }

    #[test]
    fn hidden_threads_are_excluded_from_thread_picker() {
        let mut state = AppState::with_sample_data();
        state.hide_thread(0, 0);
        let paths: Vec<String> = state
            .all_threads_display()
            .into_iter()
            .map(|(_, _, path)| path)
            .collect();
        assert!(!paths.contains(&"Work/Edge Device Pipeline".to_string()));
        assert!(paths.contains(&"Work/Model Training Infra".to_string()));
    }

    #[test]
    fn resolve_collection_selection() {
        let state = AppState::with_sample_data();
        let id = state.collections[0].id.to_string();
        match state.resolve_selection(&[id]) {
            SelectedItem::Collection(idx) => assert_eq!(idx, 0),
            _ => panic!("expected Collection"),
        }
    }

    #[test]
    fn resolve_thread_selection() {
        let state = AppState::with_sample_data();
        let col_id = state.collections[0].id.to_string();
        let thread_id = state.collections[0].threads[1].id.to_string();
        match state.resolve_selection(&[col_id, thread_id]) {
            SelectedItem::Thread(col_idx, thread_idx) => {
                assert_eq!(col_idx, 0);
                assert_eq!(thread_idx, 1);
            }
            _ => panic!("expected Thread"),
        }
    }

    #[test]
    fn resolve_empty_selection() {
        let state = AppState::new();
        match state.resolve_selection(&[]) {
            SelectedItem::None => {}
            _ => panic!("expected None"),
        }
    }

    #[test]
    fn session_prefix_for_valid() {
        let state = AppState::with_sample_data();
        let prefix = state.session_prefix_for(0, 0).unwrap();
        assert_eq!(prefix, "tws_work_edge-device-pipeline");
    }

    #[test]
    fn make_session_name_labeled() {
        let state = AppState::with_sample_data();
        let name = state.make_session_name(0, 0, "bugfix").unwrap();
        assert_eq!(name, "tws_work_edge-device-pipeline_bugfix");
    }

    #[test]
    fn make_session_name_slugifies_label() {
        let state = AppState::with_sample_data();
        let name = state.make_session_name(0, 0, "Hot Fix 2").unwrap();
        assert_eq!(name, "tws_work_edge-device-pipeline_hot-fix-2");
    }

    #[test]
    fn refresh_sessions_discovers_labeled() {
        let mut state = AppState::with_sample_data();
        let live = vec![
            ("tws_work_edge-device-pipeline_bugfix".to_string(), 0),
            ("tws_work_edge-device-pipeline_hotfix".to_string(), 0),
        ];
        state.refresh_sessions(&live);
        assert_eq!(state.active_sessions.len(), 2);
        assert_eq!(state.active_sessions[0].display_name, "bugfix");
        assert_eq!(state.active_sessions[1].display_name, "hotfix");
        assert_eq!(
            state.active_sessions[0].thread_id,
            state.collections[0].threads[0].id
        );
    }

    #[test]
    fn refresh_sessions_ignores_non_matching() {
        let mut state = AppState::with_sample_data();
        let live = vec![("some-other-session".to_string(), 0)];
        state.refresh_sessions(&live);
        assert!(state.active_sessions.is_empty());
    }

    #[test]
    fn refresh_sessions_ignores_bare_prefix() {
        let mut state = AppState::with_sample_data();
        // The bare prefix without _label should NOT match
        let live = vec![("tws_work_edge-device-pipeline".to_string(), 0)];
        state.refresh_sessions(&live);
        assert!(state.active_sessions.is_empty());
    }

    #[test]
    fn has_active_session_works() {
        let mut state = AppState::with_sample_data();
        assert!(!state.has_active_session(0, 0));
        let live = vec![("tws_work_edge-device-pipeline_bugfix".to_string(), 0)];
        state.refresh_sessions(&live);
        assert!(state.has_active_session(0, 0));
        assert!(!state.has_active_session(0, 1));
    }

    #[test]
    fn resolve_session_selection() {
        let mut state = AppState::with_sample_data();
        let live = vec![("tws_work_edge-device-pipeline_bugfix".to_string(), 0)];
        state.refresh_sessions(&live);

        let col_id = state.collections[0].id.to_string();
        let thread_id = state.collections[0].threads[0].id.to_string();
        let sess_name = "tws_work_edge-device-pipeline_bugfix".to_string();
        match state.resolve_selection(&[col_id, thread_id, sess_name]) {
            SelectedItem::Session(col_idx, thread_idx, sess_idx) => {
                assert_eq!(col_idx, 0);
                assert_eq!(thread_idx, 0);
                assert_eq!(sess_idx, 0);
            }
            _ => panic!("expected Session"),
        }
    }

    #[test]
    fn resolve_session_selection_multiple() {
        let mut state = AppState::with_sample_data();
        let live = vec![
            ("tws_work_edge-device-pipeline_bugfix".to_string(), 0),
            ("tws_work_edge-device-pipeline_hotfix".to_string(), 0),
        ];
        state.refresh_sessions(&live);

        let col_id = state.collections[0].id.to_string();
        let thread_id = state.collections[0].threads[0].id.to_string();

        // Select the second session
        let sess_name = "tws_work_edge-device-pipeline_hotfix".to_string();
        match state.resolve_selection(&[col_id, thread_id, sess_name]) {
            SelectedItem::Session(_, _, sess_idx) => assert_eq!(sess_idx, 1),
            _ => panic!("expected Session"),
        }
    }

    fn claude_agent(session: &str, pane: &str) -> AgentSession {
        AgentSession {
            agent_type: AgentType::ClaudeCode,
            tmux_session_name: session.to_string(),
            window_index: 0,
            pane_id: pane.to_string(),
            display_name: "claude".to_string(),
            renamed: false,
            pin_slot: None,
        }
    }

    #[test]
    fn resolve_stale_session_selection_returns_none() {
        // A 3-segment path whose session vanished must NOT fall back to the
        // parent thread — actions would silently retarget.
        let mut state = AppState::with_sample_data();
        state.refresh_sessions(&[("tws_work_edge-device-pipeline_bugfix".to_string(), 0)]);

        let col_id = state.collections[0].id.to_string();
        let thread_id = state.collections[0].threads[0].id.to_string();
        let stale = "tws_work_edge-device-pipeline_gone".to_string();
        match state.resolve_selection(&[col_id, thread_id, stale]) {
            SelectedItem::None => {}
            _ => panic!("expected None for stale session path"),
        }
    }

    #[test]
    fn resolve_stale_agent_selection_returns_none() {
        let mut state = AppState::with_sample_data();
        state.refresh_sessions(&[("tws_work_edge-device-pipeline_bugfix".to_string(), 0)]);
        state
            .agent_sessions
            .push(claude_agent("tws_work_edge-device-pipeline_bugfix", "%1"));

        let col_id = state.collections[0].id.to_string();
        let thread_id = state.collections[0].threads[0].id.to_string();
        let sess = "tws_work_edge-device-pipeline_bugfix".to_string();

        // Live pane resolves to the agent…
        match state.resolve_selection(&[
            col_id.clone(),
            thread_id.clone(),
            sess.clone(),
            "%1".into(),
        ]) {
            SelectedItem::Agent(_, _, _, _) => {}
            _ => panic!("expected Agent"),
        }
        // …a dead pane must resolve to None, never to the parent session.
        match state.resolve_selection(&[col_id, thread_id, sess, "%99".into()]) {
            SelectedItem::None => {}
            _ => panic!("expected None for stale agent path"),
        }
    }

    #[test]
    fn resolve_root_agent_selection_and_stale_pane() {
        let mut state = AppState::new();
        state.ensure_general_thread();
        state.refresh_sessions(&[("twsr_general_quick".to_string(), 100)]);
        state
            .agent_sessions
            .push(claude_agent("twsr_general_quick", "%2"));

        let thread_id = state.collections[0].threads[0].id.to_string();
        let sess = "twsr_general_quick".to_string();

        match state.resolve_selection(&[thread_id.clone(), sess.clone(), "%2".into()]) {
            SelectedItem::Agent(col_idx, thread_idx, sess_idx, agent_idx) => {
                assert_eq!((col_idx, thread_idx, sess_idx, agent_idx), (0, 0, 0, 0));
            }
            _ => panic!("expected Agent"),
        }
        match state.resolve_selection(&[thread_id, sess, "%77".into()]) {
            SelectedItem::None => {}
            _ => panic!("expected None for stale root agent path"),
        }
    }

    #[test]
    fn resolve_worktree_selection() {
        let mut state = AppState::with_sample_data();
        let thread_id = state.collections[0].threads[0].id;
        state.refresh_worktree_sessions(vec![WorktreeSession {
            tmux_session_name: "tws_work_edge-device-pipeline_feature-x".into(),
            display_name: "feature-x".into(),
            thread_id,
            repo: std::path::PathBuf::from("/tmp/repo"),
            path: std::path::PathBuf::from("/tmp/feature-x"),
            branch: Some("refs/heads/feature/x".into()),
            head: Some("abcdef123456".into()),
            prunable: false,
            is_main: false,
            path_exists: true,
            launchable: true,
        }]);

        let col_id = state.collections[0].id.to_string();
        let thread_uuid = state.collections[0].threads[0].id.to_string();
        match state.resolve_selection(&[
            col_id,
            thread_uuid,
            "tws_work_edge-device-pipeline_feature-x".into(),
        ]) {
            SelectedItem::Worktree(_, _, wt_idx) => assert_eq!(wt_idx, 0),
            _ => panic!("expected Worktree"),
        }
    }

    #[test]
    fn active_session_hides_matching_worktree() {
        let mut state = AppState::with_sample_data();
        let thread_id = state.collections[0].threads[0].id;
        state.refresh_worktree_sessions(vec![WorktreeSession {
            tmux_session_name: "tws_work_edge-device-pipeline_feature-x".into(),
            display_name: "feature-x".into(),
            thread_id,
            repo: std::path::PathBuf::from("/tmp/repo"),
            path: std::path::PathBuf::from("/tmp/feature-x"),
            branch: Some("refs/heads/feature/x".into()),
            head: Some("abcdef123456".into()),
            prunable: false,
            is_main: false,
            path_exists: true,
            launchable: true,
        }]);
        assert_eq!(state.worktrees_for_thread(thread_id).len(), 1);

        state.refresh_sessions(&[("tws_work_edge-device-pipeline_feature-x".into(), 0)]);
        assert!(state.worktrees_for_thread(thread_id).is_empty());
    }

    #[test]
    fn worktrees_for_thread_sorts_mainlines_then_alphabetically() {
        let mut state = AppState::with_sample_data();
        let thread_id = state.collections[0].threads[0].id;
        let make_worktree = |name: &str, branch: &str| WorktreeSession {
            tmux_session_name: format!("tws_work_edge-device-pipeline_{}", name),
            display_name: name.into(),
            thread_id,
            repo: std::path::PathBuf::from("/tmp/repo"),
            path: std::path::PathBuf::from(format!("/tmp/{}", name)),
            branch: Some(format!("refs/heads/{}", branch)),
            head: Some("abcdef123456".into()),
            prunable: false,
            is_main: false,
            path_exists: true,
            launchable: true,
        };
        state.refresh_worktree_sessions(vec![
            make_worktree("zeta", "zeta"),
            make_worktree("develop", "develop"),
            make_worktree("main", "main"),
            make_worktree("alpha", "alpha"),
            make_worktree("dev", "dev"),
            make_worktree("master", "master"),
        ]);

        let names: Vec<&str> = state
            .worktrees_for_thread(thread_id)
            .iter()
            .map(|w| w.display_name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["main", "master", "dev", "develop", "alpha", "zeta"]
        );
    }

    #[test]
    fn ensure_root_collection_creates_once() {
        let mut state = AppState::new();
        let idx1 = state.ensure_root_collection();
        let idx2 = state.ensure_root_collection();
        assert_eq!(idx1, idx2);
        assert_eq!(state.collections.len(), 1);
        assert!(state.collections[idx1].is_root);
    }

    #[test]
    fn ensure_general_thread_creates_once() {
        let mut state = AppState::new();
        let (c1, t1) = state.ensure_general_thread();
        let (c2, t2) = state.ensure_general_thread();
        assert_eq!((c1, t1), (c2, t2));
        assert_eq!(state.collections[c1].threads.len(), 1);
        assert_eq!(state.collections[c1].threads[t1].name, "general");
    }

    #[test]
    fn refresh_sessions_discovers_root_sessions() {
        let mut state = AppState::new();
        let (col_idx, _) = state.ensure_general_thread();
        state.add_thread(col_idx, "scratch".into());
        let live = vec![
            ("twsr_general_quick".to_string(), 100),
            ("twsr_scratch_dev".to_string(), 200),
        ];
        state.refresh_sessions(&live);
        assert_eq!(state.active_sessions.len(), 2);
        assert_eq!(state.active_sessions[0].display_name, "quick");
        assert_eq!(state.active_sessions[1].display_name, "dev");
    }

    #[test]
    fn resolve_root_thread_selection() {
        let mut state = AppState::new();
        state.ensure_general_thread();
        let thread_id = state.collections[0].threads[0].id.to_string();
        match state.resolve_selection(&[thread_id]) {
            SelectedItem::Thread(col_idx, thread_idx) => {
                assert_eq!(col_idx, 0);
                assert_eq!(thread_idx, 0);
            }
            _ => panic!("expected Thread"),
        }
    }

    #[test]
    fn resolve_root_session_selection() {
        let mut state = AppState::new();
        state.ensure_general_thread();
        let live = vec![("twsr_general_quick".to_string(), 100)];
        state.refresh_sessions(&live);

        let thread_id = state.collections[0].threads[0].id.to_string();
        let sess_name = "twsr_general_quick".to_string();
        match state.resolve_selection(&[thread_id, sess_name]) {
            SelectedItem::Session(col_idx, thread_idx, sess_idx) => {
                assert_eq!(col_idx, 0);
                assert_eq!(thread_idx, 0);
                assert_eq!(sess_idx, 0);
            }
            _ => panic!("expected Session"),
        }
    }

    #[test]
    fn make_session_name_root() {
        let mut state = AppState::new();
        state.ensure_general_thread();
        let name = state.make_session_name(0, 0, "bugfix").unwrap();
        assert_eq!(name, "twsr_general_bugfix");
    }

    #[test]
    fn resolve_thread_path_root() {
        let mut state = AppState::new();
        state.ensure_general_thread();
        let thread_id = state.collections[0].threads[0].id;
        let (col_name, thread_name) = state.resolve_thread_path(thread_id).unwrap();
        assert!(col_name.is_none());
        assert_eq!(thread_name, "general");
    }

    #[test]
    fn session_display_path_regular() {
        let mut state = AppState::with_sample_data();
        let live = vec![("tws_work_edge-device-pipeline_bugfix".to_string(), 0)];
        state.refresh_sessions(&live);
        let path = state
            .session_display_path(&state.active_sessions[0])
            .unwrap();
        assert_eq!(path, "Work/Edge Device Pipeline/bugfix");
    }

    #[test]
    fn session_display_path_hidden_thread_returns_none() {
        let mut state = AppState::with_sample_data();
        let live = vec![("tws_work_edge-device-pipeline_bugfix".to_string(), 100)];
        state.refresh_sessions(&live);
        state.hide_thread(0, 0);
        assert!(
            state
                .session_display_path(&state.active_sessions[0])
                .is_none()
        );
        assert!(state.recent_sessions(5).is_empty());
    }

    #[test]
    fn session_display_path_root() {
        let mut state = AppState::new();
        state.ensure_general_thread();
        let live = vec![("twsr_general_quick".to_string(), 0)];
        state.refresh_sessions(&live);
        let path = state
            .session_display_path(&state.active_sessions[0])
            .unwrap();
        assert_eq!(path, "general/quick");
    }

    #[test]
    fn resolve_selection_prefers_collection_over_root_thread() {
        // Mixed state: regular collections + root collection
        let mut state = AppState::with_sample_data();
        state.ensure_general_thread();

        // 1-segment path with a regular collection UUID → must resolve to Collection, not root thread
        let col_id = state.collections[0].id.to_string();
        match state.resolve_selection(&[col_id]) {
            SelectedItem::Collection(idx) => assert_eq!(idx, 0),
            _ => panic!("expected Collection"),
        }
    }

    #[test]
    fn resolve_selection_prefers_regular_thread_over_root_session() {
        // Mixed state: regular collections + root collection with an active session
        let mut state = AppState::with_sample_data();
        state.ensure_general_thread();
        let live = vec![("twsr_general_quick".to_string(), 0)];
        state.refresh_sessions(&live);

        // 2-segment path with (col_uuid, thread_uuid) → must resolve to regular Thread
        let col_id = state.collections[0].id.to_string();
        let thread_id = state.collections[0].threads[0].id.to_string();
        match state.resolve_selection(&[col_id, thread_id]) {
            SelectedItem::Thread(col_idx, thread_idx) => {
                assert_eq!(col_idx, 0);
                assert_eq!(thread_idx, 0);
                assert_eq!(state.collections[col_idx].name, "Work");
            }
            _ => panic!("expected Thread"),
        }
    }

    #[test]
    fn refresh_sessions_ignores_bare_root_prefix() {
        let mut state = AppState::new();
        state.ensure_general_thread();
        // Bare root prefix without _label should NOT match
        let live = vec![("twsr_general".to_string(), 0)];
        state.refresh_sessions(&live);
        assert!(state.active_sessions.is_empty());
    }

    fn make_agent(pane_id: &str) -> super::AgentSession {
        use super::super::model::{AgentSession, AgentType};
        AgentSession {
            agent_type: AgentType::ClaudeCode,
            tmux_session_name: "tws_x_y_a".into(),
            window_index: 0,
            pane_id: pane_id.into(),
            display_name: "claude".into(),
            renamed: false,
            pin_slot: None,
        }
    }

    #[test]
    fn pin_agent_auto_returns_slot_zero_on_first_pin() {
        let mut state = AppState::new();
        state.agent_sessions.push(make_agent("%1"));
        assert_eq!(state.pin_agent_auto("%1"), Some(0));
        assert_eq!(state.agent_sessions[0].pin_slot, Some(0));
    }

    #[test]
    fn pin_agent_to_empty_slot_assigns() {
        let mut state = AppState::new();
        state.agent_sessions.push(make_agent("%1"));
        state.pin_agent_to("%1", 3);
        assert_eq!(state.agent_sessions[0].pin_slot, Some(3));
    }

    #[test]
    fn pin_agent_to_same_slot_is_noop() {
        let mut state = AppState::new();
        let mut a = make_agent("%1");
        a.pin_slot = Some(3);
        state.agent_sessions.push(a);
        state.pin_agent_to("%1", 3);
        assert_eq!(state.agent_sessions[0].pin_slot, Some(3));
    }

    #[test]
    fn pin_agent_to_both_pinned_swaps_slots() {
        let mut state = AppState::new();
        let mut a = make_agent("%1");
        a.pin_slot = Some(2);
        let mut b = make_agent("%2");
        b.pin_slot = Some(5);
        state.agent_sessions.push(a);
        state.agent_sessions.push(b);

        state.pin_agent_to("%2", 2);
        assert_eq!(state.agent_sessions[0].pin_slot, Some(5));
        assert_eq!(state.agent_sessions[1].pin_slot, Some(2));
    }

    #[test]
    fn pin_agent_to_unpinned_into_occupied_re_auto_pins_occupant() {
        let mut state = AppState::new();
        for (i, slot) in [(1u32, 0u8), (2, 1), (3, 3)] {
            let mut a = make_agent(&format!("%{}", i));
            a.pin_slot = Some(slot);
            state.agent_sessions.push(a);
        }
        state.agent_sessions.push(make_agent("%4"));
        state.pin_agent_to("%4", 1);

        let by_id = |id: &str| {
            state
                .agent_sessions
                .iter()
                .find(|a| a.pane_id == id)
                .unwrap()
                .pin_slot
        };
        assert_eq!(by_id("%4"), Some(1));
        assert_eq!(by_id("%2"), Some(2));
    }

    #[test]
    fn unpin_agent_clears_slot() {
        let mut state = AppState::new();
        let mut a = make_agent("%1");
        a.pin_slot = Some(3);
        state.agent_sessions.push(a);
        state.unpin_agent("%1");
        assert_eq!(state.agent_sessions[0].pin_slot, None);
    }

    #[test]
    fn unpin_agent_noop_when_not_pinned() {
        let mut state = AppState::new();
        state.agent_sessions.push(make_agent("%1"));
        state.unpin_agent("%1");
        assert_eq!(state.agent_sessions[0].pin_slot, None);
    }

    #[test]
    fn pin_agent_auto_picks_lowest_free_slot() {
        let mut state = AppState::new();
        state.agent_sessions.push(make_agent("%1"));
        state.agent_sessions.push(make_agent("%2"));
        state.agent_sessions.push(make_agent("%3"));
        state.agent_sessions[0].pin_slot = Some(0);
        state.agent_sessions[1].pin_slot = Some(1);
        state.agent_sessions[2].pin_slot = Some(3);
        state.agent_sessions.push(make_agent("%4"));
        assert_eq!(state.pin_agent_auto("%4"), Some(2));
    }

    #[test]
    fn pin_agent_auto_returns_none_when_full() {
        let mut state = AppState::new();
        for i in 0..10 {
            let mut a = make_agent(&format!("%{}", i));
            a.pin_slot = Some(i as u8);
            state.agent_sessions.push(a);
        }
        state.agent_sessions.push(make_agent("%11"));
        assert_eq!(state.pin_agent_auto("%11"), None);
        assert_eq!(state.agent_sessions.last().unwrap().pin_slot, None);
    }

    #[test]
    fn pin_agent_auto_idempotent_for_already_pinned() {
        let mut state = AppState::new();
        let mut a = make_agent("%1");
        a.pin_slot = Some(5);
        state.agent_sessions.push(a);
        assert_eq!(state.pin_agent_auto("%1"), Some(5));
    }

    #[test]
    fn recent_sessions_sorted_by_recency() {
        let mut state = AppState::with_sample_data();
        let live = vec![
            ("tws_work_edge-device-pipeline_bugfix".to_string(), 1000),
            ("tws_work_edge-device-pipeline_hotfix".to_string(), 3000),
            ("tws_work_model-training-infra_main".to_string(), 2000),
        ];
        state.refresh_sessions(&live);

        let recent = state.recent_sessions(5);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].display_name, "hotfix"); // ts 3000
        assert_eq!(recent[1].display_name, "main"); // ts 2000
        assert_eq!(recent[2].display_name, "bugfix"); // ts 1000

        // Truncation works
        let recent2 = state.recent_sessions(2);
        assert_eq!(recent2.len(), 2);
        assert_eq!(recent2[0].display_name, "hotfix");
        assert_eq!(recent2[1].display_name, "main");
    }

    #[test]
    fn all_agents_flat_sorts_pinned_first_with_gaps_preserved() {
        let mut state = AppState::with_sample_data();
        let live = vec![("tws_work_edge-device-pipeline_one".to_string(), 0)];
        state.refresh_sessions(&live);

        use crate::model::{AgentSession, AgentType};
        let mk = |id: &str, slot: Option<u8>| AgentSession {
            agent_type: AgentType::ClaudeCode,
            tmux_session_name: "tws_work_edge-device-pipeline_one".into(),
            window_index: 0,
            pane_id: id.into(),
            display_name: id.into(),
            renamed: false,
            pin_slot: slot,
        };
        state.agent_sessions.push(mk("%a", None));
        state.agent_sessions.push(mk("%b", Some(3)));
        state.agent_sessions.push(mk("%c", Some(0)));
        state.agent_sessions.push(mk("%d", None));

        let flat = state.all_agents_flat();
        let ids: Vec<&str> = flat.iter().map(|f| f.pane_id.as_str()).collect();
        assert_eq!(ids, vec!["%c", "%b", "%a", "%d"]);
    }

    #[test]
    fn pin_slot_survives_agent_list_rebuild() {
        use crate::model::{AgentSession, AgentType};
        let mut state = AppState::new();
        state.agent_sessions.push(AgentSession {
            agent_type: AgentType::ClaudeCode,
            tmux_session_name: "tws_x_y_a".into(),
            window_index: 0,
            pane_id: "%1".into(),
            display_name: "claude".into(),
            renamed: false,
            pin_slot: Some(2),
        });

        let saved_pin = state
            .agent_sessions
            .iter()
            .find(|a| a.pane_id == "%1")
            .and_then(|a| a.pin_slot);

        state.agent_sessions.clear();
        state.agent_sessions.push(AgentSession {
            agent_type: AgentType::ClaudeCode,
            tmux_session_name: "tws_x_y_a".into(),
            window_index: 0,
            pane_id: "%1".into(),
            display_name: "claude".into(),
            renamed: false,
            pin_slot: None,
        });

        if let Some(agent) = state.agent_sessions.iter_mut().find(|a| a.pane_id == "%1") {
            agent.pin_slot = saved_pin;
        }

        assert_eq!(state.agent_sessions[0].pin_slot, Some(2));
    }

    fn make_pi_agent(pane_id: &str, tmux_session_name: &str) -> super::AgentSession {
        use super::super::model::{AgentSession, AgentType};
        AgentSession {
            agent_type: AgentType::Pi,
            tmux_session_name: tmux_session_name.into(),
            window_index: 0,
            pane_id: pane_id.into(),
            display_name: "pi".into(),
            renamed: false,
            pin_slot: None,
        }
    }

    fn make_pi_status(pane_id: &str, tmux_session_name: &str, work_state: PiWorkState) -> PiStatus {
        make_pi_status_updated(pane_id, tmux_session_name, work_state, 0)
    }

    fn make_pi_status_updated(
        pane_id: &str,
        tmux_session_name: &str,
        work_state: PiWorkState,
        updated_at_ms: u64,
    ) -> PiStatus {
        PiStatus {
            pane_id: pane_id.into(),
            tmux_session_name: tmux_session_name.into(),
            work_state,
            updated_at_ms,
        }
    }

    #[test]
    fn pi_indicator_stale_working_becomes_failed_without_live_pi_agent() {
        let mut state = AppState::new();
        state
            .pi_statuses
            .push(make_pi_status("%1", "tws_x_y_a", PiWorkState::Working));

        // A vanished process with a leftover "working" sidecar means Pi stopped
        // before it could report a terminal state.
        assert_eq!(
            state.pi_indicator_for_session("tws_x_y_a"),
            Some(PiIndicator::Failed)
        );

        state.agent_sessions.push(make_pi_agent("%1", "tws_x_y_a"));
        assert_eq!(
            state.pi_indicator_for_session("tws_x_y_a"),
            Some(PiIndicator::Working)
        );
    }

    #[test]
    fn pi_indicator_done_persists_without_live_agent() {
        let mut state = AppState::new();
        state
            .pi_statuses
            .push(make_pi_status("%1", "tws_x_y_a", PiWorkState::Done));
        assert_eq!(
            state.pi_indicator_for_session("tws_x_y_a"),
            Some(PiIndicator::Done)
        );
        // Other sessions are unaffected.
        assert_eq!(state.pi_indicator_for_session("tws_other"), None);
    }

    #[test]
    fn pi_indicator_terminal_outcomes_persist_without_live_agent() {
        for (work_state, expected) in [
            (PiWorkState::Cancelled, PiIndicator::Cancelled),
            (PiWorkState::Incomplete, PiIndicator::Incomplete),
            (PiWorkState::Failed, PiIndicator::Failed),
        ] {
            let mut state = AppState::new();
            state
                .pi_statuses
                .push(make_pi_status("%1", "tws_x_y_a", work_state));
            assert_eq!(state.pi_indicator_for_session("tws_x_y_a"), Some(expected));
        }
    }

    #[test]
    fn pi_indicator_retrying_requires_live_pi_agent() {
        let mut state = AppState::new();
        state
            .pi_statuses
            .push(make_pi_status("%1", "tws_x_y_a", PiWorkState::Retrying));
        assert_eq!(
            state.pi_indicator_for_session("tws_x_y_a"),
            Some(PiIndicator::Failed)
        );

        state.agent_sessions.push(make_pi_agent("%1", "tws_x_y_a"));
        assert_eq!(
            state.pi_indicator_for_session("tws_x_y_a"),
            Some(PiIndicator::Retrying)
        );
    }

    #[test]
    fn pi_indicator_working_wins_over_failed_and_done() {
        let mut state = AppState::new();
        state
            .pi_statuses
            .push(make_pi_status("%1", "tws_x_y_a", PiWorkState::Done));
        state
            .pi_statuses
            .push(make_pi_status("%2", "tws_x_y_a", PiWorkState::Failed));
        state
            .pi_statuses
            .push(make_pi_status("%3", "tws_x_y_a", PiWorkState::Working));
        state.agent_sessions.push(make_pi_agent("%3", "tws_x_y_a"));
        assert_eq!(
            state.pi_indicator_for_session("tws_x_y_a"),
            Some(PiIndicator::Working)
        );
    }

    #[test]
    fn pi_indicator_priority_orders_outcomes() {
        let mut state = AppState::new();
        state
            .pi_statuses
            .push(make_pi_status("%1", "tws_x_y_a", PiWorkState::Done));
        state
            .pi_statuses
            .push(make_pi_status("%2", "tws_x_y_a", PiWorkState::Cancelled));
        state
            .pi_statuses
            .push(make_pi_status("%3", "tws_x_y_a", PiWorkState::Incomplete));
        state
            .pi_statuses
            .push(make_pi_status("%4", "tws_x_y_a", PiWorkState::Failed));
        assert_eq!(
            state.pi_indicator_for_session("tws_x_y_a"),
            Some(PiIndicator::Failed)
        );
    }

    #[test]
    fn pi_indicator_ignores_older_status_for_same_live_pane() {
        let mut state = AppState::new();
        state.pi_statuses.push(make_pi_status_updated(
            "%22",
            "tws_init_etb_lp-678",
            PiWorkState::Working,
            1000,
        ));
        state.pi_statuses.push(make_pi_status_updated(
            "%22",
            "tws_init_etb_lp-678",
            PiWorkState::Done,
            2000,
        ));
        state
            .agent_sessions
            .push(make_pi_agent("%22", "tws_init_etb_lp-678"));

        assert_eq!(
            state.pi_indicator_for_session("tws_init_etb_lp-678"),
            Some(PiIndicator::Done)
        );
        assert_eq!(
            state.pi_indicator_for_agent(&make_pi_agent("%22", "tws_init_etb_lp-678")),
            Some(PiIndicator::Done)
        );
    }

    #[test]
    fn pi_indicator_bubbles_to_thread_and_collection() {
        let mut state = AppState::with_sample_data();
        let done_session = "tws_work_edge-device-pipeline_one".to_string();
        let working_session = "tws_work_edge-device-pipeline_two".to_string();
        state.refresh_sessions(&[(done_session.clone(), 0), (working_session.clone(), 0)]);
        state
            .pi_statuses
            .push(make_pi_status("%1", &done_session, PiWorkState::Done));
        state
            .pi_statuses
            .push(make_pi_status("%2", &working_session, PiWorkState::Working));
        state
            .agent_sessions
            .push(make_pi_agent("%2", &working_session));

        let thread_id = state.collections[0].threads[0].id;
        assert_eq!(
            state.pi_indicator_for_thread(thread_id),
            Some(PiIndicator::Working)
        );
        assert_eq!(
            state.pi_indicator_for_collection(0),
            Some(PiIndicator::Working)
        );
        assert_eq!(state.pi_indicator_for_collection(1), None);
    }

    #[test]
    fn pi_indicator_done_bubbles_when_no_working_session() {
        let mut state = AppState::with_sample_data();
        let session_name = "tws_work_edge-device-pipeline_one".to_string();
        state.refresh_sessions(&[(session_name.clone(), 0)]);
        state
            .pi_statuses
            .push(make_pi_status("%1", &session_name, PiWorkState::Done));

        let thread_id = state.collections[0].threads[0].id;
        assert_eq!(
            state.pi_indicator_for_thread(thread_id),
            Some(PiIndicator::Done)
        );
        assert_eq!(
            state.pi_indicator_for_collection(0),
            Some(PiIndicator::Done)
        );
    }

    #[test]
    fn pi_indicator_failed_bubbles_over_done() {
        let mut state = AppState::with_sample_data();
        let done_session = "tws_work_edge-device-pipeline_one".to_string();
        let failed_session = "tws_work_edge-device-pipeline_two".to_string();
        state.refresh_sessions(&[(done_session.clone(), 0), (failed_session.clone(), 0)]);
        state
            .pi_statuses
            .push(make_pi_status("%1", &done_session, PiWorkState::Done));
        state
            .pi_statuses
            .push(make_pi_status("%2", &failed_session, PiWorkState::Failed));

        let thread_id = state.collections[0].threads[0].id;
        assert_eq!(
            state.pi_indicator_for_thread(thread_id),
            Some(PiIndicator::Failed)
        );
        assert_eq!(
            state.pi_indicator_for_collection(0),
            Some(PiIndicator::Failed)
        );
    }

    #[test]
    fn pi_indicator_idle_and_shutdown_show_nothing() {
        let mut state = AppState::new();
        state
            .pi_statuses
            .push(make_pi_status("%1", "tws_x_y_a", PiWorkState::Idle));
        state
            .pi_statuses
            .push(make_pi_status("%2", "tws_x_y_a", PiWorkState::Shutdown));
        state.agent_sessions.push(make_pi_agent("%1", "tws_x_y_a"));
        assert_eq!(state.pi_indicator_for_session("tws_x_y_a"), None);
    }

    #[test]
    fn pi_indicator_for_agent_ignores_non_pi_agents() {
        let mut state = AppState::new();
        state
            .pi_statuses
            .push(make_pi_status("%1", "tws_x_y_a", PiWorkState::Working));
        // Claude agent on the same pane id → no Pi indicator.
        let claude = make_agent("%1");
        assert_eq!(state.pi_indicator_for_agent(&claude), None);

        let pi = make_pi_agent("%1", "tws_x_y_a");
        assert_eq!(
            state.pi_indicator_for_agent(&pi),
            Some(PiIndicator::Working)
        );
    }

    #[test]
    fn all_agents_flat_carries_pi_indicator() {
        let mut state = AppState::with_sample_data();
        let session_name = "tws_work_edge-device-pipeline_one".to_string();
        state.refresh_sessions(&[(session_name.clone(), 0)]);
        state
            .agent_sessions
            .push(make_pi_agent("%1", &session_name));
        state
            .pi_statuses
            .push(make_pi_status("%1", &session_name, PiWorkState::Working));

        let flat = state.all_agents_flat();
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].pi_indicator, Some(PiIndicator::Working));
    }
}
