//! Outputs as reported by wlr-randr, and the settings saved for them.

use std::collections::HashSet;

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Deserializer, Serialize};

pub const SCALE_MIN: f64 = 0.5;
pub const SCALE_MAX: f64 = 3.0;
pub const SCALE_STEP: f64 = 0.05;

const COMMON_SCALES: [f64; 15] = [
    0.50, 0.75, 0.90, 1.00, 1.10, 1.25, 1.33, 1.50, 1.66, 1.75, 2.00, 2.25, 2.50, 2.75, 3.00,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Transform {
    #[default]
    #[serde(rename = "normal")]
    Normal,
    #[serde(rename = "90")]
    Rotate90,
    #[serde(rename = "180")]
    Rotate180,
    #[serde(rename = "270")]
    Rotate270,
    #[serde(rename = "flipped")]
    Flipped,
    #[serde(rename = "flipped-90")]
    Flipped90,
    #[serde(rename = "flipped-180")]
    Flipped180,
    #[serde(rename = "flipped-270")]
    Flipped270,
}

impl Transform {
    pub const ALL: [Transform; 8] = [
        Self::Normal,
        Self::Rotate90,
        Self::Rotate180,
        Self::Rotate270,
        Self::Flipped,
        Self::Flipped90,
        Self::Flipped180,
        Self::Flipped270,
    ];

    /// The name wlr-randr, sway and niri use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Rotate90 => "90",
            Self::Rotate180 => "180",
            Self::Rotate270 => "270",
            Self::Flipped => "flipped",
            Self::Flipped90 => "flipped-90",
            Self::Flipped180 => "flipped-180",
            Self::Flipped270 => "flipped-270",
        }
    }

    pub fn is_rotated(self) -> bool {
        matches!(
            self,
            Self::Rotate90 | Self::Rotate270 | Self::Flipped90 | Self::Flipped270
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub struct Size {
    pub width: i32,
    pub height: i32,
}

/// Width, height and refresh rate in millihertz: a mode's identity, which
/// ignores the current/preferred flags.
pub type ModeKey = (i32, i32, i64);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mode {
    pub width: i32,
    pub height: i32,
    pub refresh: f64,
    #[serde(default, skip_serializing)]
    pub preferred: bool,
    #[serde(default, skip_serializing)]
    pub current: bool,
}

impl Mode {
    pub fn new(width: i32, height: i32, refresh: f64) -> Self {
        Self {
            width,
            height,
            refresh,
            preferred: false,
            current: false,
        }
    }

    pub fn key(&self) -> ModeKey {
        (
            self.width,
            self.height,
            (self.refresh * 1000.0).round() as i64,
        )
    }

    pub fn size_text(&self) -> String {
        format!("{}x{}", self.width, self.height)
    }

    pub fn label(&self) -> String {
        format!("{} @ {} Hz", self.size_text(), fmt_num(self.refresh, 2))
    }

    /// The `--mode` value for wlr-randr. Virtual outputs report a refresh
    /// rate of 0, which wlr-randr rejects, so it is left out for them.
    pub fn wlr_randr_arg(&self) -> String {
        if self.refresh > 0.0 {
            format!("{}@{}", self.size_text(), fmt_num(self.refresh, 6))
        } else {
            self.size_text()
        }
    }

    /// `1920x1080@60.000`, the form sway, niri and mango configs use.
    pub fn config_text(&self) -> String {
        format!("{}@{:.3}", self.size_text(), self.refresh)
    }
}

/// An output and its current settings, as read from wlr-randr (plus whatever
/// the compositor integration adds).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(from = "WlrOutput")]
pub struct Output {
    pub name: String,
    pub description: String,
    pub make: String,
    pub model: String,
    pub serial: String,
    pub physical_size: Size,
    pub enabled: bool,
    pub modes: Vec<Mode>,
    pub mode: Option<Mode>,
    pub position: Position,
    pub transform: Transform,
    pub scale: f64,
    pub adaptive_sync: bool,
    // Compositor-specific settings; `None` means unknown or not managed.
    pub hdr: Option<bool>,
    pub bit_depth: Option<u8>,
    pub color_profile: Option<String>,
}

/// One entry of `wlr-randr --json`.
#[derive(Deserialize)]
struct WlrOutput {
    name: String,
    #[serde(default, deserialize_with = "nullable")]
    description: String,
    #[serde(default, deserialize_with = "nullable")]
    make: String,
    #[serde(default, deserialize_with = "nullable")]
    model: String,
    #[serde(default, deserialize_with = "nullable")]
    serial: String,
    #[serde(default)]
    physical_size: Size,
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default)]
    modes: Vec<Mode>,
    #[serde(default)]
    position: Position,
    #[serde(default)]
    transform: Transform,
    #[serde(default = "one")]
    scale: f64,
    #[serde(default)]
    adaptive_sync: bool,
}

impl From<WlrOutput> for Output {
    fn from(raw: WlrOutput) -> Self {
        let mode = raw
            .modes
            .iter()
            .find(|mode| mode.current)
            .or_else(|| raw.modes.iter().find(|mode| mode.preferred))
            .or(raw.modes.first())
            .cloned();
        Self {
            name: raw.name,
            description: raw.description,
            make: raw.make,
            model: raw.model,
            serial: raw.serial,
            physical_size: raw.physical_size,
            enabled: raw.enabled,
            modes: raw.modes,
            mode,
            position: raw.position,
            transform: raw.transform,
            scale: raw.scale,
            adaptive_sync: raw.adaptive_sync,
            hdr: None,
            bit_depth: None,
            color_profile: None,
        }
    }
}

fn yes() -> bool {
    true
}

fn one() -> f64 {
    1.0
}

fn nullable<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::deserialize(deserializer)?.unwrap_or_default())
}

impl Output {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            make: String::new(),
            model: String::new(),
            serial: String::new(),
            physical_size: Size::default(),
            enabled: true,
            modes: Vec::new(),
            mode: None,
            position: Position::default(),
            transform: Transform::Normal,
            scale: 1.0,
            adaptive_sync: false,
            hdr: None,
            bit_depth: None,
            color_profile: None,
        }
    }

    /// A readable name: make and model, else the description, else the connector.
    pub fn title(&self) -> String {
        let make_model = join_nonempty(" ", [&self.make, &self.model]);
        if !make_model.is_empty() {
            make_model
        } else if !self.description.is_empty() {
            self.description.clone()
        } else {
            self.name.clone()
        }
    }

    /// A best-effort stable identity built from EDID fields.
    pub fn hardware_id(&self) -> String {
        join_nonempty("/", [&self.make, &self.model, &self.serial])
    }

    /// `make model serial` as sway and niri match outputs. Empty unless all
    /// three are known, since a partial identifier would not match anything.
    pub fn identifier(&self) -> String {
        if [&self.make, &self.model, &self.serial]
            .iter()
            .all(|part| !part.is_empty())
        {
            join_nonempty(" ", [&self.make, &self.model, &self.serial])
        } else {
            String::new()
        }
    }

    /// Size in layout pixels: the mode divided by the scale, rotated.
    pub fn logical_size(&self) -> (i64, i64) {
        let Some(mode) = &self.mode else {
            return (0, 0);
        };
        let scale = self.scale.max(0.01);
        let mut width = f64::from(mode.width) / scale;
        let mut height = f64::from(mode.height) / scale;
        if self.transform.is_rotated() {
            std::mem::swap(&mut width, &mut height);
        }
        (
            (width.round() as i64).max(1),
            (height.round() as i64).max(1),
        )
    }

    pub fn state(&self) -> OutputState {
        OutputState {
            name: self.name.clone(),
            enabled: self.enabled,
            mode: self.mode.clone(),
            position: self.position,
            scale: self.scale,
            transform: self.transform,
            adaptive_sync: self.adaptive_sync,
            hdr: self.hdr,
            bit_depth: self.bit_depth,
            color_profile: self.color_profile.clone(),
            hardware_id: self.hardware_id(),
            description: self.description.clone(),
        }
    }

    /// Unique modes, current first, then preferred, then largest and fastest.
    pub fn mode_options(&self) -> Vec<Mode> {
        let mut modes = self.modes.clone();
        modes.sort_by(|a, b| {
            let area = |mode: &Mode| i64::from(mode.width) * i64::from(mode.height);
            (!a.current)
                .cmp(&!b.current)
                .then((!a.preferred).cmp(&!b.preferred))
                .then(area(b).cmp(&area(a)))
                .then(b.refresh.total_cmp(&a.refresh))
        });
        let mut seen = HashSet::new();
        modes.retain(|mode| seen.insert(mode.key()));
        modes
    }
}

/// The settings that can be applied to an output. This is also what profiles
/// and the compositor include remember, keyed by output name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputState {
    #[serde(skip)]
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub mode: Option<Mode>,
    #[serde(default)]
    pub position: Position,
    #[serde(default = "one")]
    pub scale: f64,
    #[serde(default)]
    pub transform: Transform,
    #[serde(default)]
    pub adaptive_sync: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdr: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_profile: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hardware_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

impl OutputState {
    /// Reject what a hand-edited file could contain that the rest of the
    /// program does not expect.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.scale.is_finite() && self.scale > 0.0,
            "invalid scale {}",
            self.scale
        );
        if let Some(mode) = &self.mode {
            ensure!(
                mode.width > 0
                    && mode.height > 0
                    && mode.refresh.is_finite()
                    && mode.refresh >= 0.0,
                "invalid mode {}@{}",
                mode.size_text(),
                mode.refresh
            );
        }
        if self.bit_depth == Some(0) {
            bail!("invalid bit depth 0");
        }
        Ok(())
    }

    /// Compare everything that affects the on-screen layout. Settings that
    /// are unknown (`None`) on either side match anything.
    pub fn matches_config(&self, other: &OutputState) -> bool {
        fn same<T: PartialEq>(a: &Option<T>, b: &Option<T>) -> bool {
            a.is_none() || b.is_none() || a == b
        }
        self.enabled == other.enabled
            && self.mode.as_ref().map(Mode::key) == other.mode.as_ref().map(Mode::key)
            && self.position == other.position
            && (self.scale - other.scale).abs() < 0.0001
            && self.transform == other.transform
            && self.adaptive_sync == other.adaptive_sync
            && same(&self.hdr, &other.hdr)
            && same(&self.bit_depth, &other.bit_depth)
            && same(&self.color_profile, &other.color_profile)
    }

    /// Copy these settings onto an output.
    ///
    /// Compositor-specific settings are only copied when they are set, so
    /// older profiles do not reset HDR or color settings. With `exact` they
    /// are copied as they are (used to undo edits).
    pub fn apply_to(&self, output: &mut Output, exact: bool) {
        output.enabled = self.enabled;
        output.position = self.position;
        output.scale = self.scale;
        output.transform = self.transform;
        output.adaptive_sync = self.adaptive_sync;
        if exact || self.hdr.is_some() {
            output.hdr = self.hdr;
        }
        if exact || self.bit_depth.is_some() {
            output.bit_depth = self.bit_depth;
        }
        if exact || self.color_profile.is_some() {
            output.color_profile.clone_from(&self.color_profile);
        }
        if let Some(mode) = &self.mode {
            // Prefer the output's own mode, which carries the preferred flag.
            let own = output.modes.iter().find(|own| own.key() == mode.key());
            output.mode = Some(own.unwrap_or(mode).clone());
        }
    }
}

/// The saved state for `output`: by connector name, else by EDID id.
///
/// The EDID fallback (for connectors renamed after a replug) only applies
/// when it is unambiguous: exactly one saved state and one connected output
/// carry the id. Identical monitors often share an EDID serial, and guessing
/// would give both the same settings.
pub fn match_state<'a>(
    saved: &'a [OutputState],
    output: &OutputState,
    connected: &[OutputState],
) -> Option<&'a OutputState> {
    if let Some(state) = saved.iter().find(|state| state.name == output.name) {
        return Some(state);
    }
    let id = &output.hardware_id;
    let unique = |states: &[OutputState]| {
        states
            .iter()
            .filter(|state| state.hardware_id == *id)
            .count()
            == 1
    };
    if id.is_empty() || !unique(connected) || !unique(saved) {
        return None;
    }
    saved.iter().find(|state| state.hardware_id == *id)
}

/// True if applying `states` would leave no output enabled. Outputs without
/// a state keep the enabled state they have in `current`.
pub fn disables_everything(current: &[(String, bool)], states: &[OutputState]) -> bool {
    let enabled = |name: &str, fallback: bool| {
        states
            .iter()
            .rev()
            .find(|state| state.name == name)
            .map_or(fallback, |state| state.enabled)
    };
    let any_current = current.iter().any(|(name, on)| enabled(name, *on));
    let any_extra = states
        .iter()
        .any(|state| state.enabled && !current.iter().any(|(name, _)| *name == state.name));
    (!current.is_empty() || !states.is_empty()) && !any_current && !any_extra
}

/// The scale presets, plus `current` when it is not one of them.
pub fn common_scales(current: f64) -> Vec<f64> {
    let mut scales = COMMON_SCALES.to_vec();
    if current.is_finite() && scales.iter().all(|item| (current - item).abs() > 0.0001) {
        scales.push(round_to(current, 3));
        scales.sort_by(f64::total_cmp);
    }
    scales
}

pub fn clamp_scale(value: f64) -> f64 {
    let value = if value.is_nan() { 1.0 } else { value };
    round_to(value.clamp(SCALE_MIN, SCALE_MAX), 2)
}

pub fn step_scale(value: f64, steps: i32) -> f64 {
    clamp_scale(value + SCALE_STEP * f64::from(steps))
}

pub fn round_to(value: f64, decimals: i32) -> f64 {
    let factor = 10f64.powi(decimals);
    (value * factor).round() / factor
}

/// `value` with at most `decimals` decimals and no trailing zeros: `1.50`
/// becomes `1.5` and `2.0` becomes `2`.
pub fn fmt_num(value: f64, decimals: usize) -> String {
    let text = format!("{value:.decimals$}");
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.')
    } else {
        &text
    };
    if text == "-0" { "0" } else { text }.to_string()
}

fn join_nonempty<const N: usize>(separator: &str, parts: [&String; N]) -> String {
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(separator)
}

/// True if `text` has no control characters, so it cannot add lines to a
/// config file or a command.
pub fn is_printable(text: &str) -> bool {
    !text.chars().any(char::is_control)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub const FIXTURE: &str = include_str!("../tests/fixtures/outputs.json");

    pub fn fixture_outputs() -> Vec<Output> {
        serde_json::from_str(FIXTURE).unwrap()
    }

    #[test]
    fn parses_wlr_randr_json() {
        let outputs = fixture_outputs();
        let (hdmi, edp) = (&outputs[0], &outputs[1]);
        assert_eq!(hdmi.name, "HDMI-A-1");
        assert_eq!(hdmi.mode.as_ref().unwrap().refresh, 100.0);
        assert_eq!(hdmi.hardware_id(), "Microstep/MSI MP275/PC3M394200857");
        assert_eq!(edp.transform, Transform::Normal);
        assert!(edp.adaptive_sync);
    }

    #[test]
    fn missing_fields_take_defaults() {
        let output: Output = serde_json::from_str(r#"{"name":"X-1","make":null}"#).unwrap();
        assert!(output.enabled && output.make.is_empty() && output.mode.is_none());
        assert_eq!(output.scale, 1.0);
        assert!(serde_json::from_str::<Output>(r#"{"name":"X","transform":"bad"}"#).is_err());
    }

    #[test]
    fn logical_size_follows_scale_and_rotation() {
        let mut output = fixture_outputs().remove(0);
        output.scale = 2.0;
        output.transform = Transform::Rotate90;
        assert_eq!(output.logical_size(), (540, 960));
    }

    #[test]
    fn mode_options_are_unique_and_ordered() {
        let modes = fixture_outputs()[0].mode_options();
        assert!(modes[0].current);
        let keys: HashSet<_> = modes.iter().map(Mode::key).collect();
        assert_eq!(keys.len(), modes.len());
    }

    #[test]
    fn state_matching_prefers_name_then_unique_hardware() {
        let outputs = fixture_outputs();
        let states: Vec<_> = outputs.iter().map(Output::state).collect();
        let mut renamed = states[0].clone();
        renamed.name = "DP-9".into();
        assert!(match_state(&states, &renamed, &[renamed.clone()]).is_some());
        let mut twin = renamed.clone();
        twin.name = "DP-10".into();
        assert!(match_state(&states, &renamed, &[renamed.clone(), twin]).is_none());
    }

    #[test]
    fn refusing_to_disable_every_output() {
        let outputs = fixture_outputs();
        let current: Vec<_> = outputs
            .iter()
            .map(|o| (o.name.clone(), o.enabled))
            .collect();
        let mut states: Vec<_> = outputs.iter().map(Output::state).collect();
        assert!(!disables_everything(&current, &states));
        for state in &mut states {
            state.enabled = false;
        }
        assert!(disables_everything(&current, &states));
        assert!(!disables_everything(&current, &states[..1]));
    }

    #[test]
    fn numbers_are_trimmed() {
        assert_eq!(fmt_num(1.0, 2), "1");
        assert_eq!(fmt_num(59.939999, 2), "59.94");
        assert_eq!(fmt_num(-0.0, 2), "0");
        assert_eq!(step_scale(1.0, 1), 1.05);
        assert_eq!(clamp_scale(9.0), SCALE_MAX);
    }
}
