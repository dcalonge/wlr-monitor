//! Drawing the main view, the dialogs and the notifications.
//!
//! Stacked sections from top to bottom: the layout map (the active section,
//! heavy green border, status in the bottom border), the outputs table, the
//! selected output's details and the help bar. Small terminals hide the
//! details, then the table, so the map stays usable in floating windows.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{BorderType, Paragraph};

use crate::app::{App, COMPACT_HEIGHT, MAIN_HELP, Modal, TINY_HEIGHT};
use crate::screens::titled_block;
use crate::theme;
use crate::widgets::{
    DetailsView, MapRender, center, details, ellipsize, help_lines, render_map, table_lines,
};

/// The area inside the screen's `1 1 0 1` padding.
fn padded(screen: Rect) -> Rect {
    Rect::new(
        screen.x + 1.min(screen.width),
        screen.y + 1.min(screen.height),
        screen.width.saturating_sub(2),
        screen.height.saturating_sub(1),
    )
}

/// A section's content area: inside the border and the `0 1` padding.
fn content_of(area: Rect) -> Rect {
    Rect::new(
        area.x + 2.min(area.width),
        area.y + 1.min(area.height),
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    )
}

/// Heights of the map, outputs table, details block and help bar.
pub struct Sections {
    pub map: Rect,
    pub outputs: Option<Rect>,
    pub details: Option<Rect>,
    pub help: Rect,
}

pub fn sections(
    screen: Rect,
    output_count: usize,
    details_lines: usize,
    help_count: usize,
) -> Sections {
    let area = padded(screen);
    let compact = screen.height < COMPACT_HEIGHT;
    let tiny = screen.height < TINY_HEIGHT;
    let mut remaining = area.height;

    let help_height = (help_count.max(2) as u16).min(remaining);
    remaining -= help_height;

    let max_outputs = (u32::from(area.height) * 40 / 100) as u16;
    let mut outputs_height = if tiny {
        0
    } else {
        ((4 + output_count.max(1)) as u16).min(max_outputs)
    };
    let mut details_height = if compact {
        0
    } else {
        ((2 + details_lines) as u16).min(9)
    };
    // The map keeps at least five rows; the other sections give way.
    let map_min = 5.min(remaining);
    while remaining.saturating_sub(outputs_height + details_height) < map_min {
        if details_height > 0 {
            details_height -= 1;
        } else if outputs_height > 0 {
            outputs_height -= 1;
        } else {
            break;
        }
    }
    if details_height < 3 {
        details_height = 0;
    }
    if outputs_height < 3 {
        outputs_height = 0;
    }
    let map_height = remaining.saturating_sub(outputs_height + details_height);

    let mut y = area.y;
    let mut next = |height: u16| {
        let rect = Rect::new(area.x, y, area.width, height);
        y += height;
        rect
    };
    let map = next(map_height);
    let outputs = (outputs_height > 0).then(|| next(outputs_height));
    let details = (details_height > 0).then(|| next(details_height));
    let help = next(help_height);
    Sections {
        map,
        outputs,
        details,
        help,
    }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let screen = frame.area();
    let area = padded(screen);
    let content_width = usize::from(area.width.saturating_sub(4));

    let dirty = app.dirty_names();
    let current = app.current().cloned();
    let baseline = current
        .as_ref()
        .and_then(|output| app.baseline_state(&output.name));
    let view = DetailsView {
        output: current.as_ref(),
        baseline: baseline.as_ref(),
        error: app.error.as_deref(),
        profile_name: app.active_profile.as_deref(),
        capabilities: app.capabilities(),
        loading: app.loading,
    };
    let (details_title, details_lines) = details(&view, content_width);
    let help = help_lines(&MAIN_HELP, usize::from(area.width.saturating_sub(2)));
    let layout = sections(screen, app.outputs.len(), details_lines.len(), help.len());

    // Layout map: the active section. The status is its bottom title, one
    // line, cut to fit ` ┗━` + ` text ` + `━┛` like Textual's subtitle.
    let status = app.status_line();
    let status = status.lines().next().unwrap_or("");
    let max_status = usize::from(layout.map.width).saturating_sub(6);
    let mut map_block = titled_block("Layout", BorderType::Thick, theme::accent(), true);
    if max_status > 0 {
        map_block = map_block.title_bottom(
            Line::from(vec![
                Span::raw(format!(" {} ", ellipsize(status, max_status))),
                Span::styled("━", theme::accent()),
            ])
            .right_aligned(),
        );
    }
    frame.render_widget(map_block, layout.map);
    let map_content = content_of(layout.map);
    let selected_name = current.as_ref().map(|output| output.name.clone());
    let map = if app.loading {
        MapRender {
            lines: vec![Line::styled("Reading outputs…", theme::dim())],
            ..MapRender::default()
        }
    } else {
        render_map(
            &app.outputs,
            selected_name.as_deref(),
            &dirty,
            &app.guides,
            i64::from(map_content.width),
            i64::from(map_content.height),
        )
    };
    frame.render_widget(Paragraph::new(map.lines.clone()), map_content);
    app.regions.map_widget = layout.map;
    app.regions.map_content = map_content;
    app.regions.map = map;

    // Outputs table.
    app.regions.outputs_content = Rect::default();
    if let Some(rect) = layout.outputs {
        frame.render_widget(
            titled_block("Outputs", BorderType::Plain, theme::plain(), false),
            rect,
        );
        let content = content_of(rect);
        let lines = if app.loading {
            vec![Line::styled(
                center("Reading outputs…", usize::from(content.width)),
                theme::dim(),
            )]
        } else {
            table_lines(
                &app.outputs,
                app.selected,
                &dirty,
                usize::from(content.width),
            )
        };
        frame.render_widget(Paragraph::new(lines), content);
        app.regions.outputs_content = content;
    }

    // Details of the selected output.
    if let Some(rect) = layout.details {
        let max_title = usize::from(rect.width).saturating_sub(6);
        let title = ellipsize(&details_title, max_title);
        frame.render_widget(
            titled_block(&title, BorderType::Plain, theme::plain(), false),
            rect,
        );
        frame.render_widget(Paragraph::new(details_lines), content_of(rect));
    }

    // Help bar.
    let help_area = Rect::new(
        layout.help.x + 1.min(layout.help.width),
        layout.help.y,
        layout.help.width.saturating_sub(2),
        layout.help.height,
    );
    let rows = help.len() as u16;
    let top = help_area.y + help_area.height.saturating_sub(rows) / 2;
    frame.render_widget(
        Paragraph::new(help),
        Rect::new(
            help_area.x,
            top,
            help_area.width,
            rows.min(help_area.height),
        ),
    );

    // Dialogs, bottom to top, then notifications over everything.
    for modal in &mut app.modals {
        match modal {
            Modal::Choice(dialog, _) => dialog.render(frame, screen),
            Modal::Prompt(dialog, _) => dialog.render(frame, screen),
            Modal::Confirm(dialog, _) => dialog.render(frame, screen),
            Modal::Advanced(dialog) => dialog.render(frame, screen),
            Modal::Profiles(dialog) => dialog.render(frame, screen),
            Modal::Help(dialog) => dialog.render(frame, screen),
        }
    }
    app.notifications.render(frame, padded(screen));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_follow_the_terminal_height() {
        let tall = sections(Rect::new(0, 0, 110, 34), 2, 4, 2);
        assert_eq!(tall.help.height, 2);
        assert_eq!(tall.outputs.unwrap().height, 6);
        assert_eq!(tall.details.unwrap().height, 6);
        assert_eq!(tall.map.y, 1);
        assert_eq!(tall.map.height, 33 - 2 - 6 - 6);

        let compact = sections(Rect::new(0, 0, 80, 24), 2, 4, 2);
        assert!(compact.details.is_none());
        assert!(compact.outputs.is_some());

        let tiny = sections(Rect::new(0, 0, 40, 14), 2, 4, 2);
        assert!(tiny.details.is_none() && tiny.outputs.is_none());
        assert_eq!(tiny.map.height, 13 - 2);

        let many = sections(Rect::new(0, 0, 100, 30), 20, 4, 2);
        assert_eq!(many.outputs.unwrap().height, 29 * 40 / 100);
        assert!(many.map.height >= 5);
    }
}
