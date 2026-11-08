//! Styles that follow the terminal's own palette: only the 16 ANSI colors,
//! so wlr-monitor matches whatever theme the terminal uses.
//!
//! The look: heavy green active borders, dark-gray selection, blue help keys.

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

/// Errors.
pub fn error() -> Style {
    Style::new().fg(Color::Red)
}

/// Labels and section headings.
pub fn label() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

/// The help line and dialog hints.
pub fn help() -> Style {
    Style::new().fg(Color::Blue)
}

/// The keys in the help line.
pub fn help_key() -> Style {
    help().add_modifier(Modifier::BOLD)
}
