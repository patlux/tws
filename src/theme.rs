use ratatui::style::{Color, Modifier, Style};

use crate::config::palette::Palette;

// ---------------------------------------------------------------------------
// Helper functions for color manipulation
// ---------------------------------------------------------------------------

/// Brighten an RGB color by adding `amount` to each channel, capped at 255.
fn brighten(color: Color, amount: u8) -> Color {
    if let Color::Rgb(r, g, b) = color {
        Color::Rgb(
            r.saturating_add(amount),
            g.saturating_add(amount),
            b.saturating_add(amount),
        )
    } else {
        color
    }
}

/// Darken a color by blending it toward `target` by `fraction` (0.0 = unchanged, 1.0 = target).
fn darken_toward(color: Color, target: Color, fraction: f32) -> Color {
    if let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (color, target) {
        let blend = |a: u8, b: u8| -> u8 {
            (a as f32 + (b as f32 - a as f32) * fraction).round() as u8
        };
        Color::Rgb(blend(r1, r2), blend(g1, g2), blend(b1, b2))
    } else {
        color
    }
}

/// Midpoint between two colors.
fn midpoint(a: Color, b: Color) -> Color {
    darken_toward(a, b, 0.5)
}

// ---------------------------------------------------------------------------
// Theme — all styles derived from a Palette
// ---------------------------------------------------------------------------

pub struct Theme {
    // Full-screen background fill
    pub background: Style,

    // Raw color (for components that need Color, not Style)
    pub dim_text: Color,

    // Tree hierarchy
    pub collection: Style,
    pub thread: Style,
    pub thread_dim: Style,
    pub session: Style,
    pub worktree: Style,
    pub worktree_meta: Style,
    pub worktree_prunable: Style,
    pub highlight: Style,

    // Pin badge in agents view
    pub pin_badge: Style,

    // Chrome
    pub separator: Style,

    // Status bar
    pub statusbar_key: Style,
    pub statusbar_desc: Style,

    // Cursor
    pub cursor: Style,

    // Modals
    pub modal_border: Style,
    pub modal_title: Style,
    pub modal_muted: Style,

    // Error modal
    pub error_border: Style,
    pub error_title: Style,

    // Empty state
    pub empty_title: Style,
    pub empty_hint: Style,

    // Agents
    pub agent: Style,
    pub agent_connector: Style,
    /// Pi work-status indicator while actively working.
    pub pi_working: Style,
    /// Pi work-status indicator: retry/cancel/incomplete warning.
    pub pi_warning: Style,
    /// Pi work-status indicator: checkmark when finished.
    pub pi_done: Style,
    /// Pi work-status indicator: technical failure.
    pub pi_failed: Style,

    // Badges
    pub badge_dot: Style,
    pub badge_count: Style,

    // Flash
    pub flash: Style,

    // Recent bar
    pub recent_number: Style,
    pub recent_name: Style,

    // Scrollbar
    pub scrollbar_thumb: Style,
    pub scrollbar_track: Style,

    // Agent preview
    pub preview_border: Style,
    pub preview_title: Style,
    pub preview_placeholder: Style,
}

impl Theme {
    pub fn build(p: &Palette) -> Self {
        let dim_text = p.dim;
        let muted_text = p.muted;
        let subtle_border = p.border;

        // Derived: statusbar key is between dim and muted
        let statusbar_key_color = midpoint(p.dim, p.muted);
        // Derived: statusbar desc is between muted and border
        let statusbar_desc_color = midpoint(p.muted, p.border);
        // Derived: agent color is a light gray (between fg and dim)
        let agent_color = midpoint(p.fg, p.dim);

        Self {
            // Full-screen background
            background: Style::new().bg(p.bg),

            // Raw color
            dim_text,

            // Tree hierarchy
            collection: Style::new()
                .fg(brighten(p.accent, 16))
                .add_modifier(Modifier::BOLD),
            thread: Style::new().fg(p.accent),
            thread_dim: Style::new().fg(darken_toward(p.accent, p.border, 0.5)),
            session: Style::new().fg(p.green),
            worktree: Style::new().fg(darken_toward(p.green, p.border, 0.45)),
            worktree_meta: Style::new().fg(muted_text),
            worktree_prunable: Style::new().fg(darken_toward(p.accent, p.border, 0.55)),
            highlight: Style::new()
                .fg(p.bg)
                .bg(p.accent)
                .add_modifier(Modifier::BOLD),
            pin_badge: Style::new().fg(p.accent).add_modifier(Modifier::BOLD),

            // Chrome
            separator: Style::new().fg(subtle_border),

            // Status bar
            statusbar_key: Style::new().fg(statusbar_key_color),
            statusbar_desc: Style::new().fg(statusbar_desc_color),

            // Cursor
            cursor: Style::new()
                .fg(p.accent)
                .add_modifier(Modifier::SLOW_BLINK),

            // Modals
            modal_border: Style::new().fg(p.accent),
            modal_title: Style::new().fg(p.accent).add_modifier(Modifier::BOLD),
            modal_muted: Style::new().fg(muted_text),

            // Error modal
            error_border: Style::new().fg(brighten(p.accent, 48)),
            error_title: Style::new().fg(brighten(p.accent, 48)).add_modifier(Modifier::BOLD),

            // Empty state
            empty_title: Style::new().fg(p.accent).add_modifier(Modifier::BOLD),
            empty_hint: Style::new().fg(muted_text),

            // Agents
            agent: Style::new().fg(agent_color),
            agent_connector: Style::new().fg(muted_text),
            pi_working: Style::new().fg(p.accent).add_modifier(Modifier::BOLD),
            pi_warning: Style::new().fg(p.orange).add_modifier(Modifier::BOLD),
            pi_done: Style::new().fg(p.green),
            pi_failed: Style::new().fg(p.red).add_modifier(Modifier::BOLD),

            // Badges
            badge_dot: Style::new().fg(p.green),
            badge_count: Style::new().fg(muted_text),

            // Flash
            flash: Style::new().fg(p.accent).add_modifier(Modifier::BOLD),

            // Recent bar
            recent_number: Style::new().fg(p.accent).add_modifier(Modifier::BOLD),
            recent_name: Style::new().fg(dim_text),

            // Scrollbar
            scrollbar_thumb: Style::new().fg(muted_text),
            scrollbar_track: Style::new().fg(subtle_border),

            // Agent preview
            preview_border: Style::new().fg(subtle_border),
            preview_title: Style::new().fg(dim_text),
            preview_placeholder: Style::new().fg(muted_text),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_matches_old_constants() {
        let p = Palette::default();
        let t = Theme::build(&p);

        // Collection: bold, brightened accent
        assert_eq!(
            t.collection,
            Style::new()
                .fg(Color::Rgb(220, 136, 66))
                .add_modifier(Modifier::BOLD)
        );
        // Thread: plain accent
        assert_eq!(t.thread, Style::new().fg(Color::Rgb(204, 120, 50)));
        // Session: green
        assert_eq!(t.session, Style::new().fg(Color::Rgb(130, 180, 130)));
        // Pi warning and failure statuses use distinct semantic colors.
        assert_eq!(
            t.pi_warning,
            Style::new()
                .fg(Color::Rgb(224, 157, 73))
                .add_modifier(Modifier::BOLD)
        );
        assert_eq!(
            t.pi_failed,
            Style::new()
                .fg(Color::Rgb(220, 90, 80))
                .add_modifier(Modifier::BOLD)
        );
        // Highlight: bg=accent, fg=bg, bold
        assert_eq!(
            t.highlight,
            Style::new()
                .fg(Color::Rgb(30, 30, 30))
                .bg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD)
        );
        // Modal border = accent
        assert_eq!(t.modal_border, Style::new().fg(Color::Rgb(204, 120, 50)));
    }

    #[test]
    fn custom_palette_changes_derived_styles() {
        let p = Palette {
            accent: Color::Rgb(255, 0, 0),
            ..Palette::default()
        };
        let t = Theme::build(&p);

        // Thread should use the new accent
        assert_eq!(t.thread, Style::new().fg(Color::Rgb(255, 0, 0)));
        // Collection brightened
        assert_eq!(
            t.collection,
            Style::new()
                .fg(Color::Rgb(255, 16, 16))
                .add_modifier(Modifier::BOLD)
        );
        // Highlight bg should be new accent
        assert_eq!(
            t.highlight,
            Style::new()
                .fg(Color::Rgb(30, 30, 30))
                .bg(Color::Rgb(255, 0, 0))
                .add_modifier(Modifier::BOLD)
        );
    }

    #[test]
    fn brighten_caps_at_255() {
        assert_eq!(brighten(Color::Rgb(250, 250, 250), 16), Color::Rgb(255, 255, 255));
    }

    #[test]
    fn darken_toward_fraction_zero_is_unchanged() {
        let c = Color::Rgb(200, 100, 50);
        assert_eq!(darken_toward(c, Color::Rgb(0, 0, 0), 0.0), c);
    }

    #[test]
    fn midpoint_blends_evenly() {
        let a = Color::Rgb(100, 100, 100);
        let b = Color::Rgb(200, 200, 200);
        assert_eq!(midpoint(a, b), Color::Rgb(150, 150, 150));
    }

}
