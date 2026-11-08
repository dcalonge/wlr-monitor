//! Drawing the dialog on top of the stack.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, Paragraph, Wrap};

use crate::app::App;
use crate::dialog::{Advanced, Choice, Dialog, Help, Profiles, Prompt};
use crate::theme;

const HELP_TEXT: &str = "\
Move
  ←↓↑→ / hjkl            move by the grid size
  Shift + arrows         move by 10× the grid size
  g                      cycle grid size (1, 8, 16, 32, 64 px)
  L                      toggle snapping to other outputs

Display
  Tab / Shift+Tab        select next / previous output
  r                      pick scale (0.50x – 3.00x, custom)
  [ / ]                  decrease / increase scale by 0.05
  f                      pick resolution and refresh rate
  t                      pick rotation
  v                      toggle adaptive sync (VRR)
  c / d                  advanced settings (HDR, color depth, color profile)
  Space / Enter          enable / disable the selected output

Layout
  a                      apply pending changes
  Ctrl+R                 reload outputs, discarding pending changes
  z                      reset pending changes to the last applied state
  Z                      revert to the layout before the last apply
  S                      save the current layout as the startup profile

Profiles
  p                      save the current layout as a named profile
  o                      open the profile browser
  Enter (in browser)     apply a profile
  r (in browser)         rename a profile
  d (in browser)         delete a profile
  Shift+↑ / Shift+↓      reorder profiles

Mouse
  left click / drag      select and move an output
  right click            enable / disable an output
  scroll wheel           adjust scale by 0.05

Other
  ?                      show this help
  q / Ctrl+C             quit
  Ctrl+Q                 quit from anywhere";

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let compositor = app.compositor().name();
    let Some(dialog) = app.dialogs.last_mut() else {
        return;
    };
    match dialog {
        Dialog::Choice(choice) => draw_choice(frame, area, choice),
        Dialog::Prompt(prompt) => draw_prompt(frame, area, prompt),
        Dialog::Confirm(message) => draw_confirm(frame, area, message),
        Dialog::Advanced(advanced) => draw_advanced(frame, area, advanced, compositor),
        Dialog::Profiles(profiles) => draw_profiles(frame, area, profiles),
        Dialog::Help(help) => draw_help(frame, area, help),
    }
}

/// Clear a centered box, draw its border and return the area inside.
fn open(frame: &mut Frame, area: Rect, width: u16, height: u16, title: &str) -> Rect {
    let [rect] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(area);
    let [rect] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(rect);
    let block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(theme::accent())
        .title(format!(" {title} "));
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    inner
}

fn hint(text: &str) -> Line<'static> {
    Line::styled(text.to_string(), theme::dim())
}

fn draw_choice(frame: &mut Frame, area: Rect, choice: &mut Choice) {
    let widest = choice
        .labels
        .iter()
        .map(|label| label.chars().count())
        .max()
        .unwrap_or(0) as u16;
    let height = (choice.labels.len() as u16 + 2).min(area.height.saturating_sub(2));
    let inner = open(
        frame,
        area,
        widest.max(24) + 6,
        height,
        &format!("{} · {}", choice.title, choice.output),
    );
    let list =
        List::new(choice.labels.iter().map(String::as_str)).highlight_style(theme::highlight());
    frame.render_stateful_widget(list, inner, &mut choice.state);
}

fn draw_prompt(frame: &mut Frame, area: Rect, prompt: &Prompt) {
    let inner = open(frame, area, 60, 4, &prompt.title);
    let width = usize::from(inner.width.max(1));
    let scroll = prompt.input.visual_scroll(width);
    let text = if prompt.input.value().is_empty() {
        Line::styled(prompt.placeholder, theme::dim())
    } else {
        Line::raw(prompt.input.value())
    };
    frame.render_widget(Paragraph::new(text).scroll((0, scroll as u16)), inner);
    frame.render_widget(
        Paragraph::new(hint("↵ OK, Esc Cancel")),
        Rect {
            y: inner.y + 1,
            height: 1,
            ..inner
        },
    );
    let cursor = prompt.input.visual_cursor().saturating_sub(scroll) as u16;
    frame.set_cursor_position((inner.x + cursor.min(inner.width.saturating_sub(1)), inner.y));
}

fn draw_confirm(frame: &mut Frame, area: Rect, message: &str) {
    let inner = open(frame, area, 60, 6, "Confirm");
    let lines = vec![Line::raw(message), Line::raw(""), hint("y Yes, n No")];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn draw_advanced(frame: &mut Frame, area: Rect, advanced: &Advanced, compositor: &str) {
    let title = format!("Advanced · {}", advanced.output.name);
    let inner = open(frame, area, 62, advanced.fields.len() as u16 + 7, &title);
    let mut lines: Vec<Line> = advanced
        .fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let (label, value) = advanced.row(*field);
            let line = Line::from(vec![
                Span::raw(format!(" {label:<14}")),
                Span::raw(format!("‹ {value} ›")),
            ]);
            if index == advanced.focus {
                line.style(theme::highlight())
            } else {
                line
            }
        })
        .collect();
    lines.push(Line::raw(""));
    let icc = if advanced.capabilities.icc {
        ", i ICC file"
    } else {
        ""
    };
    lines.push(hint(&format!(
        "↑↓ Nav, ←→ Adjust, Space Toggle, ↵ Done, Esc Cancel{icc}"
    )));
    lines.push(hint(
        "Changes stay pending until you press a in the main view.",
    ));
    if !advanced.capabilities.hdr || advanced.capabilities.bit_depths.is_empty() {
        lines.push(hint(&format!(
            "Not supported by {compositor}: HDR and color depth need mango, sway or niri."
        )));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn draw_profiles(frame: &mut Frame, area: Rect, profiles: &mut Profiles) {
    let mut footer = vec![Line::raw("")];
    if profiles.confirm_delete {
        let name = profiles.selected_name().unwrap_or_default();
        footer.push(Line::styled(
            format!("Delete {name}? y/n"),
            theme::pending(),
        ));
    } else {
        let keys = if profiles.standalone {
            "l Editor"
        } else {
            "s Save current"
        };
        footer.push(hint(&format!(
            "↵ Apply, {keys}, r Rename, d Delete, Shift+↑↓ Reorder, Esc Close"
        )));
    }
    if let Some(error) = &profiles.error {
        footer.push(Line::styled(error.as_str(), Style::new().fg(Color::Red)));
    }
    footer.extend(
        profiles
            .problems
            .iter()
            .map(|problem| Line::styled(problem.as_str(), theme::pending())),
    );

    let rows = profiles.entries.len().max(1) as u16;
    let inner = open(frame, area, 70, rows + footer.len() as u16 + 2, "Profiles");
    let [list_area, footer_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(footer.len() as u16)])
            .areas(inner);
    if profiles.entries.is_empty() {
        frame.render_widget(Paragraph::new(hint("No saved profiles yet.")), list_area);
    } else {
        let items = profiles.entries.iter().map(|entry| {
            let marker = if entry.active { "●" } else { " " };
            Line::raw(format!(
                "{marker} {} ({} outputs)",
                entry.name, entry.outputs
            ))
        });
        frame.render_stateful_widget(
            List::new(items).highlight_style(theme::highlight()),
            list_area,
            &mut profiles.state,
        );
    }
    frame.render_widget(
        Paragraph::new(footer).wrap(Wrap { trim: true }),
        footer_area,
    );
}

fn draw_help(frame: &mut Frame, area: Rect, help: &mut Help) {
    let text = format!("{HELP_TEXT}\n\n{}", help.extra);
    let inner = open(
        frame,
        area,
        80,
        area.height.saturating_sub(2),
        "Help · Esc to close",
    );
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
    let max = paragraph
        .line_count(inner.width)
        .saturating_sub(usize::from(inner.height));
    help.scroll = help.scroll.min(max as u16);
    frame.render_widget(paragraph.scroll((help.scroll, 0)), inner);
}
