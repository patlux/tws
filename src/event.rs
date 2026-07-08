use std::time::Duration;

use crossterm::event::{self, Event, KeyEvent, KeyEventKind};

pub enum AppEvent {
    Key(KeyEvent),
    /// Terminal was resized to the given width — the next draw must relayout.
    Resize(u16),
}

pub fn poll_event(timeout: Duration) -> std::io::Result<Option<AppEvent>> {
    if event::poll(timeout)? {
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                return Ok(Some(AppEvent::Key(key)));
            }
            Event::Resize(w, _) => return Ok(Some(AppEvent::Resize(w))),
            _ => {}
        }
    }
    Ok(None)
}
