//! Modal dialogs, notifications and the one-line text editor.
//!
//! Dialogs are drawn like impala / bluetui popups: centered, a heavy green
//! border with the title in it, and a short key hint at the bottom.

use std::path::Path;
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthChar;

use crate::compositor::{Capabilities, color_profile_label};
use crate::model::{Output, OutputState, TRANSFORMS};
use crate::paths;
use crate::profiles::ProfileStore;
use crate::text::repr;
use crate::theme;
use crate::widgets::{center_line, ellipsize, fit_spans, hint_line, text_line, wrap_line};

// ----------------------------------------------------------------------
// Keys
// ----------------------------------------------------------------------

/// A printable key without Ctrl/Alt (Shift is part of the character).
pub fn plain_char(key: &KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            Some(c)
        }
        _ => None,
    }
}

pub fn is_ctrl(key: &KeyEvent, c: char) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char(k) if k.eq_ignore_ascii_case(&c))
}

fn shifted(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::SHIFT)
}

// ----------------------------------------------------------------------
// Notifications
// ----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Information,
    Warning,
    Error,
}

impl Severity {
    pub fn title(self) -> &'static str {
        match self {
            Self::Information => "Info",
            Self::Warning => "Warning",
            Self::Error => "Error",
        }
    }

    pub fn color(self) -> Color {
        match self {
            Self::Information => Color::Green,
            Self::Warning => Color::Yellow,
            Self::Error => Color::Red,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub title: String,
    pub message: String,
    pub severity: Severity,
    pub expires: Instant,
}

/// Toasts in the top-right corner, each with its own timeout.
#[derive(Debug, Default)]
pub struct Notifications {
    pub toasts: Vec<Toast>,
    /// Where each toast was drawn last (same order as `toasts`).
    areas: Vec<Rect>,
}

/// Textual's default notification timeout, used by the dialogs.
pub const DIALOG_TOAST_SECONDS: u64 = 5;

impl Notifications {
    pub fn notify(&mut self, message: &str, title: &str, severity: Severity, seconds: u64) {
        self.toasts.push(Toast {
            title: title.to_string(),
            message: message.to_string(),
            severity,
            expires: Instant::now() + Duration::from_secs(seconds),
        });
    }

    pub fn messages(&self) -> Vec<String> {
        self.toasts
            .iter()
            .map(|toast| toast.message.clone())
            .collect()
    }

    /// Drop expired toasts; true if anything changed.
    pub fn expire(&mut self, now: Instant) -> bool {
        let before = self.toasts.len();
        self.toasts.retain(|toast| toast.expires > now);
        let changed = before != self.toasts.len();
        if changed {
            // The rest move up: no hits until they are drawn again.
            self.areas.clear();
        }
        changed
    }

    pub fn next_expiry(&self) -> Option<Instant> {
        self.toasts.iter().map(|toast| toast.expires).min()
    }

    /// The toast drawn at this cell, if any.
    pub fn hit(&self, column: u16, row: u16) -> Option<usize> {
        self.areas
            .iter()
            .take(self.toasts.len())
            .position(|area| area.contains((column, row).into()))
    }

    /// Remove a toast (a click dismisses it, like Textual's).
    pub fn dismiss(&mut self, index: usize) {
        if index < self.toasts.len() {
            self.toasts.remove(index);
            self.areas.clear();
        }
    }

    pub fn render(&mut self, frame: &mut Frame, screen: Rect) {
        // `max-width: 60%` of the toast rack (two cells narrower than the
        // padded screen); toasts keep one blank column on the right
        // (`margin: 0 1 0 0`).
        let max_width = ((u32::from(screen.width.saturating_sub(2)) * 60 / 100) as u16).max(10);
        let mut y = screen.y;
        self.areas.clear();
        for toast in &self.toasts {
            let inner_max = max_width.saturating_sub(4).max(1) as usize;
            let mut lines: Vec<Line<'static>> = Vec::new();
            let mut wrapped = false;
            let mut add = |line: Line<'static>, lines: &mut Vec<Line<'static>>| {
                let parts = wrap_line(line, inner_max);
                wrapped |= parts.len() > 1;
                lines.extend(parts);
            };
            if !toast.title.is_empty() {
                add(Line::styled(toast.title.clone(), theme::bold()), &mut lines);
            }
            for part in crate::text::splitlines(&toast.message) {
                add(Line::raw(part.to_string()), &mut lines);
            }
            let content = lines
                .iter()
                .map(|line| line.width())
                .max()
                .unwrap_or(0)
                .max(20) as u16;
            // `width: auto` grows to the maximum as soon as the text wraps.
            let width = if wrapped {
                max_width
            } else {
                (content + 4).min(max_width)
            }
            .min(screen.width);
            let height = (lines.len() as u16 + 2).min(screen.bottom().saturating_sub(y));
            if height < 3 {
                break;
            }
            let x = screen.right().saturating_sub(width + 2).max(screen.x);
            let area = Rect::new(x, y, width, height);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Thick)
                .border_style(theme::fg(toast.severity.color()))
                .padding(ratatui::widgets::Padding::horizontal(1));
            frame.render_widget(Clear, area);
            let inner = usize::from(width.saturating_sub(4));
            let centered: Vec<Line> = lines
                .into_iter()
                .map(|line| center_line(line, inner))
                .collect();
            frame.render_widget(Paragraph::new(centered).block(block), area);
            self.areas.push(area);
            y += height;
        }
    }
}

// ----------------------------------------------------------------------
// Dialog frame
// ----------------------------------------------------------------------

/// How tall a dialog is.
#[derive(Clone, Copy)]
enum Height {
    /// A share of the screen height, like `height: 75%`.
    Percent(u16),
    /// Fit the content, up to a share of the screen.
    Auto { content: u16, max_percent: u16 },
}

/// The dialog box (centered, heavy green border, title) and its content area
/// after the `1 2 0 2` padding.
fn dialog_frame(frame: &mut Frame, screen: Rect, title: &str, width: u16, height: Height) -> Rect {
    // The screen keeps a one-cell margin at the top and sides.
    let area = Rect::new(
        screen.x + 1.min(screen.width),
        screen.y + 1.min(screen.height),
        screen.width.saturating_sub(2),
        screen.height.saturating_sub(1),
    );
    let width = width.min(area.width);
    let height = match height {
        Height::Percent(percent) => (u32::from(area.height) * u32::from(percent) / 100) as u16,
        Height::Auto {
            content,
            max_percent,
        } => {
            let max = (u32::from(area.height) * u32::from(max_percent) / 100) as u16;
            (content + 3).min(max.max(3))
        }
    }
    .min(area.height);
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, rect);
    // Textual cuts a border title with `…`, keeping `┏━ ` and ` ━┓`.
    let title = ellipsize(title, usize::from(width.saturating_sub(6)));
    let block = titled_block(&title, BorderType::Thick, theme::accent(), true);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    Rect::new(
        inner.x + 2.min(inner.width),
        inner.y + 1.min(inner.height),
        inner.width.saturating_sub(4),
        inner.height.saturating_sub(1),
    )
}

/// A bordered block with the title drawn after one border cell: `┏━ Title ━━┓`.
pub fn titled_block(
    title: &str,
    kind: BorderType,
    border: Style,
    bold_title: bool,
) -> Block<'static> {
    let horizontal = match kind {
        BorderType::Thick => "━",
        _ => "─",
    };
    let title_style = if bold_title {
        theme::bold()
    } else {
        theme::plain()
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(kind)
        .border_style(border);
    if !title.is_empty() {
        block = block.title(Line::from(vec![
            Span::styled(horizontal.to_string(), border),
            Span::styled(format!(" {title} "), title_style),
        ]));
    }
    block
}

/// The width of a dialog's content area (inside the border and the `0 2`
/// side padding), as `dialog_frame` will lay it out.
fn content_width(screen: Rect, width: u16) -> usize {
    usize::from(width.min(screen.width.saturating_sub(2)).saturating_sub(6)).max(1)
}

/// The key hint under a dialog's body (`text-align: center; padding: 1 0`):
/// a blank line, the hint wrapped and centered line by line, a blank line.
fn hint_block(hint: Line<'static>, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::default()];
    lines.extend(
        wrap_line(hint, width)
            .into_iter()
            .map(|line| center_line(line, width)),
    );
    lines.push(Line::default());
    lines
}

/// Stack blocks from the top like a Textual `Vertical`: fixed heights
/// (`Some`) keep their size, the flexible block (`None`, `1fr`) gets what is
/// left (one row when nothing is, like Textual), and whatever does not fit
/// is cropped at the bottom.
fn stack(content: Rect, heights: &[Option<u16>]) -> Vec<Rect> {
    let fixed: u16 = heights
        .iter()
        .flatten()
        .fold(0, |sum, h| sum.saturating_add(*h));
    let flexible = match content.height.saturating_sub(fixed) {
        0 => 1,
        rows => rows,
    };
    let mut y = content.y;
    heights
        .iter()
        .map(|height| {
            let height = height.unwrap_or(flexible).min(content.bottom() - y);
            let rect = Rect::new(content.x, y, content.width, height);
            y += height;
            rect
        })
        .collect()
}

// ----------------------------------------------------------------------
// Option list
// ----------------------------------------------------------------------

/// A scrolling list of options with one highlighted row.
#[derive(Debug, Clone, Default)]
pub struct OptionList {
    /// (id, label)
    pub options: Vec<(String, String)>,
    pub highlighted: Option<usize>,
    offset: usize,
    /// Scroll to the highlight at the next draw (false after the wheel
    /// scrolled the list on its own).
    follow: bool,
    /// Where the list was drawn last, for mouse clicks.
    area: Rect,
    /// Columns of `area` taken by the scrollbar.
    bar_width: u16,
}

/// Rows the mouse wheel scrolls (Textual's `scroll_sensitivity_y`).
const WHEEL_ROWS: usize = 2;

impl OptionList {
    pub fn new(options: Vec<(String, String)>, highlighted: Option<usize>) -> Self {
        let mut list = Self {
            options,
            ..Self::default()
        };
        list.highlight(highlighted.unwrap_or(0));
        list
    }

    pub fn highlight(&mut self, index: usize) {
        self.follow = true;
        self.highlighted = if self.options.is_empty() {
            None
        } else {
            Some(index.min(self.options.len() - 1))
        };
    }

    pub fn highlighted_id(&self) -> Option<&str> {
        self.highlighted
            .and_then(|index| self.options.get(index))
            .map(|(id, _)| id.as_str())
    }

    pub fn labels(&self) -> Vec<&str> {
        self.options
            .iter()
            .map(|(_, label)| label.as_str())
            .collect()
    }

    fn page(&self) -> usize {
        usize::from(self.area.height).max(1)
    }

    fn step(&mut self, delta: i64) {
        if let Some(current) = self.highlighted {
            let last = self.options.len() as i64 - 1;
            self.highlight((current as i64 + delta).clamp(0, last.max(0)) as usize);
        }
    }

    /// Navigation keys; `Some(true)` for Enter (select), `None` if unhandled.
    pub fn handle_key(&mut self, key: &KeyEvent) -> Option<bool> {
        match key.code {
            KeyCode::Up if !shifted(key) => self.step(-1),
            KeyCode::Down if !shifted(key) => self.step(1),
            KeyCode::Home => self.highlight(0),
            KeyCode::End => self.highlight(usize::MAX),
            KeyCode::PageUp => self.step(-(self.page() as i64)),
            KeyCode::PageDown => self.step(self.page() as i64),
            KeyCode::Enter => return Some(self.highlighted.is_some()),
            _ => return None,
        }
        Some(false)
    }

    /// Clicks select an option; the wheel scrolls the list and leaves the
    /// highlight alone (Textual's `OptionList`). Returns true when an
    /// option was chosen.
    pub fn handle_mouse(&mut self, event: &MouseEvent) -> bool {
        let inside = self.area.contains((event.column, event.row).into());
        let max_offset = self
            .options
            .len()
            .saturating_sub(usize::from(self.area.height));
        match event.kind {
            MouseEventKind::Down(MouseButton::Left)
                if inside && event.column < self.area.right() - self.bar_width =>
            {
                let index = self.offset + usize::from(event.row - self.area.y);
                if index < self.options.len() {
                    self.highlight(index);
                    return true;
                }
            }
            MouseEventKind::ScrollUp if inside => {
                self.offset = self.offset.saturating_sub(WHEEL_ROWS);
                self.follow = false;
            }
            MouseEventKind::ScrollDown if inside => {
                self.offset = (self.offset + WHEEL_ROWS).min(max_offset);
                self.follow = false;
            }
            _ => {}
        }
        false
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        self.area = area;
        let height = usize::from(area.height);
        if let Some(index) = self.highlighted
            && self.follow
        {
            if index < self.offset {
                self.offset = index;
            } else if height > 0 && index >= self.offset + height {
                self.offset = index + 1 - height;
            }
        }
        self.offset = self.offset.min(self.options.len().saturating_sub(height));
        // A list longer than its area gets Textual's two-cell scrollbar.
        let scrollbar = self.options.len() > height && area.width > 2;
        self.bar_width = if scrollbar { 2 } else { 0 };
        let width = usize::from(area.width - self.bar_width);
        let lines: Vec<Line> = self
            .options
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(height)
            .map(|(index, (_, label))| {
                let text = ellipsize(label, width);
                if Some(index) == self.highlighted {
                    Line::from(fit_spans(vec![Span::raw(text)], width)).style(theme::highlight())
                } else {
                    Line::raw(text)
                }
            })
            .collect();
        let text_area = Rect::new(area.x, area.y, area.width - self.bar_width, area.height);
        frame.render_widget(Paragraph::new(lines), text_area);
        if scrollbar {
            render_scrollbar(
                frame,
                Rect::new(text_area.right(), area.y, 2, area.height),
                self.options.len(),
                self.offset,
            );
        }
    }
}

// ----------------------------------------------------------------------
// One-line text editor
// ----------------------------------------------------------------------

/// The app's clipboard, shared by every text prompt (Textual's
/// `App.clipboard`). Copies also go to the terminal's clipboard (OSC 52).
#[derive(Debug, Clone, Default)]
pub struct Clipboard {
    pub text: String,
    /// Text to send to the terminal with OSC 52 at the next draw.
    pub pending: Option<String>,
}

impl Clipboard {
    pub fn copy(&mut self, text: &str) {
        self.text = text.to_string();
        self.pending = Some(text.to_string());
    }
}

/// `\w` in Python's `re`: Unicode letters and digits, and `_`.
fn is_word(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// A one-line editor with Textual's `Input` keys: a selection from
/// `anchor` to `cursor` (character indices), word motions on Textual's
/// `(?<=\W)\w` word starts, and cut/copy/paste through the app clipboard.
#[derive(Debug, Clone, Default)]
pub struct LineEdit {
    pub value: String,
    /// Where the selection started; equal to `cursor` when there is none.
    pub anchor: usize,
    /// The cursor (the selection's end), in characters.
    pub cursor: usize,
    scroll: usize,
    /// True while the mouse is selecting.
    selecting: bool,
    /// Where the text was drawn last (for the mouse).
    area: Rect,
}

impl LineEdit {
    pub fn new(value: &str) -> Self {
        let end = value.chars().count();
        Self {
            value: value.to_string(),
            anchor: end,
            cursor: end,
            ..Self::default()
        }
    }

    fn byte_index(&self, chars: usize) -> usize {
        self.value
            .char_indices()
            .nth(chars)
            .map_or(self.value.len(), |(index, _)| index)
    }

    fn len(&self) -> usize {
        self.value.chars().count()
    }

    /// Move the cursor and drop the selection.
    fn set_cursor(&mut self, position: usize) {
        self.cursor = position.min(self.len());
        self.anchor = self.cursor;
    }

    /// Select from `anchor` to `cursor` (both clamped).
    fn select(&mut self, anchor: usize, cursor: usize) {
        self.anchor = anchor.min(self.len());
        self.cursor = cursor.min(self.len());
    }

    fn selection(&self) -> (usize, usize) {
        (self.anchor.min(self.cursor), self.anchor.max(self.cursor))
    }

    pub fn has_selection(&self) -> bool {
        self.anchor != self.cursor
    }

    pub fn selected_text(&self) -> String {
        let (start, end) = self.selection();
        self.value.chars().skip(start).take(end - start).collect()
    }

    /// Replace the characters `start..end` with `text` (control characters
    /// dropped); the cursor ends up after the new text.
    fn replace(&mut self, text: &str, start: usize, end: usize) {
        let (start, end) = (start.min(end), end.max(start).min(self.len()));
        let start = start.min(end);
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        let (from, to) = (self.byte_index(start), self.byte_index(end));
        self.value.replace_range(from..to, &text);
        self.set_cursor(start + text.chars().count());
    }

    fn delete_selection(&mut self) {
        let (start, end) = self.selection();
        self.replace("", start, end);
    }

    /// Type or paste `text`, replacing the selection.
    pub fn insert(&mut self, text: &str) {
        let (start, end) = self.selection();
        self.replace(text, start, end);
    }

    /// Word starts (`(?<=\W)\w`) in the characters `from..to`, searched as
    /// if that slice were the whole string (no look-behind past `from`).
    fn word_starts(&self, from: usize, to: usize) -> impl DoubleEndedIterator<Item = usize> {
        let chars: Vec<char> = self.value.chars().collect();
        (from + 1..to.min(chars.len()))
            .filter(move |&index| !is_word(chars[index - 1]) && is_word(chars[index]))
    }

    /// The start of the last word before the cursor, or 0.
    fn word_left(&self) -> usize {
        self.word_starts(0, self.cursor).next_back().unwrap_or(0)
    }

    /// The start of the next word after the cursor, or the end.
    fn word_right(&self) -> Option<usize> {
        self.word_starts(self.cursor, self.len()).next()
    }

    /// Editing keys; false if the key is not one of the input's.
    pub fn handle_key(&mut self, key: &KeyEvent, clipboard: &mut Clipboard) -> bool {
        let modifiers = key.modifiers;
        let ctrl = modifiers.contains(KeyModifiers::CONTROL);
        let alt = modifiers.contains(KeyModifiers::ALT);
        let shift = modifiers.contains(KeyModifiers::SHIFT);
        let sup = modifiers.contains(KeyModifiers::SUPER);
        let plain = !ctrl && !alt && !sup;
        let len = self.len();
        let cursor = self.cursor;
        let (start, end) = self.selection();
        let empty = !self.has_selection();
        match key.code {
            KeyCode::Char(c) if plain => self.insert(&c.to_string()),
            KeyCode::Left if plain && shift => self.select(self.anchor, cursor.saturating_sub(1)),
            KeyCode::Left if plain => self.set_cursor(if empty {
                cursor.saturating_sub(1)
            } else {
                start
            }),
            KeyCode::Right if plain && shift => self.select(self.anchor, cursor + 1),
            KeyCode::Right if plain => self.set_cursor(if empty { cursor + 1 } else { end }),
            KeyCode::Left if ctrl && !alt && shift => self.select(self.anchor, self.word_left()),
            KeyCode::Left if ctrl && !alt => self.set_cursor(self.word_left()),
            KeyCode::Right if ctrl && !alt => {
                let target = self.word_right().unwrap_or(len);
                if shift {
                    self.select(self.anchor, target);
                } else {
                    self.set_cursor(target);
                }
            }
            KeyCode::Home if plain && shift => self.select(cursor, 0),
            KeyCode::End if plain && shift => self.select(cursor, len),
            KeyCode::Home if plain => self.set_cursor(0),
            KeyCode::End if plain => self.set_cursor(len),
            // Ctrl+Shift+A selects everything (legacy terminals send it as
            // Ctrl+A, which goes to the start like Home).
            KeyCode::Char('a' | 'A') if ctrl && shift => self.select(0, len),
            KeyCode::Char('a') if ctrl => self.set_cursor(0),
            KeyCode::Char('e') if ctrl => self.set_cursor(len),
            KeyCode::Backspace if plain => {
                if empty {
                    self.replace("", cursor.saturating_sub(1), cursor);
                } else {
                    self.delete_selection();
                }
            }
            KeyCode::Delete if plain => {
                if empty {
                    self.replace("", cursor, cursor + 1);
                } else {
                    self.delete_selection();
                }
            }
            KeyCode::Char('d') if ctrl => {
                if empty {
                    self.replace("", cursor, cursor + 1);
                } else {
                    self.delete_selection();
                }
            }
            KeyCode::Char('w') if ctrl => {
                if empty {
                    let target = self.word_left();
                    self.replace("", target, cursor);
                } else {
                    self.delete_selection();
                }
            }
            KeyCode::Char('u') if ctrl => {
                if empty {
                    self.replace("", 0, cursor);
                } else {
                    self.delete_selection();
                }
            }
            KeyCode::Backspace if sup => {
                if empty {
                    self.replace("", 0, cursor);
                } else {
                    self.delete_selection();
                }
            }
            // Textual binds these to deleting the word to the *right*.
            KeyCode::Backspace if ctrl || alt => {
                if empty {
                    let target = self.word_right().unwrap_or(len);
                    self.replace("", cursor, target);
                } else {
                    self.delete_selection();
                }
            }
            KeyCode::Char('k') if ctrl => {
                if empty {
                    self.replace("", cursor, len);
                } else {
                    self.delete_selection();
                }
            }
            KeyCode::Char('x') if ctrl => {
                // An empty cut leaves the clipboard alone.
                if !empty {
                    clipboard.copy(&self.selected_text());
                    self.delete_selection();
                }
            }
            KeyCode::Char('c') if ctrl || sup => {
                if empty {
                    // Nothing to copy: the key is not used.
                    return false;
                }
                clipboard.copy(&self.selected_text());
            }
            KeyCode::Char('v') if ctrl => {
                let text = clipboard.text.clone();
                self.insert(&text);
            }
            _ => return false,
        }
        true
    }

    /// The character under a column of the drawn text.
    fn index_at(&self, column: u16) -> usize {
        let offset = usize::from(column.saturating_sub(self.area.x));
        let mut cells = 0;
        for (index, character) in self.value.chars().enumerate().skip(self.scroll) {
            let width = character.width().unwrap_or(0);
            if offset < cells + width {
                return index;
            }
            cells += width;
        }
        self.len()
    }

    /// A click puts the cursor under the pointer; dragging selects.
    pub fn handle_mouse(&mut self, event: &MouseEvent) {
        match event.kind {
            MouseEventKind::Down(MouseButton::Left)
                if self.area.contains((event.column, event.row).into()) =>
            {
                let index = self.index_at(event.column);
                self.set_cursor(index);
                self.selecting = true;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.selecting => {
                let index = self.index_at(event.column);
                self.select(self.anchor, index);
            }
            MouseEventKind::Up(_) | MouseEventKind::Moved => self.selecting = false,
            _ => {}
        }
    }

    /// Draw the bordered input (heavy green: it always has the focus) with
    /// Textual's `padding: 0 2`.
    pub fn render(&mut self, frame: &mut Frame, area: Rect, placeholder: &str) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Thick)
            .border_style(theme::accent())
            .padding(ratatui::widgets::Padding::horizontal(2));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        // The cursor may sit in the first padding cell after the text.
        let width = usize::from(inner.width).max(1);
        let text_area = Rect::new(
            inner.x,
            inner.y,
            (inner.width + 1).min(area.width.saturating_sub(inner.x - area.x + 1)),
            inner.height,
        );
        self.area = text_area;
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        }
        let chars: Vec<char> = self.value.chars().collect();
        let cells = |from: usize, to: usize| -> usize {
            chars[from..to].iter().map(|c| c.width().unwrap_or(0)).sum()
        };
        while self.scroll < self.cursor && cells(self.scroll, self.cursor) >= width {
            self.scroll += 1;
        }
        let cursor_style = Style::default().fg(Color::Black).bg(Color::Gray);
        let selection_style = Style::default().fg(Color::Gray).bg(Color::DarkGray);
        let line = if self.value.is_empty() {
            // The cursor sits on the placeholder's first character.
            let text = ellipsize(placeholder, usize::from(text_area.width));
            let mut rest = text.chars();
            let first = rest.next().map_or(" ".to_string(), String::from);
            Line::from(vec![
                Span::styled(first, cursor_style),
                Span::styled(rest.collect::<String>(), theme::dim()),
            ])
        } else {
            let (start, end) = self.selection();
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (index, character) in chars.iter().enumerate().skip(self.scroll) {
                let style = if index == self.cursor {
                    cursor_style
                } else if (start..end).contains(&index) {
                    selection_style
                } else {
                    theme::plain()
                };
                match spans.last_mut() {
                    Some(span) if span.style == style => span.content.to_mut().push(*character),
                    _ => spans.push(Span::styled(character.to_string(), style)),
                }
            }
            if self.cursor >= chars.len() {
                spans.push(Span::styled(" ", cursor_style));
            }
            Line::from(spans)
        };
        frame.render_widget(Paragraph::new(line), text_area);
    }
}

// ----------------------------------------------------------------------
// Choice
// ----------------------------------------------------------------------

/// What a dialog did with a key.
#[derive(Debug, Clone, PartialEq)]
pub enum Response<T> {
    /// Not a key for this dialog (the app may still use it, e.g. `q`).
    Ignored,
    Handled,
    Close(T),
}

/// A generic option picker.
#[derive(Debug, Clone)]
pub struct ChoiceDialog {
    pub title: String,
    pub subtitle: String,
    pub list: OptionList,
}

impl ChoiceDialog {
    pub fn new(title: &str, choices: Vec<(String, String)>, selected: Option<usize>) -> Self {
        Self {
            title: title.to_string(),
            subtitle: String::new(),
            list: OptionList::new(choices, selected),
        }
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> Response<Option<String>> {
        if key.code == KeyCode::Esc || matches!(plain_char(key), Some('q')) {
            return Response::Close(None);
        }
        match self.list.handle_key(key) {
            Some(true) => Response::Close(self.list.highlighted_id().map(str::to_string)),
            Some(false) => Response::Handled,
            None => Response::Ignored,
        }
    }

    pub fn handle_mouse(&mut self, event: &MouseEvent) -> Response<Option<String>> {
        if self.list.handle_mouse(event) {
            return Response::Close(self.list.highlighted_id().map(str::to_string));
        }
        Response::Handled
    }

    pub fn render(&mut self, frame: &mut Frame, screen: Rect) {
        let content = dialog_frame(frame, screen, &self.title, 72, Height::Percent(75));
        let width = usize::from(content.width).max(1);
        let hint = hint_block(hint_line(&[("↵", "Select"), ("Esc", "Cancel")]), width);
        let subtitle = (!self.subtitle.is_empty()).then(|| {
            let mut lines = wrap_line(text_line(&self.subtitle, theme::dim()), width);
            lines.push(Line::default());
            lines
        });
        let subtitle_height = subtitle.as_ref().map_or(0, |lines| lines.len() as u16);
        let areas = stack(
            content,
            &[Some(subtitle_height), None, Some(hint.len() as u16)],
        );
        if let Some(lines) = subtitle {
            frame.render_widget(Paragraph::new(lines), areas[0]);
        }
        self.list.render(frame, areas[1]);
        frame.render_widget(Paragraph::new(hint), areas[2]);
    }
}

// ----------------------------------------------------------------------
// Text prompt
// ----------------------------------------------------------------------

/// A one-line text prompt.
#[derive(Debug, Clone)]
pub struct PromptDialog {
    pub title: String,
    pub placeholder: String,
    pub input: LineEdit,
}

impl PromptDialog {
    pub fn new(title: &str, placeholder: &str, value: &str) -> Self {
        Self {
            title: title.to_string(),
            placeholder: placeholder.to_string(),
            input: LineEdit::new(value),
        }
    }

    pub fn handle_key(
        &mut self,
        key: &KeyEvent,
        clipboard: &mut Clipboard,
    ) -> Response<Option<String>> {
        match key.code {
            KeyCode::Esc => Response::Close(None),
            KeyCode::Enter => {
                let value = self.input.value.trim();
                Response::Close((!value.is_empty()).then(|| value.to_string()))
            }
            _ if self.input.handle_key(key, clipboard) => Response::Handled,
            _ => Response::Ignored,
        }
    }

    /// Bracketed paste: only the first line, like Textual's Input.
    pub fn paste(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let line = crate::text::splitlines(text).first().copied().unwrap_or("");
        self.input.insert(line);
    }

    pub fn handle_mouse(&mut self, event: &MouseEvent) {
        self.input.handle_mouse(event);
    }

    pub fn render(&mut self, frame: &mut Frame, screen: Rect) {
        let width = content_width(screen, 72);
        let hint = hint_block(hint_line(&[("↵", "Confirm"), ("Esc", "Cancel")]), width);
        let content = dialog_frame(
            frame,
            screen,
            &self.title,
            72,
            Height::Auto {
                content: 3 + hint.len() as u16,
                max_percent: 100,
            },
        );
        let areas = stack(content, &[Some(3), Some(hint.len() as u16)]);
        if areas[0].height == 3 {
            self.input.render(frame, areas[0], &self.placeholder);
        }
        frame.render_widget(Paragraph::new(hint), areas[1]);
    }
}

// ----------------------------------------------------------------------
// Confirmation
// ----------------------------------------------------------------------

/// A small yes/no confirmation dialog.
#[derive(Debug, Clone)]
pub struct ConfirmDialog {
    pub title: String,
    pub message: String,
}

impl ConfirmDialog {
    pub fn new(title: &str, message: &str) -> Self {
        Self {
            title: title.to_string(),
            message: message.to_string(),
        }
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> Response<bool> {
        match (key.code, plain_char(key)) {
            (KeyCode::Enter, _) | (_, Some('y' | 'Y')) => Response::Close(true),
            (KeyCode::Esc, _) | (_, Some('n' | 'N')) => Response::Close(false),
            _ => Response::Ignored,
        }
    }

    pub fn render(&mut self, frame: &mut Frame, screen: Rect) {
        let width = content_width(screen, 72);
        let lines = wrap_line(text_line(&self.message, theme::plain()), width);
        let hint = hint_block(
            hint_line(&[("y", "Yes"), ("n", "No"), ("Esc", "Cancel")]),
            width,
        );
        let content = dialog_frame(
            frame,
            screen,
            &self.title,
            72,
            Height::Auto {
                content: (lines.len() + hint.len()) as u16,
                max_percent: 100,
            },
        );
        let areas = stack(
            content,
            &[Some(lines.len() as u16), Some(hint.len() as u16)],
        );
        frame.render_widget(Paragraph::new(lines), areas[0]);
        frame.render_widget(Paragraph::new(hint), areas[1]);
    }
}

// ----------------------------------------------------------------------
// Advanced settings
// ----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Transform,
    Vrr,
    Enabled,
    Hdr,
    BitDepth,
    Color,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AdvancedResponse {
    Ignored,
    Handled,
    Apply,
    Cancel,
    /// Ask for an ICC file path, starting with this value.
    PickIcc(String),
}

/// Rotation, VRR, on/off and the compositor-specific color settings.
///
/// Edits a copy of the output; the app takes it over on Apply. HDR, color
/// depth and color profiles are only offered when the compositor
/// integration supports them.
#[derive(Debug, Clone)]
pub struct AdvancedDialog {
    pub output: Output,
    pub capabilities: Capabilities,
    pub compositor_label: String,
    /// False when this is the only enabled output (same rule as Space).
    pub can_disable: bool,
    pub fields: Vec<Field>,
    pub focus: usize,
    /// The custom color profile offered next to the presets.
    custom_profile: Option<String>,
}

fn cycle<T: PartialEq + Clone>(options: &[T], current: &T, delta: i64) -> T {
    let index = options.iter().position(|item| item == current).unwrap_or(0) as i64;
    let count = options.len() as i64;
    options[(index + delta).rem_euclid(count) as usize].clone()
}

impl AdvancedDialog {
    pub fn new(
        output: Output,
        capabilities: Capabilities,
        compositor_label: &str,
        can_disable: bool,
    ) -> Self {
        let mut fields = vec![Field::Transform, Field::Vrr, Field::Enabled];
        if capabilities.hdr {
            fields.push(Field::Hdr);
        }
        if !capabilities.bit_depths.is_empty() {
            fields.push(Field::BitDepth);
        }
        if capabilities.color_profile() {
            fields.push(Field::Color);
        }
        Self {
            custom_profile: output.color_profile.clone(),
            output,
            capabilities,
            compositor_label: compositor_label.to_string(),
            can_disable,
            fields,
            focus: 0,
        }
    }

    fn value(&self, field: Field) -> (&'static str, String) {
        let output = &self.output;
        let on = |value: bool| if value { "On" } else { "Off" }.to_string();
        match field {
            Field::Transform => ("Rotation", output.transform.clone()),
            Field::Vrr => ("VRR", on(output.adaptive_sync)),
            Field::Enabled => ("Output", on(output.enabled)),
            Field::Hdr => ("HDR", output.hdr.map_or_else(|| "default".to_string(), on)),
            Field::BitDepth => (
                "Color depth",
                match output.bit_depth {
                    Some(depth) if depth != 0 => format!("{depth} bit"),
                    _ => "default".to_string(),
                },
            ),
            Field::Color => (
                "Color profile",
                color_profile_label(output.color_profile.as_deref()),
            ),
        }
    }

    fn cycle_transform(&mut self, delta: i64) {
        let transforms: Vec<String> = TRANSFORMS.iter().map(|t| t.to_string()).collect();
        self.output.transform = cycle(&transforms, &self.output.transform, delta);
    }

    fn adjust(&mut self, delta: i64, notifications: &mut Notifications) {
        match self.fields[self.focus] {
            Field::Transform => self.cycle_transform(delta),
            Field::Vrr => self.output.adaptive_sync = !self.output.adaptive_sync,
            Field::Enabled => {
                if self.output.enabled && !self.can_disable {
                    notifications.notify(
                        "Cannot disable the last active monitor.",
                        "Warning",
                        Severity::Warning,
                        DIALOG_TOAST_SECONDS,
                    );
                    return;
                }
                self.output.enabled = !self.output.enabled;
            }
            Field::Hdr => self.output.hdr = Some(self.output.hdr != Some(true)),
            Field::BitDepth => {
                let mut depths: Vec<Option<i64>> = vec![None];
                depths.extend(self.capabilities.bit_depths.iter().copied().map(Some));
                self.output.bit_depth = cycle(&depths, &self.output.bit_depth, delta);
            }
            Field::Color => {
                let mut profiles: Vec<Option<String>> = vec![None];
                profiles.extend(self.capabilities.color_presets.iter().cloned().map(Some));
                // A custom (ICC) profile stays in the cycle, so cycling past
                // it does not lose it.
                for custom in [
                    self.custom_profile.clone(),
                    self.output.color_profile.clone(),
                ]
                .into_iter()
                .flatten()
                {
                    if !custom.is_empty() && !profiles.contains(&Some(custom.clone())) {
                        profiles.push(Some(custom));
                    }
                }
                self.output.color_profile = cycle(&profiles, &self.output.color_profile, delta);
            }
        }
    }

    fn pick_icc(&self) -> AdvancedResponse {
        if !self.capabilities.icc {
            return AdvancedResponse::Handled;
        }
        let current = self.output.color_profile.as_deref().unwrap_or("");
        AdvancedResponse::PickIcc(current.strip_prefix("icc:").unwrap_or("").to_string())
    }

    pub fn handle_key(
        &mut self,
        key: &KeyEvent,
        notifications: &mut Notifications,
    ) -> AdvancedResponse {
        let count = self.fields.len();
        match (key.code, plain_char(key)) {
            (KeyCode::Esc, _) => return AdvancedResponse::Cancel,
            (KeyCode::Enter, _) => return AdvancedResponse::Apply,
            (KeyCode::Up | KeyCode::BackTab, _) | (_, Some('k')) => {
                self.focus = (self.focus + count - 1) % count
            }
            (KeyCode::Down | KeyCode::Tab, _) | (_, Some('j')) => {
                self.focus = (self.focus + 1) % count
            }
            (KeyCode::Left, _) | (_, Some('h')) => self.adjust(-1, notifications),
            (KeyCode::Right, _) | (_, Some('l')) => self.adjust(1, notifications),
            (_, Some(' ')) => {
                if self.fields[self.focus] == Field::Color
                    && self.capabilities.icc
                    && self.capabilities.color_presets.is_empty()
                {
                    return self.pick_icc();
                }
                self.adjust(1, notifications);
            }
            (_, Some('t')) => self.cycle_transform(1),
            (_, Some('v')) => self.output.adaptive_sync = !self.output.adaptive_sync,
            (_, Some('i')) => return self.pick_icc(),
            _ => return AdvancedResponse::Ignored,
        }
        AdvancedResponse::Handled
    }

    /// The ICC prompt finished with `path` (None if cancelled).
    pub fn finish_icc(
        &mut self,
        path: Option<String>,
        home: &Path,
        color_ok: &dyn Fn(&str) -> bool,
        notifications: &mut Notifications,
    ) {
        let Some(path) = path else { return };
        // Compositors open the path verbatim, so store it absolute.
        let path = paths::expanduser(&path, home);
        if !path.is_absolute() {
            notifications.notify(
                "Use an absolute path (or ~/…) for the ICC profile.",
                "Error",
                Severity::Error,
                DIALOG_TOAST_SECONDS,
            );
            return;
        }
        let candidate = format!("icc:{}", path.display());
        if !color_ok(&candidate) {
            notifications.notify(
                &format!("{} cannot use that ICC path.", self.compositor_label),
                "Error",
                Severity::Error,
                DIALOG_TOAST_SECONDS,
            );
            return;
        }
        if !path.is_file() {
            notifications.notify(
                &format!("{} does not exist on this machine.", path.display()),
                "Warning",
                Severity::Warning,
                DIALOG_TOAST_SECONDS,
            );
        }
        self.output.color_profile = Some(candidate.clone());
        self.custom_profile = Some(candidate);
        if let Some(index) = self.fields.iter().position(|field| *field == Field::Color) {
            self.focus = index;
        }
    }

    pub fn body_lines(&self, width: usize) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let title = self.output.title();
        if !title.is_empty() && title != self.output.name {
            lines.push(Line::styled(title, theme::dim()));
            lines.push(Line::default());
        }
        for (index, field) in self.fields.iter().enumerate() {
            let focused = index == self.focus;
            let (label, value) = self.value(*field);
            let spans = vec![
                Span::styled(if focused { "▸ " } else { "  " }, theme::bold()),
                Span::styled(
                    format!("{label:<16}"),
                    if focused {
                        theme::bold_fg(Color::Yellow)
                    } else {
                        theme::bold()
                    },
                ),
                Span::styled(
                    value,
                    if focused {
                        theme::plain()
                    } else {
                        theme::dim()
                    },
                ),
            ];
            let mut line = Line::from(fit_spans(spans, width));
            if focused {
                line = line.patch_style(Style::default().bg(Color::DarkGray));
            }
            lines.push(line);
        }
        let caps = &self.capabilities;
        let mut missing = Vec::new();
        if !caps.hdr {
            missing.push("HDR");
        }
        if caps.bit_depths.is_empty() {
            missing.push("color depth");
        }
        if !caps.color_profile() {
            missing.push("color profile");
        }
        if !missing.is_empty() {
            lines.push(Line::default());
            let note = format!(
                "Not supported by {}: {}.",
                self.compositor_label,
                missing.join(", ")
            );
            lines.extend(wrap_line(
                Line::styled(note, theme::dim().add_modifier(Modifier::ITALIC)),
                width,
            ));
        }
        lines.push(Line::default());
        lines.extend(wrap_line(
            Line::from(vec![
                Span::styled("Changes stay pending until you press ", theme::dim()),
                Span::styled("a", theme::bold()),
                Span::styled(" in the main view.", theme::dim()),
            ]),
            width,
        ));
        lines
    }

    pub fn render(&mut self, frame: &mut Frame, screen: Rect) {
        let width = content_width(screen, 76);
        let body = self.body_lines(width);
        let mut hint = vec![
            ("↑↓", "Nav"),
            ("←→", "Adjust"),
            ("Space", "Toggle"),
            ("↵", "Done"),
            ("Esc", "Cancel"),
        ];
        if self.capabilities.icc {
            hint.push(("i", "ICC file"));
        }
        let hint = hint_block(hint_line(&hint), width);
        let content = dialog_frame(
            frame,
            screen,
            &format!("Advanced · {}", self.output.name),
            76,
            Height::Auto {
                content: (body.len() + hint.len()) as u16,
                max_percent: 90,
            },
        );
        // The body comes first; a dialog too small for everything crops the
        // hint, not the fields.
        let areas = stack(content, &[Some(body.len() as u16), Some(hint.len() as u16)]);
        frame.render_widget(Paragraph::new(body), areas[0]);
        frame.render_widget(Paragraph::new(hint), areas[1]);
    }
}

// ----------------------------------------------------------------------
// Profile browser
// ----------------------------------------------------------------------

pub const SAVE_ID: &str = "__save__";
pub const LAUNCH_ID: &str = "__launch__";

#[derive(Debug, Clone, PartialEq)]
pub enum ProfilesResponse {
    Ignored,
    Handled,
    /// The chosen profile name, `SAVE_ID`, `LAUNCH_ID`, or None (closed).
    Close(Option<String>),
    /// Ask for a new name for this profile.
    Rename(String),
}

/// Browse, apply, save, rename, reorder and delete named profiles.
#[derive(Debug, Clone)]
pub struct ProfilesDialog {
    pub list: OptionList,
    pub standalone: bool,
    pub allow_save: bool,
    pub current_states: Vec<OutputState>,
    pub confirm_delete: Option<String>,
    /// The last failed action, shown above the file's own problems until
    /// the next action succeeds (reloading the list must not hide it).
    pub action_error: Option<String>,
    pub problems: Vec<String>,
    /// Set when a rename, move or delete changed the store (the app then
    /// re-checks which profile is active).
    pub changed: bool,
}

impl ProfilesDialog {
    pub fn new(
        store: &ProfileStore,
        current_states: Vec<OutputState>,
        initial_highlight: Option<&str>,
        standalone: bool,
        allow_save: bool,
    ) -> Self {
        let mut dialog = Self {
            list: OptionList::default(),
            standalone,
            allow_save,
            current_states,
            confirm_delete: None,
            action_error: None,
            problems: Vec::new(),
            changed: false,
        };
        dialog.reload(store, initial_highlight);
        dialog
    }

    /// Re-read the store. The highlight moves to `highlight` if given, else
    /// stays on the same profile.
    pub fn reload(&mut self, store: &ProfileStore, highlight: Option<&str>) {
        let keep = highlight
            .map(str::to_string)
            .or_else(|| self.list.highlighted_id().map(str::to_string));
        let previous_index = self.list.highlighted.unwrap_or(0);
        let mut names = Vec::new();
        let mut profiles = Vec::new();
        let mut active = None;
        let loaded = (|| {
            profiles = store.load()?;
            active = store.find_active(&self.current_states)?;
            names = store.names()?;
            // Broken profiles are skipped, but say why they are missing.
            self.problems = store.problems()?;
            Ok::<(), crate::profiles::ProfileError>(())
        })();
        if let Err(error) = loaded {
            self.problems = vec![error.0];
        }

        let mut options = Vec::new();
        for name in &names {
            let Some(profile) = profiles.iter().find(|profile| &profile.name == name) else {
                continue;
            };
            let count = profile.outputs.len();
            let suffix = if count == 1 { "output" } else { "outputs" };
            let marker = if active.as_ref() == Some(name) {
                " *"
            } else {
                ""
            };
            options.push((name.clone(), format!("{name}{marker}  ({count} {suffix})")));
        }
        if self.allow_save {
            options.push((SAVE_ID.to_string(), "Save current layout…".to_string()));
        }
        if self.standalone {
            options.push((LAUNCH_ID.to_string(), "[ Open monitor editor ]".to_string()));
        }
        let target = self
            .confirm_delete
            .clone()
            .or(keep)
            .and_then(|name| options.iter().position(|(id, _)| *id == name));
        self.list.options = options;
        self.list
            .highlight(target.unwrap_or(previous_index.min(names.len().saturating_sub(1))));
    }

    fn selected_name(&self) -> Option<String> {
        self.list
            .highlighted_id()
            .filter(|id| *id != SAVE_ID && *id != LAUNCH_ID)
            .map(str::to_string)
    }

    /// Run a store change; on failure keep the error visible.
    fn store_call<T>(
        &mut self,
        call: impl FnOnce() -> Result<T, crate::profiles::ProfileError>,
    ) -> Option<T> {
        match call() {
            Ok(value) => {
                self.action_error = None;
                Some(value)
            }
            Err(error) => {
                self.action_error = Some(error.0);
                None
            }
        }
    }

    pub fn messages(&self) -> Vec<String> {
        let mut messages: Vec<String> = self.action_error.iter().cloned().collect();
        messages.extend(self.problems.iter().cloned());
        messages
    }

    fn move_selected(&mut self, offset: i64, store: &ProfileStore) {
        let Some(name) = self.selected_name() else {
            return;
        };
        if self.store_call(|| store.move_by(&name, offset)) == Some(true) {
            self.changed = true;
            self.reload(store, Some(&name));
        }
    }

    pub fn handle_key(
        &mut self,
        key: &KeyEvent,
        store: &ProfileStore,
        notifications: &mut Notifications,
    ) -> ProfilesResponse {
        if let Some(name) = self.confirm_delete.clone() {
            match (key.code, plain_char(key)) {
                (_, Some('y' | 'Y')) => {
                    self.confirm_delete = None;
                    if self.store_call(|| store.delete(&name)) == Some(true) {
                        self.changed = true;
                        notifications.notify(
                            &format!("Deleted profile {}.", repr(&name)),
                            "Info",
                            Severity::Information,
                            DIALOG_TOAST_SECONDS,
                        );
                    }
                    self.reload(store, None);
                }
                (KeyCode::Esc, _) | (_, Some('n' | 'N')) => {
                    self.confirm_delete = None;
                    self.reload(store, Some(&name));
                }
                _ => {}
            }
            return ProfilesResponse::Handled;
        }

        match (key.code, plain_char(key)) {
            (KeyCode::Esc, _) | (_, Some('q')) => return ProfilesResponse::Close(None),
            (KeyCode::Up, _) if shifted(key) => self.move_selected(-1, store),
            (KeyCode::Down, _) if shifted(key) => self.move_selected(1, store),
            (_, Some('s')) => {
                if self.allow_save {
                    return ProfilesResponse::Close(Some(SAVE_ID.to_string()));
                }
            }
            (_, Some('l')) => {
                if self.standalone {
                    return ProfilesResponse::Close(Some(LAUNCH_ID.to_string()));
                }
            }
            (_, Some('d')) => {
                if let Some(name) = self.selected_name() {
                    self.confirm_delete = Some(name);
                    self.reload(store, None);
                }
            }
            (_, Some('r')) => {
                if let Some(name) = self.selected_name() {
                    return ProfilesResponse::Rename(name);
                }
            }
            (_, Some('k')) => self.list.step(-1),
            (_, Some('j')) => self.list.step(1),
            _ => match self.list.handle_key(key) {
                Some(true) => {
                    return ProfilesResponse::Close(self.list.highlighted_id().map(str::to_string));
                }
                Some(false) => {}
                None => return ProfilesResponse::Ignored,
            },
        }
        ProfilesResponse::Handled
    }

    pub fn handle_mouse(&mut self, event: &MouseEvent) -> ProfilesResponse {
        if self.confirm_delete.is_none() && self.list.handle_mouse(event) {
            return ProfilesResponse::Close(self.list.highlighted_id().map(str::to_string));
        }
        ProfilesResponse::Handled
    }

    /// The rename prompt finished.
    pub fn finish_rename(&mut self, old: &str, value: Option<String>, store: &ProfileStore) {
        let Some(value) = value.filter(|value| value != old) else {
            return;
        };
        if self.store_call(|| store.rename(old, &value)) == Some(true) {
            self.changed = true;
            let clean = value.trim().to_string();
            self.reload(store, Some(&clean));
        }
    }

    fn hint(&self) -> Line<'static> {
        if let Some(name) = &self.confirm_delete {
            let mut line = Line::from(vec![
                Span::styled("Delete profile ", theme::hint()),
                Span::styled(name.clone(), theme::hint_key()),
                Span::styled("?  ", theme::hint()),
            ]);
            line.spans
                .extend(hint_line(&[("y", "Yes"), ("n", "No")]).spans);
            return line;
        }
        if self.standalone {
            hint_line(&[
                ("↵", "Apply & exit"),
                ("r", "Rename"),
                ("d", "Delete"),
                ("⇧↑↓", "Reorder"),
                ("l", "Editor"),
                ("Esc", "Quit"),
            ])
        } else {
            hint_line(&[
                ("↵", "Apply"),
                ("s", "Save"),
                ("r", "Rename"),
                ("d", "Delete"),
                ("⇧↑↓", "Reorder"),
                ("Esc", "Close"),
            ])
        }
    }

    pub fn render(&mut self, frame: &mut Frame, screen: Rect) {
        let content = dialog_frame(frame, screen, "Profiles", 72, Height::Percent(75));
        let width = usize::from(content.width).max(1);
        let hint = hint_block(self.hint(), width);
        let messages: Vec<Line> = self
            .messages()
            .into_iter()
            .flat_map(|message| wrap_line(text_line(&message, theme::fg(Color::Red)), width))
            .collect();
        // The messages never take the list's last row.
        let room = content.height.saturating_sub(hint.len() as u16 + 1);
        let message_height = (messages.len() as u16).min(room);
        let areas = stack(
            content,
            &[Some(message_height), None, Some(hint.len() as u16)],
        );
        frame.render_widget(Paragraph::new(messages), areas[0]);
        self.list.render(frame, areas[1]);
        frame.render_widget(Paragraph::new(hint), areas[2]);
    }
}

// ----------------------------------------------------------------------
// Help
// ----------------------------------------------------------------------

pub const HELP_TEXT: &str = "\
**Move**
  ←↓↑→ / hjkl            move by the grid size
  Shift + arrows         move by 10× the grid size
  g                      cycle grid size (1, 8, 16, 32, 64 px)
  L                      cycle snap mode (Off, Edges, Centers, Both)

**Display**
  Tab / Shift+Tab        select next / previous output
  r                      pick scale (0.50x – 3.00x, custom)
  [ / ]                  decrease / increase scale by 0.05
  f                      pick resolution and refresh rate
  t                      pick rotation
  v                      toggle adaptive sync (VRR)
  c / d                  advanced settings (HDR, color depth, color profile)
  Space / Enter          enable / disable the selected output

**Layout**
  a                      apply pending changes
  Ctrl+R                 reload outputs, discarding pending changes
  z                      reset pending changes to the last applied state
  Z                      revert to the layout before the last apply
  S                      save the current layout as the startup profile

**Profiles**
  p                      save the current layout as a named profile
  o                      open the profile browser
  Enter (in browser)     apply a profile
  r (in browser)         rename a profile
  d (in browser)         delete a profile
  Shift+↑ / Shift+↓      reorder profiles

**Mouse**
  left click / drag      select and move an output
  right click            enable / disable an output
  scroll wheel           adjust scale by 0.05

**Other**
  ?                      show this help
  q / Ctrl+C             quit";

/// Scrollable keyboard reference.
#[derive(Debug, Clone)]
pub struct HelpDialog {
    pub extra: String,
    pub scroll: usize,
    page: usize,
    max_scroll: usize,
    area: Rect,
}

impl HelpDialog {
    pub fn new(extra: String) -> Self {
        Self {
            extra,
            scroll: 0,
            page: 10,
            max_scroll: usize::MAX,
            area: Rect::default(),
        }
    }

    fn lines(&self, width: usize) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        for line in HELP_TEXT.lines() {
            match line
                .strip_prefix("**")
                .and_then(|rest| rest.strip_suffix("**"))
            {
                Some(heading) => lines.push(Line::styled(heading.to_string(), theme::bold())),
                None => lines.extend(wrap_line(Line::raw(line.to_string()), width)),
            }
        }
        // The key reference ends in a newline (a blank line), and the
        // compositor note has `padding-top: 1`.
        lines.push(Line::default());
        if !self.extra.is_empty() {
            lines.push(Line::default());
            lines.extend(wrap_line(text_line(&self.extra, theme::plain()), width));
        }
        lines.extend(hint_block(
            hint_line(&[("↑↓", "Scroll"), ("PgUp/PgDn", "Page"), ("Esc", "Close")]),
            width,
        ));
        lines
    }

    fn scroll_by(&mut self, delta: i64) {
        let target = (self.scroll as i64 + delta).max(0) as usize;
        self.scroll = target.min(self.max_scroll);
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> Response<()> {
        match (key.code, plain_char(key)) {
            (KeyCode::Esc, _) | (_, Some('q' | '?')) => return Response::Close(()),
            (KeyCode::Up, _) | (_, Some('k')) => self.scroll_by(-1),
            (KeyCode::Down, _) | (_, Some('j')) => self.scroll_by(1),
            (KeyCode::PageUp, _) => self.scroll_by(-(self.page as i64)),
            (KeyCode::PageDown, _) => self.scroll_by(self.page as i64),
            (KeyCode::Home, _) => self.scroll = 0,
            (KeyCode::End, _) => self.scroll = self.max_scroll,
            _ => return Response::Ignored,
        }
        Response::Handled
    }

    pub fn handle_mouse(&mut self, event: &MouseEvent) {
        if !self.area.contains((event.column, event.row).into()) {
            return;
        }
        match event.kind {
            MouseEventKind::ScrollUp => self.scroll_by(-(WHEEL_ROWS as i64)),
            MouseEventKind::ScrollDown => self.scroll_by(WHEEL_ROWS as i64),
            _ => {}
        }
    }

    pub fn render(&mut self, frame: &mut Frame, screen: Rect) {
        let width = content_width(screen, 80);
        let mut lines = self.lines(width);
        let content = dialog_frame(
            frame,
            screen,
            "Keys",
            80,
            Height::Auto {
                content: lines.len() as u16,
                max_percent: 90,
            },
        );
        let height = usize::from(content.height);
        // Text that does not fit gets a two-cell scrollbar on the right (in
        // the scrollbar's color, like Textual's) and wraps a bit narrower.
        let scrollbar = lines.len() > height && content.width > 2;
        let mut text = content;
        if scrollbar {
            text.width -= 2;
            lines = self.lines(usize::from(text.width).max(1));
        }
        self.area = content;
        self.page = height.max(1);
        self.max_scroll = lines.len().saturating_sub(height);
        self.scroll = self.scroll.min(self.max_scroll);
        let total = lines.len();
        let visible: Vec<Line> = lines.into_iter().skip(self.scroll).collect();
        frame.render_widget(Paragraph::new(visible), text);
        if scrollbar && height > 0 {
            render_scrollbar(
                frame,
                Rect::new(text.right(), content.y, 2, content.height),
                total,
                self.scroll,
            );
        }
    }
}

/// Textual's vertical scrollbar (`ScrollBarRender.render_bar`): the thumb
/// in the scrollbar color, with eighth-block glyphs at its ends.
fn render_scrollbar(frame: &mut Frame, area: Rect, total: usize, scroll: usize) {
    const BARS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", " "];
    let size = f64::from(area.height);
    let virtual_size = total as f64;
    if area.height == 0 || virtual_size <= size {
        return;
    }
    let thumb = (size / (virtual_size / size)).max(1.0);
    let position = (size - thumb) * (scroll as f64 / (virtual_size - size));
    let start = (position * 8.0) as usize;
    let end = start + (thumb * 8.0).ceil() as usize;
    let (start_index, start_bar) = (start / 8, start % 8);
    let (end_index, end_bar) = (end / 8, end % 8);
    let bar = Style::default().fg(Color::DarkGray);
    let buffer = frame.buffer_mut();
    for row in 0..usize::from(area.height) {
        let (symbol, style) = if row == start_index && start_bar > 0 {
            (BARS[7 - start_bar], bar)
        } else if row == end_index && end_bar > 0 {
            (BARS[7 - end_bar], bar.add_modifier(Modifier::REVERSED))
        } else if (start_index..end_index).contains(&row) {
            (" ", Style::default().bg(Color::DarkGray))
        } else {
            continue;
        };
        for column in area.x..area.right() {
            if let Some(cell) = buffer.cell_mut((column, area.y + row as u16)) {
                cell.set_symbol(symbol).set_style(style);
            }
        }
    }
}

#[cfg(test)]
mod tests;
