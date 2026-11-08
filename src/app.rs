//! The interactive monitor-layout editor: state, actions and the event loop.

use std::collections::{HashMap, HashSet};
use std::io;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::Rect;
use serde_json::{Map, Value};

use crate::compositor::{Backend, Capabilities, Compositor};
use crate::layout::{Guide, SnapMode, snap_position};
use crate::model::{
    Mode, Output, OutputState, SCALE_MAX, SCALE_MIN, TRANSFORMS, clamp_scale, common_scales,
    disables_everything, fmt_g, match_state, step_scale,
};
use crate::profiles::ProfileStore;
use crate::screens::{
    AdvancedDialog, AdvancedResponse, ChoiceDialog, Clipboard, ConfirmDialog, HelpDialog,
    LAUNCH_ID, Notifications, ProfilesDialog, ProfilesResponse, PromptDialog, Response, SAVE_ID,
    Severity, is_ctrl, plain_char,
};
use crate::text::repr;
use crate::widgets::{HelpItem, MapRender, help};
use crate::wlr::ApplyResult;

pub const GRID_SIZES: [i64; 5] = [1, 8, 16, 32, 64];
pub const DEFAULT_GRID: i64 = 32;
pub const DEFAULT_SNAP: SnapMode = SnapMode::Edges;
pub const DEFAULT_SNAP_THRESHOLD: i64 = 10;
pub const STARTUP_PROFILE: &str = "default";

/// Below these terminal heights the details and outputs sections are
/// hidden, so the layout map stays usable in small floating windows.
pub const COMPACT_HEIGHT: u16 = 26;
pub const TINY_HEIGHT: u16 = 17;

pub const MAIN_HELP: [HelpItem; 14] = [
    help("←↓↑→", "Move", 0),
    help("⇥", "Next", 1),
    help("f", "Mode", 2),
    help("r", "Scale", 2),
    help("t", "Rotation", 4),
    help("v", "VRR", 4),
    help("c", "Advanced", 3),
    help("Space", "On/off", 4),
    help("a", "Apply", 0),
    help("z", "Reset", 6),
    help("p", "Save profile", 5),
    help("o", "Profiles", 3),
    help("?", "Help", 1),
    help("q", "Quit", 1),
];

/// What a finished picker should change.
#[derive(Debug, Clone)]
pub enum ChoicePurpose {
    Mode { output: String, modes: Vec<Mode> },
    Scale { output: String },
    Transform { output: String },
}

#[derive(Debug, Clone)]
pub enum PromptPurpose {
    CustomScale {
        output: String,
    },
    SaveProfile,
    /// Answered back to the profile browser below it.
    RenameProfile {
        old: String,
    },
    /// Answered back to the advanced dialog below it.
    Icc,
}

#[derive(Debug, Clone, Copy)]
pub enum ConfirmPurpose {
    OverwriteStartup,
}

/// The dialogs, stacked on top of the main view.
#[derive(Debug, Clone)]
pub enum Modal {
    Choice(ChoiceDialog, ChoicePurpose),
    Prompt(PromptDialog, PromptPurpose),
    Confirm(ConfirmDialog, ConfirmPurpose),
    Advanced(Box<AdvancedDialog>),
    Profiles(Box<ProfilesDialog>),
    Help(HelpDialog),
}

/// An ongoing mouse drag of an output in the layout map.
#[derive(Debug, Clone)]
struct Drag {
    name: String,
    last: (u16, u16),
    accum: (f64, f64),
}

/// Where the last frame put things, for mouse handling.
#[derive(Debug, Clone, Default)]
pub struct Regions {
    /// The layout map including its border.
    pub map_widget: Rect,
    /// The map's content area (what `map` was rendered into).
    pub map_content: Rect,
    pub map: MapRender,
    /// The outputs table content area (empty when hidden).
    pub outputs_content: Rect,
}

pub struct App {
    pub backend: Backend,
    pub store: ProfileStore,
    pub outputs: Vec<Output>,
    /// The last applied state of every output, as profile-style dicts.
    pub baseline: HashMap<String, Map<String, Value>>,
    pub selected: usize,
    pub grid: i64,
    pub snap_mode: SnapMode,
    pub snap_threshold: i64,
    pub guides: Vec<Guide>,
    pub active_profile: Option<String>,
    pub status: String,
    pub error: Option<String>,
    /// True until the first read of the outputs finished.
    pub loading: bool,
    pub profiles_only: bool,
    pub modals: Vec<Modal>,
    pub notifications: Notifications,
    pub clipboard: Clipboard,
    pub quit: bool,
    pub regions: Regions,
    drag: Option<Drag>,
    drag_raw: HashMap<String, (f64, f64)>,
    rollback: Option<Vec<OutputState>>,
}

impl App {
    pub fn new(backend: Backend, store: ProfileStore, profiles_only: bool) -> Self {
        Self {
            backend,
            store,
            outputs: Vec::new(),
            baseline: HashMap::new(),
            selected: 0,
            grid: DEFAULT_GRID,
            snap_mode: DEFAULT_SNAP,
            snap_threshold: DEFAULT_SNAP_THRESHOLD,
            guides: Vec::new(),
            active_profile: None,
            status: "Ready.".to_string(),
            error: None,
            loading: true,
            profiles_only,
            modals: Vec::new(),
            notifications: Notifications::default(),
            clipboard: Clipboard::default(),
            quit: false,
            regions: Regions::default(),
            drag: None,
            drag_raw: HashMap::new(),
            rollback: None,
        }
    }

    pub fn compositor(&self) -> &Compositor {
        &self.backend.compositor
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.backend.compositor.capabilities
    }

    /// Read the outputs and do the start-up work (the first frame is
    /// already on screen by then).
    pub fn start(&mut self) {
        if self.reload_from_backend() {
            self.ensure_include();
        }
        self.loading = false;
        if self.profiles_only {
            self.action_open_profiles();
        }
        let hint = self.compositor().include_hint();
        if !hint.is_empty() {
            self.toast(&hint, "Setup", Severity::Information, 8);
        }
    }

    /// Start with these outputs instead of reading them (tests).
    pub fn start_with(&mut self, outputs: Vec<Output>) {
        self.set_outputs(outputs, None);
        self.loading = false;
        if self.profiles_only {
            self.action_open_profiles();
        }
    }

    /// Create the include from the live layout if it does not exist yet.
    ///
    /// Only for compositors that refuse a missing include (niri), so the
    /// suggested line can be added safely. mango and sway ignore a missing
    /// file, and writing one there could trigger a config reload just by
    /// opening the TUI.
    fn ensure_include(&mut self) {
        let compositor = self.compositor();
        let Some(path) = compositor.include_path() else {
            return;
        };
        if !compositor.include_must_exist() || !compositor.write_include_enabled || path.exists() {
            return;
        }
        match compositor.write_include(&self.outputs, None) {
            Ok(_) => {}
            // The include exists now, which is all niri needs.
            Err(error) if error.state_file => {}
            Err(error) => self.notify(&error.message, Severity::Warning),
        }
    }

    // ------------------------------------------------------------------
    // State helpers
    // ------------------------------------------------------------------

    pub fn current(&self) -> Option<&Output> {
        self.outputs.get(self.selected)
    }

    fn current_name(&self) -> Option<String> {
        self.current().map(|output| output.name.clone())
    }

    pub fn dirty_names(&self) -> HashSet<String> {
        self.outputs
            .iter()
            .filter(|output| self.baseline.get(&output.name) != Some(&output.state().to_dict()))
            .map(|output| output.name.clone())
            .collect()
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.outputs.iter().position(|output| output.name == name)
    }

    /// The last applied state of every output, as profile-ready objects.
    pub fn baseline_states(&self) -> Vec<OutputState> {
        self.outputs
            .iter()
            .filter_map(|output| {
                let raw = self.baseline.get(&output.name)?;
                OutputState::from_dict(&output.name, &Value::Object(raw.clone())).ok()
            })
            .collect()
    }

    pub fn baseline_state(&self, name: &str) -> Option<OutputState> {
        let raw = self.baseline.get(name)?;
        OutputState::from_dict(name, &Value::Object(raw.clone())).ok()
    }

    fn detect_active_profile(&self) -> Option<String> {
        self.store
            .find_active(&self.baseline_states())
            .ok()
            .flatten()
    }

    pub fn set_outputs(&mut self, outputs: Vec<Output>, keep_name: Option<&str>) {
        self.baseline = outputs
            .iter()
            .map(|output| (output.name.clone(), output.state().to_dict()))
            .collect();
        self.outputs = outputs;
        if let Some(name) = keep_name {
            match self.find(name) {
                Some(index) => self.selected = index,
                None => self.selected = self.selected.min(self.outputs.len().saturating_sub(1)),
            }
        } else if self.selected >= self.outputs.len() {
            self.selected = self.outputs.len().saturating_sub(1);
        }
        self.guides.clear();
        self.active_profile = self.detect_active_profile();
    }

    pub fn status_line(&self) -> String {
        let mut parts = vec![
            format!("grid {}px", self.grid),
            format!("snap {}", self.snap_mode.label()),
        ];
        if let Some(profile) = &self.active_profile {
            parts.push(format!("profile {profile}"));
        }
        let dirty = self.dirty_names().len();
        if dirty > 0 {
            parts.push(format!("{dirty} pending"));
        }
        parts.push(if self.loading {
            "Reading outputs…".to_string()
        } else {
            self.status.clone()
        });
        parts.join(" · ")
    }

    /// Open a dialog. A drag in progress ends first, so the output does not
    /// keep moving once the dialog is closed.
    fn push_modal(&mut self, modal: Modal) {
        self.end_drag();
        self.modals.push(modal);
    }

    fn end_drag(&mut self) {
        if let Some(drag) = self.drag.take() {
            self.on_drag_finished(&drag.name);
        }
    }

    /// A toast notification.
    pub fn toast(&mut self, message: &str, title: &str, severity: Severity, seconds: u64) {
        self.notifications.notify(message, title, severity, seconds);
    }

    /// Show `message` in the status line and as a toast.
    pub fn notify(&mut self, message: &str, severity: Severity) {
        self.status = message.to_string();
        self.toast(message, severity.title(), severity, 4);
    }

    pub fn reload_from_backend(&mut self) -> bool {
        let current = self.current_name();
        match self.backend.read_outputs() {
            Ok(outputs) => {
                self.error = None;
                self.set_outputs(outputs, current.as_deref());
                true
            }
            Err(error) => {
                self.error = Some(error.0.clone());
                self.notify(&error.0, Severity::Error);
                false
            }
        }
    }

    fn apply_states(
        &mut self,
        states: Vec<OutputState>,
        success_message: &str,
        remember_previous: bool,
        profile: Option<&str>,
    ) -> bool {
        if states.is_empty() {
            self.notify("Nothing to apply.", Severity::Warning);
            return false;
        }
        let live_enabled: Vec<(String, bool)> = self
            .outputs
            .iter()
            .map(|output| {
                let enabled = self
                    .baseline
                    .get(&output.name)
                    .and_then(|raw| raw.get("enabled"))
                    .and_then(Value::as_bool)
                    .unwrap_or(output.enabled);
                (output.name.clone(), enabled)
            })
            .collect();
        if disables_everything(&live_enabled, &states) {
            // The same rule as the on/off toggle, enforced for every path
            // (advanced dialog, profiles, revert).
            self.notify(
                "Refusing to apply: it would turn off every monitor.",
                Severity::Error,
            );
            return false;
        }
        let previous = remember_previous.then(|| self.baseline_states());
        let results = match self.backend.apply_states(&states, profile) {
            Ok(results) => results,
            Err(error) => {
                let message = error.to_string();
                self.error = Some(message.clone());
                self.notify(&message, Severity::Error);
                return false;
            }
        };
        let failures: Vec<&ApplyResult> = results.iter().filter(|result| !result.ok).collect();
        if !failures.is_empty() {
            let message = format_failures(&failures);
            self.error = Some(message.clone());
            self.notify(&message, Severity::Error);
            return false;
        }
        let warnings: Vec<String> = results
            .iter()
            .filter(|result| !result.warning.is_empty())
            .map(|result| result.warning.clone())
            .collect();

        self.error = None;
        if let Some(previous) = previous {
            self.rollback = Some(previous);
        }
        let applied: HashSet<&str> = states.iter().map(|state| state.name.as_str()).collect();
        for output in &self.outputs {
            if applied.contains(output.name.as_str()) {
                self.baseline
                    .insert(output.name.clone(), output.state().to_dict());
            }
        }
        self.status = success_message.to_string();
        let current = self.current_name();
        match self.backend.read_outputs() {
            Ok(outputs) => self.set_outputs(outputs, current.as_deref()),
            // Keep the local state if the refresh fails.
            Err(_) => self.active_profile = self.detect_active_profile(),
        }
        self.notify(success_message, Severity::Information);
        self.show_warnings(&warnings);
        true
    }

    fn show_warnings(&mut self, warnings: &[String]) {
        let mut seen = HashSet::new();
        for warning in warnings {
            if seen.insert(warning.clone()) {
                self.toast(warning, "Warning", Severity::Warning, 8);
            }
        }
    }

    // ------------------------------------------------------------------
    // Keyboard actions
    // ------------------------------------------------------------------

    pub fn action_quit(&mut self) {
        self.quit = true;
    }

    pub fn action_help(&mut self) {
        let extra = self.help_extra();
        self.push_modal(Modal::Help(HelpDialog::new(extra)));
    }

    fn help_extra(&self) -> String {
        let compositor = self.compositor();
        let mut lines = vec![format!("Compositor: {}", compositor.label())];
        match compositor.include_path() {
            Some(path) if compositor.write_include_enabled => {
                lines.push(format!(
                    "Applied layouts are written to {}.",
                    compositor.display_path(&path)
                ));
                if compositor.include_is_sourced() {
                    lines.push("Your compositor config includes it.".into());
                } else {
                    lines.push(format!(
                        "Add this line to your config: {}",
                        compositor.include_line()
                    ));
                }
            }
            Some(_) => {}
            None => lines.push(
                "Changes are applied with wlr-randr only; HDR and color settings need mango, \
                 sway or niri."
                    .into(),
            ),
        }
        lines.join("\n")
    }

    pub fn action_next_output(&mut self) {
        if self.outputs.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.outputs.len();
        self.guides.clear();
    }

    pub fn action_previous_output(&mut self) {
        if self.outputs.is_empty() {
            return;
        }
        self.selected = (self.selected + self.outputs.len() - 1) % self.outputs.len();
        self.guides.clear();
    }

    fn move_by(&mut self, dx: i64, dy: i64, multiplier: i64) {
        let Some(output) = self.current() else { return };
        let new_x = output.x + dx * self.grid * multiplier;
        let new_y = output.y + dy * self.grid * multiplier;
        let (x, y, guides) = snap_position(
            &self.outputs,
            self.selected,
            new_x,
            new_y,
            self.snap_mode,
            self.snap_threshold,
        );
        self.guides = guides;
        let output = &mut self.outputs[self.selected];
        output.x = x;
        output.y = y;
        self.status = format!("{}: {},{}", output.name, output.x, output.y);
    }

    pub fn action_move_up(&mut self) {
        self.move_by(0, -1, 1);
    }

    pub fn action_move_down(&mut self) {
        self.move_by(0, 1, 1);
    }

    pub fn action_move_left(&mut self) {
        self.move_by(-1, 0, 1);
    }

    pub fn action_move_right(&mut self) {
        self.move_by(1, 0, 1);
    }

    pub fn action_cycle_grid(&mut self) {
        let index = GRID_SIZES
            .iter()
            .position(|size| *size == self.grid)
            .unwrap_or(0);
        self.grid = GRID_SIZES[(index + 1) % GRID_SIZES.len()];
        self.status = format!("Grid: {}px", self.grid);
    }

    pub fn action_cycle_snap(&mut self) {
        self.snap_mode = self.snap_mode.cycled();
        self.guides.clear();
        self.status = if self.snap_mode == SnapMode::Off {
            "Snap: Off".to_string()
        } else {
            format!(
                "Snap: {} ({}px)",
                self.snap_mode.label(),
                self.snap_threshold
            )
        };
    }

    pub fn action_pick_mode(&mut self) {
        let Some(output) = self.current() else { return };
        let modes = output.mode_options();
        if modes.is_empty() {
            self.notify("This output reports no modes.", Severity::Warning);
            return;
        }
        let current_key = output.mode.as_ref().map(Mode::key);
        let mut selected = None;
        let choices = modes
            .iter()
            .enumerate()
            .map(|(index, mode)| {
                let is_current = Some(mode.key()) == current_key;
                if is_current {
                    selected = Some(index);
                }
                let marker = if is_current { "●" } else { " " };
                let suffix = if mode.preferred { "  preferred" } else { "" };
                (
                    index.to_string(),
                    format!("{marker} {}{suffix}", mode.label()),
                )
            })
            .collect();
        let name = output.name.clone();
        self.push_modal(Modal::Choice(
            ChoiceDialog::new("Resolution and refresh rate", choices, selected),
            ChoicePurpose::Mode {
                output: name,
                modes,
            },
        ));
    }

    pub fn action_pick_scale(&mut self) {
        let Some(output) = self.current() else { return };
        let scales = common_scales(Some(output.scale));
        let mut choices: Vec<(String, String)> = scales
            .iter()
            .map(|scale| (fmt_g(*scale), format!("{}x", fmt_g(*scale))))
            .collect();
        choices.push(("custom".into(), "Custom value…".into()));
        let selected = scales
            .iter()
            .position(|scale| (scale - output.scale).abs() < 0.0001)
            .unwrap_or(scales.len());
        let name = output.name.clone();
        self.push_modal(Modal::Choice(
            ChoiceDialog::new("Scale", choices, Some(selected)),
            ChoicePurpose::Scale { output: name },
        ));
    }

    fn prompt_scale(&mut self, name: &str) {
        let Some(index) = self.find(name) else { return };
        let value = fmt_g(self.outputs[index].scale);
        self.push_modal(Modal::Prompt(
            PromptDialog::new(&format!("Scale for {name}"), "1.25", &value),
            PromptPurpose::CustomScale {
                output: name.to_string(),
            },
        ));
    }

    fn finish_custom_scale(&mut self, name: &str, value: Option<String>) {
        let Some(value) = value else { return };
        let Ok(scale) = value.replace(',', ".").trim().parse::<f64>() else {
            self.notify(&format!("Invalid scale: {}", repr(&value)), Severity::Error);
            return;
        };
        if !(SCALE_MIN..=SCALE_MAX).contains(&scale) {
            self.notify(
                &format!(
                    "Scale must be between {} and {}.",
                    fmt_g(SCALE_MIN),
                    fmt_g(SCALE_MAX)
                ),
                Severity::Error,
            );
            return;
        }
        self.set_scale(name, scale);
    }

    fn set_scale(&mut self, name: &str, scale: f64) {
        let Some(index) = self.find(name) else { return };
        let output = &mut self.outputs[index];
        output.scale = clamp_scale(scale);
        self.status = format!("{}: scale {}", output.name, fmt_g(output.scale));
        self.guides.clear();
    }

    fn step_scale_of(&mut self, index: usize, steps: i32) {
        let Some(output) = self.outputs.get_mut(index) else {
            return;
        };
        output.scale = step_scale(output.scale, steps);
        self.status = format!("{}: scale {}", output.name, fmt_g(output.scale));
        self.guides.clear();
    }

    pub fn action_scale_down(&mut self) {
        self.step_scale_of(self.selected, -1);
    }

    pub fn action_scale_up(&mut self) {
        self.step_scale_of(self.selected, 1);
    }

    pub fn action_pick_transform(&mut self) {
        let Some(output) = self.current() else { return };
        let choices = TRANSFORMS
            .iter()
            .map(|transform| (transform.to_string(), transform.to_string()))
            .collect();
        let selected = TRANSFORMS
            .iter()
            .position(|transform| *transform == output.transform)
            .unwrap_or(0);
        let name = output.name.clone();
        self.push_modal(Modal::Choice(
            ChoiceDialog::new("Rotation", choices, Some(selected)),
            ChoicePurpose::Transform { output: name },
        ));
    }

    pub fn action_toggle_adaptive(&mut self) {
        let Some(output) = self.outputs.get_mut(self.selected) else {
            return;
        };
        output.adaptive_sync = !output.adaptive_sync;
        self.status = format!(
            "{}: VRR {}",
            output.name,
            if output.adaptive_sync { "on" } else { "off" }
        );
        self.guides.clear();
    }

    fn toggle_output(&mut self, name: &str) {
        let Some(index) = self.find(name) else { return };
        let enabled_count = self.outputs.iter().filter(|output| output.enabled).count();
        if self.outputs[index].enabled && enabled_count <= 1 {
            self.notify("Cannot disable the last active monitor.", Severity::Warning);
            return;
        }
        let output = &mut self.outputs[index];
        output.enabled = !output.enabled;
        self.status = format!(
            "{}: {}",
            output.name,
            if output.enabled {
                "enabled"
            } else {
                "disabled"
            }
        );
        self.guides.clear();
    }

    pub fn action_toggle_enabled(&mut self) {
        if let Some(name) = self.current_name() {
            self.toggle_output(&name);
        }
    }

    pub fn action_advanced(&mut self) {
        let Some(output) = self.current() else { return };
        let can_disable = self
            .outputs
            .iter()
            .enumerate()
            .any(|(index, item)| index != self.selected && item.enabled);
        let dialog = AdvancedDialog::new(
            output.clone(),
            self.capabilities().clone(),
            self.compositor().label(),
            can_disable,
        );
        self.push_modal(Modal::Advanced(Box::new(dialog)));
    }

    fn finish_advanced(&mut self, dialog: AdvancedDialog, applied: bool) {
        let name = dialog.output.name.clone();
        if applied && let Some(index) = self.find(&name) {
            self.outputs[index] = dialog.output;
        }
        self.guides.clear();
        self.status = if applied {
            format!("{name}: advanced settings updated")
        } else {
            format!("{name}: advanced settings cancelled")
        };
    }

    pub fn action_apply(&mut self) {
        let dirty = self.dirty_names();
        let states: Vec<OutputState> = self
            .outputs
            .iter()
            .filter(|output| dirty.contains(&output.name))
            .map(Output::state)
            .collect();
        let message = format!("Applied {} output(s).", states.len());
        self.apply_states(states, &message, true, None);
    }

    pub fn action_reload(&mut self) {
        if self.reload_from_backend() {
            self.status = "Reloaded outputs.".into();
            self.notify("Reloaded outputs from wlr-randr.", Severity::Information);
        }
    }

    pub fn action_reset(&mut self) {
        self.error = None;
        for index in 0..self.outputs.len() {
            let name = self.outputs[index].name.clone();
            if let Some(state) = self.baseline_state(&name) {
                state.apply_to(&mut self.outputs[index], true);
            }
        }
        self.guides.clear();
        self.status = "Reset pending changes.".into();
    }

    pub fn action_revert_live(&mut self) {
        let Some(rollback) = self.rollback.clone().filter(|states| !states.is_empty()) else {
            self.notify("No previous layout to revert to.", Severity::Warning);
            return;
        };
        let previous = self.baseline_states();
        self.guides.clear();
        if self.apply_states(rollback, "Reverted to the previous layout.", false, None) {
            self.rollback = Some(previous);
        }
    }

    pub fn action_save_profile(&mut self) {
        if self.outputs.is_empty() {
            self.notify("There are no outputs to save.", Severity::Warning);
            return;
        }
        self.push_modal(Modal::Prompt(
            PromptDialog::new("Save layout as profile", "home, work, docked…", ""),
            PromptPurpose::SaveProfile,
        ));
    }

    fn finish_save_profile(&mut self, name: Option<String>) {
        let Some(name) = name else { return };
        let states: Vec<OutputState> = self.outputs.iter().map(Output::state).collect();
        match self.store.save(&name, &states) {
            Ok(profile) => {
                self.active_profile = self.detect_active_profile();
                self.notify(
                    &format!(
                        "Saved profile {} ({} outputs).",
                        repr(&profile.name),
                        profile.outputs.len()
                    ),
                    Severity::Information,
                );
            }
            Err(error) => self.notify(&error.0, Severity::Error),
        }
    }

    fn save_startup_profile(&mut self) {
        if self.outputs.is_empty() {
            self.notify("There are no outputs to save.", Severity::Warning);
            return;
        }
        let states: Vec<OutputState> = self.outputs.iter().map(Output::state).collect();
        match self.store.save(STARTUP_PROFILE, &states) {
            Ok(profile) => {
                self.active_profile = self.detect_active_profile();
                self.notify(
                    &format!(
                        "Saved startup profile {} ({} outputs).",
                        repr(&profile.name),
                        profile.outputs.len()
                    ),
                    Severity::Information,
                );
            }
            Err(error) => self.notify(&error.0, Severity::Error),
        }
    }

    pub fn action_save_startup(&mut self) {
        if self.outputs.is_empty() {
            self.notify("There are no outputs to save.", Severity::Warning);
            return;
        }
        match self.store.exists(STARTUP_PROFILE) {
            Err(error) => self.notify(&error.0, Severity::Error),
            Ok(false) => self.save_startup_profile(),
            Ok(true) => self.push_modal(Modal::Confirm(
                ConfirmDialog::new(
                    "Overwrite startup profile?",
                    &format!(
                        "A profile named {} already exists. Overwrite it with the current layout?",
                        repr(STARTUP_PROFILE)
                    ),
                ),
                ConfirmPurpose::OverwriteStartup,
            )),
        }
    }

    pub fn action_open_profiles(&mut self) {
        let standalone = self.profiles_only;
        let dialog = ProfilesDialog::new(
            &self.store,
            self.baseline_states(),
            self.active_profile.as_deref(),
            standalone,
            !standalone,
        );
        self.push_modal(Modal::Profiles(Box::new(dialog)));
    }

    fn finish_profiles(&mut self, choice: Option<String>) {
        let standalone = self.profiles_only;
        // The browser may have renamed, moved or deleted the active profile.
        self.active_profile = self.detect_active_profile();
        let Some(choice) = choice else {
            if standalone {
                self.quit = true;
            }
            return;
        };
        if choice == LAUNCH_ID {
            self.profiles_only = false;
            self.status = "Monitor editor ready.".into();
            return;
        }
        if choice == SAVE_ID {
            self.action_save_profile();
            return;
        }
        let applied = self.apply_profile(&choice);
        if standalone {
            if applied {
                self.quit = true;
            } else {
                self.profiles_only = false;
            }
        }
    }

    pub fn apply_profile(&mut self, name: &str) -> bool {
        let profile = match self.store.get(name) {
            Ok(Some(profile)) => profile,
            Ok(None) => {
                self.notify(
                    &format!("No profile named {}.", repr(name)),
                    Severity::Error,
                );
                return false;
            }
            Err(error) => {
                self.notify(&error.0, Severity::Error);
                return false;
            }
        };
        let connected: Vec<String> = self.outputs.iter().map(Output::hardware_id).collect();
        // The profile goes into the editor only if it is applied: a refused
        // or failed apply leaves the outputs (and their pending edits) as
        // they were.
        let before = self.outputs.clone();
        let mut states = Vec::new();
        for index in 0..self.outputs.len() {
            let output = &self.outputs[index];
            let Some(state) = match_state(
                &profile.outputs,
                &output.name,
                &output.hardware_id(),
                &connected,
            ) else {
                continue;
            };
            let state = state.clone();
            state.apply_to(&mut self.outputs[index], false);
            // A profile saved on another compositor may carry settings this
            // one cannot apply; drop them so they don't show as pending.
            self.backend
                .compositor
                .drop_unsupported([&mut self.outputs[index]]);
            states.push(self.outputs[index].state());
        }
        if states.is_empty() {
            self.notify(
                &format!(
                    "Profile {} does not match any connected output.",
                    repr(name)
                ),
                Severity::Warning,
            );
            return false;
        }
        let message = format!("Applied profile {}.", repr(name));
        let applied = self.apply_states(states, &message, true, Some(name));
        if !applied {
            self.outputs = before;
        }
        applied
    }

    // ------------------------------------------------------------------
    // Mouse actions
    // ------------------------------------------------------------------

    pub fn on_output_selected(&mut self, name: &str) {
        let Some(index) = self.find(name) else { return };
        let output = &self.outputs[index];
        self.drag_raw
            .insert(name.to_string(), (output.x as f64, output.y as f64));
        self.guides.clear();
        self.selected = index;
    }

    pub fn on_output_moved(&mut self, name: &str, dx: i64, dy: i64) {
        let Some(index) = self.find(name) else { return };
        let output = &self.outputs[index];
        let (base_x, base_y) = self
            .drag_raw
            .get(name)
            .copied()
            .unwrap_or((output.x as f64, output.y as f64));
        let raw = (base_x + dx as f64, base_y + dy as f64);
        self.drag_raw.insert(name.to_string(), raw);
        let new_x = quantize(raw.0, self.grid);
        let new_y = quantize(raw.1, self.grid);
        let (x, y, guides) = snap_position(
            &self.outputs,
            index,
            new_x,
            new_y,
            self.snap_mode,
            self.snap_threshold,
        );
        self.guides = guides;
        let output = &mut self.outputs[index];
        output.x = x;
        output.y = y;
        self.status = format!("{}: {},{}", output.name, output.x, output.y);
    }

    pub fn on_drag_finished(&mut self, name: &str) {
        let Some(index) = self.find(name) else { return };
        self.drag_raw.remove(name);
        self.guides.clear();
        let output = &self.outputs[index];
        self.status = format!("{}: {},{}", output.name, output.x, output.y);
    }

    pub fn on_output_toggled(&mut self, name: &str) {
        self.toggle_output(name);
    }

    pub fn on_output_scale_step(&mut self, name: &str, steps: i32) {
        if let Some(index) = self.find(name) {
            self.step_scale_of(index, steps);
        }
    }

    // ------------------------------------------------------------------
    // Events
    // ------------------------------------------------------------------

    pub fn handle_event(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key),
            Event::Mouse(mouse) => self.on_mouse(mouse),
            Event::Paste(text) => {
                if let Some(Modal::Prompt(dialog, _)) = self.modals.last_mut() {
                    dialog.paste(&text);
                }
            }
            _ => {}
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        // Ctrl+Q is a priority binding: it quits from anywhere. Everything
        // else in a dialog stays in the dialog (Textual's modal screens cut
        // off the app's bindings), so `q` there never drops pending edits.
        if is_ctrl(&key, 'q') {
            self.action_quit();
            return;
        }
        if self.modals.is_empty() {
            self.on_main_key(&key);
        } else {
            self.on_modal_key(&key);
        }
    }

    fn on_main_key(&mut self, key: &KeyEvent) {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        // Ctrl/Alt combinations of the navigation keys are not bindings.
        let modified = key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        if is_ctrl(key, 'c') {
            self.action_quit();
            return;
        }
        if is_ctrl(key, 'r') {
            self.action_reload();
            return;
        }
        match key.code {
            _ if modified && !matches!(key.code, KeyCode::Char(_)) => {}
            KeyCode::Up if shift => self.move_by(0, -1, 10),
            KeyCode::Down if shift => self.move_by(0, 1, 10),
            KeyCode::Left if shift => self.move_by(-1, 0, 10),
            KeyCode::Right if shift => self.move_by(1, 0, 10),
            KeyCode::Up => self.action_move_up(),
            KeyCode::Down => self.action_move_down(),
            KeyCode::Left => self.action_move_left(),
            KeyCode::Right => self.action_move_right(),
            KeyCode::Tab => self.action_next_output(),
            KeyCode::BackTab => self.action_previous_output(),
            KeyCode::Enter if !shift => self.action_toggle_enabled(),
            _ => {
                let Some(character) = plain_char(key) else {
                    return;
                };
                match character {
                    'q' | 'Q' => self.action_quit(),
                    '?' => self.action_help(),
                    'k' => self.action_move_up(),
                    'j' => self.action_move_down(),
                    'h' => self.action_move_left(),
                    'l' => self.action_move_right(),
                    'K' => self.move_by(0, -1, 10),
                    'J' => self.move_by(0, 1, 10),
                    'H' => self.move_by(-1, 0, 10),
                    'g' | 'G' => self.action_cycle_grid(),
                    'L' => self.action_cycle_snap(),
                    'r' | 'R' => self.action_pick_scale(),
                    'f' | 'F' => self.action_pick_mode(),
                    't' => self.action_pick_transform(),
                    'v' => self.action_toggle_adaptive(),
                    '[' => self.action_scale_down(),
                    ']' => self.action_scale_up(),
                    'c' | 'C' | 'd' | 'D' => self.action_advanced(),
                    ' ' => self.action_toggle_enabled(),
                    'a' | 'A' => self.action_apply(),
                    'p' | 'P' => self.action_save_profile(),
                    'S' => self.action_save_startup(),
                    'o' | 'O' => self.action_open_profiles(),
                    'z' => self.action_reset(),
                    'Z' => self.action_revert_live(),
                    _ => {}
                }
            }
        }
    }

    fn on_modal_key(&mut self, key: &KeyEvent) {
        let Some(modal) = self.modals.last_mut() else {
            return;
        };
        match modal {
            Modal::Choice(dialog, _) => {
                if let Response::Close(choice) = dialog.handle_key(key) {
                    self.close_choice(choice);
                }
            }
            Modal::Prompt(dialog, _) => {
                if let Response::Close(value) = dialog.handle_key(key, &mut self.clipboard) {
                    self.close_prompt(value);
                }
            }
            Modal::Confirm(dialog, _) => {
                if let Response::Close(answer) = dialog.handle_key(key) {
                    self.close_confirm(answer);
                }
            }
            Modal::Advanced(dialog) => {
                let response = dialog.handle_key(key, &mut self.notifications);
                match response {
                    AdvancedResponse::Apply | AdvancedResponse::Cancel => {
                        let applied = response == AdvancedResponse::Apply;
                        if let Some(Modal::Advanced(dialog)) = self.modals.pop() {
                            self.finish_advanced(*dialog, applied);
                        }
                    }
                    AdvancedResponse::PickIcc(value) => self.push_modal(Modal::Prompt(
                        PromptDialog::new(
                            "ICC profile",
                            "/usr/share/color/icc/display.icc",
                            &value,
                        ),
                        PromptPurpose::Icc,
                    )),
                    _ => {}
                }
            }
            Modal::Profiles(dialog) => {
                let response = dialog.handle_key(key, &self.store, &mut self.notifications);
                self.sync_profile_changes();
                match response {
                    ProfilesResponse::Close(choice) => {
                        self.modals.pop();
                        self.finish_profiles(choice);
                    }
                    ProfilesResponse::Rename(name) => self.push_modal(Modal::Prompt(
                        PromptDialog::new("Rename profile", "new name", &name),
                        PromptPurpose::RenameProfile { old: name },
                    )),
                    _ => {}
                }
            }
            Modal::Help(dialog) => {
                if let Response::Close(()) = dialog.handle_key(key) {
                    self.modals.pop();
                }
            }
        }
    }

    /// After the profile browser changed the store, re-check which profile
    /// matches the live layout (the status and details name it).
    fn sync_profile_changes(&mut self) {
        let changed = self.modals.iter_mut().any(|modal| match modal {
            Modal::Profiles(dialog) => std::mem::take(&mut dialog.changed),
            _ => false,
        });
        if changed {
            self.active_profile = self.detect_active_profile();
        }
    }

    fn close_choice(&mut self, choice: Option<String>) {
        let Some(Modal::Choice(_, purpose)) = self.modals.pop() else {
            return;
        };
        let Some(choice) = choice else { return };
        match purpose {
            ChoicePurpose::Mode { output, modes } => {
                let (Some(index), Ok(position)) = (self.find(&output), choice.parse::<usize>())
                else {
                    return;
                };
                let Some(mode) = modes.get(position) else {
                    return;
                };
                let target = &mut self.outputs[index];
                target.mode = Some(mode.clone());
                self.status = format!("{}: {}", target.name, mode.label());
                self.guides.clear();
            }
            ChoicePurpose::Scale { output } => {
                if choice == "custom" {
                    self.prompt_scale(&output);
                } else if let Ok(scale) = choice.parse::<f64>() {
                    self.set_scale(&output, scale);
                }
            }
            ChoicePurpose::Transform { output } => {
                let Some(index) = self.find(&output) else {
                    return;
                };
                let target = &mut self.outputs[index];
                target.transform = choice.clone();
                self.status = format!("{}: rotation {choice}", target.name);
                self.guides.clear();
            }
        }
    }

    fn close_prompt(&mut self, value: Option<String>) {
        let Some(Modal::Prompt(_, purpose)) = self.modals.pop() else {
            return;
        };
        match purpose {
            PromptPurpose::CustomScale { output } => self.finish_custom_scale(&output, value),
            PromptPurpose::SaveProfile => self.finish_save_profile(value),
            PromptPurpose::RenameProfile { old } => {
                if let Some(Modal::Profiles(dialog)) = self.modals.last_mut() {
                    dialog.finish_rename(&old, value, &self.store);
                }
                self.sync_profile_changes();
            }
            PromptPurpose::Icc => {
                let compositor = self.backend.compositor.clone();
                if let Some(Modal::Advanced(dialog)) = self.modals.last_mut() {
                    dialog.finish_icc(
                        value,
                        &compositor.home(),
                        &|candidate| compositor.color_profile_supported(candidate),
                        &mut self.notifications,
                    );
                }
            }
        }
    }

    fn close_confirm(&mut self, answer: bool) {
        let Some(Modal::Confirm(_, purpose)) = self.modals.pop() else {
            return;
        };
        match purpose {
            ConfirmPurpose::OverwriteStartup => {
                if answer {
                    self.save_startup_profile();
                }
            }
        }
    }

    pub fn on_mouse(&mut self, event: MouseEvent) {
        // Toasts are on top of everything: a click dismisses one, and no
        // event over a toast reaches what is drawn under it. A drag has the
        // mouse captured, so its moves and release still reach the map.
        if self.drag.is_none()
            && let Some(index) = self.notifications.hit(event.column, event.row)
        {
            if matches!(event.kind, MouseEventKind::Down(_)) {
                self.notifications.dismiss(index);
            }
            return;
        }
        if let Some(modal) = self.modals.last_mut() {
            match modal {
                Modal::Choice(dialog, _) => {
                    if let Response::Close(choice) = dialog.handle_mouse(&event) {
                        self.close_choice(choice);
                    }
                }
                Modal::Prompt(dialog, _) => dialog.handle_mouse(&event),
                Modal::Profiles(dialog) => {
                    if let ProfilesResponse::Close(choice) = dialog.handle_mouse(&event) {
                        self.modals.pop();
                        self.finish_profiles(choice);
                    }
                }
                Modal::Help(dialog) => dialog.handle_mouse(&event),
                _ => {}
            }
            return;
        }
        self.on_main_mouse(&event);
    }

    /// Mouse position relative to the map content, if it is inside it.
    fn map_local(&self, column: u16, row: u16) -> (i64, i64) {
        let area = self.regions.map_content;
        (
            i64::from(column) - i64::from(area.x),
            i64::from(row) - i64::from(area.y),
        )
    }

    fn on_main_mouse(&mut self, event: &MouseEvent) {
        let position = (event.column, event.row);
        let in_map = self.regions.map_widget.contains(position.into());
        let (x, y) = self.map_local(event.column, event.row);
        match event.kind {
            MouseEventKind::Down(button) => {
                if in_map {
                    let Some(name) = self.regions.map.hit(x, y) else {
                        return;
                    };
                    match button {
                        MouseButton::Right => self.on_output_toggled(&name),
                        MouseButton::Left => {
                            self.on_output_selected(&name);
                            self.drag = Some(Drag {
                                name,
                                last: position,
                                accum: (0.0, 0.0),
                            });
                        }
                        MouseButton::Middle => {}
                    }
                    return;
                }
                // Any button selects a table row (Textual's `Click`).
                let table = self.regions.outputs_content;
                if table.contains(position.into()) {
                    let row = usize::from(event.row - table.y);
                    if let Some(index) = row
                        .checked_sub(2)
                        .filter(|index| *index < self.outputs.len())
                    {
                        let name = self.outputs[index].name.clone();
                        self.on_output_selected(&name);
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(mut drag) = self.drag.take() else {
                    return;
                };
                let dx = i64::from(event.column) - i64::from(drag.last.0);
                let dy = i64::from(event.row) - i64::from(drag.last.1);
                drag.last = position;
                let step = match &self.regions.map.geometry {
                    Some(geometry) if geometry.scale > 0.0 => {
                        let (px, py) = geometry.cell_delta_to_pixels(dx, dy);
                        drag.accum.0 += px;
                        drag.accum.1 += py;
                        let step_x = drag.accum.0.round_ties_even();
                        let step_y = drag.accum.1.round_ties_even();
                        drag.accum.0 -= step_x;
                        drag.accum.1 -= step_y;
                        Some((step_x as i64, step_y as i64))
                    }
                    _ => None,
                };
                let name = drag.name.clone();
                self.drag = Some(drag);
                if let Some((step_x, step_y)) = step
                    && (step_x, step_y) != (0, 0)
                {
                    self.on_output_moved(&name, step_x, step_y);
                }
            }
            // Motion without a button: the release was lost (e.g. outside
            // the window), so the drag is over.
            MouseEventKind::Up(_) | MouseEventKind::Moved => {
                if let Some(drag) = self.drag.take() {
                    self.on_drag_finished(&drag.name);
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if in_map => {
                let steps = if event.kind == MouseEventKind::ScrollUp {
                    1
                } else {
                    -1
                };
                let name = self.regions.map.hit(x, y).or_else(|| self.current_name());
                if let Some(name) = name {
                    self.on_output_scale_step(&name, steps);
                }
            }
            _ => {}
        }
    }
}

/// Quantize a position to a grid, truncating like hyprmon does.
pub fn quantize(value: f64, grid: i64) -> i64 {
    if grid <= 1 {
        return value.round_ties_even() as i64;
    }
    (value / grid as f64).trunc() as i64 * grid
}

fn format_failures(failures: &[&ApplyResult]) -> String {
    let mut parts: Vec<String> = failures
        .iter()
        .take(3)
        .map(|failure| {
            let detail = failure.detail();
            let detail = if detail.is_empty() {
                "unknown error".to_string()
            } else {
                detail
            };
            format!("{}: {detail}", failure.name)
        })
        .collect();
    if failures.len() > 3 {
        parts.push(format!("and {} more", failures.len() - 3));
    }
    format!("Apply failed — {}", parts.join("; "))
}

// ----------------------------------------------------------------------
// Terminal
// ----------------------------------------------------------------------

type Terminal = ratatui::Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>;

fn restore_terminal() {
    use ratatui::crossterm::event::{DisableBracketedPaste, DisableMouseCapture};
    use ratatui::crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};
    use ratatui::crossterm::{cursor::Show, execute};
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        Show
    );
}

fn setup_terminal() -> io::Result<Terminal> {
    use ratatui::crossterm::event::{EnableBracketedPaste, EnableMouseCapture};
    use ratatui::crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};
    use ratatui::crossterm::{cursor::Hide, execute};

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous(info);
    }));
    let result = (|| {
        enable_raw_mode()?;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste,
            Hide
        )?;
        ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(io::stdout()))
    })();
    if result.is_err() {
        restore_terminal();
    }
    result
}

/// Run the TUI until the user quits.
pub fn run(backend: Backend, store: ProfileStore, profiles_only: bool) -> io::Result<()> {
    let mut terminal = setup_terminal()?;
    let mut app = App::new(backend, store, profiles_only);
    let result = event_loop(&mut terminal, &mut app);
    restore_terminal();
    result
}

fn event_loop(terminal: &mut Terminal, app: &mut App) -> io::Result<()> {
    // Draw once before the (comparatively slow) wlr-randr read.
    terminal.draw(|frame| crate::ui::draw(frame, app))?;
    app.start();
    loop {
        terminal.draw(|frame| crate::ui::draw(frame, app))?;
        if app.quit {
            return Ok(());
        }
        let timeout = app
            .notifications
            .next_expiry()
            .map_or(Duration::from_secs(3600), |expiry| {
                expiry.saturating_duration_since(Instant::now())
            });
        if event::poll(timeout)? {
            app.handle_event(event::read()?);
            // Handle everything that is already queued before redrawing, so
            // drags and key repeat never lag behind.
            while !app.quit && event::poll(Duration::ZERO)? {
                app.handle_event(event::read()?);
            }
        }
        if let Some(text) = app.clipboard.pending.take() {
            // Copy to the terminal's clipboard too, like Textual (OSC 52).
            use std::io::Write;
            let backend = terminal.backend_mut();
            write!(
                backend,
                "\x1b]52;c;{}\x07",
                crate::text::base64(text.as_bytes())
            )?;
            backend.flush()?;
        }
        app.notifications.expire(Instant::now());
    }
}

#[cfg(test)]
mod tests;
