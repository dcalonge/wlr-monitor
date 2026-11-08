//! Styles that follow the terminal's own palette.
//!
//! impala and bluetui only use the 16 ANSI colors (plus the terminal's default
//! foreground/background), which is why they match foot, kitty, alacritty, …
//! themes without any configuration. wlr-monitor does the same: every color
//! below is an ANSI palette index, never RGB.

use ratatui::style::{Color, Modifier, Style};

pub fn plain() -> Style {
    Style::default()
}

pub fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

pub fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

pub fn fg(color: Color) -> Style {
    Style::default().fg(color)
}

pub fn bold_fg(color: Color) -> Style {
    fg(color).add_modifier(Modifier::BOLD)
}

/// The active section and dialogs: heavy green borders (impala).
pub fn accent() -> Style {
    fg(Color::Green)
}

/// The selected row: bold white on bright black.
pub fn highlight() -> Style {
    Style::default()
        .fg(Color::Gray)
        .bg(Color::DarkGray)
        .add_modifier(Modifier::BOLD)
}

/// Key hints in the help bar and dialogs.
pub fn hint() -> Style {
    fg(Color::Blue)
}

pub fn hint_key() -> Style {
    bold_fg(Color::Blue)
}
