//! Turns terminal events into actions on the `App`.

use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::app::App;

pub fn handle_event(app: &mut App, event: Event) {
    match event {
        Event::Key(key) if key.kind != KeyEventKind::Release => handle_key(app, key),
        // Dialogs are keyboard-only; the mouse works on the main view.
        Event::Mouse(mouse) if app.dialogs.is_empty() => handle_mouse(app, mouse),
        _ => {}
    }
}

fn handle_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    // Ctrl+Q quits from anywhere; in a dialog `q` only closes the dialog, so
    // pending edits are never dropped by accident.
    if ctrl && key.code == KeyCode::Char('q') {
        app.quit = true;
    } else if !app.dialogs.is_empty() {
        app.dialog_key(key);
    } else {
        main_key(app, key);
    }
}

fn main_key(app: &mut App, key: KeyEvent) {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    if key.modifiers.intersects(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('c') => app.quit = true,
            KeyCode::Char('r') => app.reload_action(),
            _ => {}
        }
        return;
    }
    if key
        .modifiers
        .intersects(KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        return;
    }
    let step = if shift { 10 } else { 1 };
    match key.code {
        KeyCode::Up => app.move_by(0, -1, step),
        KeyCode::Down => app.move_by(0, 1, step),
        KeyCode::Left => app.move_by(-1, 0, step),
        KeyCode::Right => app.move_by(1, 0, step),
        KeyCode::Tab => app.select_next(1),
        KeyCode::BackTab => app.select_next(-1),
        KeyCode::Enter | KeyCode::Char(' ') => app.toggle_enabled(app.selected),
        KeyCode::Char(c) => match c {
            'q' | 'Q' => app.quit = true,
            '?' => app.open_help(),
            'k' => app.move_by(0, -1, 1),
            'j' => app.move_by(0, 1, 1),
            'h' => app.move_by(-1, 0, 1),
            'l' => app.move_by(1, 0, 1),
            'K' => app.move_by(0, -1, 10),
            'J' => app.move_by(0, 1, 10),
            'H' => app.move_by(-1, 0, 10),
            'g' | 'G' => app.cycle_grid(),
            'L' => app.cycle_snap(),
            'r' | 'R' => app.pick_scale(),
            'f' | 'F' => app.pick_mode(),
            't' => app.pick_transform(),
            'v' => app.toggle_adaptive_sync(),
            '[' => app.step_scale(app.selected, -1),
            ']' => app.step_scale(app.selected, 1),
            'c' | 'C' | 'd' | 'D' => app.open_advanced(),
            'a' | 'A' => app.apply(),
            'p' | 'P' => app.prompt_save_profile(),
            'S' => app.save_startup_profile(),
            'o' | 'O' => app.open_profiles(),
            'z' => app.reset(),
            'Z' => app.revert(),
            _ => {}
        },
        _ => {}
    }
}

fn handle_mouse(app: &mut App, event: MouseEvent) {
    let (column, row) = (event.column, event.row);
    let on_map = app.areas.map.contains((column, row).into());
    match event.kind {
        MouseEventKind::Down(button) => {
            if on_map {
                match (button, app.areas.output_at(column, row)) {
                    (MouseButton::Left, Some(index)) => app.start_drag(index, column, row),
                    (MouseButton::Right, Some(index)) => app.toggle_enabled(index),
                    _ => {}
                }
            } else if let Some(index) = app.areas.table_row_at(column, row, app.outputs.len()) {
                app.select(index);
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => app.drag_to(column, row),
        // Motion without a button: the release was lost, so the drag is over.
        MouseEventKind::Up(_) | MouseEventKind::Moved => app.end_drag(),
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if on_map => {
            let steps = if event.kind == MouseEventKind::ScrollUp {
                1
            } else {
                -1
            };
            let index = app.areas.output_at(column, row).unwrap_or(app.selected);
            app.step_scale(index, steps);
        }
        _ => {}
    }
}
