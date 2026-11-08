//! Drawing the dialog on top of the stack.
//!
//! Popups are centered, have a heavy green border with the title in it and a
//! centered blue key hint at the bottom.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, Padding, Paragraph, Wrap};

use crate::app::App;
use crate::compositor::Capabilities;
use crate::dialog::{Advanced, Choice, Dialog, Help, Profiles, Prompt};
use crate::theme;

const DIALOG_WIDTH: u16 = 72;
const ADVANCED_WIDTH: u16 = 76;
const HELP_WIDTH: u16 = 80;

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

/// Center a box, draw its heavy green border and return the padded content.
fn open(frame: &mut Frame, area: Rect, width: u16, height: u16, title: &str) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(theme::accent())
        .title(Line::styled(format!(" {title} "), theme::label()))
        .padding(Padding::new(2, 2, 1, 0));
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    inner
}

/// Split the content into the body and the hint row, leaving a blank line
/// above and below the hint.
fn body_and_hint(area: Rect) -> (Rect, Rect) {
    if area.height < 4 || area.width == 0 {
        return (area, Rect::default());
    }
    let body = Rect {
        height: area.height - 3,
        ..area
    };
    let hint = Rect {
        y: area.bottom() - 2,
        height: 1,
        ..area
    };
    (body, hint)
}

fn hint(parts: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, (key, label)) in parts.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" | ", theme::help()));
        }
        spans.push(Span::styled((*key).to_string(), theme::help_key()));
        spans.push(Span::styled(format!(" {label}"), theme::help()));
    }
    Line::from(spans)
}

fn draw_hint(frame: &mut Frame, area: Rect, line: Line<'static>) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
}

fn draw_choice(frame: &mut Frame, area: Rect, choice: &mut Choice) {
    let height = (u32::from(area.height) * 75 / 100) as u16;
    let inner = open(frame, area, DIALOG_WIDTH, height, choice.title);
    let (body, hint_area) = body_and_hint(inner);
    let list =
        List::new(choice.labels.iter().map(String::as_str)).highlight_style(theme::highlight());
    frame.render_stateful_widget(list, body, &mut choice.state);
    draw_hint(
        frame,
        hint_area,
        hint(&[("↵", "Select"), ("Esc", "Cancel")]),
    );
}

fn draw_prompt(frame: &mut Frame, area: Rect, prompt: &Prompt) {
    let inner = open(frame, area, DIALOG_WIDTH, 9, &prompt.title);
    let (body, hint_area) = body_and_hint(inner);
    let input_area = Rect {
        height: body.height.min(3),
        ..body
    };
    let block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(theme::accent());
    let input_inner = block.inner(input_area);
    frame.render_widget(block, input_area);
    if input_inner.width == 0 || input_inner.height == 0 {
        return;
    }

    let width = usize::from(input_inner.width.max(1));
    let scroll = prompt.input.visual_scroll(width);
    let text = if prompt.input.value().is_empty() {
        Line::styled(prompt.placeholder, theme::dim())
    } else {
        Line::raw(prompt.input.value())
    };
    frame.render_widget(Paragraph::new(text).scroll((0, scroll as u16)), input_inner);
    let cursor = prompt.input.visual_cursor().saturating_sub(scroll) as u16;
    frame.set_cursor_position((
        input_inner.x + cursor.min(input_inner.width.saturating_sub(1)),
        input_inner.y,
    ));
    draw_hint(
        frame,
        hint_area,
        hint(&[("↵", "Confirm"), ("Esc", "Cancel")]),
    );
}

fn draw_confirm(frame: &mut Frame, area: Rect, message: &str) {
    let inner = open(frame, area, DIALOG_WIDTH, 9, "Confirm");
    let (body, hint_area) = body_and_hint(inner);
    frame.render_widget(Paragraph::new(message).wrap(Wrap { trim: true }), body);
    draw_hint(
        frame,
        hint_area,
        hint(&[("y", "Yes"), ("n", "No"), ("Esc", "Cancel")]),
    );
}

fn missing_settings(capabilities: &Capabilities) -> Option<String> {
    let mut missing = Vec::new();
    if !capabilities.hdr {
        missing.push("HDR");
    }
    if capabilities.bit_depths.is_empty() {
        missing.push("color depth");
    }
    if !capabilities.color_profile() {
        missing.push("color profile");
    }
    (!missing.is_empty()).then(|| missing.join(", "))
}

fn draw_advanced(frame: &mut Frame, area: Rect, advanced: &Advanced, compositor: &str) {
    let title = format!("Advanced · {}", advanced.output.name);
    let output_title = advanced.output.title();
    let show_title = !output_title.is_empty() && output_title != advanced.output.name;
    let missing = missing_settings(&advanced.capabilities);
    let body_lines = advanced.fields.len() as u16
        + 1
        + u16::from(missing.is_some())
        + 1
        + 1
        + if show_title { 2 } else { 0 };
    let max_height = (u32::from(area.height) * 90 / 100) as u16;
    let height = (body_lines + 6).min(max_height);
    let inner = open(frame, area, ADVANCED_WIDTH, height, &title);
    let (body, hint_area) = body_and_hint(inner);

    let mut lines = Vec::new();
    if show_title {
        lines.push(Line::styled(output_title, theme::dim()));
        lines.push(Line::raw(""));
    }
    for (index, field) in advanced.fields.iter().enumerate() {
        let (label, value) = advanced.row(*field);
        let focused = index == advanced.focus;
        let mut line = Line::from(vec![
            Span::styled(if focused { "▸ " } else { "  " }, theme::label()),
            Span::styled(
                format!("{label:<16}"),
                if focused {
                    theme::pending().add_modifier(Modifier::BOLD)
                } else {
                    theme::label()
                },
            ),
            Span::styled(value, if focused { Style::new() } else { theme::dim() }),
        ]);
        if focused {
            line = line.style(Style::new().bg(Color::DarkGray));
        }
        lines.push(line);
    }
    lines.push(Line::raw(""));
    if let Some(missing) = missing {
        lines.push(Line::styled(
            format!("Not supported by {compositor}: {missing}."),
            theme::dim().add_modifier(Modifier::ITALIC),
        ));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled("Changes stay pending until you press ", theme::dim()),
        Span::styled("a", theme::label()),
        Span::styled(" in the main view.", theme::dim()),
    ]));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), body);

    let mut parts = vec![
        ("↑↓", "Nav"),
        ("←→", "Adjust"),
        ("Space", "Toggle"),
        ("↵", "Done"),
        ("Esc", "Cancel"),
    ];
    if advanced.capabilities.icc {
        parts.push(("i", "ICC file"));
    }
    draw_hint(frame, hint_area, hint(&parts));
}

fn draw_profiles(frame: &mut Frame, area: Rect, profiles: &mut Profiles) {
    let height = (u32::from(area.height) * 75 / 100) as u16;
    let inner = open(frame, area, DIALOG_WIDTH, height, "Profiles");
    let (body, hint_area) = body_and_hint(inner);

    let mut list_area = body;
    if let Some(error) = &profiles.error
        && list_area.height > 0
    {
        frame.render_widget(
            Paragraph::new(error.as_str()).style(theme::error()),
            Rect {
                height: 1,
                ..list_area
            },
        );
        list_area = Rect {
            y: list_area.y + 1,
            height: list_area.height.saturating_sub(1),
            ..list_area
        };
    }
    for problem in &profiles.problems {
        if list_area.height == 0 {
            break;
        }
        frame.render_widget(
            Paragraph::new(problem.as_str()).style(theme::pending()),
            Rect {
                height: 1,
                ..list_area
            },
        );
        list_area = Rect {
            y: list_area.y + 1,
            height: list_area.height.saturating_sub(1),
            ..list_area
        };
    }

    if profiles.entries.is_empty() {
        frame.render_widget(
            Paragraph::new("No saved profiles yet.").style(theme::dim()),
            list_area,
        );
    } else {
        let items = profiles.entries.iter().map(|entry| {
            let marker = if entry.active { " *" } else { "" };
            let suffix = if entry.outputs == 1 {
                "output"
            } else {
                "outputs"
            };
            Line::raw(format!(
                "{}{marker}  ({} {suffix})",
                entry.name, entry.outputs
            ))
        });
        frame.render_stateful_widget(
            List::new(items).highlight_style(theme::highlight()),
            list_area,
            &mut profiles.state,
        );
    }

    if profiles.confirm_delete {
        let name = profiles.selected_name().unwrap_or_default();
        let line = Line::from(vec![
            Span::styled(format!("Delete profile {name}?  "), theme::help()),
            Span::styled("y", theme::help_key()),
            Span::styled(" Yes", theme::help()),
            Span::styled(" | ", theme::help()),
            Span::styled("n", theme::help_key()),
            Span::styled(" No", theme::help()),
        ]);
        draw_hint(frame, hint_area, line);
    } else {
        let mut parts = vec![("↵", "Apply")];
        if profiles.standalone {
            parts.extend([
                ("r", "Rename"),
                ("d", "Delete"),
                ("⇧↑↓", "Reorder"),
                ("l", "Editor"),
                ("Esc", "Quit"),
            ]);
        } else {
            parts.extend([
                ("s", "Save"),
                ("r", "Rename"),
                ("d", "Delete"),
                ("⇧↑↓", "Reorder"),
                ("Esc", "Close"),
            ]);
        }
        draw_hint(frame, hint_area, hint(&parts));
    }
}

fn styled_help(text: &str) -> Vec<Line<'static>> {
    text.lines()
        .map(|line| {
            if line.is_empty() || line.starts_with(' ') {
                Line::raw(line.to_string())
            } else {
                Line::styled(line.to_string(), theme::label())
            }
        })
        .collect()
}

fn draw_help(frame: &mut Frame, area: Rect, help: &mut Help) {
    let max_height = (u32::from(area.height) * 90 / 100) as u16;
    let inner = open(frame, area, HELP_WIDTH, max_height, "Keys");
    let (body, hint_area) = body_and_hint(inner);

    let mut lines = styled_help(HELP_TEXT);
    if !help.extra.is_empty() {
        lines.push(Line::raw(""));
        lines.extend(help.extra.lines().map(|line| Line::raw(line.to_string())));
    }
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let max = paragraph
        .line_count(body.width)
        .saturating_sub(usize::from(body.height));
    help.scroll = help.scroll.min(max as u16);
    frame.render_widget(paragraph.scroll((help.scroll, 0)), body);
    draw_hint(
        frame,
        hint_area,
        hint(&[("↑↓", "Scroll"), ("PgUp/PgDn", "Page"), ("Esc", "Close")]),
    );
}
