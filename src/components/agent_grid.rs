use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Paragraph};

use crate::core::pi_status::{PiIndicator, WORKING_INDICATOR};
use crate::core::state::FlatAgent;
use crate::theme::Theme;

const MIN_TILE_WIDTH: u16 = 42;
const MIN_TILE_HEIGHT: u16 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridMetrics {
    pub columns: usize,
    pub rows: usize,
    pub page_start: usize,
    pub page_len: usize,
}

pub fn metrics(area: Rect, item_count: usize, cursor: usize) -> GridMetrics {
    let columns = usize::from((area.width / MIN_TILE_WIDTH).max(1));
    let rows = usize::from((area.height / MIN_TILE_HEIGHT).max(1));
    let page_capacity = columns.saturating_mul(rows).max(1);
    let page_start = (cursor / page_capacity) * page_capacity;
    let page_len = item_count.saturating_sub(page_start).min(page_capacity);
    GridMetrics {
        columns,
        rows,
        page_start,
        page_len,
    }
}

pub fn render(
    frame: &mut Frame,
    agents: &[FlatAgent],
    captures: &HashMap<String, Text<'static>>,
    cursor: usize,
    area: Rect,
    theme: &Theme,
) {
    if agents.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "No active coding agents",
                theme.thread_dim,
            )))
            .centered(),
            area,
        );
        return;
    }

    let grid = metrics(area, agents.len(), cursor.min(agents.len() - 1));
    let visible = &agents[grid.page_start..grid.page_start + grid.page_len];
    let visible_rows = visible.len().div_ceil(grid.columns).max(1);
    let row_constraints = vec![Constraint::Ratio(1, visible_rows as u32); visible_rows];
    let row_areas = Layout::vertical(row_constraints).split(area);

    for (row_idx, row_area) in row_areas.iter().enumerate() {
        let row_start = row_idx * grid.columns;
        let row_end = (row_start + grid.columns).min(visible.len());
        let row = &visible[row_start..row_end];
        let col_constraints = vec![Constraint::Ratio(1, row.len() as u32); row.len()];
        let col_areas = Layout::horizontal(col_constraints).split(*row_area);

        for (col_idx, agent) in row.iter().enumerate() {
            let absolute_idx = grid.page_start + row_start + col_idx;
            render_tile(
                frame,
                agent,
                captures.get(&agent.pane_id),
                absolute_idx == cursor,
                col_areas[col_idx],
                theme,
            );
        }
    }
}

fn render_tile(
    frame: &mut Frame,
    agent: &FlatAgent,
    content: Option<&Text<'static>>,
    selected: bool,
    area: Rect,
    theme: &Theme,
) {
    let status = match agent.pi_indicator {
        Some(PiIndicator::Working) => WORKING_INDICATOR,
        Some(PiIndicator::Retrying) => "↻",
        Some(PiIndicator::Done) => "✓",
        Some(PiIndicator::Cancelled) => "■",
        Some(PiIndicator::Incomplete) => "…",
        Some(PiIndicator::Failed) => "!",
        None => "·",
    };
    let pin = agent
        .pin_slot
        .map(|slot| format!(" [{}]", slot))
        .unwrap_or_default();
    let title = format!(
        " {} {} · {} / {}{} ",
        status,
        agent.agent_type.icon(),
        agent.thread_name,
        agent.session_display_name,
        pin,
    );
    let border_style = if selected {
        theme.modal_border
    } else {
        theme.preview_border
    };
    let title_style = if selected {
        theme.modal_title
    } else {
        theme.preview_title
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(title)
        .title_style(title_style);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }
    match content {
        Some(text) => {
            // Clone only the visible tail, not the complete captured pane.
            let start = text.lines.len().saturating_sub(inner.height as usize);
            let visible = Text::from(text.lines[start..].to_vec());
            frame.render_widget(Paragraph::new(visible), inner);
        }
        None => {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "Capturing pane…",
                    theme.preview_placeholder,
                ))),
                inner,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_use_multiple_columns_on_wide_terminals() {
        let result = metrics(Rect::new(0, 0, 120, 30), 8, 0);
        assert_eq!(result.columns, 2);
        assert_eq!(result.rows, 3);
        assert_eq!(result.page_len, 6);
    }

    #[test]
    fn metrics_page_to_the_selected_agent() {
        let result = metrics(Rect::new(0, 0, 84, 20), 9, 6);
        assert_eq!(result.columns, 2);
        assert_eq!(result.rows, 2);
        assert_eq!(result.page_start, 4);
        assert_eq!(result.page_len, 4);
    }
}
