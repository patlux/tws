use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;

use crate::config::keys::{Action, KeyMode, Keymap};
use crate::theme::Theme;

const WIDTH_PERCENT: u16 = 74;
const POPUP_HEIGHT: u16 = 25;

pub fn render(frame: &mut Frame, area: Rect, theme: &Theme, keymap: &Keymap) {
    let popup = centered_rect(WIDTH_PERCENT, popup_height(area), area);
    frame.render_widget(Clear, popup);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .style(theme.background)
        .title(" Help ")
        .title_style(theme.modal_title)
        .border_style(theme.modal_border)
        .padding(Padding::new(2, 2, 1, 1));

    let paragraph = Paragraph::new(help_lines(theme, keymap))
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup);
}

fn help_lines(theme: &Theme, keymap: &Keymap) -> Vec<Line<'static>> {
    vec![
        section("Global", theme),
        row(format!("{} / Esc / Enter / q", keymap.key_hint(KeyMode::Normal, Action::Help)), "close help", theme),
        row(format!("{} / Ctrl+C", keymap.key_hint(KeyMode::Normal, Action::Quit)), "quit tws", theme),
        row(format!("{} / {}", keymap.key_hint(KeyMode::Normal, Action::Finder), keymap.key_hint(KeyMode::Normal, Action::ToggleView)), "find / toggle agents view", theme),
        row("1-5", "attach recent session", theme),
        blank(),
        section("Tree", theme),
        row(format!("{}  {}", keymap.key_hint_pair(KeyMode::Normal, Action::MoveUp, Action::MoveDown), keymap.key_hint_pair(KeyMode::Normal, Action::MoveLeft, Action::MoveRight)), "move, collapse, expand", theme),
        row("gg / G", "jump top / bottom", theme),
        row(keymap.key_hint(KeyMode::Normal, Action::Enter), "new, attach, or launch", theme),
        row(format!("{} / {}", keymap.key_hint(KeyMode::Normal, Action::Add), keymap.key_hint(KeyMode::Normal, Action::AddCollection)), "add thread / collection", theme),
        row(format!("{} / {} / {}", keymap.key_hint(KeyMode::Normal, Action::Rename), keymap.key_hint(KeyMode::Normal, Action::Delete), keymap.key_hint(KeyMode::Normal, Action::KillSession)), "rename / delete / kill", theme),
        row(format!("{} / {}", keymap.key_hint(KeyMode::Normal, Action::Hide), keymap.key_hint(KeyMode::Normal, Action::ShowHidden)), "hide / show hidden", theme),
        blank(),
        section("Notes", theme),
        row("Tab / Ctrl+→", "focus notes", theme),
        row(format!("{} / {}", keymap.key_hint(KeyMode::Notes, Action::OpenEditor), keymap.key_hint(KeyMode::Notes, Action::Cancel)), "edit / return to tree", theme),
        blank(),
        section("Agents", theme),
        row(format!("{} / {} / {}", keymap.key_hint(KeyMode::Agents, Action::Enter), keymap.key_hint(KeyMode::Agents, Action::PinAgent), keymap.key_hint(KeyMode::Agents, Action::PinAgentSlot)), "attach / pin / set slot", theme),
        row("0-9", "jump to pinned agent", theme),
    ]
}

fn section(title: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(title.to_string(), theme.modal_title))
}

fn row(key: impl Into<String>, description: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {:<16}", key.into()), theme.statusbar_key),
        Span::styled(description.to_string(), theme.modal_muted),
    ])
}

fn blank() -> Line<'static> {
    Line::from("")
}

fn popup_height(area: Rect) -> u16 {
    POPUP_HEIGHT.min(area.height.saturating_sub(2).max(1))
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([Constraint::Length(height)]).flex(Flex::Center);
    let horizontal = Layout::horizontal([Constraint::Percentage(percent_x)]).flex(Flex::Center);
    let [area] = vertical.areas(area);
    let [area] = horizontal.areas(area);
    area
}
