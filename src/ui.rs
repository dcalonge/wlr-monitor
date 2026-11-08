//! Drawing the main view. Dialogs are drawn by `dialogs`.

mod dialogs;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Cell, Clear, Padding, Paragraph, Row, Table, TableState, Wrap,
};

use crate::app::App;
use crate::layout::{Geometry, Guide};
use crate::notification::Toast;
use crate::output::{Mode, Output, color_profile_label, fmt_num};
use crate::theme;

/// Below these terminal heights the details and outputs sections are hidden,
/// so the layout map stays usable in small floating windows.
const COMPACT_HEIGHT: u16 = 26;
const TINY_HEIGHT: u16 = 17;

/// Details are laid out in cells this wide, with the label taking the first
/// 15 columns.
const DETAIL_CELL: u16 = 32;
const DETAIL_LABEL: usize = 15;

/// The marker column of the outputs table.
const MARKER_WIDTH: u16 = 2;

#[derive(Clone, Copy)]
struct HelpItem {
    key: &'static str,
    label: &'static str,
    /// Lower survives longer on narrow terminals.
    priority: u8,
}

const HELP: [HelpItem; 14] = [
    HelpItem {
        key: "←↓↑→",
        label: "Move",
        priority: 0,
    },
    HelpItem {
        key: "⇥",
        label: "Next",
        priority: 1,
    },
    HelpItem {
        key: "f",
        label: "Mode",
        priority: 2,
    },
    HelpItem {
        key: "r",
        label: "Scale",
        priority: 2,
    },
    HelpItem {
        key: "t",
        label: "Rotation",
        priority: 4,
    },
    HelpItem {
        key: "v",
        label: "VRR",
        priority: 4,
    },
    HelpItem {
        key: "c",
        label: "Advanced",
        priority: 3,
    },
    HelpItem {
        key: "Space",
        label: "On/off",
        priority: 4,
    },
    HelpItem {
        key: "a",
        label: "Apply",
        priority: 0,
    },
    HelpItem {
        key: "z",
        label: "Reset",
        priority: 6,
    },
    HelpItem {
        key: "p",
        label: "Save profile",
        priority: 5,
    },
    HelpItem {
        key: "o",
        label: "Profiles",
        priority: 3,
    },
    HelpItem {
        key: "?",
        label: "Help",
        priority: 1,
    },
    HelpItem {
        key: "q",
        label: "Quit",
        priority: 1,
    },
];

pub fn draw(frame: &mut Frame, app: &mut App) {
    let screen = frame.area();
    let outer = content_area(screen);

    let help_lines = layout_help(&HELP, outer.width.saturating_sub(2).max(1), 2);
    let help_height = (help_lines.len() as u16).max(2);

    let show_outputs = screen.height >= TINY_HEIGHT;
    let show_details = screen.height >= COMPACT_HEIGHT;

    let mut constraints = vec![Constraint::Min(5)];
    if show_outputs {
        let max = (u32::from(outer.height) * 40 / 100) as u16;
        let wanted = app.outputs.len().max(1) as u16 + 4;
        constraints.push(Constraint::Length(wanted.min(max).max(3)));
    }
    if show_details {
        constraints.push(Constraint::Length(details_height(app, outer.width)));
    }
    constraints.push(Constraint::Length(help_height));

    let areas = Layout::vertical(constraints).split(outer);
    let mut next = 0;
    draw_map(frame, app, areas[next]);
    next += 1;
    if show_outputs {
        draw_table(frame, app, areas[next]);
        next += 1;
    } else {
        app.areas.table_rows = Rect::default();
        app.areas.table_offset = 0;
    }
    if show_details {
        draw_details(frame, app, areas[next]);
        next += 1;
    }
    draw_help(frame, areas[next], &help_lines);

    dialogs::draw(frame, app);
    draw_toasts(frame, app);
}

/// The screen padding: one cell at the top and on the sides, none at the
/// bottom.
fn content_area(area: Rect) -> Rect {
    if area.width < 3 || area.height < 2 {
        return area;
    }
    Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width - 2,
        height: area.height - 1,
    }
}

/// A section: plain border and title normally, heavy green border and bold
/// title when it is the active one.
fn section<'a>(title: &str, active: bool) -> Block<'a> {
    let title_style = if active { theme::label() } else { Style::new() };
    let block = Block::bordered()
        .title(Line::styled(format!(" {title} "), title_style))
        .padding(Padding::horizontal(1));
    if active {
        block
            .border_type(BorderType::Thick)
            .border_style(theme::accent())
    } else {
        block
    }
}

// ----- Map -----

fn draw_map(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = section("Layout", true)
        .title_bottom(Line::from(format!(" {} ", app.status_line())).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let geometry = Geometry::new(&app.outputs, inner.width, inner.height);
    let order = geometry
        .as_ref()
        .map(|geometry| geometry.draw_order(app.selected))
        .unwrap_or_default();
    if let Some(geometry) = &geometry {
        draw_guides(frame.buffer_mut(), app, geometry, inner);
        for &index in &order {
            if let Some(rect) = geometry.boxes[index] {
                draw_output(frame, app, index, translate(rect, inner));
            }
        }
    } else if !app.loading {
        frame.render_widget(
            Paragraph::new("No outputs with a mode")
                .style(theme::dim())
                .alignment(Alignment::Center),
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
    let dirty = app.is_dirty(output);
    let (border_style, label_style, detail_style) = if !output.enabled && !selected {
        (theme::dim(), theme::dim(), theme::dim())
    } else if dirty {
        (
            if selected {
                theme::pending().add_modifier(Modifier::BOLD)
            } else {
                theme::pending()
            },
            theme::pending().add_modifier(Modifier::BOLD),
            theme::pending(),
        )
    } else if selected {
        (
            theme::accent().add_modifier(Modifier::BOLD),
            theme::label(),
            Style::new(),
        )
    } else {
        (Style::new(), theme::label(), theme::dim())
    };

    let border = if selected {
        BorderType::Thick
    } else {
        BorderType::Plain
    };
    let block = Block::bordered()
        .border_type(border)
        .border_style(border_style);
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let width = usize::from(inner.width);
    let state = if output.enabled { "[ON]" } else { "[OFF]" };
    let mut name = format!("{} {state}", output.name);
    if name.chars().count() > width {
        name = output.name.clone();
    }
    if name.chars().count() > width {
        name = fit(&name, width);
    }

    let mut lines = vec![Line::styled(name, label_style)];
    if inner.height >= 4
        && width >= 8
        && let Some(mode) = &output.mode
    {
        let detail = format!("{}@{}Hz", mode.size_text(), fmt_num(mode.refresh, 2));
        lines.push(Line::styled(fit(&detail, width), detail_style));
    }
    if inner.height >= 5 && width >= 6 {
        let badges = badges(output);
        if !badges.is_empty() {
            lines.push(Line::styled(fit(&badges, width), detail_style));
        }
    }

    // Labels sit at the top of the box, like the map's output labels.
    let text_area = Rect {
        height: (lines.len() as u16).min(inner.height),
        ..inner
    };
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        text_area,
    );

    // Disabled outputs are filled with dots; the labels were drawn first so
    // they are left untouched.
    if !output.enabled {
        let buf = frame.buffer_mut();
        for y in inner.y..inner.bottom() {
            for x in inner.x..inner.right() {
                if let Some(cell) = buf.cell_mut((x, y))
                    && cell.symbol() == " "
                {
                    cell.set_symbol("·").set_style(theme::dim());
                }
            }
        }
    }
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
        let name = output.transform.as_str();
        badges.push(if name.chars().all(|c| c.is_ascii_digit()) {
            format!("{name}°")
        } else {
            name.into()
        });
    }
    if (output.scale - 1.0).abs() > 0.0001 {
        badges.push(format!("x{}", fmt_num(output.scale, 3)));
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
            .set_style(Style::new().fg(ratatui::style::Color::Blue));
    }
}

// ----- Outputs table -----

#[derive(Clone, Copy)]
struct Column {
    header: &'static str,
    width: u16,
    /// Lower survives longer when the terminal is narrow.
    priority: u8,
    value: fn(&Output) -> String,
}

const COLUMNS: [Column; 7] = [
    Column {
        header: "Name",
        width: 12,
        priority: 0,
        value: name_text,
    },
    Column {
        header: "Mode",
        width: 15,
        priority: 1,
        value: mode_text,
    },
    Column {
        header: "State",
        width: 5,
        priority: 2,
        value: state_text,
    },
    Column {
        header: "Scale",
        width: 5,
        priority: 3,
        value: scale_text,
    },
    Column {
        header: "Position",
        width: 11,
        priority: 4,
        value: position_text,
    },
    Column {
        header: "Rotation",
        width: 11,
        priority: 6,
        value: rotation_text,
    },
    Column {
        header: "VRR",
        width: 3,
        priority: 5,
        value: vrr_text,
    },
];

fn name_text(output: &Output) -> String {
    output.name.clone()
}

fn state_text(output: &Output) -> String {
    if output.enabled { "On" } else { "Off" }.into()
}

fn scale_text(output: &Output) -> String {
    fmt_num(output.scale, 3)
}

fn position_text(output: &Output) -> String {
    format!("{},{}", output.position.x, output.position.y)
}

fn rotation_text(output: &Output) -> String {
    output.transform.as_str().into()
}

fn vrr_text(output: &Output) -> String {
    if output.adaptive_sync { "On" } else { "Off" }.into()
}

fn mode_text(output: &Output) -> String {
    output.mode.as_ref().map_or_else(
        || "-".into(),
        |mode| format!("{}@{}", mode.size_text(), fmt_num(mode.refresh, 2)),
    )
}

/// Drop the least important columns until the table fits `width`.
fn visible_columns(width: u16) -> Vec<Column> {
    let mut columns = COLUMNS.to_vec();
    while columns.len() > 1 {
        let needed = MARKER_WIDTH
            + columns.iter().map(|column| column.width).sum::<u16>()
            + columns.len() as u16;
        if needed <= width {
            break;
        }
        let index = columns
            .iter()
            .enumerate()
            .max_by_key(|(_, column)| column.priority)
            .map(|(index, _)| index)
            .unwrap_or(0);
        columns.remove(index);
    }
    columns
}

fn draw_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = section("Outputs", false);
    let inner = block.inner(area);
    if app.outputs.is_empty() {
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new("No outputs found")
                .style(theme::dim())
                .alignment(Alignment::Center),
            inner,
        );
        app.areas.table_rows = Rect::default();
        app.areas.table_offset = 0;
        return;
    }

    let columns = visible_columns(inner.width);
    let mut widths = vec![Constraint::Length(MARKER_WIDTH)];
    widths.extend(
        columns
            .iter()
            .map(|column| Constraint::Length(column.width)),
    );

    let mut header_cells = vec![Cell::from("")];
    header_cells.extend(
        columns
            .iter()
            .map(|column| Cell::from(Line::from(column.header).centered())),
    );
    let header = Row::new(header_cells)
        .style(theme::label())
        .bottom_margin(1);

    let rows: Vec<Row> = app
        .outputs
        .iter()
        .enumerate()
        .map(|(index, output)| {
            let dirty = app.is_dirty(output);
            let marker = if dirty {
                "●"
            } else if index == app.selected {
                "▸"
            } else {
                " "
            };
            let marker_style = if dirty {
                theme::pending()
            } else {
                Style::new()
            };
            let mut cells = vec![Cell::from(marker).style(marker_style)];
            for column in &columns {
                let mut cell = Cell::from(Line::from((column.value)(output)).centered());
                if !output.enabled {
                    cell = cell.style(theme::dim());
                }
                if column.header == "State" && !output.enabled {
                    cell = cell.style(theme::error());
                }
                if column.header == "Name" && dirty {
                    cell = cell.style(theme::pending());
                }
                cells.push(cell);
            }
            Row::new(cells)
        })
        .collect();

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .column_spacing(1)
        .flex(Flex::SpaceAround)
        .row_highlight_style(theme::highlight());
    let mut state = TableState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(table, area, &mut state);

    // The header is one line and has a one-line bottom margin.
    app.areas.table_rows = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(2),
        ..inner
    };
    app.areas.table_offset = state.offset();
}

// ----- Details -----

fn details_height(app: &App, section_width: u16) -> u16 {
    let content_width = section_width.saturating_sub(4).max(1);
    let per_line = (usize::from(content_width) / usize::from(DETAIL_CELL)).max(1);
    let mut lines = if let Some(output) = app.current() {
        let rows = detail_rows(app, output).len() + usize::from(app.active_profile.is_some());
        rows.div_ceil(per_line) + 1
    } else {
        1
    };
    if app.error.is_some() {
        lines += 1;
    }
    (lines as u16 + 2).clamp(3, 9)
}

fn draw_details(frame: &mut Frame, app: &App, area: Rect) {
    let title = app.current().map_or_else(
        || "Output".to_string(),
        |output| {
            let title = output.title();
            if title.is_empty() || title == output.name {
                output.name.clone()
            } else {
                format!("{} · {title}", output.name)
            }
        },
    );
    let block = section(&title, false);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(detail_lines(app, inner.width)), inner);
}

fn detail_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(error) = &app.error {
        lines.push(Line::from(vec![
            Span::styled("Error: ", theme::error().add_modifier(Modifier::BOLD)),
            Span::styled(error.clone(), theme::error()),
        ]));
    }
    let Some(output) = app.current() else {
        lines.push(Line::from(vec![
            Span::styled(
                "No outputs detected. ",
                theme::pending().add_modifier(Modifier::BOLD),
            ),
            Span::raw(
                "Make sure wlr-randr is installed and this terminal runs inside a wlroots compositor.",
            ),
        ]));
        return lines;
    };
    if width == 0 {
        return lines;
    }

    let mut rows = detail_rows(app, output);
    if let Some(profile) = &app.active_profile {
        rows.push(("Profile".into(), format!("{profile} (active)")));
    }
    let per_line = (usize::from(width) / usize::from(DETAIL_CELL)).max(1);
    let cell = usize::from(width) / per_line;
    for group in rows.chunks(per_line) {
        let mut spans = Vec::new();
        for (label, value) in group {
            let value_style = if label == "Profile" {
                theme::accent()
            } else {
                Style::new()
            };
            spans.extend(detail_chunk(label, value, cell, value_style));
        }
        lines.push(Line::from(spans));
    }

    let pending = pending_changes(app, output);
    if pending.is_empty() {
        lines.push(Line::styled("No pending changes", theme::dim()));
    } else {
        lines.push(Line::from(vec![
            Span::styled("Pending  ", theme::pending().add_modifier(Modifier::BOLD)),
            Span::styled(pending.join(" · "), theme::pending()),
        ]));
    }
    lines
}

fn detail_chunk(label: &str, value: &str, cell: usize, value_style: Style) -> Vec<Span<'static>> {
    if cell == 0 {
        return Vec::new();
    }
    let width = cell.saturating_sub(1);
    let label_width = DETAIL_LABEL.min(width);
    let value_width = width.saturating_sub(label_width);
    let mut spans = vec![Span::styled(fit_pad(label, label_width), theme::label())];
    if value_width > 0 {
        spans.push(Span::styled(fit_pad(value, value_width), value_style));
    }
    spans.push(Span::raw(" "));
    spans
}

fn detail_rows(app: &App, output: &Output) -> Vec<(String, String)> {
    let capabilities = app.compositor().capabilities;
    let on_off = |value: bool| if value { "On" } else { "Off" }.to_string();
    let optional = |value: Option<bool>| value.map_or("default".into(), on_off);
    let (width, height) = output.logical_size();
    let mut rows = vec![
        ("State".into(), on_off(output.enabled)),
        (
            "Mode".into(),
            output.mode.as_ref().map_or("none".into(), Mode::label),
        ),
        (
            "Position".into(),
            format!("{},{}", output.position.x, output.position.y),
        ),
        ("Logical".into(), format!("{width}x{height}")),
        ("Scale".into(), fmt_num(output.scale, 3)),
        ("Rotation".into(), output.transform.as_str().to_string()),
        ("VRR".into(), on_off(output.adaptive_sync)),
    ];
    if capabilities.hdr {
        rows.push(("HDR".into(), optional(output.hdr)));
    }
    if !capabilities.bit_depths.is_empty() {
        rows.push((
            "Color depth".into(),
            crate::dialog::depth_label(output.bit_depth),
        ));
    }
    if capabilities.color_profile() {
        rows.push((
            "Color profile".into(),
            color_profile_label(output.color_profile.as_deref()),
        ));
    }
    rows.push((
        "Physical".into(),
        format!(
            "{}x{} mm",
            output.physical_size.width, output.physical_size.height
        ),
    ));
    rows
}

fn pending_changes(app: &App, output: &Output) -> Vec<String> {
    let Some(old) = app.baseline.get(&output.name) else {
        return Vec::new();
    };
    let current = output.state();
    let on_off = |value: bool| if value { "on" } else { "off" };
    let optional =
        |value: Option<bool>| value.map_or("default".into(), |value| on_off(value).to_string());
    let mut changes = Vec::new();

    if current.enabled != old.enabled {
        changes.push(format!(
            "state {} → {}",
            on_off(old.enabled),
            on_off(current.enabled)
        ));
    }
    let old_mode = old.mode.as_ref().map(Mode::key);
    let new_mode = current.mode.as_ref().map(Mode::key);
    if old_mode != new_mode && current.mode.is_some() {
        let old_label = old.mode.as_ref().map_or("none".into(), Mode::label);
        let new_label = current.mode.as_ref().map_or("none".into(), Mode::label);
        changes.push(format!("mode {old_label} → {new_label}"));
    }
    if current.position != old.position {
        changes.push(format!(
            "position {},{} → {},{}",
            old.position.x, old.position.y, current.position.x, current.position.y
        ));
    }
    if (current.scale - old.scale).abs() > 0.0001 {
        changes.push(format!(
            "scale {} → {}",
            fmt_num(old.scale, 3),
            fmt_num(current.scale, 3)
        ));
    }
    if current.transform != old.transform {
        changes.push(format!(
            "rotation {} → {}",
            old.transform.as_str(),
            current.transform.as_str()
        ));
    }
    if current.adaptive_sync != old.adaptive_sync {
        changes.push(format!(
            "VRR {} → {}",
            on_off(old.adaptive_sync),
            on_off(current.adaptive_sync)
        ));
    }
    if current.hdr != old.hdr {
        changes.push(format!(
            "HDR {} → {}",
            optional(old.hdr),
            optional(current.hdr)
        ));
    }
    if current.bit_depth != old.bit_depth {
        changes.push(format!(
            "color depth {} → {}",
            crate::dialog::depth_label(old.bit_depth),
            crate::dialog::depth_label(current.bit_depth)
        ));
    }
    if current.color_profile != old.color_profile {
        changes.push(format!(
            "color profile {} → {}",
            color_profile_label(old.color_profile.as_deref()),
            color_profile_label(current.color_profile.as_deref())
        ));
    }
    changes
}

/// Truncate `text` to `width` cells, adding an ellipsis when it does not fit.
fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut fitted: String = text.chars().take(width - 1).collect();
    fitted.push('…');
    fitted
}

fn fit_pad(text: &str, width: usize) -> String {
    let fitted = fit(text, width);
    let padding = width.saturating_sub(fitted.chars().count());
    format!("{fitted}{}", " ".repeat(padding))
}

// ----- Help bar -----

fn layout_help(items: &[HelpItem], width: u16, max_lines: usize) -> Vec<Vec<HelpItem>> {
    const SEPARATOR: usize = 3; // " | "
    let width = usize::from(width.max(1));
    let mut remaining = items.to_vec();
    loop {
        let mut lines: Vec<Vec<HelpItem>> = vec![Vec::new()];
        let mut used = 0;
        for item in &remaining {
            let size = item.key.chars().count() + 1 + item.label.chars().count();
            let extra = if lines.last().is_some_and(Vec::is_empty) {
                size
            } else {
                size + SEPARATOR
            };
            let last = lines.last_mut().expect("at least one line");
            if !last.is_empty() && used + extra > width {
                lines.push(vec![*item]);
                used = size;
            } else {
                last.push(*item);
                used += extra;
            }
        }
        if lines.len() <= max_lines || remaining.len() <= 1 {
            lines.truncate(max_lines);
            return lines;
        }
        let index = remaining
            .iter()
            .enumerate()
            .max_by_key(|(_, item)| item.priority)
            .map(|(index, _)| index)
            .unwrap_or(0);
        remaining.remove(index);
    }
}

fn draw_help(frame: &mut Frame, area: Rect, lines: &[Vec<HelpItem>]) {
    if area.width < 3 || area.height == 0 {
        return;
    }
    let content = Rect {
        x: area.x + 1,
        width: area.width - 2,
        ..area
    };
    let rendered: Vec<Line> = lines
        .iter()
        .map(|items| {
            let mut spans = Vec::new();
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    spans.push(Span::styled(" | ", theme::help()));
                }
                spans.push(Span::styled(item.key, theme::help_key()));
                spans.push(Span::styled(format!(" {}", item.label), theme::help()));
            }
            Line::from(spans)
        })
        .collect();
    let height = rendered.len() as u16;
    let y = content.y + content.height.saturating_sub(height) / 2;
    frame.render_widget(
        Paragraph::new(rendered).alignment(Alignment::Center),
        Rect {
            y,
            height: height.min(content.height),
            ..content
        },
    );
}

// ----- Toasts -----

fn draw_toasts(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let width = ((u32::from(area.width) * 60 / 100) as u16)
        .max(24)
        .min(area.width);
    let mut y = area.y;
    for Toast {
        message, severity, ..
    } in &app.notifications.toasts
    {
        let text_width = usize::from(width.saturating_sub(4).max(1));
        let lines = message.chars().count().div_ceil(text_width).max(1) as u16;
        let height = (lines + 2).min(area.height.saturating_sub(y));
        if height < 3 {
            break;
        }
        let rect = Rect::new(
            area.right()
                .saturating_sub(width)
                .saturating_sub(1)
                .max(area.x),
            y,
            width,
            height,
        );
        let block = Block::bordered()
            .border_type(BorderType::Thick)
            .border_style(Style::new().fg(severity.color()))
            .title(severity.title());
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(message.as_str())
                .wrap(Wrap { trim: true })
                .alignment(Alignment::Center)
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
        app.baseline = app
            .outputs
            .iter()
            .map(|output| (output.name.clone(), output.state()))
            .collect();
        app.outputs[0].scale = 1.5;
        app.loading = false;

        let screen = render(&mut app, 100, 40);
        for text in [
            "Layout",
            "Outputs",
            "HDMI-A-1 [ON]",
            "eDP-1",
            "1920x1080@100Hz",
            "Pending",
            "Apply",
        ] {
            assert!(screen.contains(text), "missing {text:?}\n{screen}");
        }
        // The map is hit-testable after a frame: the selected output is at its center.
        let rect = app.areas.geometry.as_ref().unwrap().boxes[0].unwrap();
        let (column, row) = (app.areas.map.x + rect.x + 1, app.areas.map.y + rect.y + 1);
        assert_eq!(app.areas.output_at(column, row), Some(0));

        app.open_help();
        assert!(render(&mut app, 100, 40).contains("Keys"));
        app.dialogs.clear();
        app.open_advanced();
        assert!(render(&mut app, 100, 40).contains("Rotation"));
        // A tiny terminal must not panic.
        render(&mut app, 20, 5);
    }
}
