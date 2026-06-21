use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;

use crate::theme::Theme;

const WIDTH_PERCENT: u16 = 70;

pub fn render(frame: &mut Frame, message: &str, area: Rect, theme: &Theme) {
    let height = popup_height(message, area);
    let popup = centered_rect(WIDTH_PERCENT, height, area);
    frame.render_widget(Clear, popup);

    let mut text: Vec<Line> = message.lines().map(Line::from).collect();
    if text.is_empty() {
        text.push(Line::from("Unknown error"));
    }
    text.push(Line::from(""));
    text.push(Line::from(vec![
        Span::styled("enter", theme.modal_title),
        Span::styled(" or ", theme.modal_muted),
        Span::styled("esc", theme.modal_title),
        Span::styled(" to close", theme.modal_muted),
    ]));

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .style(theme.background)
        .title(" Error ")
        .title_style(theme.error_title)
        .border_style(theme.error_border)
        .padding(Padding::new(2, 2, 1, 1));

    let paragraph = Paragraph::new(text)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup);
}

fn popup_height(message: &str, area: Rect) -> u16 {
    let popup_width = (area.width as usize * WIDTH_PERCENT as usize / 100).max(1);
    // Border left/right + padding left/right.
    let text_width = popup_width.saturating_sub(6).max(1);
    let message_lines = wrapped_line_count(message, text_width);
    // Message + blank line + close hint + top/bottom borders + top/bottom padding.
    let desired = (message_lines + 6).max(7);
    let max_height = area.height.max(1) as usize;
    desired.min(max_height).max(1) as u16
}

fn wrapped_line_count(message: &str, width: usize) -> usize {
    let width = width.max(1);
    let count: usize = message
        .lines()
        .map(|line| line.chars().count().saturating_sub(1) / width + 1)
        .sum();
    count.max(1)
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([Constraint::Length(height)]).flex(Flex::Center);
    let horizontal = Layout::horizontal([Constraint::Percentage(percent_x)]).flex(Flex::Center);
    let [area] = vertical.areas(area);
    let [area] = horizontal.areas(area);
    area
}
