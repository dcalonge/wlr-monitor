//! The editor's state and what each action does to it.

mod dialogs;

use std::collections::{HashMap, HashSet};

use ratatui::layout::Rect;

use crate::backend::Backend;
use crate::compositor::Compositor;
use crate::dialog::{Advanced, Choice, ChoiceFor, Dialog, Help, Profiles, Prompt, PromptFor};
use crate::layout::{Geometry, Guide, snap};
use crate::notification::{Notifications, Severity};
use crate::output::{
    Mode, Output, OutputState, SCALE_MAX, SCALE_MIN, Transform, clamp_scale, common_scales,
    disables_everything, fmt_num, step_scale,
};
use crate::profile::ProfileStore;

pub const GRID_SIZES: [i32; 5] = [1, 8, 16, 32, 64];
const SNAP_THRESHOLD: i32 = 10;
const STARTUP_PROFILE: &str = "default";

/// An output being dragged on the map.
pub struct Drag {
    pub index: usize,
    pub last: (u16, u16),
    /// Sub-pixel movement not yet applied.
    pub accum: (f64, f64),
    /// The position before snapping to the grid and other outputs.
    pub raw: (f64, f64),
}

/// Where the last frame drew things, for the mouse.
#[derive(Default)]
pub struct Areas {
    pub map: Rect,
    pub geometry: Option<Geometry>,
    /// Output indexes in drawing order; the last one is on top.
    pub draw_order: Vec<usize>,
    /// The table rows that show outputs, one line each.
    pub table_rows: Rect,
    /// The first output shown by the (possibly scrolled) table.
    pub table_offset: usize,
}

impl Areas {
    /// The output drawn at this screen cell.
    pub fn output_at(&self, column: u16, row: u16) -> Option<usize> {
        let geometry = self.geometry.as_ref()?;
        let (x, y) = (
            column.checked_sub(self.map.x)?,
            row.checked_sub(self.map.y)?,
        );
        self.draw_order
            .iter()
            .rev()
            .copied()
            .find(|&index| geometry.boxes[index].is_some_and(|rect| rect.contains((x, y).into())))
    }
}

impl Areas {
    /// The output whose table row is at this screen cell.
    pub fn table_row_at(&self, column: u16, row: u16, count: usize) -> Option<usize> {
        let index = self.table_offset + usize::from(row.checked_sub(self.table_rows.y)?);
        (self.table_rows.contains((column, row).into()) && index < count).then_some(index)
    }
}

pub struct App {
    pub backend: Backend,
    pub store: ProfileStore,
    pub outputs: Vec<Output>,
    /// The last applied state of every output.
    pub baseline: HashMap<String, OutputState>,
    pub selected: usize,
    pub grid: i32,
    pub snap: bool,
    pub guides: Vec<Guide>,
    pub active_profile: Option<String>,
    pub status: String,
    pub error: Option<String>,
    /// True until the first read of the outputs finished.
    pub loading: bool,
    /// Started with `profiles`: only the profile browser, until it is left.
    pub profiles_only: bool,
    pub dialogs: Vec<Dialog>,
    pub notifications: Notifications,
    pub quit: bool,
    pub areas: Areas,
    pub drag: Option<Drag>,
    /// The layout before the last apply, for `Z`.
    rollback: Vec<OutputState>,
}

impl App {
    pub fn new(backend: Backend, store: ProfileStore, profiles_only: bool) -> Self {
        Self {
            backend,
            store,
            outputs: Vec::new(),
            baseline: HashMap::new(),
            selected: 0,
            grid: 32,
            snap: true,
            guides: Vec::new(),
            active_profile: None,
            status: "Ready.".into(),
            error: None,
            loading: true,
            profiles_only,
            dialogs: Vec::new(),
            notifications: Notifications::default(),
            quit: false,
            areas: Areas::default(),
            drag: None,
            rollback: Vec::new(),
        }
    }

    pub fn compositor(&self) -> &Compositor {
        &self.backend.compositor
    }

    /// Read the outputs and do the start-up work (the first frame is
    /// already on screen by then).
    pub fn start(&mut self) {
        if self.reload() {
            self.ensure_include();
        }
        self.loading = false;
        if self.profiles_only {
            self.open_profiles();
        }
        let hint = self.compositor().include_hint();
        if !hint.is_empty() {
            self.notifications.push(hint, Severity::Info, 8);
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
        if !compositor.include_must_exist() || !compositor.include_enabled || path.exists() {
            return;
        }
        let result = compositor.write_include(&self.outputs, None);
        if let Err(error) = result {
            self.notify(error.to_string(), Severity::Warning);
        }
    }

    // ----- State helpers -----

    pub fn current(&self) -> Option<&Output> {
        self.outputs.get(self.selected)
    }

    pub fn is_dirty(&self, output: &Output) -> bool {
        self.baseline.get(&output.name) != Some(&output.state())
    }

    pub fn dirty_count(&self) -> usize {
        self.outputs
            .iter()
            .filter(|output| self.is_dirty(output))
            .count()
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.outputs.iter().position(|output| output.name == name)
    }

    fn baseline_states(&self) -> Vec<OutputState> {
        self.outputs
            .iter()
            .filter_map(|output| self.baseline.get(&output.name).cloned())
            .collect()
    }

    fn detect_active_profile(&mut self) {
        self.active_profile = self
            .store
            .find_active(&self.baseline_states())
            .ok()
            .flatten();
    }

    fn set_outputs(&mut self, outputs: Vec<Output>) {
        let current = self.current().map(|output| output.name.clone());
        self.baseline = outputs
            .iter()
            .map(|output| (output.name.clone(), output.state()))
            .collect();
        self.outputs = outputs;
        self.selected = current
            .and_then(|name| self.find(&name))
            .unwrap_or(self.selected)
            .min(self.outputs.len().saturating_sub(1));
        self.guides.clear();
        self.detect_active_profile();
    }

    pub fn status_line(&self) -> String {
        let mut parts = vec![
            format!("grid {}px", self.grid),
            format!("snap {}", if self.snap { "Edges" } else { "Off" }),
        ];
        if let Some(profile) = &self.active_profile {
            parts.push(format!("profile {profile}"));
        }
        match self.dirty_count() {
            0 => {}
            count => parts.push(format!("{count} pending")),
        }
        parts.push(if self.loading {
            "Reading outputs…".into()
        } else {
            self.status.clone()
        });
        parts.join(" · ")
    }

    /// Show `message` in the status line and as a toast.
    pub fn notify(&mut self, message: impl Into<String>, severity: Severity) {
        let message = message.into();
        self.status.clone_from(&message);
        self.notifications.push(message, severity, 4);
    }

    fn fail(&mut self, message: String) {
        self.error = Some(message.clone());
        self.notify(message, Severity::Error);
    }

    /// Open a dialog. A drag in progress ends first, so the output does not
    /// keep moving once the dialog is closed.
    fn open(&mut self, dialog: Dialog) {
        self.end_drag();
        self.dialogs.push(dialog);
    }

    pub fn reload(&mut self) -> bool {
        match self.backend.read_outputs() {
            Ok(outputs) => {
                self.error = None;
                self.set_outputs(outputs);
                true
            }
            Err(error) => {
                self.fail(error.to_string());
                false
            }
        }
    }

    pub fn reload_action(&mut self) {
        if self.reload() {
            self.status = "Reloaded outputs.".into();
            self.notify("Reloaded outputs from wlr-randr.", Severity::Info);
        }
    }

    fn apply_states(
        &mut self,
        states: Vec<OutputState>,
        message: &str,
        remember_previous: bool,
        profile: Option<&str>,
    ) -> bool {
        if states.is_empty() {
            self.notify("Nothing to apply.", Severity::Warning);
            return false;
        }
        // The same rule as the on/off toggle, enforced for every path
        // (advanced dialog, profiles, revert).
        let live: Vec<_> = self
            .outputs
            .iter()
            .map(|o| {
                (
                    o.name.clone(),
                    self.baseline.get(&o.name).map_or(o.enabled, |s| s.enabled),
                )
            })
            .collect();
        if disables_everything(&live, &states) {
            self.notify(
                "Refusing to apply: it would turn off every monitor.",
                Severity::Error,
            );
            return false;
        }
        let previous = self.baseline_states();
        let applied = match self.backend.apply(&states, profile) {
            Ok(applied) => applied,
            Err(error) => {
                self.fail(error.to_string());
                return false;
            }
        };
        let failures: Vec<_> = applied
            .failures()
            .map(|f| {
                format!(
                    "{}: {}",
                    f.name,
                    f.error.as_deref().unwrap_or("unknown error")
                )
            })
            .collect();
        if !failures.is_empty() {
            let more = failures.len().saturating_sub(3);
            let mut text = failures[..failures.len() - more].join("; ");
            if more > 0 {
                text += &format!("; and {more} more");
            }
            self.fail(format!("Apply failed — {text}"));
            return false;
        }

        self.error = None;
        if remember_previous {
            self.rollback = previous;
        }
        let applied_names: HashSet<_> = states.iter().map(|state| &state.name).collect();
        for output in &self.outputs {
            if applied_names.contains(&output.name) {
                self.baseline.insert(output.name.clone(), output.state());
            }
        }
        match self.backend.read_outputs() {
            Ok(outputs) => self.set_outputs(outputs),
            // Keep the local state if the refresh fails.
            Err(_) => self.detect_active_profile(),
        }
        self.notify(message, Severity::Info);
        for warning in &applied.warnings {
            self.notifications.push(warning, Severity::Warning, 8);
        }
        true
    }

    // ----- Actions on the selected output -----

    pub fn select_next(&mut self, step: isize) {
        let count = self.outputs.len() as isize;
        if count > 0 {
            self.selected = (self.selected as isize + step).rem_euclid(count) as usize;
            self.guides.clear();
        }
    }

    pub fn move_by(&mut self, dx: i32, dy: i32, multiplier: i32) {
        let Some(output) = self.current() else { return };
        let x = output.position.x + dx * self.grid * multiplier;
        let y = output.position.y + dy * self.grid * multiplier;
        self.place(self.selected, x, y);
    }

    /// Move an output to a position, snapped to the other outputs.
    fn place(&mut self, index: usize, x: i32, y: i32) {
        let (x, y, guides) = if self.snap {
            snap(&self.outputs, index, x, y, SNAP_THRESHOLD)
        } else {
            (x, y, Vec::new())
        };
        self.guides = guides;
        let output = &mut self.outputs[index];
        output.position.x = x;
        output.position.y = y;
        self.status = format!("{}: {x},{y}", output.name);
    }

    pub fn cycle_grid(&mut self) {
        let index = GRID_SIZES
            .iter()
            .position(|size| *size == self.grid)
            .unwrap_or(0);
        self.grid = GRID_SIZES[(index + 1) % GRID_SIZES.len()];
        self.status = format!("Grid: {}px", self.grid);
    }

    pub fn toggle_snap(&mut self) {
        self.snap = !self.snap;
        self.guides.clear();
        self.status = format!("Snap: {}", if self.snap { "on" } else { "off" });
    }

    /// Edit the output at `index`, then drop the guides and say what changed.
    fn edit(&mut self, index: usize, change: impl FnOnce(&mut Output) -> String) {
        if let Some(output) = self.outputs.get_mut(index) {
            let what = change(output);
            self.status = format!("{}: {what}", output.name);
            self.guides.clear();
        }
    }

    pub fn pick_mode(&mut self) {
        let Some(output) = self.current() else { return };
        let modes = output.mode_options();
        if modes.is_empty() {
            self.notify("This output reports no modes.", Severity::Warning);
            return;
        }
        let current = output.mode.as_ref().map(Mode::key);
        let selected = modes.iter().position(|mode| Some(mode.key()) == current);
        let labels = modes
            .iter()
            .enumerate()
            .map(|(i, mode)| {
                let marker = if Some(i) == selected { "●" } else { " " };
                let preferred = if mode.preferred { "  preferred" } else { "" };
                format!("{marker} {}{preferred}", mode.label())
            })
            .collect();
        let name = output.name.clone();
        let choice = Choice::new(
            "Resolution and refresh rate",
            &name,
            ChoiceFor::Mode(modes),
            labels,
            selected.unwrap_or(0),
        );
        self.open(Dialog::Choice(choice));
    }

    pub fn pick_scale(&mut self) {
        let Some(output) = self.current() else { return };
        let scales = common_scales(output.scale);
        let selected = scales
            .iter()
            .position(|s| (s - output.scale).abs() < 0.0001);
        let mut labels: Vec<_> = scales
            .iter()
            .map(|s| format!("{}x", fmt_num(*s, 3)))
            .collect();
        labels.push("Custom value…".into());
        let selected = selected.unwrap_or(scales.len());
        let name = output.name.clone();
        let choice = Choice::new("Scale", &name, ChoiceFor::Scale(scales), labels, selected);
        self.open(Dialog::Choice(choice));
    }

    pub fn pick_transform(&mut self) {
        let Some(output) = self.current() else { return };
        let labels = Transform::ALL
            .iter()
            .map(|t| t.as_str().to_string())
            .collect();
        let selected = Transform::ALL
            .iter()
            .position(|t| *t == output.transform)
            .unwrap_or(0);
        let name = output.name.clone();
        let choice = Choice::new("Rotation", &name, ChoiceFor::Transform, labels, selected);
        self.open(Dialog::Choice(choice));
    }

    pub fn prompt_custom_scale(&mut self, name: &str) {
        let Some(index) = self.find(name) else { return };
        let value = fmt_num(self.outputs[index].scale, 3);
        let prompt = Prompt::new(
            format!("Scale for {name}"),
            "1.25",
            PromptFor::CustomScale(name.into()),
            &value,
        );
        self.open(Dialog::Prompt(prompt));
    }

    pub fn set_custom_scale(&mut self, name: &str, text: &str) {
        let Ok(scale) = text.replace(',', ".").parse::<f64>() else {
            self.notify(format!("Invalid scale: {text:?}"), Severity::Error);
            return;
        };
        if !(SCALE_MIN..=SCALE_MAX).contains(&scale) {
            let range = format!("{} and {}", fmt_num(SCALE_MIN, 2), fmt_num(SCALE_MAX, 2));
            self.notify(format!("Scale must be between {range}."), Severity::Error);
            return;
        }
        self.set_scale(name, scale);
    }

    fn set_scale(&mut self, name: &str, scale: f64) {
        if let Some(index) = self.find(name) {
            self.edit(index, |output| {
                output.scale = clamp_scale(scale);
                format!("scale {}", fmt_num(output.scale, 3))
            });
        }
    }

    pub fn step_scale(&mut self, index: usize, steps: i32) {
        self.edit(index, |output| {
            output.scale = step_scale(output.scale, steps);
            format!("scale {}", fmt_num(output.scale, 3))
        });
    }

    pub fn toggle_adaptive_sync(&mut self) {
        self.edit(self.selected, |output| {
            output.adaptive_sync = !output.adaptive_sync;
            format!("VRR {}", if output.adaptive_sync { "on" } else { "off" })
        });
    }

    pub fn toggle_enabled(&mut self, index: usize) {
        let enabled = self.outputs.iter().filter(|output| output.enabled).count();
        if self.outputs.get(index).is_some_and(|o| o.enabled) && enabled <= 1 {
            self.notify("Cannot disable the last active monitor.", Severity::Warning);
            return;
        }
        self.edit(index, |output| {
            output.enabled = !output.enabled;
            if output.enabled {
                "enabled"
            } else {
                "disabled"
            }
            .into()
        });
    }

    pub fn open_advanced(&mut self) {
        let Some(output) = self.current() else { return };
        let can_disable = self
            .outputs
            .iter()
            .enumerate()
            .any(|(i, o)| i != self.selected && o.enabled);
        let advanced = Advanced::new(output.clone(), self.compositor().capabilities, can_disable);
        self.open(Dialog::Advanced(Box::new(advanced)));
    }

    pub fn open_help(&mut self) {
        let c = self.compositor();
        let mut lines = vec![format!("Compositor: {}", c.name())];
        match c.include_path() {
            Some(path) if c.include_enabled => {
                lines.push(format!("Applied layouts are written to {}.", c.display_path(path)));
                lines.push(if c.include_is_sourced() {
                    "Your compositor config includes it.".into()
                } else {
                    format!("Add this line to your config: {}", c.include_line())
                });
            }
            Some(_) => {}
            None => lines.push(
                "Changes are applied with wlr-randr only; HDR and color settings need mango, sway or niri.".into(),
            ),
        }
        self.open(Dialog::Help(Help {
            scroll: 0,
            extra: lines.join("\n"),
        }));
    }

    // ----- Apply, reset, profiles -----

    pub fn apply(&mut self) {
        let states: Vec<_> = self
            .outputs
            .iter()
            .filter(|output| self.is_dirty(output))
            .map(Output::state)
            .collect();
        let message = format!("Applied {} output(s).", states.len());
        self.apply_states(states, &message, true, None);
    }

    /// Back to the last applied state.
    pub fn reset(&mut self) {
        self.error = None;
        for output in &mut self.outputs {
            if let Some(state) = self.baseline.get(&output.name) {
                state.apply_to(output, true);
            }
        }
        self.guides.clear();
        self.status = "Reset pending changes.".into();
    }

    /// Apply the layout from before the last apply.
    pub fn revert(&mut self) {
        if self.rollback.is_empty() {
            self.notify("No previous layout to revert to.", Severity::Warning);
            return;
        }
        let previous = self.baseline_states();
        let states = self.rollback.clone();
        if self.apply_states(states, "Reverted to the previous layout.", false, None) {
            self.rollback = previous;
        }
    }

    pub fn prompt_save_profile(&mut self) {
        if self.outputs.is_empty() {
            self.notify("There are no outputs to save.", Severity::Warning);
            return;
        }
        let prompt = Prompt::new(
            "Save layout as profile",
            "home, work, docked…",
            PromptFor::SaveProfile,
            "",
        );
        self.open(Dialog::Prompt(prompt));
    }

    fn save_profile(&mut self, name: &str) {
        let states: Vec<_> = self.outputs.iter().map(Output::state).collect();
        match self.store.save(name, &states) {
            Ok(()) => {
                self.detect_active_profile();
                self.notify(
                    format!("Saved profile {name:?} ({} outputs).", states.len()),
                    Severity::Info,
                );
            }
            Err(error) => self.notify(error.to_string(), Severity::Error),
        }
    }

    /// Save the layout as the profile `wlr-monitor --profile default` applies at login.
    pub fn save_startup_profile(&mut self) {
        if self.outputs.is_empty() {
            self.notify("There are no outputs to save.", Severity::Warning);
            return;
        }
        match self.store.exists(STARTUP_PROFILE) {
            Err(error) => self.notify(error.to_string(), Severity::Error),
            Ok(false) => self.save_profile(STARTUP_PROFILE),
            Ok(true) => {
                let message = format!(
                    "A profile named {STARTUP_PROFILE:?} already exists. Overwrite it with the current layout?"
                );
                self.open(Dialog::Confirm(message));
            }
        }
    }

    pub fn open_profiles(&mut self) {
        let profiles = Profiles::new(&self.store, &self.baseline_states(), self.profiles_only);
        self.open(Dialog::Profiles(Box::new(profiles)));
    }

    /// Apply a saved profile to the matching connected outputs.
    pub fn apply_profile(&mut self, name: &str) -> bool {
        let profile = match self.store.get(name) {
            Ok(Some(profile)) => profile,
            Ok(None) => {
                self.notify(format!("No profile named {name:?}."), Severity::Error);
                return false;
            }
            Err(error) => {
                self.notify(error.to_string(), Severity::Error);
                return false;
            }
        };
        // The profile goes into the editor only if it is applied: a refused
        // or failed apply leaves the outputs (and their pending edits) as
        // they were.
        let mut outputs = self.outputs.clone();
        let matched = profile.apply_to(&mut outputs);
        // A profile saved on another compositor may carry settings this one
        // cannot apply; drop them so they don't show as pending.
        self.backend.compositor.drop_unsupported(&mut outputs);
        let states: Vec<_> = outputs
            .iter()
            .filter(|output| matched.applied.contains(&output.name))
            .map(Output::state)
            .collect();
        if states.is_empty() {
            self.notify(
                format!("Profile {name:?} does not match any connected output."),
                Severity::Warning,
            );
            return false;
        }
        let before = std::mem::replace(&mut self.outputs, outputs);
        let applied = self.apply_states(
            states,
            &format!("Applied profile {name:?}."),
            true,
            Some(name),
        );
        if !applied {
            self.outputs = before;
        }
        applied
    }

    // ----- Mouse -----

    pub fn select(&mut self, index: usize) {
        self.selected = index;
        self.guides.clear();
    }

    pub fn start_drag(&mut self, index: usize, column: u16, row: u16) {
        self.select(index);
        let position = self.outputs[index].position;
        self.drag = Some(Drag {
            index,
            last: (column, row),
            accum: (0.0, 0.0),
            raw: (f64::from(position.x), f64::from(position.y)),
        });
    }

    pub fn drag_to(&mut self, column: u16, row: u16) {
        let (Some(drag), Some(geometry)) = (self.drag.as_mut(), self.areas.geometry.as_ref())
        else {
            return;
        };
        let columns = i32::from(column) - i32::from(drag.last.0);
        let rows = i32::from(row) - i32::from(drag.last.1);
        drag.last = (column, row);
        let (dx, dy) = geometry.to_pixels(columns, rows);
        drag.accum.0 += dx;
        drag.accum.1 += dy;
        let step = (
            drag.accum.0.round_ties_even(),
            drag.accum.1.round_ties_even(),
        );
        if step == (0.0, 0.0) {
            return;
        }
        drag.accum = (drag.accum.0 - step.0, drag.accum.1 - step.1);
        drag.raw = (drag.raw.0 + step.0, drag.raw.1 + step.1);
        let (index, raw, grid) = (drag.index, drag.raw, self.grid);
        self.place(index, quantize(raw.0, grid), quantize(raw.1, grid));
    }

    pub fn end_drag(&mut self) {
        if self.drag.take().is_some() {
            self.guides.clear();
        }
    }
}

/// Round a position down to the grid, towards zero.
fn quantize(value: f64, grid: i32) -> i32 {
    if grid <= 1 {
        return value.round_ties_even() as i32;
    }
    (value / f64::from(grid)).trunc() as i32 * grid
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::Kind;
    use crate::output::tests::fixture_outputs;
    use crate::wlr_randr::WlrRandr;

    fn app() -> App {
        let compositor = Compositor::new(Kind::Wlroots, None, false);
        let backend = Backend::new(WlrRandr::new(Some("/nonexistent")), compositor);
        let store = ProfileStore::new(Some(
            std::env::temp_dir().join("wlr-monitor-test/none.json"),
        ));
        let mut app = App::new(backend, store, false);
        app.set_outputs(fixture_outputs());
        app
    }

    #[test]
    fn moving_marks_the_output_pending_and_reset_undoes_it() {
        let mut app = app();
        assert_eq!(app.dirty_count(), 0);
        app.snap = false;
        app.move_by(1, 0, 1);
        assert_eq!(app.outputs[0].position.x, 32);
        assert_eq!(app.dirty_count(), 1);
        app.reset();
        assert_eq!(app.dirty_count(), 0);
    }

    #[test]
    fn the_last_enabled_output_cannot_be_disabled() {
        let mut app = app();
        app.toggle_enabled(0);
        assert!(!app.outputs[0].enabled);
        app.toggle_enabled(1);
        assert!(app.outputs[1].enabled);
    }

    #[test]
    fn dragging_moves_by_logical_pixels_on_the_grid() {
        let mut app = app();
        app.snap = false;
        app.areas.geometry = Geometry::new(&app.outputs, 100, 30);
        app.start_drag(0, 5, 5);
        app.drag_to(15, 5);
        assert!(app.outputs[0].position.x > 0);
        assert_eq!(app.outputs[0].position.x % 32, 0);
    }
}
