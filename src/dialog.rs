//! The dialogs: their state and how they react to keys.
//!
//! A dialog only edits its own state. `App` acts on what it returns.

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::compositor::Capabilities;
use crate::output::{Mode, Output, Transform, color_profile_label};
use crate::profile::ProfileStore;

pub enum Dialog {
    Choice(Choice),
    Prompt(Prompt),
    /// Overwrite the startup profile? Holds the question.
    Confirm(String),
    Advanced(Box<Advanced>),
    Profiles(Box<Profiles>),
    Help(Help),
}

/// What a list-like dialog did with a key.
pub enum ListKey {
    Ignored,
    Moved,
    Pick(usize),
    Close,
}

fn list_key(state: &mut ListState, key: &KeyEvent) -> ListKey {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => return ListKey::Close,
        KeyCode::Enter => return state.selected().map_or(ListKey::Close, ListKey::Pick),
        KeyCode::Up | KeyCode::Char('k') => state.select_previous(),
        KeyCode::Down | KeyCode::Char('j') => state.select_next(),
        KeyCode::PageUp => state.scroll_up_by(10),
        KeyCode::PageDown => state.scroll_down_by(10),
        KeyCode::Home => state.select_first(),
        KeyCode::End => state.select_last(),
        _ => return ListKey::Ignored,
    }
    ListKey::Moved
}

// ----- Choice -----

pub enum ChoiceFor {
    Mode(Vec<Mode>),
    /// The scale presets; one more row asks for a custom value.
    Scale(Vec<f64>),
    Transform,
}

/// Pick one entry from a list.
pub struct Choice {
    pub title: &'static str,
    pub output: String,
    pub choice_for: ChoiceFor,
    pub labels: Vec<String>,
    pub state: ListState,
}

impl Choice {
    pub fn new(
        title: &'static str,
        output: &str,
        choice_for: ChoiceFor,
        labels: Vec<String>,
        selected: usize,
    ) -> Self {
        Self {
            title,
            output: output.into(),
            choice_for,
            labels,
            state: ListState::default().with_selected(Some(selected)),
        }
    }

    pub fn key(&mut self, key: &KeyEvent) -> ListKey {
        list_key(&mut self.state, key)
    }
}

// ----- Prompt -----

pub enum PromptFor {
    CustomScale(String),
    SaveProfile,
    /// The old name; the answer goes back to the profile browser.
    RenameProfile(String),
    /// The answer goes back to the advanced dialog.
    IccPath,
}

/// A one-line text prompt.
pub struct Prompt {
    pub title: String,
    pub placeholder: &'static str,
    pub prompt_for: PromptFor,
    pub input: Input,
}

pub enum PromptKey {
    Editing,
    Cancel,
    /// A non-empty answer.
    Submit(String),
}

impl Prompt {
    pub fn new(
        title: impl Into<String>,
        placeholder: &'static str,
        prompt_for: PromptFor,
        value: &str,
    ) -> Self {
        Self {
            title: title.into(),
            placeholder,
            prompt_for,
            input: Input::new(value.into()),
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> PromptKey {
        match key.code {
            KeyCode::Esc => PromptKey::Cancel,
            KeyCode::Enter => match self.input.value().trim() {
                "" => PromptKey::Cancel,
                value => PromptKey::Submit(value.into()),
            },
            _ => {
                self.input.handle_event(&Event::Key(key));
                PromptKey::Editing
            }
        }
    }
}

// ----- Help -----

#[derive(Default)]
pub struct Help {
    pub scroll: u16,
    pub extra: String,
}

// ----- Advanced -----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Transform,
    Vrr,
    Enabled,
    Hdr,
    BitDepth,
    ColorProfile,
}

/// Rotation, VRR, on/off and the compositor-specific color settings. It edits
/// a copy of the output that `App` takes over on Enter.
pub struct Advanced {
    pub output: Output,
    pub capabilities: Capabilities,
    pub fields: Vec<Field>,
    pub focus: usize,
    /// False for the only enabled output.
    can_disable: bool,
    /// The ICC profile offered next to the presets.
    custom_profile: Option<String>,
}

pub enum AdvancedKey {
    Handled,
    Apply,
    Cancel,
    /// Ask for an ICC file path, starting with this value.
    PickIcc(String),
    Warn(&'static str),
}

fn cycle<T: PartialEq + Clone>(options: &[T], current: &T, delta: isize) -> T {
    let index = options.iter().position(|item| item == current).unwrap_or(0);
    options[(index as isize + delta).rem_euclid(options.len() as isize) as usize].clone()
}

impl Advanced {
    pub fn new(output: Output, capabilities: Capabilities, can_disable: bool) -> Self {
        let mut fields = vec![Field::Transform, Field::Vrr, Field::Enabled];
        if capabilities.hdr {
            fields.push(Field::Hdr);
        }
        if !capabilities.bit_depths.is_empty() {
            fields.push(Field::BitDepth);
        }
        if capabilities.color_profile() {
            fields.push(Field::ColorProfile);
        }
        Self {
            custom_profile: output.color_profile.clone(),
            output,
            capabilities,
            fields,
            focus: 0,
            can_disable,
        }
    }

    /// The label and value of a field.
    pub fn row(&self, field: Field) -> (&'static str, String) {
        let output = &self.output;
        let on = |value| if value { "On" } else { "Off" }.to_string();
        match field {
            Field::Transform => ("Rotation", output.transform.as_str().into()),
            Field::Vrr => ("VRR", on(output.adaptive_sync)),
            Field::Enabled => ("Output", on(output.enabled)),
            Field::Hdr => ("HDR", output.hdr.map_or("default".into(), on)),
            Field::BitDepth => ("Color depth", depth_label(output.bit_depth)),
            Field::ColorProfile => (
                "Color profile",
                color_profile_label(output.color_profile.as_deref()),
            ),
        }
    }

    fn adjust(&mut self, delta: isize) -> Option<&'static str> {
        let output = &mut self.output;
        match self.fields[self.focus] {
            Field::Transform => output.transform = cycle(&Transform::ALL, &output.transform, delta),
            Field::Vrr => output.adaptive_sync = !output.adaptive_sync,
            Field::Enabled => {
                if output.enabled && !self.can_disable {
                    return Some("Cannot disable the last active monitor.");
                }
                output.enabled = !output.enabled;
            }
            Field::Hdr => output.hdr = Some(output.hdr != Some(true)),
            Field::BitDepth => {
                let depths: Vec<_> = std::iter::once(None)
                    .chain(self.capabilities.bit_depths.iter().copied().map(Some))
                    .collect();
                output.bit_depth = cycle(&depths, &output.bit_depth, delta);
            }
            Field::ColorProfile => {
                let mut profiles = vec![None];
                profiles.extend(
                    self.capabilities
                        .color_presets
                        .iter()
                        .map(|p| Some(p.to_string())),
                );
                // A custom (ICC) profile stays in the cycle, so cycling past
                // it does not lose it.
                for custom in [&self.custom_profile, &output.color_profile]
                    .into_iter()
                    .flatten()
                {
                    if !profiles.contains(&Some(custom.clone())) {
                        profiles.push(Some(custom.clone()));
                    }
                }
                output.color_profile = cycle(&profiles, &output.color_profile, delta);
            }
        }
        None
    }

    fn pick_icc(&self) -> AdvancedKey {
        if !self.capabilities.icc {
            return AdvancedKey::Handled;
        }
        let current = self.output.color_profile.as_deref().unwrap_or("");
        AdvancedKey::PickIcc(current.strip_prefix("icc:").unwrap_or("").into())
    }

    pub fn key(&mut self, key: &KeyEvent) -> AdvancedKey {
        let count = self.fields.len();
        let warning = match key.code {
            KeyCode::Esc => return AdvancedKey::Cancel,
            KeyCode::Enter => return AdvancedKey::Apply,
            KeyCode::Up | KeyCode::BackTab | KeyCode::Char('k') => {
                self.focus = (self.focus + count - 1) % count;
                None
            }
            KeyCode::Down | KeyCode::Tab | KeyCode::Char('j') => {
                self.focus = (self.focus + 1) % count;
                None
            }
            KeyCode::Left | KeyCode::Char('h') => self.adjust(-1),
            KeyCode::Right | KeyCode::Char('l') => self.adjust(1),
            KeyCode::Char(' ') => self.adjust(1),
            KeyCode::Char('i') => return self.pick_icc(),
            _ => None,
        };
        warning.map_or(AdvancedKey::Handled, AdvancedKey::Warn)
    }

    /// The ICC prompt gave this (already validated) profile.
    pub fn set_icc(&mut self, profile: String) {
        self.output.color_profile = Some(profile.clone());
        self.custom_profile = Some(profile);
        if let Some(index) = self.fields.iter().position(|f| *f == Field::ColorProfile) {
            self.focus = index;
        }
    }
}

pub fn depth_label(depth: Option<u8>) -> String {
    match depth {
        Some(depth) if depth != 0 => format!("{depth} bit"),
        _ => "default".into(),
    }
}

// ----- Profiles -----

/// What the profile browser asks `App` to do.
pub enum ProfilesKey {
    Handled,
    Close,
    Apply(String),
    SaveCurrent,
    OpenEditor,
    Rename(String),
}

pub struct ProfileEntry {
    pub name: String,
    pub outputs: usize,
    pub active: bool,
}

/// Browse, apply, save, rename, reorder and delete profiles.
pub struct Profiles {
    pub entries: Vec<ProfileEntry>,
    pub state: ListState,
    /// Opened with `wlr-monitor profiles`: applying exits the program.
    pub standalone: bool,
    pub confirm_delete: bool,
    /// Why the last action failed.
    pub error: Option<String>,
    /// Saved profiles that cannot be read.
    pub problems: Vec<String>,
    /// The store changed since the last `take_changed`.
    changed: bool,
}

impl Profiles {
    pub fn new(
        store: &ProfileStore,
        current: &[crate::output::OutputState],
        standalone: bool,
    ) -> Self {
        let mut profiles = Self {
            entries: Vec::new(),
            state: ListState::default(),
            standalone,
            confirm_delete: false,
            error: None,
            problems: Vec::new(),
            changed: false,
        };
        let active = store.find_active(current).ok().flatten();
        profiles.reload(store, active.as_deref(), current);
        profiles
    }

    /// Re-read the store, keeping `select` (or the current row) highlighted.
    pub fn reload(
        &mut self,
        store: &ProfileStore,
        select: Option<&str>,
        current: &[crate::output::OutputState],
    ) {
        let keep = select.map(str::to_string).or_else(|| self.selected_name());
        let row = self.state.selected().unwrap_or(0);
        let active = store.find_active(current).ok().flatten();
        let names = store.names().unwrap_or_else(|error| {
            self.error = Some(error.to_string());
            Vec::new()
        });
        self.problems = store.problems().unwrap_or_default();
        self.entries = names
            .into_iter()
            .map(|name| ProfileEntry {
                outputs: store
                    .get(&name)
                    .ok()
                    .flatten()
                    .map_or(0, |p| p.outputs.len()),
                active: active.as_deref() == Some(&name),
                name,
            })
            .collect();
        let row = keep
            .and_then(|name| self.entries.iter().position(|entry| entry.name == name))
            .unwrap_or_else(|| row.min(self.entries.len().saturating_sub(1)));
        self.state.select(Some(row));
    }

    pub fn selected_name(&self) -> Option<String> {
        let entry = self.entries.get(self.state.selected()?)?;
        Some(entry.name.clone())
    }

    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// Run a change on the store, keeping the error visible if it fails.
    fn change(&mut self, result: anyhow::Result<bool>) -> bool {
        match result {
            Ok(done) => {
                self.error = None;
                self.changed |= done;
                done
            }
            Err(error) => {
                self.error = Some(error.to_string());
                false
            }
        }
    }

    pub fn key(
        &mut self,
        key: &KeyEvent,
        store: &ProfileStore,
        current: &[crate::output::OutputState],
    ) -> ProfilesKey {
        let name = self.selected_name();
        if self.confirm_delete {
            self.confirm_delete = false;
            if let (KeyCode::Char('y' | 'Y'), Some(name)) = (key.code, &name)
                && self.change(store.delete(name))
            {
                self.reload(store, None, current);
            }
            return ProfilesKey::Handled;
        }
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match (key.code, name) {
            (KeyCode::Esc | KeyCode::Char('q'), _) => return ProfilesKey::Close,
            (KeyCode::Char('s'), _) if !self.standalone => return ProfilesKey::SaveCurrent,
            (KeyCode::Char('l'), _) if self.standalone => return ProfilesKey::OpenEditor,
            (KeyCode::Enter, Some(name)) => return ProfilesKey::Apply(name),
            (KeyCode::Char('r'), Some(name)) => return ProfilesKey::Rename(name),
            (KeyCode::Char('d'), Some(_)) => self.confirm_delete = true,
            (KeyCode::Up | KeyCode::Down, Some(name)) if shift => {
                let offset = if key.code == KeyCode::Up { -1 } else { 1 };
                if self.change(store.move_by(&name, offset)) {
                    self.reload(store, Some(&name), current);
                }
            }
            _ => {
                list_key(&mut self.state, key);
            }
        }
        ProfilesKey::Handled
    }

    pub fn rename(
        &mut self,
        store: &ProfileStore,
        old: &str,
        new: &str,
        current: &[crate::output::OutputState],
    ) {
        if old != new && self.change(store.rename(old, new)) {
            self.reload(store, Some(new.trim()), current);
        }
    }
}
