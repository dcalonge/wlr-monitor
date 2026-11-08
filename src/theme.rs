//! Styles that follow the terminal's own palette: only the 16 ANSI colors,
//! so wlr-monitor matches whatever theme the terminal uses.

use ratatui::style::{Color, Modifier, Style};

pub fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

/// The active section and dialogs.
pub fn accent() -> Style {
    Style::new().fg(Color::Green)
}

/// The selected row.
pub fn highlight() -> Style {
    Style::new()
        .fg(Color::Gray)
        .bg(Color::DarkGray)
        .add_modifier(Modifier::BOLD)
}

/// Pending (not yet applied) changes.
pub fn pending() -> Style {
    Style::new().fg(Color::Yellow)
}
