use ratatui::style::Color;
use ratatui::text::Text;

/// Preserve the application background when ANSI input resets its background.
pub(crate) fn clear_reset_backgrounds(text: &mut Text<'static>) {
    for line in &mut text.lines {
        if line.style.bg == Some(Color::Reset) {
            line.style.bg = None;
        }
        for span in &mut line.spans {
            if span.style.bg == Some(Color::Reset) {
                span.style.bg = None;
            }
        }
    }
}
