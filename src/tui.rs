use std::io::{self, Stdout, stdout};

use crossterm::{
    cursor::MoveTo,
    execute,
    terminal::{
        Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode,
    },
};
use ratatui::{Terminal, prelude::CrosstermBackend};

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

pub fn init() -> io::Result<Tui> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(stdout()))
}

pub fn restore() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;
    Ok(())
}

/// Clear the terminal's main screen before handing it to an interactive child
/// or returning to the TUI. This prevents output from previous child processes
/// from flashing while alternate screens are switched.
pub fn clear_main_screen() -> io::Result<()> {
    execute!(stdout(), Clear(ClearType::All), MoveTo(0, 0))?;
    Ok(())
}
