//! wlr-monitor: a terminal monitor-layout editor for wlroots compositors.

pub mod app;
pub mod cli;
pub mod compositor;
pub mod layout;
pub mod model;
pub mod paths;
pub mod process;
pub mod profiles;
pub mod screens;
pub mod sha256;
pub mod text;
pub mod theme;
pub mod ui;
pub mod widgets;
pub mod wlr;

#[cfg(test)]
pub(crate) mod testutil;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
