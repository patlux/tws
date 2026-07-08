use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};
use ratatui::Frame;

use crate::theme::Theme;

pub fn render(frame: &mut Frame, message: &str, enter_confirms: bool, area: Rect, theme: &Theme) {
    let popup = centered_rect(50, 7, area);
    frame.render_widget(Clear, popup);

    let confirm_hint = if enter_confirms { "y/Enter" } else { "y" };
    let text = vec![
        Line::from(message),
        Line::from(""),
        Line::from(vec![
            Span::styled(confirm_hint, theme.modal_title),
            Span::styled(" to confirm  ", theme.modal_muted),
            Span::styled("esc", theme.modal_title),
            Span::styled(" to cancel", theme.modal_muted),
        ]),
    ];

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .style(theme.background)
        .title(" Confirm ")
        .title_style(theme.modal_title)
        .border_style(theme.modal_border)
        .padding(Padding::new(1, 1, 1, 0));

    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, popup);
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([Constraint::Length(height)]).flex(Flex::Center);
    let horizontal = Layout::horizontal([Constraint::Percentage(percent_x)]).flex(Flex::Center);
    let [area] = vertical.areas(area);
    let [area] = horizontal.areas(area);
    area
}
