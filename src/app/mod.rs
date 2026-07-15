use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};

use ansi_to_tui::IntoText;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, Paragraph};
use tui_tree_widget::{Tree, TreeState};

use crate::components::status_bar::{self, StatusContext};
use crate::components::{
    agent_grid, agent_preview, agents_view, confirm_modal, error_modal, finder_modal, help_modal,
    input_modal, recent_bar, tree_view,
};
use crate::config::keys::{Action, KeyMode, Keymap};
use crate::config::{self, StartDirConfig, WorktreeConfig};
use crate::core::model::{AgentSession, WorktreeSession, slugify};
use crate::core::persistence;
use crate::core::pi_status;
use crate::core::state::{AppState, FlatAgent, SelectedItem};
use crate::event;
use crate::git::worktrees::{self, DiscoverOptions};
use crate::theme::Theme;
use crate::tui::{self, Tui};
use tws_mux as mux;

mod draw;
mod input;
mod refresh;

/// Single-line edit buffer with cursor navigation for the input modal.
#[derive(Default)]
struct InputBuffer {
    content: String,
    /// Cursor position in chars (0..=len).
    cursor: usize,
}

impl InputBuffer {
    fn from(content: String) -> Self {
        let cursor = content.chars().count();
        Self { content, cursor }
    }

    fn byte_index(&self) -> usize {
        self.content
            .char_indices()
            .nth(self.cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.content.len())
    }

    fn insert(&mut self, c: char) {
        let idx = self.byte_index();
        self.content.insert(idx, c);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        let idx = self.byte_index();
        self.content.remove(idx);
    }

    fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.content.chars().count());
    }

    fn home(&mut self) {
        self.cursor = 0;
    }

    fn end(&mut self) {
        self.cursor = self.content.chars().count();
    }

    /// Ctrl+U: delete everything before the cursor.
    fn kill_to_start(&mut self) {
        let idx = self.byte_index();
        self.content.drain(..idx);
        self.cursor = 0;
    }
}

/// What the input modal is being used for.
enum InputPurpose {
    AddCollection,
    AddThread {
        collection_idx: usize,
    },
    RenameCollection {
        idx: usize,
    },
    RenameThread {
        col_idx: usize,
        thread_idx: usize,
    },
    NewSession {
        col_idx: usize,
        thread_idx: usize,
    },
    NewWorktree {
        thread_id: uuid::Uuid,
        repo: PathBuf,
        worktree_dir: Option<PathBuf>,
    },
    RenameSession {
        col_idx: usize,
        thread_idx: usize,
        old_tmux_name: String,
    },
    RenameAgent {
        pane_id: String,
    },
}

/// What the confirm modal is confirming.
enum ConfirmPurpose {
    DeleteCollection {
        idx: usize,
        name: String,
    },
    DeleteThread {
        col_idx: usize,
        thread_idx: usize,
        name: String,
    },
    KillSession {
        session_name: String,
    },
    KillAllSessions {
        col_idx: usize,
        thread_idx: usize,
        thread_name: String,
    },
    KillMarkedSessions {
        session_names: Vec<String>,
    },
    DeleteWorktree {
        repo: PathBuf,
        path: PathBuf,
        name: String,
        tmux_session_name: String,
        kill_session: bool,
    },
}

impl ConfirmPurpose {
    /// Multi-item destructive actions require an explicit `y` — Enter is
    /// ignored so a double-press can't wipe a whole collection/thread.
    fn requires_explicit_yes(&self) -> bool {
        matches!(
            self,
            ConfirmPurpose::DeleteCollection { .. }
                | ConfirmPurpose::DeleteThread { .. }
                | ConfirmPurpose::KillAllSessions { .. }
                | ConfirmPurpose::KillMarkedSessions { .. }
        )
    }
}

struct PendingWorktreeDelete {
    parent_selection: Vec<String>,
}

struct WorktreeDeleteResult {
    tmux_session_name: String,
    name: String,
    result: Result<(), String>,
}

struct WorktreeCreateResult {
    thread_id: uuid::Uuid,
    branch: String,
    path: PathBuf,
    result: Result<(), String>,
}

/// Payload of a background session refresh: live tmux sessions plus the raw
/// worktree discoveries per thread (name-building happens on the main thread).
struct SessionsPayload {
    live: Vec<(String, i64)>,
    discoveries: Vec<(
        uuid::Uuid,
        PathBuf,
        DiscoverOptions,
        Vec<worktrees::DiscoveredWorktree>,
    )>,
}

/// Captured pane content converted off-thread, tagged with its pane id.
struct PreviewResult {
    pane_id: String,
    text: Text<'static>,
}

struct GridPreviewResult {
    captures: HashMap<String, Text<'static>>,
}

/// Result of a background refresh or agent-only scan.
struct RefreshResult {
    /// Epoch at request time; results from before a synchronous refresh
    /// are stale and dropped.
    epoch: u64,
    /// `Some` for full refreshes, `None` for agent-only scans.
    sessions: Option<SessionsPayload>,
    agents: Vec<crate::core::model::AgentSession>,
    pi_statuses: Vec<pi_status::PiStatus>,
    trigger_mtime: Option<SystemTime>,
}

struct Notification {
    message: String,
    is_error: bool,
    created_at: Instant,
}

struct FinderState {
    query: String,
    /// (tmux_session_name, "Collection/Thread/session_label"), sorted by recency.
    all_entries: Vec<(String, String)>,
    /// Indices into all_entries matching current query.
    filtered: Vec<usize>,
    /// Cursor position within filtered.
    cursor: usize,
}

impl FinderState {
    fn new(entries: Vec<(String, String)>) -> Self {
        let filtered = (0..entries.len()).collect();
        Self {
            query: String::new(),
            all_entries: entries,
            filtered,
            cursor: 0,
        }
    }

    fn update_filter(&mut self) {
        let q = self.query.to_lowercase();
        self.filtered = if q.is_empty() {
            (0..self.all_entries.len()).collect()
        } else {
            self.all_entries
                .iter()
                .enumerate()
                .filter(|(_, (_, path))| path.to_lowercase().contains(&q))
                .map(|(i, _)| i)
                .collect()
        };
        if self.cursor >= self.filtered.len() {
            self.cursor = self.filtered.len().saturating_sub(1);
        }
    }
}

/// Which pane has keyboard focus during normal mode.
/// Which primary view is active.
enum ViewMode {
    Tree,
    Agents,
    AgentGrid,
}

enum Mode {
    Normal,
    Input {
        purpose: InputPurpose,
        buffer: InputBuffer,
    },
    Confirm {
        purpose: ConfirmPurpose,
    },
    Error {
        message: String,
    },
    Help {
        scroll: u16,
    },
    Finder {
        state: FinderState,
    },
    ThreadPicker {
        state: FinderState,
        session_name: String,
        session_label: String,
    },
}

pub struct App {
    pub state: AppState,
    pub tree_state: TreeState<String>,
    pub running: bool,
    /// Warning to print after the terminal is restored (raw mode swallows
    /// eprintln), e.g. a failed UI-state save on exit.
    pub exit_warning: Option<String>,
    mode: Mode,
    last_refresh: Instant,
    last_agent_trigger_mtime: Option<SystemTime>,
    flash: Option<(String, Instant)>,
    /// Cached pane content for agent preview, converted to ratatui Text.
    preview_content: Option<Text<'static>>,
    /// Which pane_id the cached preview is for (invalidate on selection change).
    preview_pane_id: Option<String>,
    /// When the preview was last refreshed.
    last_preview_refresh: Instant,
    /// Whether to show the tree or agents flat-list view.
    view_mode: ViewMode,
    /// Cursor position shared by the agent list and grid.
    agent_list_cursor: usize,
    /// Latest live captures keyed by agent pane id.
    grid_captures: HashMap<String, Text<'static>>,
    last_grid_refresh: Instant,
    grid_refresh_in_flight: bool,
    /// Runtime theme derived from the palette.
    theme: Theme,
    /// Key bindings (user-configurable).
    keymap: Keymap,
    /// Start-directory defaults loaded from config.toml.
    start_dir_configs: Vec<StartDirConfig>,
    /// Git worktree integrations loaded from config.toml.
    worktree_configs: Vec<WorktreeConfig>,
    /// Pins loaded from UiState waiting to be reapplied at first scan.
    /// Drained on first successful agent rebuild; entries whose pane_id
    /// is no longer live are silently dropped.
    pending_pin_restore: Vec<(String, u8)>,
    /// While Some, the next keystroke in agents view assigns this pane to a slot
    /// (digit 0-9), or cancels (Esc / any other key). Captured by pressing `P`.
    pin_assign_pending: Option<String>,
    /// True after a first `g` in normal navigation; the next `g` jumps to top.
    jump_to_top_pending: bool,
    /// Active tmux sessions marked for batch operations, keyed by tmux session name.
    marked_sessions: HashSet<String>,
    /// Worktree delete jobs currently running in background threads, keyed by tmux session name.
    pending_worktree_deletes: HashMap<String, PendingWorktreeDelete>,
    /// Worktree create jobs currently running in background threads, keyed by target path.
    pending_worktree_creates: HashMap<PathBuf, String>,
    /// Cached worktree discoveries per (repo, options) so expand/collapse
    /// keypresses don't shell out to git. Refreshed by the periodic background
    /// refresh, invalidated on worktree deletion.
    worktree_cache:
        HashMap<(PathBuf, DiscoverOptions), (Instant, Vec<worktrees::DiscoveredWorktree>)>,
    worktree_delete_tx: Sender<WorktreeDeleteResult>,
    worktree_delete_rx: Receiver<WorktreeDeleteResult>,
    worktree_create_tx: Sender<WorktreeCreateResult>,
    worktree_create_rx: Receiver<WorktreeCreateResult>,
    refresh_tx: Sender<RefreshResult>,
    refresh_rx: Receiver<RefreshResult>,
    preview_tx: Sender<PreviewResult>,
    preview_rx: Receiver<PreviewResult>,
    grid_preview_tx: Sender<GridPreviewResult>,
    grid_preview_rx: Receiver<GridPreviewResult>,
    /// True while a background pane capture is running.
    preview_in_flight: bool,
    /// True while a background full refresh is running.
    refresh_in_flight: bool,
    /// True while a background agent-only scan is running.
    scan_in_flight: bool,
    /// Bumped by synchronous refreshes to invalidate in-flight async results.
    refresh_epoch: u64,
    /// Selection restored from UiState, applied after the first refresh lands
    /// (session paths only resolve once active_sessions is populated).
    pending_selection_restore: Option<Vec<String>>,
    /// Debounce for hook-triggered agent scans.
    last_trigger_scan: Instant,
    notification: Option<Notification>,
    animation_start: Instant,
    /// Set whenever state changed in a way that requires a repaint.
    /// The event loop only draws when this is set or an animation is active.
    needs_redraw: bool,
    /// Last known terminal width, updated on resize events. Used to size
    /// the markdown render without a per-frame terminal size syscall.
    last_width: u16,
}

/// How often to poll the active multiplexer for session changes (seconds).
const REFRESH_INTERVAL: Duration = Duration::from_secs(30);

/// How long cached worktree discoveries stay valid for the synchronous
/// (keypress-driven) refresh path.
const WORKTREE_CACHE_TTL: Duration = Duration::from_secs(30);

/// How often to re-capture the agent pane preview (seconds).
const PREVIEW_REFRESH_INTERVAL: Duration = Duration::from_secs(3);
const GRID_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

/// Minimum spacing between hook-triggered agent scans.
const TRIGGER_SCAN_DEBOUNCE: Duration = Duration::from_millis(500);

const NOTIFICATION_DURATION: Duration = Duration::from_secs(4);
const DELETE_SPINNER: [&str; 4] = ["◐", "◓", "◑", "◒"];
/// How long "done" markers survive after the Pi pane died before pruning.
const PI_STATUS_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
            return home.join(rest);
        }
    PathBuf::from(path)
}

fn short_head(head: &str) -> &str {
    head.get(..8).unwrap_or(head)
}

fn parent_selection_path(selected: &[String]) -> Vec<String> {
    selected
        .iter()
        .take(selected.len().saturating_sub(1))
        .cloned()
        .collect()
}

fn render_notification(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    message: &str,
    is_error: bool,
    theme: &Theme,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let width = ((message.chars().count() as u16) + 4).min(area.width);
    let height = if area.height >= 3 && width >= 4 { 3 } else { 1 };
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width),
        y: area.y,
        width,
        height,
    };
    let style = if is_error {
        theme.worktree_prunable
    } else {
        theme.flash
    };

    frame.render_widget(Clear, rect);
    if height == 3 {
        let block = Block::bordered().border_style(style);
        frame.render_widget(
            Paragraph::new(message.to_string())
                .style(style)
                .block(block),
            rect,
        );
    } else {
        frame.render_widget(
            Paragraph::new(Line::styled(message.to_string(), style)),
            rect,
        );
    }
}

impl App {
    pub fn new(
        state: AppState,
        theme: Theme,
        keymap: Keymap,
        start_dir_configs: Vec<StartDirConfig>,
        worktree_configs: Vec<WorktreeConfig>,
    ) -> Self {
        let (worktree_delete_tx, worktree_delete_rx) = mpsc::channel();
        let (worktree_create_tx, worktree_create_rx) = mpsc::channel();
        let (refresh_tx, refresh_rx) = mpsc::channel();
        let (preview_tx, preview_rx) = mpsc::channel();
        let (grid_preview_tx, grid_preview_rx) = mpsc::channel();
        Self {
            state,
            tree_state: TreeState::default(),
            running: true,
            exit_warning: None,
            mode: Mode::Normal,
            last_refresh: Instant::now(),
            last_agent_trigger_mtime: None,
            flash: None,
            preview_content: None,
            preview_pane_id: None,
            last_preview_refresh: Instant::now(),
            view_mode: ViewMode::Tree,
            agent_list_cursor: 0,
            grid_captures: HashMap::new(),
            last_grid_refresh: Instant::now()
                .checked_sub(GRID_REFRESH_INTERVAL)
                .unwrap_or_else(Instant::now),
            grid_refresh_in_flight: false,
            theme,
            keymap,
            start_dir_configs,
            worktree_configs,
            pending_pin_restore: Vec::new(),
            pin_assign_pending: None,
            jump_to_top_pending: false,
            marked_sessions: HashSet::new(),
            pending_worktree_deletes: HashMap::new(),
            pending_worktree_creates: HashMap::new(),
            worktree_cache: HashMap::new(),
            worktree_delete_tx,
            worktree_delete_rx,
            worktree_create_tx,
            worktree_create_rx,
            refresh_tx,
            refresh_rx,
            preview_tx,
            preview_rx,
            grid_preview_tx,
            grid_preview_rx,
            preview_in_flight: false,
            refresh_in_flight: false,
            scan_in_flight: false,
            refresh_epoch: 0,
            pending_selection_restore: None,
            last_trigger_scan: Instant::now()
                .checked_sub(TRIGGER_SCAN_DEBOUNCE)
                .unwrap_or_else(Instant::now),
            notification: None,
            animation_start: Instant::now(),
            needs_redraw: true,
            last_width: crossterm::terminal::size().map(|(w, _)| w).unwrap_or(80),
        }
    }

    fn set_flash(&mut self, msg: &str) {
        self.flash = Some((msg.to_string(), Instant::now()));
    }

    fn set_notification(&mut self, message: impl Into<String>, is_error: bool) {
        self.notification = Some(Notification {
            message: message.into(),
            is_error,
            created_at: Instant::now(),
        });
    }

    fn set_error(&mut self, msg: impl Into<String>) {
        let message = msg.into();
        let message = if message.trim().is_empty() {
            "Unknown error".to_string()
        } else {
            message
        };
        self.flash = None;
        self.mode = Mode::Error { message };
    }

    fn worktree_spinner_frame(&self) -> &'static str {
        let ticks = self.animation_start.elapsed().as_millis() / 180;
        DELETE_SPINNER[(ticks as usize) % DELETE_SPINNER.len()]
    }

    fn worktree_delete_progress_labels(&self) -> HashMap<String, String> {
        self.pending_worktree_deletes
            .keys()
            .map(|session_name| {
                let name = self
                    .state
                    .find_worktree_by_tmux_name(session_name)
                    .map(|w| w.display_name.clone())
                    .or_else(|| {
                        self.state
                            .active_sessions
                            .iter()
                            .find(|s| s.tmux_session_name == *session_name)
                            .map(|s| s.display_name.clone())
                    })
                    .unwrap_or_else(|| "worktree".to_string());
                (session_name.clone(), format!("deleting {}", name))
            })
            .collect()
    }

    fn poll_worktree_delete_results(&mut self) {
        while let Ok(result) = self.worktree_delete_rx.try_recv() {
            self.needs_redraw = true;
            // A worktree was (possibly) removed — cached discoveries are stale.
            self.worktree_cache.clear();
            let pending = self
                .pending_worktree_deletes
                .remove(&result.tmux_session_name);
            match result.result {
                Ok(()) => {
                    self.marked_sessions.remove(&result.tmux_session_name);
                    self.do_refresh_sessions();
                    if let Some(pending) = pending
                        && !pending.parent_selection.is_empty()
                    {
                            self.tree_state.select(pending.parent_selection);
                        }
                    self.set_notification(format!("Deleted worktree {}", result.name), false);
                }
                Err(err) => {
                    self.do_refresh_sessions();
                    self.set_notification(format!("Delete failed: {}", err), true);
                }
            }
        }
    }

    fn poll_worktree_create_results(&mut self) {
        while let Ok(result) = self.worktree_create_rx.try_recv() {
            self.needs_redraw = true;
            self.pending_worktree_creates.remove(&result.path);
            match result.result {
                Ok(()) => {
                    // A worktree was created — cached discoveries are stale.
                    self.worktree_cache.clear();
                    self.refresh_worktree_sessions();
                    self.select_worktree_by_path(result.thread_id, &result.path);
                    self.set_notification(format!("Created worktree {}", result.branch), false);
                }
                Err(err) => {
                    self.set_notification(format!("Create failed: {}", err), true);
                }
            }
        }
    }

    fn prune_marked_sessions(&mut self) {
        let live: HashSet<String> = self
            .state
            .active_sessions
            .iter()
            .map(|s| s.tmux_session_name.clone())
            .collect();
        self.marked_sessions.retain(|name| live.contains(name));
    }

    fn selected_session_name_for_marking(&self) -> Option<String> {
        match self.state.resolve_selection(self.tree_state.selected()) {
            SelectedItem::Session(col_idx, thread_idx, sess_idx) => {
                let thread_id = self
                    .state
                    .collections
                    .get(col_idx)?
                    .threads
                    .get(thread_idx)?
                    .id;
                self.state
                    .sessions_for_thread(thread_id)
                    .get(sess_idx)
                    .map(|s| s.tmux_session_name.clone())
            }
            SelectedItem::Agent(col_idx, thread_idx, sess_idx, _) => {
                let thread_id = self
                    .state
                    .collections
                    .get(col_idx)?
                    .threads
                    .get(thread_idx)?
                    .id;
                self.state
                    .sessions_for_thread(thread_id)
                    .get(sess_idx)
                    .map(|s| s.tmux_session_name.clone())
            }
            _ => None,
        }
    }

    fn toggle_mark_current_session(&mut self) {
        let Some(session_name) = self.selected_session_name_for_marking() else {
            self.set_flash("Select a session to mark");
            return;
        };
        if !self.marked_sessions.insert(session_name.clone()) {
            self.marked_sessions.remove(&session_name);
            self.set_flash("Session unmarked");
        } else {
            self.set_flash("Session marked");
        }
    }

    fn clear_marked_sessions(&mut self) {
        if self.marked_sessions.is_empty() {
            self.set_flash("No marked sessions");
            return;
        }
        self.marked_sessions.clear();
        self.set_flash("Marks cleared");
    }

    fn marked_session_names(&self) -> Vec<String> {
        self.state
            .active_sessions
            .iter()
            .filter(|session| self.marked_sessions.contains(&session.tmux_session_name))
            .map(|session| session.tmux_session_name.clone())
            .collect()
    }

    pub fn run(
        &mut self,
        terminal: &mut Tui,
        ui_state: persistence::UiState,
    ) -> std::io::Result<()> {
        // Stage pin restore before the initial scan so the first agent scan picks it up.
        self.pending_pin_restore = ui_state.pins;

        // Restore expansion state before the initial refresh so Git worktrees are
        // discovered only for threads the user actually has expanded.
        for path in ui_state.open_nodes {
            self.tree_state.open(path);
        }

        // Stale-while-revalidate startup: draw the first frame from persisted
        // state immediately and load sessions/worktrees/agents in the background.
        self.request_refresh();

        // Restore last selection now (collections/threads resolve without live
        // sessions) and stage it for re-application once the first refresh
        // lands, so session/agent selections survive the async load.
        if let Some(sel) = ui_state.selected {
            self.tree_state.select(sel.clone());
            self.pending_selection_restore = Some(sel);
        }
        // Restore view mode and agents cursor
        if ui_state.agent_grid_active {
            self.view_mode = ViewMode::AgentGrid;
        } else if ui_state.agents_view_active {
            self.view_mode = ViewMode::Agents;
        }
        self.state.active_filter = ui_state.active_filter;
        self.agent_list_cursor = ui_state.agent_list_cursor;

        while self.running {
            self.poll_worktree_delete_results();
            self.poll_worktree_create_results();
            self.poll_refresh_results();

            // Periodic session refresh (includes agent scan), off-thread.
            if self.last_refresh.elapsed() >= REFRESH_INTERVAL && !self.refresh_in_flight {
                self.request_refresh();
            }

            // Hook-triggered agent scan (sub-250ms latency), debounced + off-thread.
            if self.check_agent_trigger()
                && !self.scan_in_flight
                && !self.refresh_in_flight
                && self.last_trigger_scan.elapsed() >= TRIGGER_SCAN_DEBOUNCE
            {
                self.request_agent_scan();
            }

            // Refresh the selected-agent sidebar or all panes in grid view.
            let selected = self.resolve_current_selected();
            if matches!(self.view_mode, ViewMode::AgentGrid) {
                self.request_grid_refresh();
                self.poll_grid_preview_results();
            } else {
            self.refresh_preview(&selected);
            self.poll_preview_results();
            }

            // Dirty-flag rendering: skip the (expensive) full redraw unless
            // something changed or a short-lived animation/message is on screen.
            if self.needs_redraw || self.animation_active() {
                self.draw(terminal)?;
                self.needs_redraw = false;
            }
            match event::poll_event(Duration::from_millis(250))? {
                Some(event::AppEvent::Resize(w)) => {
                    self.last_width = w;
                    self.needs_redraw = true;
                }
                Some(event::AppEvent::Key(key)) => {
                    // Any keypress can change visible state.
                    self.needs_redraw = true;
                    // User is navigating — don't yank the selection later.
                    self.pending_selection_restore = None;
                    // Ctrl+C always quits
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('c')
                    {
                        self.running = false;
                        continue;
                    }

                    match &self.mode {
                        Mode::Normal => {
                            self.handle_normal_mode(key.code, key.modifiers, terminal)?;
                        }
                        Mode::Input { .. } => {
                            self.handle_input_key(key.code, key.modifiers, terminal)?
                        }
                        Mode::Confirm { .. } => self.handle_confirm_key(key.code, key.modifiers),
                        Mode::Error { .. } => self.handle_error_key(key.code, key.modifiers),
                        Mode::Help { .. } => self.handle_help_key(key.code),
                        Mode::Finder { .. } => {
                            self.handle_finder_key(key.code, key.modifiers, terminal)?
                        }
                        Mode::ThreadPicker { .. } => {
                            self.handle_thread_picker_key(key.code, key.modifiers)?
                        }
                    }
                }
                None => {}
            }
        }
        self.save_ui_state();
        Ok(())
    }

    /// True while something on screen animates or a transient message is
    /// visible — forces tick-rate redraws for deletion progress and messages.
    fn animation_active(&self) -> bool {
        !self.pending_worktree_deletes.is_empty()
            || !self.pending_worktree_creates.is_empty()
            || self.flash.is_some()
            || self.notification.is_some()
    }

    fn save_state(&mut self) {
        if let Err(e) = persistence::save(&self.state.collections) {
            // eprintln! is invisible in raw-mode/alternate screen — surface it.
            self.set_error(format!("Failed to save state: {}", e));
        }
    }

    fn save_ui_state(&mut self) {
        let open_nodes: Vec<Vec<String>> = self.tree_state.opened().iter().cloned().collect();
        let selected = {
            let sel = self.tree_state.selected();
            if sel.is_empty() || matches!(self.state.resolve_selection(sel), SelectedItem::None) {
                None
            } else {
                Some(sel.to_vec())
            }
        };
        let agents_view_active = matches!(self.view_mode, ViewMode::Agents);
        let agent_grid_active = matches!(self.view_mode, ViewMode::AgentGrid);
        let agent_list_cursor = self.agent_list_cursor;
        let pins: Vec<(String, u8)> = self
            .state
            .agent_sessions
            .iter()
            .filter_map(|a| a.pin_slot.map(|s| (a.pane_id.clone(), s)))
            .collect();
        let active_filter = self.state.active_filter;
        let ui = persistence::UiState {
            open_nodes,
            selected,
            agents_view_active,
            agent_grid_active,
            active_filter,
            agent_list_cursor,
            pins,
        };
        if let Err(e) = persistence::save_ui(&ui) {
            self.exit_warning = Some(format!("Failed to save UI state: {}", e));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::palette::Palette;

    #[test]
    fn working_pi_status_does_not_force_tick_redraws() {
        let palette = Palette::default();
        let mut app = App::new(
            AppState::empty(mux::Backend::Tmux),
            Theme::build(&palette),
            Keymap::default_bindings(),
            Vec::new(),
            Vec::new(),
        );
        app.state.pi_statuses.push(pi_status::PiStatus {
            pane_id: "%1".to_string(),
            tmux_session_name: "tws_test".to_string(),
            work_state: pi_status::PiWorkState::Working,
            updated_at_ms: 1,
        });

        assert!(!app.animation_active());
    }
}
