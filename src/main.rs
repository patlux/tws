mod app;
mod automation;
mod components;
mod config;
mod core;
mod event;
mod git;
mod import;
mod theme;
mod tui;

use app::App;
use clap::{Parser, Subcommand};
use core::persistence;
use core::state::AppState;
use tws_mux as mux;

#[derive(Parser)]
#[command(name = "tws", about = "tmux and Zellij workspace manager", version)]
struct Cli {
    /// Multiplexer backend. Defaults to TWS_BACKEND or tmux.
    #[arg(long, value_name = "tmux|zellij")]
    backend: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Import existing sessions from the selected backend into tws
    Import,
    /// Create a collection if it does not already exist
    Collection {
        #[command(subcommand)]
        command: CollectionCommand,
    },
    /// Create a thread if it does not already exist
    Thread {
        #[command(subcommand)]
        command: ThreadCommand,
    },
    /// Create a managed session and optionally start a command in it
    Spawn(automation::SpawnArgs),
}

#[derive(Subcommand)]
enum CollectionCommand {
    /// Idempotently create a collection
    Ensure(automation::EnsureCollectionArgs),
}

#[derive(Subcommand)]
enum ThreadCommand {
    /// Idempotently create a thread in an existing collection
    Ensure(automation::EnsureThreadArgs),
}

fn main() -> std::io::Result<()> {
    let cli = Cli::parse();

    if let Some(backend) = cli.backend.as_deref()
        && let Err(error) = mux::configure(backend)
    {
        eprintln!("tws: {error}");
        std::process::exit(2);
    }
    if let Err(error) = mux::configure_config_dir(persistence::config_dir()) {
        eprintln!("tws: {error}");
        std::process::exit(2);
    }
    let needs_backend = !matches!(
        &cli.command,
        Some(Command::Collection { .. }) | Some(Command::Thread { .. })
    );
    if needs_backend
        && let Err(error) = mux::ensure_available()
    {
        eprintln!("tws: {error}");
        std::process::exit(1);
    }

    let result = match cli.command {
        Some(Command::Import) => import::run().map_err(|error| error.to_string()),
        Some(Command::Collection {
            command: CollectionCommand::Ensure(args),
        }) => automation::ensure_collection(args),
        Some(Command::Thread {
            command: ThreadCommand::Ensure(args),
        }) => automation::ensure_thread(args),
        Some(Command::Spawn(args)) => automation::spawn(args),
        None => return run_tui(),
    };

    if let Err(error) = result {
        eprintln!("tws: {error}");
        std::process::exit(1);
    }
    Ok(())
}

fn run_tui() -> std::io::Result<()> {
    let collections = match persistence::load() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            eprintln!("tws: state.json is corrupted: {}", e);
            eprintln!(
                "tws: fix or remove {}/state.json to start fresh",
                persistence::config_dir().display()
            );
            std::process::exit(1);
        }
        Err(e) => return Err(e),
    };
    let ui_state = persistence::load_ui();
    let state = AppState {
        collections,
        backend: mux::backend(),
        active_sessions: Vec::new(),
        worktree_sessions: Vec::new(),
        agent_sessions: Vec::new(),
        pi_statuses: Vec::new(),
        active_filter: false,
    };

    // Warn when another tws instance is live — state.json is last-writer-wins.
    let _lock = match persistence::acquire_instance_lock() {
        persistence::LockState::Acquired(guard) => Some(guard),
        persistence::LockState::HeldByOther(pid) => {
            eprintln!(
                "tws: another instance is running (pid {}) — changes may overwrite each other",
                pid
            );
            eprintln!("tws: press Enter to continue anyway, Ctrl+C to abort");
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            None
        }
    };

    let cfg = config::load_config();
    let palette = config::resolve_palette(&cfg);
    let theme = theme::Theme::build(&palette);
    let keymap = config::build_keymap(&cfg);
    let start_dirs = cfg.start_dirs.clone();
    let worktrees = cfg.worktrees.clone();

    let mut terminal = tui::init()?;
    let mut app = App::new(state, theme, keymap, start_dirs, worktrees);
    let result = app.run(&mut terminal, ui_state);
    tui::restore()?;
    if let Some(warning) = &app.exit_warning {
        eprintln!("tws: {}", warning);
    }
    result
}
