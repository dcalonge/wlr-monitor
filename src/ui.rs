//! Drawing the main view. Dialogs are drawn by `dialogs`.

mod dialogs;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Row, Table, Wrap};

use crate::app::App;
use crate::layout::{Geometry, Guide};
use crate::notification::Toast;
use crate::output::{Output, fmt_num};
use crate::theme;

/// Hide the table and the details when the terminal is too short for them.
const TABLE_MIN_HEIGHT: u16 = 17;
const DETAILS_MIN_HEIGHT: u16 = 26;

const HINTS: [(&str, &str); 14] = [
    ("←↑↓→", "move"),
    ("Tab", "next"),
    ("Space", "on/off"),
    ("f", "mode"),
    ("r", "scale"),
    ("t", "rotate"),
    ("v", "VRR"),
    ("c", "advanced"),
    ("a", "apply"),
    ("z", "reset"),
    ("p", "save"),
    ("o", "profiles"),
    ("?", "help"),
    ("q", "quit"),
];

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let hints = Paragraph::new(hint_line()).wrap(Wrap { trim: true });
    let hints_height = hints.line_count(area.width).min(3) as u16;

    let mut constraints = vec![Constraint::Min(6)];
    if area.height >= TABLE_MIN_HEIGHT {
        constraints.push(Constraint::Length(app.outputs.len().clamp(1, 8) as u16 + 3));
    }
    if area.height >= DETAILS_MIN_HEIGHT {
        constraints.push(Constraint::Length(DETAIL_ROWS + 2));
    }
    constraints.push(Constraint::Length(hints_height));
    let areas = Layout::vertical(constraints).split(area);

    draw_map(frame, app, areas[0]);
    if area.height >= TABLE_MIN_HEIGHT {
        draw_table(frame, app, areas[1]);
    } else {
        app.areas.table_rows = Rect::default();
    }
    if area.height >= DETAILS_MIN_HEIGHT {
        draw_details(frame, app, areas[2]);
    }
    frame.render_widget(hints, areas[areas.len() - 1]);

    dialogs::draw(frame, app);
    draw_toasts(frame, app);
}

fn hint_line() -> Line<'static> {
    let spans = HINTS.iter().flat_map(|(key, what)| {
        [
            Span::styled(*key, Style::new().fg(Color::Blue).bold()),
            Span::raw(format!(" {what}  ")),
        ]
    });
    Line::from_iter(spans)
}

fn section<'a>(title: impl Into<Line<'a>>, active: bool) -> Block<'a> {
    let block = Block::bordered().title(title);
    if active {
        block.border_style(theme::accent())
    } else {
        block
    }
}

// ----- Map -----

fn draw_map(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = section(" Layout ", true)
        .title_bottom(Line::from(format!(" {} ", app.status_line())).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let geometry = Geometry::new(&app.outputs, inner.width, inner.height);
    let order = geometry
        .as_ref()
        .map(|g| g.draw_order(app.selected))
        .unwrap_or_default();
    if let Some(geometry) = &geometry {
        for &index in &order {
            if let Some(rect) = geometry.boxes[index] {
                draw_output(frame, app, index, translate(rect, inner));
            }
        }
        draw_guides(frame.buffer_mut(), app, geometry, inner);
    } else if let Some(error) = &app.error {
        let text = Paragraph::new(error.as_str())
            .style(Style::new().fg(Color::Red))
            .wrap(Wrap { trim: true });
        frame.render_widget(text, inner);
    } else if !app.loading {
        frame.render_widget(
            Paragraph::new("No outputs.").alignment(Alignment::Center),
            inner,
        );
    }
    app.areas.map = inner;
    app.areas.geometry = geometry;
    app.areas.draw_order = order;
}

fn translate(rect: Rect, origin: Rect) -> Rect {
    Rect::new(
        rect.x + origin.x,
        rect.y + origin.y,
        rect.width,
        rect.height,
    )
    .intersection(origin)
}

fn draw_output(frame: &mut Frame, app: &App, index: usize, rect: Rect) {
    let output = &app.outputs[index];
    let selected = index == app.selected;
    let style = if selected {
        Style::new().fg(Color::Green).bold()
    } else if app.is_dirty(output) {
        theme::pending()
    } else if !output.enabled {
        theme::dim()
    } else {
        Style::new()
    };
    let border = if selected {
        BorderType::Thick
    } else {
        BorderType::Plain
    };
    let block = Block::bordered().border_type(border).border_style(style);
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);

    let state = if output.enabled { "ON" } else { "OFF" };
    let mut lines = vec![Line::from(format!("{} [{state}]", output.name))];
    if let Some(mode) = &output.mode {
        lines.push(Line::from(format!(
            "{}x{}@{}Hz",
            mode.width,
            mode.height,
            fmt_num(mode.refresh, 0)
        )));
    }
    let badges = badges(output);
    if !badges.is_empty() {
        lines.push(Line::from(badges));
    }
    // Centered vertically; a box too small for all lines shows the first ones.
    let top = inner.height.saturating_sub(lines.len() as u16) / 2;
    let text_area = Rect {
        y: inner.y + top,
        height: inner.height - top,
        ..inner
    };
    frame.render_widget(
        Paragraph::new(lines)
            .style(style)
            .alignment(Alignment::Center),
        text_area,
    );
}

fn badges(output: &Output) -> String {
    let mut badges = Vec::new();
    if output.adaptive_sync {
        badges.push("VRR".to_string());
    }
    if output.hdr == Some(true) {
        badges.push("HDR".into());
    }
    if output.transform != Default::default() {
        badges.push(output.transform.as_str().into());
    }
    if output.scale != 1.0 {
        badges.push(format!("x{}", fmt_num(output.scale, 2)));
    }
    badges.join(" ")
}

fn draw_guides(buf: &mut Buffer, app: &App, geometry: &Geometry, map: Rect) {
    for guide in &app.guides {
        match *guide {
            Guide::Vertical(x) => {
                let column = geometry.to_cell(x, 0).0;
                let Some(column) = u16::try_from(column).ok().filter(|c| *c < map.width) else {
                    continue;
                };
                for row in 0..map.height {
                    guide_cell(buf, map.x + column, map.y + row, "┊");
                }
            }
            Guide::Horizontal(y) => {
                let row = geometry.to_cell(0, y).1;
                let Some(row) = u16::try_from(row).ok().filter(|r| *r < map.height) else {
                    continue;
                };
                for column in 0..map.width {
                    guide_cell(buf, map.x + column, map.y + row, "┈");
                }
            }
        }
    }
}

fn guide_cell(buf: &mut Buffer, x: u16, y: u16, symbol: &str) {
    // Only empty cells: a guide never overwrites an output's text or border.
    if let Some(cell) = buf.cell_mut((x, y)).filter(|cell| cell.symbol() == " ") {
        cell.set_symbol(symbol)
            .set_style(Style::new().fg(Color::Blue));
    }
}

// ----- Table -----

fn draw_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = section(" Outputs ", false);
    let inner = block.inner(area);
    let header = Row::new([
        "", "Name", "Mode", "State", "Scale", "Position", "Rotation", "VRR",
    ])
    .style(Style::new().bold());
    let rows = app.outputs.iter().enumerate().map(|(index, output)| {
        let dirty = app.is_dirty(output);
        let mode = output.mode.as_ref().map_or("-".into(), |mode| mode.label());
        let state = Span::styled(
            if output.enabled {
                "Enabled"
            } else {
                "Disabled"
            },
            if output.enabled {
                Style::new()
            } else {
                Style::new().fg(Color::Red)
            },
        );
        let row = Row::new([
            Span::raw(if dirty { "●" } else { " " }),
            Span::raw(output.name.clone()),
            Span::raw(mode),
            state,
            Span::raw(format!("{}x", fmt_num(output.scale, 2))),
            Span::raw(format!("{},{}", output.position.x, output.position.y)),
            Span::raw(output.transform.as_str()),
            Span::raw(if output.adaptive_sync { "On" } else { "Off" }),
        ]);
        if index == app.selected {
            row.style(theme::highlight())
        } else if dirty {
            row.style(theme::pending())
        } else if !output.enabled {
            row.style(theme::dim())
        } else {
            row
        }
    });
    let widths = [
        Constraint::Length(1),
        Constraint::Min(10),
        Constraint::Length(22),
        Constraint::Length(9),
        Constraint::Length(7),
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Length(4),
    ];
    frame.render_widget(Table::new(rows, widths).header(header).block(block), area);
    // One line per output, right below the header.
    app.areas.table_rows = Rect {
        y: inner.y + 1,
        height: inner.height.saturating_sub(1),
        ..inner
    };
}

// ----- Details -----

const DETAIL_ROWS: u16 = 12;

fn draw_details(frame: &mut Frame, app: &App, area: Rect) {
    let title = app.current().map_or("Details".into(), Output::title);
    let profile = app.active_profile.as_deref().unwrap_or("-");
    let block = section(format!(" {title} "), false).title_bottom(format!(" Profile: {profile} "));
    let lines = match app.current() {
        Some(output) => detail_lines(app, output),
        None => vec![Line::from("No output selected.")],
    };
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn detail_lines(app: &App, output: &Output) -> Vec<Line<'static>> {
    let (width, height) = output.logical_size();
    let on_off = |on: bool| if on { "On" } else { "Off" }.to_string();
    let capabilities = app.compositor().capabilities;
    let mut rows = vec![
        (
            "State",
            if output.enabled {
                "Enabled"
            } else {
                "Disabled"
            }
            .to_string(),
        ),
        (
            "Mode",
            output.mode.as_ref().map_or("-".into(), |mode| mode.label()),
        ),
        (
            "Position",
            format!("{}, {}", output.position.x, output.position.y),
        ),
        ("Logical", format!("{width}x{height}")),
        ("Scale", format!("{}x", fmt_num(output.scale, 3))),
        ("Rotation", output.transform.as_str().to_string()),
        ("VRR", on_off(output.adaptive_sync)),
    ];
    if capabilities.hdr {
        rows.push(("HDR", output.hdr.map_or("default".into(), on_off)));
    }
    if !capabilities.bit_depths.is_empty() {
        rows.push(("Color depth", crate::dialog::depth_label(output.bit_depth)));
    }
    if capabilities.color_profile() {
        rows.push((
            "Color profile",
            crate::output::color_profile_label(output.color_profile.as_deref()),
        ));
    }
    rows.push((
        "Physical",
        format!(
            "{}x{} mm",
            output.physical_size.width, output.physical_size.height
        ),
    ));

    let mut lines: Vec<_> = rows
        .into_iter()
        .map(|(label, value)| {
            Line::from(vec![
                Span::styled(format!("{label:<14}"), theme::dim()),
                Span::raw(value),
            ])
        })
        .collect();
    let pending: Vec<_> = app
        .outputs
        .iter()
        .filter(|o| app.is_dirty(o))
        .map(|o| o.name.as_str())
        .collect();
    lines.push(if pending.is_empty() {
        Line::styled("No pending changes", theme::dim())
    } else {
        Line::styled(
            format!("Pending: {} (press a to apply)", pending.join(", ")),
            theme::pending(),
        )
    });
    lines
}

// ----- Toasts -----

fn draw_toasts(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let width = area.width.min(50);
    let mut y = area.y;
    for Toast {
        message, severity, ..
    } in &app.notifications.toasts
    {
        let text_width = usize::from(width.saturating_sub(2).max(1));
        let height = (message.chars().count().div_ceil(text_width) as u16 + 2)
            .min(area.height.saturating_sub(y));
        if height < 3 {
            break;
        }
        let rect = Rect::new(area.right() - width, y, width, height);
        let block = Block::bordered()
            .border_type(BorderType::Thick)
            .border_style(Style::new().fg(severity.color()))
            .title(severity.title());
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(Text::raw(message.as_str()))
                .wrap(Wrap { trim: true })
                .block(block),
            rect,
        );
        y += height;
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::backend::Backend;
    use crate::compositor::{Compositor, Kind};
    use crate::output::tests::fixture_outputs;
    use crate::profile::ProfileStore;
    use crate::wlr_randr::WlrRandr;

    fn render(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal.backend().to_string()
    }

    #[test]
    fn draws_every_section_and_the_dialogs() {
        let compositor = Compositor::new(Kind::Sway, None, false);
        let backend = Backend::new(WlrRandr::new(Some("/nonexistent")), compositor);
        let mut app = App::new(backend, ProfileStore::new(None), false);
        app.outputs = fixture_outputs();
        app.loading = false;

        let screen = render(&mut app, 100, 40);
        for text in [
            "Layout",
            "Outputs",
            "HDMI-A-1 [ON]",
            "eDP-1",
            "1920x1080@100Hz",
            "Pending",
            "apply",
        ] {
            assert!(screen.contains(text), "missing {text:?}\n{screen}");
        }
        // The map is hit-testable after a frame: the selected output is at its center.
        let rect = app.areas.geometry.as_ref().unwrap().boxes[0].unwrap();
        let (column, row) = (app.areas.map.x + rect.x + 1, app.areas.map.y + rect.y + 1);
        assert_eq!(app.areas.output_at(column, row), Some(0));

        app.open_help();
        assert!(render(&mut app, 100, 40).contains("Esc to close"));
        app.dialogs.clear();
        app.open_advanced();
        assert!(render(&mut app, 100, 40).contains("Rotation"));
        // A tiny terminal must not panic.
        render(&mut app, 20, 5);
    }
}
