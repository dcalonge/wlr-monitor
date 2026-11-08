//! Styles that follow the terminal's own palette: only the 16 ANSI colors,
//! so wlr-monitor matches whatever theme the terminal uses.

use ratatui::style::{Color, Modifier, Style};

pub fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

pub fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

pub fn fg(color: Color) -> Style {
    Style::new().fg(color)
}

pub fn bold_fg(color: Color) -> Style {
    fg(color).add_modifier(Modifier::BOLD)
}

/// The active section and dialogs.
pub fn accent() -> Style {
    fg(Color::Green)
}

/// The selected row.
pub fn highlight() -> Style {
    Style::new()
        .fg(Color::Gray)
        .bg(Color::DarkGray)
        .add_modifier(Modifier::BOLD)
}

/// Key names in hints.
pub fn key() -> Style {
    bold_fg(Color::Blue)
}

/// Pending (not yet applied) changes.
pub fn pending() -> Style {
    fg(Color::Yellow)
}
