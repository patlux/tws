mod app;
mod components;
mod core;
mod event;
mod git;
mod import;
mod theme;
mod tmux;
mod config;
mod tui;

use app::App;
use clap::{Parser, Subcommand};
use core::persistence;
use core::state::AppState;

#[derive(Parser)]
#[command(name = "tws", about = "tmux workspace manager", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Import existing tmux sessions into tws
    Import,
}

fn main() -> std::io::Result<()> {
    let cli = Cli::parse();

    if std::process::Command::new("tmux")
        .arg("-V")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_err()
    {
        eprintln!("tws: tmux not found on PATH — install tmux first");
        std::process::exit(1);
    }

    match cli.command {
        Some(Command::Import) => import::run(),
        None => run_tui(),
    }
}

fn run_tui() -> std::io::Result<()> {
    let collections = match persistence::load() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            eprintln!("tws: state.json is corrupted: {}", e);
            eprintln!("tws: fix or remove ~/.config/tws/state.json to start fresh");
            std::process::exit(1);
        }
        Err(e) => return Err(e),
    };
    let ui_state = persistence::load_ui();
    let state = AppState {
        collections,
        active_sessions: Vec::new(),
        worktree_sessions: Vec::new(),
        agent_sessions: Vec::new(),
        pi_statuses: Vec::new(),
    };

    let cfg = config::load_config();
    let palette = config::resolve_palette(&cfg);
    let theme = theme::Theme::build(&palette);
    let note_stylesheet = theme::NoteStyleSheet::new(&palette);
    let keymap = config::build_keymap(&cfg);
    let start_dirs = cfg.start_dirs.clone();
    let worktrees = cfg.worktrees.clone();

    let mut terminal = tui::init()?;
    let mut app = App::new(state, theme, note_stylesheet, keymap, start_dirs, worktrees);
    let result = app.run(&mut terminal, ui_state);
    tui::restore()?;
    result
}
