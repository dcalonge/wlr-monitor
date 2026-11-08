//! Terminal setup and the event loop.

use std::time::Duration;

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;

use crate::app::App;
use crate::{handler, ui};

/// How long to wait for input when no toast is about to expire.
const IDLE: Duration = Duration::from_secs(3600);

pub fn run(mut app: App) -> Result<()> {
    let mut terminal = ratatui::init();
    // `ratatui::init` restores the terminal on panic; mouse capture is ours to undo.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        hook(info);
    }));
    let result = execute!(std::io::stdout(), EnableMouseCapture)
        .map_err(Into::into)
        .and_then(|()| event_loop(&mut terminal, &mut app));
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    // Show the first frame before reading the outputs, which can be slow.
    terminal.draw(|frame| ui::draw(frame, app))?;
    app.start();
    while !app.quit {
        terminal.draw(|frame| ui::draw(frame, app))?;
        let timeout = app.notifications.next_expiry().unwrap_or(IDLE);
        if event::poll(timeout)? {
            handler::handle_event(app, event::read()?);
            // Handle everything already queued (e.g. a burst of drag events) before redrawing.
            while !app.quit && event::poll(Duration::ZERO)? {
                handler::handle_event(app, event::read()?);
            }
        }
        app.notifications.expire();
    }
    Ok(())
}
