//! Toast notifications.

use std::time::{Duration, Instant};

use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn title(self) -> &'static str {
        match self {
            Self::Info => "Info",
            Self::Warning => "Warning",
            Self::Error => "Error",
        }
    }

    pub fn color(self) -> Color {
        match self {
            Self::Info => Color::Green,
            Self::Warning => Color::Yellow,
            Self::Error => Color::Red,
        }
    }
}

#[derive(Debug)]
pub struct Toast {
    pub message: String,
    pub severity: Severity,
    expires: Instant,
}

#[derive(Debug, Default)]
pub struct Notifications {
    pub toasts: Vec<Toast>,
}

impl Notifications {
    pub fn push(&mut self, message: impl Into<String>, severity: Severity, seconds: u64) {
        let message = message.into();
        // The same message twice (e.g. a repeated warning) is shown once.
        self.toasts.retain(|toast| toast.message != message);
        self.toasts.push(Toast {
            message,
            severity,
            expires: Instant::now() + Duration::from_secs(seconds),
        });
    }

    pub fn expire(&mut self) {
        let now = Instant::now();
        self.toasts.retain(|toast| toast.expires > now);
    }

    /// How long until the next toast disappears.
    pub fn next_expiry(&self) -> Option<Duration> {
        let next = self.toasts.iter().map(|toast| toast.expires).min()?;
        Some(next.saturating_duration_since(Instant::now()))
    }
}
