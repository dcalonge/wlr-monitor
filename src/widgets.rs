//! Building blocks of the main view: the outputs table, the layout map, the
//! details block and the help bar.
//!
//! The look follows impala / bluetui: stacked bordered sections, the active
//! one drawn with a heavy green border, table headers with a blank line below
//! them, a dark-gray highlight for the selected row and a centered blue help
//! line with bold keys. Only ANSI colors are used.

use std::collections::HashSet;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::compositor::{Capabilities, color_profile_label};
use crate::layout::{CELL_ASPECT, Guide, LayoutGeometry, Orientation, Rect, compute_layout};
use crate::model::{Output, OutputState, fmt_g, format_refresh};
use crate::theme;

// ----------------------------------------------------------------------
// Text helpers
// ----------------------------------------------------------------------

pub fn text_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Cut `text` to `max` cells, ending in `…` when something was cut.
pub fn ellipsize(text: &str, max: usize) -> String {
    if text_width(text) <= max {
        return text.to_string();
    }
    let mut result = String::new();
    let mut used = 0;
    for character in text.chars() {
        let size = character.width().unwrap_or(0);
        if used + size + 1 > max {
            break;
        }
        result.push(character);
        used += size;
    }
    result.push('…');
    result
}

/// Python's `str.center(width)`, measured in terminal cells.
pub fn center(text: &str, width: usize) -> String {
    let size = text_width(text);
    if size >= width {
        return text.to_string();
    }
    let margin = width - size;
    let left = margin / 2 + (margin & width & 1);
    format!("{}{text}{}", " ".repeat(left), " ".repeat(margin - left))
}

/// The first item with the largest key (Python's `max(..., key=...)`).
pub fn first_max_by_key<T, K: Ord>(items: &[T], key: impl Fn(&T) -> K) -> Option<usize> {
    let mut best: Option<(usize, K)> = None;
    for (index, item) in items.iter().enumerate() {
        let value = key(item);
        if best.as_ref().is_none_or(|(_, current)| value > *current) {
            best = Some((index, value));
        }
    }
    best.map(|(index, _)| index)
}

fn line_width(spans: &[Span]) -> usize {
    spans.iter().map(|span| text_width(&span.content)).sum()
}

/// Fit styled spans into exactly `width` cells: cut with `…`, then pad.
pub fn fit_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let total = line_width(&spans);
    let mut result: Vec<Span<'static>> = Vec::new();
    if total <= width {
        result = spans;
        if total < width {
            result.push(Span::raw(" ".repeat(width - total)));
        }
        return result;
    }
    if width == 0 {
        return result;
    }
    let mut used = 0;
    let budget = width - 1;
    let mut last_style = Style::default();
    for span in spans {
        last_style = span.style;
        let mut kept = String::new();
        for character in span.content.chars() {
            let size = character.width().unwrap_or(0);
            if used + size > budget {
                break;
            }
            kept.push(character);
            used += size;
        }
        let complete = kept.len() == span.content.len();
        if !kept.is_empty() {
            result.push(Span::styled(kept, span.style));
        }
        if !complete {
            break;
        }
    }
    result.push(Span::styled("…", last_style));
    used += 1;
    if used < width {
        result.push(Span::raw(" ".repeat(width - used)));
    }
    result
}

/// A line of `text` that keeps its `\n`s for `wrap_line` (ratatui's
/// `Line::raw`/`Line::styled` silently drop them).
pub fn text_line(text: &str, style: Style) -> Line<'static> {
    Line::from(Span::styled(text.to_string(), style))
}

/// Greedy word wrap of a styled line into lines of at most `width` cells.
/// A `\n` inside the text starts a new line (like Rich's `Text.wrap`).
pub fn wrap_line(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    if line.spans.iter().any(|span| span.content.contains('\n')) {
        let mut parts = vec![Line::default().style(line.style)];
        for span in line.spans {
            for (index, piece) in span.content.split('\n').enumerate() {
                if index > 0 {
                    parts.push(Line::default().style(line.style));
                }
                if !piece.is_empty() {
                    let part = parts.last_mut().expect("parts is never empty");
                    part.spans.push(Span::styled(piece.to_string(), span.style));
                }
            }
        }
        return parts
            .into_iter()
            .flat_map(|part| wrap_line(part, width))
            .collect();
    }
    if line_width(&line.spans) <= width {
        return vec![line];
    }
    let chars: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|span| span.content.chars().map(move |c| (c, span.style)))
        .collect();
    let size = |character: char| character.width().unwrap_or(0);

    let mut lines: Vec<Vec<(char, Style)>> = vec![Vec::new()];
    let mut used = 0;
    let mut index = 0;
    while index < chars.len() {
        // One word and the spaces after it.
        let start = index;
        while index < chars.len() && chars[index].0 != ' ' {
            index += 1;
        }
        let word_end = index;
        while index < chars.len() && chars[index].0 == ' ' {
            index += 1;
        }
        let word = &chars[start..word_end];
        let word_width: usize = word.iter().map(|(c, _)| size(*c)).sum();
        if used > 0 && used + word_width > width {
            lines.push(Vec::new());
            used = 0;
        }
        for &item in word {
            // Words longer than a line are folded.
            if used > 0 && used + size(item.0) > width {
                lines.push(Vec::new());
                used = 0;
            }
            lines.last_mut().unwrap().push(item);
            used += size(item.0);
        }
        for &item in &chars[word_end..index] {
            if used < width {
                lines.last_mut().unwrap().push(item);
                used += 1;
            }
        }
    }
    lines
        .into_iter()
        .map(|mut chars| {
            while chars.last().is_some_and(|(c, _)| *c == ' ') {
                chars.pop();
            }
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (character, style) in chars {
                match spans.last_mut() {
                    Some(span) if span.style == style => span.content.to_mut().push(character),
                    _ => spans.push(Span::styled(character.to_string(), style)),
                }
            }
            Line::from(spans)
        })
        .collect()
}

// ----------------------------------------------------------------------
// Outputs table
// ----------------------------------------------------------------------

pub struct Column {
    pub header: &'static str,
    pub width: usize,
    /// Lower survives longer when the terminal is narrow.
    pub priority: u8,
    pub value: fn(&Output) -> String,
}

pub fn mode_text(output: &Output) -> String {
    match &output.mode {
        None => "-".to_string(),
        Some(mode) => format!("{}@{}", mode.size_text(), format_refresh(mode.refresh)),
    }
}

fn on_off(value: bool) -> String {
    if value { "On" } else { "Off" }.to_string()
}

pub const COLUMNS: [Column; 7] = [
    Column {
        header: "Name",
        width: 12,
        priority: 0,
        value: |o| o.name.clone(),
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
        value: |o| on_off(o.enabled),
    },
    Column {
        header: "Scale",
        width: 5,
        priority: 3,
        value: |o| fmt_g(o.scale),
    },
    Column {
        header: "Position",
        width: 11,
        priority: 4,
        value: |o| format!("{},{}", o.x, o.y),
    },
    Column {
        header: "Rotation",
        width: 11,
        priority: 6,
        value: |o| o.transform.clone(),
    },
    Column {
        header: "VRR",
        width: 3,
        priority: 5,
        value: |o| on_off(o.adaptive_sync),
    },
];

pub const MARKER_WIDTH: usize = 2;

/// Drop the least important columns until the table fits `width`.
pub fn visible_columns(width: usize) -> Vec<&'static Column> {
    let mut columns: Vec<&Column> = COLUMNS.iter().collect();
    while columns.len() > 1 {
        let needed =
            MARKER_WIDTH + columns.iter().map(|column| column.width).sum::<usize>() + columns.len()
                - 1;
        if needed <= width {
            break;
        }
        if let Some(index) = first_max_by_key(&columns, |column| column.priority) {
            columns.remove(index);
        }
    }
    columns
}

/// (leading space, gap) that spreads columns like ratatui's SpaceAround.
pub fn column_spacing(width: usize, columns: &[&Column]) -> (usize, usize) {
    let used = MARKER_WIDTH + columns.iter().map(|column| column.width).sum::<usize>();
    let free = width.saturating_sub(used);
    let gap = (free / (columns.len() + 1)).max(1);
    let lead = free.saturating_sub(gap * (columns.len().saturating_sub(1))) / 2;
    (lead, gap)
}

fn table_cell(value: &str, width: usize) -> String {
    center(&ellipsize(value, width), width)
}

/// The table lines: header, blank line, one row per output.
pub fn table_lines(
    outputs: &[Output],
    selected: usize,
    dirty: &HashSet<String>,
    width: usize,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    if outputs.is_empty() {
        return vec![Line::styled(
            center("No outputs found", width),
            theme::dim(),
        )];
    }
    let columns = visible_columns(width);
    let (lead, gap) = column_spacing(width, &columns);

    let mut header = vec![Span::raw(" ".repeat(MARKER_WIDTH + lead))];
    for (position, column) in columns.iter().enumerate() {
        if position > 0 {
            header.push(Span::raw(" ".repeat(gap)));
        }
        header.push(Span::styled(
            table_cell(column.header, column.width),
            theme::bold(),
        ));
    }
    let mut lines = vec![Line::from(header), Line::default()];

    for (index, output) in outputs.iter().enumerate() {
        let is_selected = index == selected;
        let is_dirty = dirty.contains(&output.name);
        let (marker, marker_style) = if is_dirty {
            ("●", theme::fg(Color::Yellow))
        } else if is_selected {
            ("▸", theme::bold())
        } else {
            (" ", theme::plain())
        };
        let mut row = vec![
            Span::styled(format!("{marker} "), marker_style),
            Span::raw(" ".repeat(lead)),
        ];
        for (position, column) in columns.iter().enumerate() {
            if position > 0 {
                row.push(Span::raw(" ".repeat(gap)));
            }
            let mut style = if output.enabled {
                theme::plain()
            } else {
                theme::dim()
            };
            if column.header == "State" && !output.enabled {
                style = theme::fg(Color::Red);
            }
            if column.header == "Name" && is_dirty {
                style = theme::fg(Color::Yellow);
            }
            row.push(Span::styled(
                table_cell(&(column.value)(output), column.width),
                style,
            ));
        }
        let used = line_width(&row);
        if used < width {
            row.push(Span::raw(" ".repeat(width - used)));
        }
        if is_selected {
            let highlight = theme::highlight();
            for span in &mut row {
                span.style = span.style.patch(highlight);
            }
        }
        lines.push(Line::from(row));
    }
    lines
}

// ----------------------------------------------------------------------
// Layout map
// ----------------------------------------------------------------------

/// The rendered map plus what mouse handling needs to hit-test it.
#[derive(Debug, Clone, Default)]
pub struct MapRender {
    pub lines: Vec<Line<'static>>,
    pub geometry: Option<LayoutGeometry>,
    /// Output names in drawing order (the last one is on top).
    pub draw_order: Vec<String>,
}

impl MapRender {
    /// The output drawn at content cell (x, y), topmost first.
    pub fn hit(&self, x: i64, y: i64) -> Option<String> {
        let geometry = self.geometry.as_ref()?;
        self.draw_order
            .iter()
            .rev()
            .find(|name| geometry.get(name).is_some_and(|rect| rect.contains(x, y)))
            .cloned()
    }
}

/// A grid of styled cells. `None` marks the second cell of a wide character.
struct Canvas {
    width: i64,
    height: i64,
    cells: Vec<Option<(char, Style)>>,
}

impl Canvas {
    fn new(width: i64, height: i64) -> Self {
        Self {
            width,
            height,
            cells: vec![Some((' ', Style::default())); (width * height).max(0) as usize],
        }
    }

    fn set(&mut self, x: i64, y: i64, character: char, style: Style) {
        if !(0..self.height).contains(&y) || !(0..self.width).contains(&x) {
            return;
        }
        let index = (y * self.width + x) as usize;
        // Overwriting half of a wide character clears the other half.
        if self.cells[index].is_none() && x > 0 {
            self.cells[index - 1] = Some((' ', style));
        }
        if let Some((old, _)) = self.cells[index]
            && old.width().unwrap_or(1) > 1
            && x + 1 < self.width
        {
            self.cells[index + 1] = Some((' ', style));
        }
        let size = character.width().unwrap_or(1) as i64;
        if size > 1 {
            if x + 1 >= self.width {
                self.cells[index] = Some((' ', style));
                return;
            }
            self.cells[index + 1] = None;
        }
        self.cells[index] = Some((character, style));
    }

    fn put(&mut self, x: i64, y: i64, text: &str, style: Style) {
        let mut column = x;
        for character in text.chars() {
            self.set(column, y, character, style);
            column += character.width().unwrap_or(1).max(1) as i64;
        }
    }

    fn into_lines(self) -> Vec<Line<'static>> {
        let mut lines = Vec::with_capacity(self.height as usize);
        for row in self.cells.chunks(self.width.max(1) as usize) {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (character, style) in row.iter().flatten() {
                match spans.last_mut() {
                    Some(span) if span.style == *style => span.content.to_mut().push(*character),
                    _ => spans.push(Span::styled(character.to_string(), *style)),
                }
            }
            lines.push(Line::from(spans));
        }
        lines
    }
}

/// Render the proportionally scaled map of `outputs` into `width`×`height`.
pub fn render_map(
    outputs: &[Output],
    selected_name: Option<&str>,
    dirty: &HashSet<String>,
    guides: &[Guide],
    width: i64,
    height: i64,
) -> MapRender {
    if width <= 0 || height <= 0 {
        return MapRender::default();
    }
    if width < 12 || height < 3 {
        return MapRender {
            lines: vec![Line::styled("Too small for the layout map", theme::dim())],
            ..MapRender::default()
        };
    }
    let Some(geometry) = compute_layout(outputs, width, height, 1, CELL_ASPECT) else {
        return MapRender {
            lines: vec![Line::styled("No outputs with a mode", theme::dim())],
            ..MapRender::default()
        };
    };

    let mut canvas = Canvas::new(width, height);
    draw_guides(&mut canvas, &geometry, guides);

    let mut drawable: Vec<&Output> = outputs
        .iter()
        .filter(|output| geometry.get(&output.name).is_some())
        .collect();
    // Larger boxes first, the selected output last, so overlaps stay readable.
    drawable.sort_by_key(|output| {
        (
            Some(output.name.as_str()) == selected_name,
            -(i128::from(output.logical_width()) * i128::from(output.logical_height())),
        )
    });
    let draw_order: Vec<String> = drawable.iter().map(|output| output.name.clone()).collect();

    for output in drawable {
        let Some(rect) = geometry.get(&output.name) else {
            continue;
        };
        let selected = Some(output.name.as_str()) == selected_name;
        let is_dirty = dirty.contains(&output.name);
        let (border, label, detail) = if !output.enabled && !selected {
            (theme::dim(), theme::dim(), theme::dim())
        } else if is_dirty {
            (
                if selected {
                    theme::bold_fg(Color::Yellow)
                } else {
                    theme::fg(Color::Yellow)
                },
                theme::bold_fg(Color::Yellow),
                theme::fg(Color::Yellow),
            )
        } else if selected {
            (theme::bold_fg(Color::Green), theme::bold(), theme::plain())
        } else {
            (theme::plain(), theme::bold(), theme::dim())
        };
        let fill = if output.enabled {
            theme::plain()
        } else {
            theme::dim()
        };
        draw_box(&mut canvas, rect, border, fill, selected, !output.enabled);
        draw_label(&mut canvas, rect, output, label, detail);
    }

    MapRender {
        lines: canvas.into_lines(),
        geometry: Some(geometry),
        draw_order,
    }
}

fn draw_guides(canvas: &mut Canvas, geometry: &LayoutGeometry, guides: &[Guide]) {
    let style = theme::fg(Color::Blue);
    for guide in guides {
        match guide.orientation {
            Orientation::Vertical => {
                let (column, _) = geometry.world_to_cell(guide.value, geometry.min_y);
                if (0..canvas.width).contains(&column) {
                    for row in 0..canvas.height {
                        canvas.set(column, row, '┊', style);
                    }
                }
            }
            Orientation::Horizontal => {
                let (_, row) = geometry.world_to_cell(geometry.min_x, guide.value);
                if (0..canvas.height).contains(&row) {
                    for column in 0..canvas.width {
                        canvas.set(column, row, '┈', style);
                    }
                }
            }
        }
    }
}

fn draw_box(
    canvas: &mut Canvas,
    rect: Rect,
    border: Style,
    fill: Style,
    heavy: bool,
    dotted: bool,
) {
    if rect.width < 2 || rect.height < 2 {
        for y in rect.y..rect.bottom() {
            canvas.set(rect.x, y, '█', border);
        }
        return;
    }
    let fill_char = if dotted { '·' } else { ' ' };
    for y in rect.y + 1..rect.bottom() - 1 {
        for x in rect.x + 1..rect.right() - 1 {
            canvas.set(x, y, fill_char, fill);
        }
    }
    // The same plain/thick pair ratatui uses for unfocused/focused blocks.
    let (h, v, tl, tr, bl, br) = if heavy {
        ('━', '┃', '┏', '┓', '┗', '┛')
    } else {
        ('─', '│', '┌', '┐', '└', '┘')
    };
    for x in rect.x + 1..rect.right() - 1 {
        canvas.set(x, rect.y, h, border);
        canvas.set(x, rect.bottom() - 1, h, border);
    }
    for y in rect.y + 1..rect.bottom() - 1 {
        canvas.set(rect.x, y, v, border);
        canvas.set(rect.right() - 1, y, v, border);
    }
    canvas.set(rect.x, rect.y, tl, border);
    canvas.set(rect.right() - 1, rect.y, tr, border);
    canvas.set(rect.x, rect.bottom() - 1, bl, border);
    canvas.set(rect.right() - 1, rect.bottom() - 1, br, border);
}

fn centered_in(canvas: &mut Canvas, rect: Rect, row: i64, text: &str, style: Style) {
    let interior = (rect.width - 2).max(0) as usize;
    let text = ellipsize(text, interior);
    let x = rect.x + 1 + (interior.saturating_sub(text_width(&text)) / 2) as i64;
    canvas.put(x, row, &text, style);
}

fn draw_label(canvas: &mut Canvas, rect: Rect, output: &Output, label: Style, detail: Style) {
    let interior = rect.width - 2;
    if interior < 3 || rect.height < 3 {
        return;
    }
    let state = if output.enabled { "[ON]" } else { "[OFF]" };
    let mut name = format!("{} {state}", output.name);
    if text_width(&name) as i64 > interior {
        name = output.name.clone();
    }
    centered_in(canvas, rect, rect.y + 1, &name, label);

    if rect.height < 4 || interior < 8 {
        return;
    }
    if let Some(mode) = &output.mode {
        let text = format!("{}@{}Hz", mode.size_text(), format_refresh(mode.refresh));
        centered_in(canvas, rect, rect.y + 2, &text, detail);
    }

    if rect.height < 5 || interior < 6 {
        return;
    }
    let mut badges: Vec<String> = Vec::new();
    if output.adaptive_sync {
        badges.push("VRR".into());
    }
    if output.hdr == Some(true) {
        badges.push("HDR".into());
    }
    if output.transform != "normal" {
        if output.transform.chars().all(|c| c.is_ascii_digit()) {
            badges.push(format!("{}°", output.transform));
        } else {
            badges.push(output.transform.clone());
        }
    }
    if (output.scale - 1.0).abs() > 0.0001 {
        badges.push(format!("x{}", fmt_g(output.scale)));
    }
    if !badges.is_empty() {
        centered_in(canvas, rect, rect.y + 3, &badges.join(" "), detail);
    }
}

// ----------------------------------------------------------------------
// Details block
// ----------------------------------------------------------------------

pub const DETAIL_CELL: usize = 32;
pub const DETAIL_LABEL: usize = 15;

fn bool_label(value: bool) -> &'static str {
    if value { "On" } else { "Off" }
}

/// Like `bool_label`, but `None` (unknown / compositor default) stays visible.
fn optional_label(value: Option<bool>) -> &'static str {
    match value {
        None => "default",
        Some(value) => bool_label(value),
    }
}

fn depth_label(value: Option<i64>) -> String {
    match value {
        Some(depth) if depth != 0 => format!("{depth} bit"),
        _ => "default".to_string(),
    }
}

pub fn detail_rows(output: &Output, caps: &Capabilities) -> Vec<(String, String)> {
    let mut rows = vec![
        ("State".to_string(), bool_label(output.enabled).to_string()),
        (
            "Mode".to_string(),
            output
                .mode
                .as_ref()
                .map_or_else(|| "none".to_string(), |mode| mode.label()),
        ),
        ("Position".to_string(), format!("{},{}", output.x, output.y)),
        (
            "Logical".to_string(),
            format!("{}x{}", output.logical_width(), output.logical_height()),
        ),
        ("Scale".to_string(), fmt_g(output.scale)),
        ("Rotation".to_string(), output.transform.clone()),
        (
            "VRR".to_string(),
            bool_label(output.adaptive_sync).to_string(),
        ),
    ];
    if caps.hdr {
        rows.push(("HDR".into(), optional_label(output.hdr).into()));
    }
    if !caps.bit_depths.is_empty() {
        rows.push(("Color depth".into(), depth_label(output.bit_depth)));
    }
    if caps.color_profile() {
        rows.push((
            "Color profile".into(),
            color_profile_label(output.color_profile.as_deref()),
        ));
    }
    rows.push((
        "Physical".into(),
        format!("{}x{} mm", output.physical_width, output.physical_height),
    ));
    rows
}

/// What changed since the last applied state, one phrase per setting.
pub fn pending_changes(output: &Output, baseline: Option<&OutputState>) -> Vec<String> {
    let Some(old) = baseline else {
        return Vec::new();
    };
    let current = output.state();
    let on = |value: bool| if value { "on" } else { "off" };
    let mut changes = Vec::new();
    if current.enabled != old.enabled {
        changes.push(format!(
            "state {} → {}",
            on(old.enabled),
            on(current.enabled)
        ));
    }
    let old_key = old.mode.as_ref().map(|mode| mode.key());
    if let Some(mode) = &current.mode
        && old_key != Some(mode.key())
    {
        let before = old
            .mode
            .as_ref()
            .map_or_else(|| "none".to_string(), |m| m.label());
        changes.push(format!("mode {before} → {}", mode.label()));
    }
    if (current.x, current.y) != (old.x, old.y) {
        changes.push(format!(
            "position {},{} → {},{}",
            old.x, old.y, current.x, current.y
        ));
    }
    if (current.scale - old.scale).abs() > 0.0001 {
        changes.push(format!(
            "scale {} → {}",
            fmt_g(old.scale),
            fmt_g(current.scale)
        ));
    }
    if current.transform != old.transform {
        changes.push(format!(
            "rotation {} → {}",
            old.transform, current.transform
        ));
    }
    if current.adaptive_sync != old.adaptive_sync {
        changes.push(format!(
            "VRR {} → {}",
            on(old.adaptive_sync),
            on(current.adaptive_sync)
        ));
    }
    if current.hdr != old.hdr {
        changes.push(format!(
            "HDR {} → {}",
            optional_label(old.hdr).to_lowercase(),
            optional_label(current.hdr).to_lowercase()
        ));
    }
    if current.bit_depth != old.bit_depth {
        changes.push(format!(
            "color depth {} → {}",
            depth_label(old.bit_depth),
            depth_label(current.bit_depth)
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

/// Everything the details block shows.
pub struct DetailsView<'a> {
    pub output: Option<&'a Output>,
    pub baseline: Option<&'a OutputState>,
    pub error: Option<&'a str>,
    pub profile_name: Option<&'a str>,
    pub capabilities: &'a Capabilities,
    pub loading: bool,
}

/// The block title and its (already wrapped) lines for `width` cells.
pub fn details(view: &DetailsView, width: usize) -> (String, Vec<Line<'static>>) {
    let wrap_width = width.max(1);
    let width = width.max(DETAIL_CELL);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let push = |line: Line<'static>, lines: &mut Vec<Line<'static>>| {
        lines.extend(wrap_line(line, wrap_width));
    };

    if view.loading {
        push(Line::styled("Reading outputs…", theme::dim()), &mut lines);
        return ("Output".into(), lines);
    }
    if let Some(error) = view.error {
        push(
            Line::from(vec![
                Span::styled("Error: ", theme::bold_fg(Color::Red)),
                Span::styled(error.to_string(), theme::fg(Color::Red)),
            ]),
            &mut lines,
        );
    }
    let Some(output) = view.output else {
        if view.error.is_none() {
            push(
                Line::from(vec![
                    Span::styled("No outputs detected. ", theme::bold_fg(Color::Yellow)),
                    Span::raw(
                        "Make sure wlr-randr is installed and this terminal runs inside a \
                         wlroots compositor.",
                    ),
                ]),
                &mut lines,
            );
        }
        return ("Output".into(), lines);
    };

    let title = output.title();
    let title = if !title.is_empty() && title != output.name {
        format!("{} · {title}", output.name)
    } else {
        output.name.clone()
    };

    let mut rows = detail_rows(output, view.capabilities);
    if let Some(profile) = view.profile_name {
        rows.push(("Profile".into(), format!("{profile} (active)")));
    }
    let per_line = (width / DETAIL_CELL).max(1);
    let cell = width / per_line;
    for chunk in rows.chunks(per_line) {
        let mut spans = Vec::new();
        for (label, value) in chunk {
            let value_style = if label == "Profile" {
                theme::fg(Color::Green)
            } else {
                theme::plain()
            };
            let cell_spans = vec![
                Span::styled(format!("{label:<DETAIL_LABEL$}"), theme::bold()),
                Span::styled(value.clone(), value_style),
            ];
            spans.extend(fit_spans(cell_spans, cell - 1));
            spans.push(Span::raw(" "));
        }
        // Rows never wrap: they were sized for the block.
        lines.push(Line::from(fit_spans(spans, wrap_width)));
    }

    let pending = pending_changes(output, view.baseline);
    if pending.is_empty() {
        push(Line::styled("No pending changes", theme::dim()), &mut lines);
    } else {
        push(
            Line::from(vec![
                Span::styled("Pending  ", theme::bold_fg(Color::Yellow)),
                Span::styled(pending.join(" · "), theme::fg(Color::Yellow)),
            ]),
            &mut lines,
        );
    }
    (title, lines)
}

// ----------------------------------------------------------------------
// Help bar
// ----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelpItem {
    pub key: &'static str,
    pub label: &'static str,
    /// Lower survives longer on narrow terminals.
    pub priority: u8,
}

pub const fn help(key: &'static str, label: &'static str, priority: u8) -> HelpItem {
    HelpItem {
        key,
        label,
        priority,
    }
}

/// Wrap help items into at most `max_lines` lines of `width` cells.
///
/// Low-priority items are dropped until everything fits, like impala's
/// narrow-terminal help line.
pub fn layout_help(items: &[HelpItem], width: usize, max_lines: usize) -> Vec<Vec<HelpItem>> {
    const SEPARATOR: usize = 3; // " | "
    let mut remaining = items.to_vec();
    loop {
        let mut lines: Vec<Vec<HelpItem>> = vec![Vec::new()];
        let mut used = 0;
        for item in &remaining {
            let size = text_width(item.key) + 1 + text_width(item.label);
            let current = lines.last_mut().unwrap();
            let extra = if current.is_empty() {
                size
            } else {
                size + SEPARATOR
            };
            if !current.is_empty() && used + extra > width {
                lines.push(vec![*item]);
                used = size;
            } else {
                current.push(*item);
                used += extra;
            }
        }
        if lines.len() <= max_lines || remaining.len() <= 1 {
            lines.truncate(max_lines);
            return lines;
        }
        if let Some(index) = first_max_by_key(&remaining, |item| item.priority) {
            remaining.remove(index);
        }
    }
}

/// Centered, blue, impala-style key hints.
pub fn help_lines(items: &[HelpItem], width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    layout_help(items, width, 2)
        .into_iter()
        .map(|line| {
            let mut spans = Vec::new();
            for (position, item) in line.iter().enumerate() {
                if position > 0 {
                    spans.push(Span::styled(" | ", theme::hint()));
                }
                spans.push(Span::styled(item.key, theme::hint_key()));
                spans.push(Span::styled(format!(" {}", item.label), theme::hint()));
            }
            center_line(Line::from(spans), width)
        })
        .collect()
}

/// A key hint line for dialogs: `[(key, label)]` joined with ` | `.
/// Left-aligned; `center_line` centers it for the space it is drawn in.
pub fn hint_line(items: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (position, (key, label)) in items.iter().enumerate() {
        if position > 0 {
            spans.push(Span::styled(" | ", theme::hint()));
        }
        spans.push(Span::styled(key.to_string(), theme::hint_key()));
        spans.push(Span::styled(format!(" {label}"), theme::hint()));
    }
    Line::from(spans)
}

/// Center a line in `width` cells like Rich does: `floor((width - len) / 2)`
/// cells on the left (ratatui's own centering rounds the other way).
pub fn center_line(line: Line<'static>, width: usize) -> Line<'static> {
    let used = line.width();
    if used >= width {
        return line;
    }
    let style = line.style;
    let mut spans = vec![Span::raw(" ".repeat((width - used) / 2))];
    spans.extend(line.spans);
    Line::from(spans).style(style)
}

/// Plain text of rendered lines (for tests and width checks).
pub fn plain_text(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Styles used anywhere in `lines` (for the ANSI-only check).
pub fn styles_of(lines: &[Line]) -> Vec<Style> {
    lines
        .iter()
        .flat_map(|line| line.spans.iter().map(|span| span.style))
        .collect()
}

#[cfg(test)]
mod tests;
