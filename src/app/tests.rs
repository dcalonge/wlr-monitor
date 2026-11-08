use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::style::Color;

use super::*;
use crate::compositor::{Compositor, Kind};
use crate::model::tests::load_outputs;
use crate::screens::Field;
use crate::testutil::{FakeWlr, TempDir};

/// A sandboxed HOME / XDG layout with an empty PATH (no IPC tools).
struct Sandbox {
    dir: TempDir,
    env: HashMap<String, String>,
}

impl Sandbox {
    fn new() -> Self {
        let dir = TempDir::new();
        fs::create_dir_all(dir.join("bin")).unwrap();
        let env = HashMap::from([
            ("HOME".to_string(), dir.join("home").display().to_string()),
            (
                "XDG_CONFIG_HOME".to_string(),
                dir.join("home/.config").display().to_string(),
            ),
            (
                "XDG_STATE_HOME".to_string(),
                dir.join("state").display().to_string(),
            ),
            ("PATH".to_string(), dir.join("bin").display().to_string()),
        ]);
        Self { dir, env }
    }

    fn profiles(&self) -> PathBuf {
        self.dir.join("profiles.json")
    }

    fn store(&self) -> ProfileStore {
        ProfileStore::new(Some(self.profiles()))
    }

    fn app(&self, kind: Kind, wlr: &FakeWlr) -> App {
        let compositor = Compositor::new(kind, Some(self.env.clone()), None, true);
        App::new(
            Backend::new(Box::new(wlr.clone()), compositor),
            self.store(),
            false,
        )
    }

    /// An app started with the fixture outputs (no wlr-randr read).
    fn started(&self, kind: Kind) -> App {
        let mut app = self.app(kind, &FakeWlr::live(load_outputs()));
        app.start_with(load_outputs());
        app
    }
}

use std::path::PathBuf;

fn key(spec: &str) -> KeyEvent {
    // `ctrl+shift+right`, `alt+enter`, `q`, … (the last part is the key).
    let mut modifiers = KeyModifiers::NONE;
    let mut name = spec;
    while let Some((prefix, rest)) = name.split_once('+')
        && !rest.is_empty()
    {
        modifiers |= match prefix {
            "shift" => KeyModifiers::SHIFT,
            "ctrl" => KeyModifiers::CONTROL,
            "alt" => KeyModifiers::ALT,
            "super" => KeyModifiers::SUPER,
            _ => break,
        };
        name = rest;
    }
    let code = match name {
        "tab" if modifiers.contains(KeyModifiers::SHIFT) => {
            return KeyEvent::new(KeyCode::BackTab, modifiers);
        }
        "tab" => KeyCode::Tab,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "enter" => KeyCode::Enter,
        "escape" => KeyCode::Esc,
        "space" => KeyCode::Char(' '),
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "backspace" => KeyCode::Backspace,
        other => {
            let character = other.chars().next().unwrap();
            let modifiers = if character.is_uppercase() {
                modifiers | KeyModifiers::SHIFT
            } else {
                modifiers
            };
            return KeyEvent::new(KeyCode::Char(character), modifiers);
        }
    };
    KeyEvent::new(code, modifiers)
}

fn press(app: &mut App, keys: &[&str]) {
    for spec in keys {
        app.on_key(key(spec));
    }
}

fn type_text(app: &mut App, text: &str) {
    for character in text.chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
}

fn render(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut text = String::new();
    for y in 0..height {
        for x in 0..width {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }
    text
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// Screen cell inside an output's box on the map (after a render).
fn map_cell(app: &App, name: &str) -> (u16, u16) {
    let rect = app
        .regions
        .map
        .geometry
        .as_ref()
        .unwrap()
        .get(name)
        .unwrap();
    let content = app.regions.map_content;
    (content.x + rect.x as u16 + 1, content.y + rect.y as u16 + 1)
}

fn top(app: &App) -> Option<&Modal> {
    app.modals.last()
}

fn messages(app: &App) -> Vec<String> {
    app.notifications.messages()
}

#[test]
fn app_loads_outputs_moves_and_uses_hyprmon_defaults() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    let names: Vec<_> = app.outputs.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["HDMI-A-1", "eDP-1"]);
    assert_eq!(app.current().unwrap().name, "HDMI-A-1");
    assert_eq!(
        (app.grid, app.snap_mode, app.snap_threshold),
        (32, SnapMode::Edges, 10)
    );
    let screen = render(&mut app, 110, 34);
    assert!(screen.contains("HDMI-A-1") && screen.contains("eDP-1"));
    assert!(screen.contains("grid 32px · snap Edges · Ready."));

    app.action_move_right();
    assert_eq!(app.outputs[0].x, 32);
    assert!(app.dirty_names().contains("HDMI-A-1"));
    assert!(!app.guides.is_empty());

    app.action_reset();
    assert_eq!(app.outputs[0].x, 0);
    assert!(app.dirty_names().is_empty());
    assert!(app.guides.is_empty());

    app.action_next_output();
    assert_eq!(app.current().unwrap().name, "eDP-1");
    press(&mut app, &["tab"]);
    assert_eq!(app.current().unwrap().name, "HDMI-A-1");
    press(&mut app, &["shift+tab"]);
    assert_eq!(app.current().unwrap().name, "eDP-1");
    press(&mut app, &["tab", "right"]);
    assert_eq!(app.outputs[0].x, 32);
    press(&mut app, &["l"]);
    assert_eq!(app.outputs[0].x, 64);
    press(&mut app, &["shift+down"]);
    assert_eq!(app.outputs[0].y, 320);
    press(&mut app, &["K"]);
    assert_eq!(app.outputs[0].y, 0);
}

#[test]
fn grid_and_snap_keys_cycle_like_hyprmon() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    press(&mut app, &["g"]);
    assert_eq!(app.grid, 64);
    press(&mut app, &["g"]);
    assert_eq!(app.grid, GRID_SIZES[0]);
    press(&mut app, &["G"]);
    assert_eq!(app.grid, GRID_SIZES[1]);
    assert_eq!(app.status, "Grid: 8px");

    press(&mut app, &["L"]);
    assert_eq!(app.snap_mode, SnapMode::Centers);
    assert_eq!(app.status, "Snap: Centers (10px)");
    press(&mut app, &["L"]);
    assert_eq!(app.snap_mode, SnapMode::Both);
    press(&mut app, &["L"]);
    assert_eq!(app.snap_mode, SnapMode::Off);
    assert_eq!(app.status, "Snap: Off");
}

#[test]
fn scale_stepping_clamps_to_hyprmon_range() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    app.outputs[0].scale = 2.95;
    press(&mut app, &["]"]);
    assert_eq!(app.outputs[0].scale, 3.0);
    press(&mut app, &["]"]);
    assert_eq!(app.outputs[0].scale, 3.0);
    press(&mut app, &["["]);
    assert_eq!(app.outputs[0].scale, 2.95);
    app.outputs[0].scale = 0.52;
    app.action_scale_down();
    assert_eq!(app.outputs[0].scale, 0.5);
    app.action_scale_down();
    assert_eq!(app.outputs[0].scale, 0.5);
    assert_eq!(app.status, "HDMI-A-1: scale 0.5");
}

#[test]
fn last_active_monitor_cannot_be_disabled() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    press(&mut app, &["space"]);
    assert!(!app.outputs[0].enabled);
    app.selected = 1;
    press(&mut app, &["enter"]);
    assert!(app.outputs[1].enabled);
    assert!(app.status.to_lowercase().contains("last active monitor"));
}

#[test]
fn single_output_cannot_be_disabled() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.app(Kind::Wlroots, &FakeWlr::live(load_outputs()));
    app.start_with(load_outputs()[..1].to_vec());
    app.action_toggle_enabled();
    assert!(app.outputs[0].enabled);
    assert!(app.status.to_lowercase().contains("last active monitor"));
}

fn advanced(app: &App) -> &AdvancedDialog {
    match top(app) {
        Some(Modal::Advanced(dialog)) => dialog,
        other => panic!("expected the advanced dialog, got {other:?}"),
    }
}

#[test]
fn advanced_dialog_applies_and_cancels() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    press(&mut app, &["c"]);
    assert_eq!(
        advanced(&app).fields,
        [Field::Transform, Field::Vrr, Field::Enabled]
    );
    press(&mut app, &["t"]);
    assert_eq!(advanced(&app).output.transform, "90");
    press(&mut app, &["down", "space"]);
    assert!(advanced(&app).output.adaptive_sync);
    press(&mut app, &["enter"]);
    assert!(app.modals.is_empty());
    assert_eq!(app.outputs[0].transform, "90");
    assert!(app.outputs[0].adaptive_sync);
    assert!(app.dirty_names().contains("HDMI-A-1"));
    assert_eq!(app.status, "HDMI-A-1: advanced settings updated");

    app.action_reset();
    press(&mut app, &["d", "t"]);
    assert_eq!(advanced(&app).output.transform, "90");
    press(&mut app, &["escape"]);
    assert!(app.modals.is_empty());
    assert_eq!(app.outputs[0].transform, "normal");
    assert!(app.dirty_names().is_empty());
    assert_eq!(app.status, "HDMI-A-1: advanced settings cancelled");

    // h/l and arrows cycle the rotation both ways; v toggles VRR.
    press(&mut app, &["C", "left"]);
    assert_eq!(advanced(&app).output.transform, "flipped-270");
    press(&mut app, &["l", "v"]);
    assert_eq!(advanced(&app).output.transform, "normal");
    assert!(advanced(&app).output.adaptive_sync);
}

#[test]
fn help_blocks_shortcuts() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    press(&mut app, &["?"]);
    assert!(matches!(top(&app), Some(Modal::Help(_))));
    let screen = render(&mut app, 80, 24);
    assert!(screen.contains("Keys") && screen.contains("move by the grid size"));
    press(&mut app, &["g"]);
    assert_eq!(app.grid, 32);
    press(&mut app, &["pagedown", "end"]);
    let scrolled = render(&mut app, 80, 24);
    assert!(scrolled.contains("show this help"));
    assert!(scrolled.contains("Compositor: wlroots"));
    assert!(scrolled.contains("Changes are applied with wlr-randr only"));
    press(&mut app, &["home"]);
    assert!(render(&mut app, 80, 24).contains("move by the grid size"));
    press(&mut app, &["escape"]);
    assert!(app.modals.is_empty());
    press(&mut app, &["?", "?"]);
    assert!(app.modals.is_empty());
}

#[test]
fn advanced_dialog_offers_compositor_color_settings() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Sway);
    press(&mut app, &["c"]);
    assert_eq!(
        advanced(&app).fields,
        [
            Field::Transform,
            Field::Vrr,
            Field::Enabled,
            Field::Hdr,
            Field::BitDepth,
            Field::Color
        ]
    );
    press(&mut app, &["down", "down", "down", "space"]);
    assert_eq!(advanced(&app).output.hdr, Some(true));
    press(&mut app, &["down", "right", "right", "right"]);
    assert_eq!(advanced(&app).output.bit_depth, Some(10));
    press(&mut app, &["down", "right"]);
    assert_eq!(
        advanced(&app).output.color_profile.as_deref(),
        Some("gamma22")
    );
    press(&mut app, &["enter"]);
    assert!(app.dirty_names().contains("HDMI-A-1"));
    let screen = render(&mut app, 110, 34);
    assert!(screen.contains("HDR"));

    // Reset must drop the new settings again, not keep them.
    app.action_reset();
    assert_eq!(app.outputs[0].hdr, None);
    assert_eq!(app.outputs[0].bit_depth, None);
    assert_eq!(app.outputs[0].color_profile, None);
    assert!(app.dirty_names().is_empty());

    press(&mut app, &["c", "down", "down", "down", "space", "escape"]);
    assert_eq!(app.outputs[0].hdr, None);
}

#[test]
fn generic_backend_marks_color_settings_unsupported() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    press(&mut app, &["c"]);
    let screen = render(&mut app, 110, 34);
    assert!(screen.contains("Not supported by wlroots: HDR, color depth, color profile."));
    assert!(screen.contains("Advanced · HDMI-A-1"));
}

#[test]
fn apply_warnings_are_notified_and_profiles_are_passed_on() {
    let sandbox = Sandbox::new();
    let store = sandbox.store();
    store
        .save(
            "desk",
            &load_outputs().iter().map(Output::state).collect::<Vec<_>>(),
        )
        .unwrap();
    // sway without swaymsg: applies through wlr-randr and warns.
    let wlr = FakeWlr {
        warning: Some("heads up".into()),
        ..FakeWlr::live(load_outputs())
    };
    let mut app = sandbox.app(Kind::Sway, &wlr);
    app.start();
    app.outputs[0].x = 64;
    app.action_apply();
    let messages = messages(&app);
    assert!(messages.contains(&"heads up".to_string()), "{messages:?}");
    assert!(messages.iter().any(|m| m.contains("swaymsg was not found")));
    assert!(app.apply_profile("desk"));
    let include = sandbox.dir.join("home/.config/sway/wlr-monitor.conf");
    assert!(
        fs::read_to_string(include)
            .unwrap()
            .contains("# Profile: desk")
    );
}

#[test]
fn small_terminals_hide_secondary_sections() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    let small = render(&mut app, 40, 14);
    assert!(small.contains("HDMI-A-1"));
    assert!(!small.contains("Outputs") && !small.contains("Physical"));
    assert_eq!(app.regions.outputs_content, Rect::default());
    let large = render(&mut app, 120, 40);
    assert!(large.contains("Outputs") && large.contains("Physical"));
    let medium = render(&mut app, 100, 22);
    assert!(medium.contains("Outputs") && !medium.contains("Physical"));
}

#[test]
fn the_main_view_matches_the_readme_layout() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    let screen = render(&mut app, 72, 24);
    let lines: Vec<&str> = screen.lines().collect();
    assert_eq!(lines[0].trim(), "");
    assert!(lines[1].starts_with(" ┏━ Layout ━"), "{}", lines[1]);
    assert!(lines[1].ends_with("┓ "));
    let bottom = lines
        .iter()
        .position(|line| line.starts_with(" ┗"))
        .unwrap();
    assert!(
        lines[bottom].ends_with(" grid 32px · snap Edges · Ready. ━┛ "),
        "{}",
        lines[bottom]
    );
    assert!(lines[bottom + 1].starts_with(" ┌─ Outputs ─"));
    assert!(screen.contains("▸ "));
    assert!(screen.contains("←↓↑→ Move | ⇥ Next"));
}

#[test]
fn only_ansi_colors_reach_the_terminal() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Sway);
    app.outputs[1].x = 2000;
    press(&mut app, &["c"]);
    let mut terminal = Terminal::new(TestBackend::new(110, 34)).unwrap();
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();
    for cell in terminal.backend().buffer().content() {
        for color in [cell.fg, cell.bg] {
            assert!(
                !matches!(color, Color::Rgb(..) | Color::Indexed(_)),
                "{color:?}"
            );
        }
    }
}

#[test]
fn app_saves_profiles_and_marks_active() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    press(&mut app, &["p"]);
    assert!(matches!(
        top(&app),
        Some(Modal::Prompt(_, PromptPurpose::SaveProfile))
    ));
    type_text(&mut app, "work");
    press(&mut app, &["enter"]);
    assert_eq!(app.active_profile.as_deref(), Some("work"));
    assert!(messages(&app).contains(&"Saved profile 'work' (2 outputs).".to_string()));

    press(&mut app, &["o"]);
    let Some(Modal::Profiles(dialog)) = top(&app) else {
        panic!("no browser")
    };
    assert!(
        dialog
            .list
            .labels()
            .iter()
            .any(|label| label.contains("work *  (2 outputs)"))
    );
    assert!(dialog.list.labels().contains(&"Save current layout…"));

    // Rename through the browser.
    press(&mut app, &["r"]);
    assert!(matches!(top(&app), Some(Modal::Prompt(..))));
    press(&mut app, &["ctrl+u"]);
    type_text(&mut app, "office");
    press(&mut app, &["enter"]);
    assert_eq!(sandbox.store().names().unwrap(), ["office"]);
    assert!(matches!(top(&app), Some(Modal::Profiles(_))));

    // Delete through the browser, with confirmation.
    press(&mut app, &["d"]);
    let screen = render(&mut app, 110, 34);
    assert!(screen.contains("Delete profile office?"), "{screen}");
    press(&mut app, &["g"]); // ignored while confirming
    press(&mut app, &["y"]);
    assert!(sandbox.store().names().unwrap().is_empty());
    assert!(messages(&app).contains(&"Deleted profile 'office'.".to_string()));

    // "Save current layout…" asks for a name like `p`.
    press(&mut app, &["enter"]);
    assert!(matches!(
        top(&app),
        Some(Modal::Prompt(_, PromptPurpose::SaveProfile))
    ));
    press(&mut app, &["escape"]);
    assert!(app.modals.is_empty());
}

#[test]
fn startup_profile_overwrite_confirmation() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    press(&mut app, &["S"]);
    assert_eq!(sandbox.store().names().unwrap(), [STARTUP_PROFILE]);
    app.outputs[0].x = 64;
    press(&mut app, &["S"]);
    assert!(matches!(top(&app), Some(Modal::Confirm(..))));
    press(&mut app, &["n"]);
    let saved = sandbox.store().get(STARTUP_PROFILE).unwrap().unwrap();
    assert_eq!(saved.output("HDMI-A-1").unwrap().x, 0);
    press(&mut app, &["S", "y"]);
    let saved = sandbox.store().get(STARTUP_PROFILE).unwrap().unwrap();
    assert_eq!(saved.output("HDMI-A-1").unwrap().x, 64);
}

#[test]
fn profiles_only_launches_browser_and_editor() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.app(Kind::Wlroots, &FakeWlr::live(load_outputs()));
    app.profiles_only = true;
    app.start();
    let Some(Modal::Profiles(dialog)) = top(&app) else {
        panic!("no browser")
    };
    assert!(dialog.list.labels().contains(&"[ Open monitor editor ]"));
    assert!(!dialog.list.labels().contains(&"Save current layout…"));
    press(&mut app, &["l"]);
    assert!(app.modals.is_empty());
    assert!(!app.profiles_only);
    assert_eq!(app.status, "Monitor editor ready.");

    // Closing the standalone browser quits.
    app.profiles_only = true;
    app.action_open_profiles();
    press(&mut app, &["escape"]);
    assert!(app.quit);
}

#[test]
fn standalone_browser_exits_after_applying() {
    let sandbox = Sandbox::new();
    let store = sandbox.store();
    let mut states: Vec<_> = load_outputs().iter().map(Output::state).collect();
    states[0].x = 64;
    store.save("home", &states).unwrap();
    let wlr = FakeWlr::live(load_outputs());
    let mut app = sandbox.app(Kind::Wlroots, &wlr);
    app.profiles_only = true;
    app.start();
    press(&mut app, &["enter"]);
    assert_eq!(wlr.applied(), ["HDMI-A-1", "eDP-1"]);
    assert!(app.quit);
}

#[test]
fn output_mouse_messages_are_wired() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    app.on_output_toggled("HDMI-A-1");
    assert!(!app.outputs[0].enabled);
    app.on_output_scale_step("HDMI-A-1", 1);
    assert_eq!(app.outputs[0].scale, 1.05);
    app.on_output_selected("HDMI-A-1");
    let start = app.outputs[0].x;
    app.on_output_moved("HDMI-A-1", 40, 0);
    assert_eq!(app.outputs[0].x, start + 32);
}

#[test]
fn apply_uses_backend_and_updates_baseline() {
    let sandbox = Sandbox::new();
    let wlr = FakeWlr::live(load_outputs());
    let mut app = sandbox.app(Kind::Wlroots, &wlr);
    app.start();
    app.action_revert_live();
    assert!(app.status.to_lowercase().contains("no previous layout"));

    app.outputs[0].x = 96;
    press(&mut app, &["a"]);
    assert_eq!(wlr.applied(), ["HDMI-A-1"]);
    assert_eq!(app.outputs[0].x, 96);
    assert!(app.dirty_names().is_empty());
    assert_eq!(app.status, "Applied 1 output(s).");

    press(&mut app, &["Z"]);
    assert_eq!(wlr.applied(), ["HDMI-A-1", "HDMI-A-1", "eDP-1"]);
    assert_eq!(app.outputs[0].x, 0);
    assert!(app.dirty_names().is_empty());
    // Reverting twice goes back to the applied layout.
    press(&mut app, &["Z"]);
    assert_eq!(app.outputs[0].x, 96);

    press(&mut app, &["A"]);
    assert_eq!(app.status, "Nothing to apply.");
}

#[test]
fn apply_failures_are_reported() {
    let sandbox = Sandbox::new();
    let wlr = FakeWlr {
        fail: vec!["HDMI-A-1".into()],
        ..FakeWlr::live(load_outputs())
    };
    let mut app = sandbox.app(Kind::Wlroots, &wlr);
    app.start();
    app.outputs[0].x = 96;
    app.action_apply();
    assert_eq!(app.status, "Apply failed — HDMI-A-1: bad mode");
    assert_eq!(
        app.error.as_deref(),
        Some("Apply failed — HDMI-A-1: bad mode")
    );
    assert!(app.dirty_names().contains("HDMI-A-1"));
    assert!(render(&mut app, 110, 34).contains("Error: Apply failed"));
}

#[test]
fn start_reads_once_and_reload_reads_again() {
    let sandbox = Sandbox::new();
    let wlr = FakeWlr::live(load_outputs()[..1].to_vec());
    let mut app = sandbox.app(Kind::Wlroots, &wlr);
    assert!(app.loading);
    assert!(render(&mut app, 110, 34).contains("Reading outputs…"));
    app.start();
    assert!(!app.loading);
    assert_eq!(wlr.read_count(), 1);
    assert_eq!(app.outputs.len(), 1);
    wlr.set_outputs(load_outputs());
    app.outputs[0].x = 500;
    press(&mut app, &["ctrl+r"]);
    assert_eq!(wlr.read_count(), 2);
    assert_eq!(app.outputs.len(), 2);
    assert_eq!(app.outputs[0].x, 0);
    assert_eq!(app.status, "Reloaded outputs from wlr-randr.");
}

#[test]
fn read_errors_are_shown() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.app(Kind::Wlroots, &FakeWlr::failing("wlr-randr was not found."));
    app.start();
    assert!(app.outputs.is_empty());
    assert_eq!(app.status, "wlr-randr was not found.");
    let screen = render(&mut app, 110, 34);
    assert!(screen.contains("Error: wlr-randr was not found."));
    assert!(screen.contains("No outputs found"));
    // Nothing to act on, but nothing breaks either.
    press(
        &mut app,
        &[
            "right", "space", "f", "r", "t", "c", "a", "p", "S", "tab", "z",
        ],
    );
    assert!(app.modals.is_empty());
}

#[test]
fn mode_and_scale_pickers() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    press(&mut app, &["f"]);
    let Some(Modal::Choice(dialog, _)) = top(&app) else {
        panic!("no picker")
    };
    assert_eq!(dialog.title, "Resolution and refresh rate");
    assert_eq!(dialog.list.labels()[0], "● 1920x1080 @ 100 Hz");
    assert_eq!(dialog.list.labels()[1], "  1920x1080 @ 60 Hz  preferred");
    press(&mut app, &["g"]);
    assert_eq!(app.grid, 32);
    press(&mut app, &["down", "enter"]);
    assert_eq!(app.outputs[0].mode.as_ref().unwrap().refresh, 60.0);
    assert_eq!(app.status, "HDMI-A-1: 1920x1080 @ 60 Hz");

    press(&mut app, &["r", "down", "enter"]);
    assert_eq!(app.outputs[0].scale, 1.1);

    // Custom value, with a comma and validation.
    press(&mut app, &["R", "end", "enter"]);
    assert!(matches!(
        top(&app),
        Some(Modal::Prompt(_, PromptPurpose::CustomScale { .. }))
    ));
    press(&mut app, &["ctrl+u"]);
    type_text(&mut app, "1,4");
    press(&mut app, &["enter"]);
    assert_eq!(app.outputs[0].scale, 1.4);
    press(&mut app, &["r", "end", "enter", "ctrl+u"]);
    type_text(&mut app, "abc");
    press(&mut app, &["enter"]);
    assert_eq!(app.status, "Invalid scale: 'abc'");
    press(&mut app, &["r", "end", "enter", "ctrl+u"]);
    type_text(&mut app, "9");
    press(&mut app, &["enter"]);
    assert_eq!(app.status, "Scale must be between 0.5 and 3.");
    assert_eq!(app.outputs[0].scale, 1.4);
    // The picker opens on a custom current value.
    press(&mut app, &["r"]);
    let Some(Modal::Choice(dialog, _)) = top(&app) else {
        panic!("no picker")
    };
    assert_eq!(dialog.list.highlighted_id(), Some("1.4"));
    press(&mut app, &["q"]);
    assert!(app.modals.is_empty());
    assert!(!app.quit);
}

#[test]
fn corrupt_profile_file_does_not_crash_browser() {
    let sandbox = Sandbox::new();
    fs::write(sandbox.profiles(), "{ nope").unwrap();
    let mut app = sandbox.started(Kind::Wlroots);
    press(&mut app, &["o"]);
    let screen = render(&mut app, 110, 34);
    assert!(screen.contains("Could not parse"), "{screen}");
    press(&mut app, &["escape"]);
    assert!(app.modals.is_empty());
}

#[test]
fn mouse_click_and_right_click_on_layout_map() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    render(&mut app, 110, 34);
    let (x, y) = map_cell(&app, "eDP-1");
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
    app.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left), x, y));
    assert_eq!(app.current().unwrap().name, "eDP-1");

    let (x, y) = map_cell(&app, "HDMI-A-1");
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Right), x, y));
    assert!(!app.outputs[0].enabled);

    // Scrolling over an output changes its scale.
    app.on_mouse(mouse(MouseEventKind::ScrollUp, x, y));
    assert_eq!(app.outputs[0].scale, 1.05);
    app.on_mouse(mouse(MouseEventKind::ScrollDown, x, y));
    assert_eq!(app.outputs[0].scale, 1.0);

    // Clicking a row of the outputs table selects it, with any button
    // (Textual's `Click`); a right-click there toggles nothing.
    let table = app.regions.outputs_content;
    app.on_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        table.x + 3,
        table.y + 2,
    ));
    assert_eq!(app.current().unwrap().name, "HDMI-A-1");
    app.on_mouse(mouse(
        MouseEventKind::Down(MouseButton::Right),
        table.x + 3,
        table.y + 3,
    ));
    assert_eq!(app.current().unwrap().name, "eDP-1");
    assert!(app.outputs[1].enabled);
}

#[test]
fn dragging_moves_outputs_on_the_grid() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    app.snap_mode = SnapMode::Off;
    render(&mut app, 110, 34);
    let scale = app.regions.map.geometry.as_ref().unwrap().scale;
    let (x, y) = map_cell(&app, "eDP-1");
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
    app.on_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), x + 10, y));
    let moved = (10.0 / (scale * 2.0)).round_ties_even();
    let expected = quantize(1920.0 + moved, 32);
    assert_eq!(app.outputs[1].x, expected);
    assert!(app.status.starts_with("eDP-1: "));
    app.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left), x + 10, y));
    assert!(app.guides.is_empty());
    assert!(app.dirty_names().contains("eDP-1"));
    // Moving the mouse without a drag does nothing.
    app.on_mouse(mouse(MouseEventKind::Moved, x + 30, y));
    assert_eq!(app.outputs[1].x, expected);

    // A dialog opened mid-drag ends it: later motion moves nothing.
    render(&mut app, 110, 34);
    let (x, y) = map_cell(&app, "eDP-1");
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
    press(&mut app, &["f"]);
    app.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left), x, y));
    press(&mut app, &["escape"]);
    app.on_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), x + 20, y));
    app.on_mouse(mouse(MouseEventKind::Moved, x + 40, y));
    assert_eq!(app.outputs[1].x, expected);

    // A lost release: plain motion ends the drag instead of moving.
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
    app.on_mouse(mouse(MouseEventKind::Moved, x + 20, y));
    app.on_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), x + 40, y));
    assert_eq!(app.outputs[1].x, expected);

    // Also when the dialog comes and goes by keyboard with the button
    // still held: the drag is finished (guides gone) as the dialog opens.
    app.snap_mode = SnapMode::Edges;
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
    app.on_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), x - 1, y));
    let held = app.outputs[1].x;
    assert!(!app.guides.is_empty());
    press(&mut app, &["f"]);
    assert!(app.guides.is_empty());
    assert!(app.drag_raw.is_empty());
    press(&mut app, &["escape"]);
    app.on_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), x + 20, y));
    assert_eq!(app.outputs[1].x, held);
}

#[test]
fn layout_map_receives_guides_and_draws_disabled_outputs() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    app.outputs[1].enabled = false;
    assert!(render(&mut app, 110, 34).contains("[OFF]"));
    app.action_move_right();
    assert!(!app.guides.is_empty());
    // The top edges line up: a horizontal guide.
    assert!(render(&mut app, 110, 34).contains('┈'));
    app.on_output_selected("HDMI-A-1");
    app.on_drag_finished("HDMI-A-1");
    assert!(app.guides.is_empty());
    assert_eq!(app.outputs[0].x, 32);
}

#[test]
fn profile_browser_reorders_profiles() {
    let sandbox = Sandbox::new();
    let store = sandbox.store();
    let states: Vec<_> = load_outputs().iter().map(Output::state).collect();
    store.save("work", &states).unwrap();
    let mut moved = states.clone();
    moved[0].x = 1920;
    store.save("home", &moved).unwrap();
    let mut app = sandbox.started(Kind::Wlroots);
    press(&mut app, &["o"]);
    let Some(Modal::Profiles(dialog)) = top(&app) else {
        panic!("no browser")
    };
    // The active profile is highlighted.
    assert_eq!(dialog.list.highlighted_id(), Some("work"));
    press(&mut app, &["shift+down"]);
    assert_eq!(store.names().unwrap(), ["home", "work"]);
    press(&mut app, &["shift+up"]);
    assert_eq!(store.names().unwrap(), ["work", "home"]);
    press(&mut app, &["j"]);
    let Some(Modal::Profiles(dialog)) = top(&app) else {
        panic!("no browser")
    };
    assert_eq!(dialog.list.highlighted_id(), Some("home"));
}

#[test]
fn startup_creates_missing_include_and_shows_hint() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.app(Kind::Niri, &FakeWlr::live(load_outputs()));
    app.start();
    let include = sandbox.dir.join("home/.config/niri/wlr-monitor.kdl");
    // niri rejects a missing include, so the file must exist first.
    let text = fs::read_to_string(include).unwrap();
    assert!(text.contains("output \"Microstep MSI MP275 PC3M394200857\" {"));
    assert!(
        messages(&app)
            .iter()
            .any(|message| message.contains("wlr-monitor.kdl"))
    );
    let screen = render(&mut app, 110, 34);
    assert!(screen.contains("Setup"), "{screen}");
}

#[test]
fn startup_never_writes_the_mango_include() {
    // mango ignores a missing source_optional file, and writing one could
    // trigger a full config reload just by opening the TUI.
    let sandbox = Sandbox::new();
    let mut app = sandbox.app(Kind::Mango, &FakeWlr::live(load_outputs()));
    app.start();
    assert!(
        !sandbox
            .dir
            .join("home/.config/mango/wlr-monitor.conf")
            .exists()
    );
}

#[test]
fn profiles_from_other_compositors_show_no_phantom_changes() {
    let sandbox = Sandbox::new();
    let mut states: Vec<_> = load_outputs().iter().map(Output::state).collect();
    states[0].hdr = Some(true);
    states[0].bit_depth = Some(10);
    sandbox.store().save("sway-hdr", &states).unwrap();
    let mut app = sandbox.app(Kind::Wlroots, &FakeWlr::live(load_outputs()));
    app.start();
    // The generic wlroots backend cannot do HDR or bit depth.
    assert!(app.apply_profile("sway-hdr"));
    assert_eq!(app.outputs[0].hdr, None);
    assert_eq!(app.outputs[0].bit_depth, None);
    assert!(app.dirty_names().is_empty());
    assert_eq!(app.active_profile.as_deref(), Some("sway-hdr"));
}

#[test]
fn rotation_is_named_consistently() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    assert!(render(&mut app, 120, 40).contains("Rotation"));
    press(&mut app, &["t"]);
    let Some(Modal::Choice(dialog, _)) = top(&app) else {
        panic!("no picker")
    };
    assert_eq!(dialog.title, "Rotation");
    press(&mut app, &["down", "enter"]);
    assert_eq!(app.status, "HDMI-A-1: rotation 90");
    let screen = render(&mut app, 120, 40);
    assert!(screen.contains("rotation normal → 90"), "{screen}");
}

#[test]
fn advanced_dialog_cannot_disable_the_last_monitor() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.app(Kind::Wlroots, &FakeWlr::live(load_outputs()));
    app.start_with(load_outputs()[..1].to_vec());
    press(&mut app, &["c", "down", "down", "space"]);
    assert!(messages(&app).contains(&"Cannot disable the last active monitor.".to_string()));
    press(&mut app, &["enter"]);
    assert!(app.outputs[0].enabled);
    assert!(app.dirty_names().is_empty());
}

#[test]
fn apply_refuses_to_turn_off_every_monitor() {
    let sandbox = Sandbox::new();
    let mut dark: Vec<_> = load_outputs().iter().map(Output::state).collect();
    for state in &mut dark {
        state.enabled = false;
    }
    sandbox.store().save("dark", &dark).unwrap();
    let wlr = FakeWlr::live(load_outputs());
    let mut app = sandbox.app(Kind::Wlroots, &wlr);
    app.start();
    assert!(!app.apply_profile("dark"));
    assert!(wlr.applied().is_empty());
    assert!(app.status.contains("every monitor"));
    // The refused profile is not left behind as pending edits.
    assert!(app.outputs.iter().all(|output| output.enabled));
    assert!(app.dirty_names().is_empty());
}

#[test]
fn failed_profile_applies_keep_the_editor_as_it_was() {
    let sandbox = Sandbox::new();
    let mut moved: Vec<_> = load_outputs().iter().map(Output::state).collect();
    moved[0].x += 64;
    sandbox.store().save("moved", &moved).unwrap();
    let mut wlr = FakeWlr::live(load_outputs());
    wlr.fail = vec!["HDMI-A-1".to_string()];
    let mut app = sandbox.app(Kind::Wlroots, &wlr);
    app.start();
    // A pending edit of its own survives the failed apply too.
    press(&mut app, &["tab", "down"]);
    let before: Vec<_> = app.outputs.iter().map(Output::state).collect();
    assert!(!app.apply_profile("moved"));
    assert!(app.error.as_deref().unwrap().starts_with("Apply failed"));
    let after: Vec<_> = app.outputs.iter().map(Output::state).collect();
    assert_eq!(before, after);
    assert_eq!(app.dirty_names(), HashSet::from(["eDP-1".to_string()]));
}

#[test]
fn profile_browser_survives_write_errors() {
    let sandbox = Sandbox::new();
    let store = sandbox.store();
    let states: Vec<_> = load_outputs().iter().map(Output::state).collect();
    store.save("work", &states).unwrap();
    store.save("home", &states).unwrap();
    let mut app = sandbox.started(Kind::Wlroots);
    press(&mut app, &["o", "home"]);
    // A read-only directory makes every write fail.
    let dir = sandbox.dir.path().to_path_buf();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
    let probe = dir.join("probe");
    let writable = fs::write(&probe, "").is_ok();
    if writable {
        // Running as root: permissions are not enforced, nothing to test.
        let _ = fs::remove_file(&probe);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    press(&mut app, &["shift+down"]);
    assert!(!app.quit);
    let errors = |app: &App| match top(app) {
        Some(Modal::Profiles(dialog)) => dialog.messages(),
        _ => panic!("no browser"),
    };
    assert!(
        errors(&app)[0].starts_with("Could not write"),
        "{:?}",
        errors(&app)
    );

    // The delete error must survive the list reload.
    press(&mut app, &["home", "d", "y"]);
    assert!(errors(&app)[0].starts_with("Could not write"));
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(store.names().unwrap(), ["work", "home"]);

    // The next successful action clears it.
    press(&mut app, &["home", "shift+down"]);
    assert!(errors(&app).is_empty());
    assert_eq!(store.names().unwrap(), ["home", "work"]);
}

#[test]
fn startup_profile_overwrite_asks_even_if_broken() {
    let sandbox = Sandbox::new();
    let store = sandbox.store();
    store
        .save(
            STARTUP_PROFILE,
            &load_outputs().iter().map(Output::state).collect::<Vec<_>>(),
        )
        .unwrap();
    let mut raw: Value =
        serde_json::from_str(&fs::read_to_string(sandbox.profiles()).unwrap()).unwrap();
    raw["profiles"][STARTUP_PROFILE]["outputs"]["HDMI-A-1"]["transform"] =
        Value::String("bogus".into());
    fs::write(sandbox.profiles(), raw.to_string()).unwrap();
    let mut app = sandbox.started(Kind::Wlroots);
    press(&mut app, &["S"]);
    assert!(matches!(top(&app), Some(Modal::Confirm(..))));
    // Only the dialog's keys work in it: `q` does not quit (Textual's modal
    // screens cut off the app's bindings).
    press(&mut app, &["q", "Q", "ctrl+c"]);
    assert!(!app.quit);
    assert!(matches!(top(&app), Some(Modal::Confirm(..))));
    press(&mut app, &["ctrl+q"]);
    assert!(app.quit);
}

#[test]
fn icc_prompt_expands_home_and_rejects_relative_paths() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Sway);
    press(&mut app, &["c", "i"]);
    assert!(matches!(
        top(&app),
        Some(Modal::Prompt(_, PromptPurpose::Icc))
    ));
    type_text(&mut app, "relative.icc");
    press(&mut app, &["enter"]);
    assert_eq!(advanced(&app).output.color_profile, None);
    assert!(messages(&app).iter().any(|m| m.contains("absolute path")));

    press(&mut app, &["i", "ctrl+u"]);
    type_text(&mut app, "~/display.icc");
    press(&mut app, &["enter"]);
    let home = &sandbox.env["HOME"];
    assert_eq!(
        advanced(&app).output.color_profile.as_deref(),
        Some(format!("icc:{home}/display.icc").as_str())
    );
    assert_eq!(advanced(&app).fields[advanced(&app).focus], Field::Color);
    assert!(messages(&app).iter().any(|m| m.contains("does not exist")));
    // The prompt opens with the current path.
    press(&mut app, &["i"]);
    let Some(Modal::Prompt(prompt, _)) = top(&app) else {
        panic!("no prompt")
    };
    assert_eq!(prompt.input.value, format!("{home}/display.icc"));
    press(&mut app, &["escape", "enter"]);
    assert_eq!(
        app.outputs[0].color_profile.as_deref(),
        Some(format!("icc:{home}/display.icc").as_str())
    );
}

#[test]
fn keys_reach_the_right_place() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    // `q` is text in a prompt; Ctrl+C copies there, and only Ctrl+Q quits.
    press(&mut app, &["p", "q"]);
    let Some(Modal::Prompt(prompt, _)) = top(&app) else {
        panic!("no prompt")
    };
    assert_eq!(prompt.input.value, "q");
    app.handle_event(Event::Paste("uick\nsave".into()));
    let Some(Modal::Prompt(prompt, _)) = top(&app) else {
        panic!("no prompt")
    };
    assert_eq!(prompt.input.value, "quick");
    press(&mut app, &["ctrl+c"]);
    assert!(!app.quit);
    press(&mut app, &["shift+home", "ctrl+c"]);
    assert!(!app.quit);
    assert_eq!(app.clipboard.text, "quick");
    press(&mut app, &["end", "ctrl+v"]);
    let Some(Modal::Prompt(prompt, _)) = top(&app) else {
        panic!("no prompt")
    };
    assert_eq!(prompt.input.value, "quickquick");
    press(&mut app, &["ctrl+q"]);
    assert!(app.quit);

    // `q`, `Q` and Ctrl+C do nothing in the advanced dialog, so its edits
    // (and the pending ones) are never dropped by accident.
    let mut app = sandbox.started(Kind::Wlroots);
    press(&mut app, &["right", "c", "t", "q", "Q", "ctrl+c"]);
    assert!(!app.quit);
    assert!(matches!(top(&app), Some(Modal::Advanced(..))));
    press(&mut app, &["escape", "q"]);
    assert!(app.quit);

    let mut app = sandbox.started(Kind::Wlroots);
    press(&mut app, &["Q"]);
    assert!(app.quit);

    let mut app = sandbox.started(Kind::Wlroots);
    press(&mut app, &["ctrl+c"]);
    assert!(app.quit);

    let mut app = sandbox.started(Kind::Wlroots);
    press(&mut app, &["?", "ctrl+c"]);
    assert!(!app.quit);
    press(&mut app, &["ctrl+q"]);
    assert!(app.quit);
}

#[test]
fn modified_navigation_keys_are_not_bindings() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    let before: Vec<_> = app.outputs.iter().map(Output::state).collect();
    press(
        &mut app,
        &[
            "ctrl+up",
            "alt+left",
            "ctrl+shift+right",
            "alt+enter",
            "ctrl+tab",
            "ctrl+space",
        ],
    );
    let after: Vec<_> = app.outputs.iter().map(Output::state).collect();
    assert_eq!(before, after);
    assert_eq!(app.selected, 0);
    // Shift+arrows still move by ten grid steps.
    let x = app.outputs[0].x;
    press(&mut app, &["shift+right"]);
    assert_eq!(app.outputs[0].x, x + 10 * app.grid);
}

#[test]
fn notifications_expire() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    app.toast("hello", "Info", Severity::Information, 4);
    assert!(render(&mut app, 110, 34).contains("hello"));
    let later = Instant::now() + Duration::from_secs(5);
    assert!(app.notifications.expire(later));
    assert!(!render(&mut app, 110, 34).contains("hello"));
}

#[test]
fn quantize_truncates_like_hyprmon() {
    assert_eq!(quantize(40.0, 32), 32);
    assert_eq!(quantize(-40.0, 32), -32);
    assert_eq!(quantize(2.5, 1), 2);
    assert_eq!(quantize(3.5, 0), 4);
}

#[test]
fn tiny_terminals_never_panic() {
    let sizes = [
        (0, 0),
        (1, 1),
        (10, 4),
        (40, 4),
        (70, 5),
        (20, 10),
        (36, 12),
        (80, 24),
    ];
    let dialogs: [&[&str]; 8] = [
        &[],
        &["c"],
        &["f"],
        &["r", "end", "enter"],
        &["p"],
        &["o"],
        &["?"],
        &["S", "S"],
    ];
    for kind in [Kind::Wlroots, Kind::Sway, Kind::Mango] {
        for keys in dialogs {
            for (width, height) in sizes {
                let sandbox = Sandbox::new();
                let mut app = sandbox.started(kind);
                press(&mut app, keys);
                render(&mut app, width, height);
                for x in 0..width {
                    for y in 0..height {
                        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
                        app.on_mouse(mouse(MouseEventKind::ScrollDown, x, y));
                        app.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left), x, y));
                    }
                }
                render(&mut app, width, height);
            }
        }
    }
    // And with nothing to show at all.
    let sandbox = Sandbox::new();
    let mut app = sandbox.app(Kind::Wlroots, &FakeWlr::failing("no outputs"));
    app.start();
    for (width, height) in sizes {
        render(&mut app, width, height);
    }
}

#[test]
fn toasts_take_the_mouse_events_over_them() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    app.toast(
        "Saved startup profile 'default' (2 outputs).",
        "Info",
        Severity::Information,
        8,
    );
    render(&mut app, 110, 34);
    // A cell of the toast drawn over an output's box.
    let (x, y) = (0..110)
        .flat_map(|x| (0..34).map(move |y| (x, y)))
        .find(|&(x, y)| {
            let (local_x, local_y) = app.map_local(x, y);
            app.notifications.hit(x, y).is_some() && app.regions.map.hit(local_x, local_y).is_some()
        })
        .expect("the toast covers an output");
    let before: Vec<_> = app.outputs.iter().map(Output::state).collect();
    app.on_mouse(mouse(MouseEventKind::ScrollUp, x, y));
    app.on_mouse(mouse(MouseEventKind::Moved, x, y));
    assert_eq!(app.notifications.toasts.len(), 1);
    // A click dismisses the toast and goes no further.
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Right), x, y));
    app.on_mouse(mouse(MouseEventKind::Up(MouseButton::Right), x, y));
    assert!(app.notifications.toasts.is_empty());
    let after: Vec<_> = app.outputs.iter().map(Output::state).collect();
    assert_eq!(before, after);
    // Without the toast the same right-click reaches the map.
    render(&mut app, 110, 34);
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Right), x, y));
    assert_ne!(
        before,
        app.outputs.iter().map(Output::state).collect::<Vec<_>>()
    );
}

#[test]
fn a_drag_keeps_the_mouse_when_it_passes_under_a_toast() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    app.snap_mode = SnapMode::Off;
    render(&mut app, 110, 34);
    let (x, y) = map_cell(&app, "HDMI-A-1");
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
    app.toast("hello", "Info", Severity::Information, 8);
    render(&mut app, 110, 34);
    let (toast_x, toast_y) = (0..110)
        .flat_map(|x| (0..34).map(move |y| (x, y)))
        .find(|&(x, y)| app.notifications.hit(x, y).is_some())
        .unwrap();
    app.on_mouse(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        toast_x,
        toast_y,
    ));
    app.on_mouse(mouse(
        MouseEventKind::Up(MouseButton::Left),
        toast_x,
        toast_y,
    ));
    assert!(app.dirty_names().contains("HDMI-A-1"));
    assert_eq!(app.notifications.toasts.len(), 1);
}

#[test]
fn small_dialogs_crop_the_hint_before_the_fields() {
    for kind in [Kind::Sway, Kind::Niri, Kind::Mango] {
        let sandbox = Sandbox::new();
        let mut app = sandbox.started(kind);
        // Focus the last field.
        press(&mut app, &["c", "up"]);
        let dialog = advanced(&app);
        let label = match dialog.fields[dialog.focus] {
            Field::Color => "▸ Color profile",
            Field::BitDepth => "▸ Color depth",
            Field::Hdr => "▸ HDR",
            other => panic!("{kind:?}: {other:?}"),
        };
        let screen = render(&mut app, 40, 14);
        assert!(screen.contains(label), "{kind:?}\n{screen}");
    }
    // The prompt keeps its input, the confirmation its message.
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    press(&mut app, &["p"]);
    type_text(&mut app, "office");
    assert!(render(&mut app, 40, 7).contains("office"));
    let mut app = sandbox.started(Kind::Wlroots);
    let states: Vec<_> = load_outputs().iter().map(Output::state).collect();
    sandbox.store().save(STARTUP_PROFILE, &states).unwrap();
    app.action_save_startup();
    assert!(matches!(top(&app), Some(Modal::Confirm(..))));
    assert!(render(&mut app, 40, 7).contains("A profile named"));
}

#[test]
fn the_status_line_uses_the_whole_border() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.app(Kind::Wlroots, &FakeWlr::live(load_outputs()));
    app.start_with(load_outputs()[..1].to_vec());
    press(&mut app, &["space"]);
    let screen = render(&mut app, 80, 24);
    assert!(
        screen.contains(
            "┗━━━━━━━━━ grid 32px · snap Edges · Cannot disable the last active monitor. ━┛"
        ),
        "{screen}"
    );
    let mut app = sandbox.started(Kind::Wlroots);
    press(&mut app, &["tab", "right"]);
    let screen = render(&mut app, 50, 26);
    assert!(
        screen.contains("┗━ grid 32px · snap Edges · 1 pending · eDP-… ━┛"),
        "{screen}"
    );
}

#[test]
fn multi_line_errors_keep_their_lines() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.started(Kind::Wlroots);
    let error = "Apply failed — HDMI-A-1: wlr-randr: first line\nsecond line of stderr";
    app.error = Some(error.into());
    app.status = error.into();
    let screen = render(&mut app, 110, 34);
    assert!(
        screen.contains("Error: Apply failed — HDMI-A-1: wlr-randr: first line "),
        "{screen}"
    );
    assert!(
        screen
            .lines()
            .any(|line| line.contains("│ second line of stderr")),
        "{screen}"
    );
    assert!(
        screen.contains("· Apply failed — HDMI-A-1: wlr-randr: first line ━┛"),
        "{screen}"
    );
    assert!(!screen.contains("linesecond"));
}

#[test]
fn the_loading_frame_says_so_at_every_size() {
    let sandbox = Sandbox::new();
    let mut app = sandbox.app(Kind::Wlroots, &FakeWlr::live(load_outputs()));
    for (width, height) in [(110, 34), (80, 24), (40, 14)] {
        let screen = render(&mut app, width, height);
        assert!(screen.contains("Reading outputs…"), "{screen}");
        assert!(!screen.contains("Ready."), "{screen}");
        assert!(!screen.contains("No outputs"), "{screen}");
    }
    app.start();
    assert!(!render(&mut app, 80, 24).contains("Reading outputs…"));
}

#[test]
fn the_browser_keeps_the_active_profile_current() {
    let sandbox = Sandbox::new();
    let states: Vec<_> = load_outputs().iter().map(Output::state).collect();
    sandbox.store().save("work", &states).unwrap();
    let mut app = sandbox.started(Kind::Wlroots);
    assert_eq!(app.active_profile.as_deref(), Some("work"));
    press(&mut app, &["o", "r", "ctrl+u"]);
    type_text(&mut app, "office");
    press(&mut app, &["enter"]);
    assert_eq!(app.active_profile.as_deref(), Some("office"));
    press(&mut app, &["escape"]);
    assert!(render(&mut app, 110, 34).contains("profile office"));
    // `q` does not answer the delete confirmation.
    press(&mut app, &["o", "d", "q"]);
    let Some(Modal::Profiles(dialog)) = top(&app) else {
        panic!("no browser")
    };
    assert_eq!(dialog.confirm_delete.as_deref(), Some("office"));
    press(&mut app, &["y"]);
    assert_eq!(app.active_profile, None);
    press(&mut app, &["escape"]);
    assert!(top(&app).is_none());
    assert!(!render(&mut app, 110, 34).contains("profile office"));
}
