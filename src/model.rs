//! Data model for wlr-randr output information.

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde_json::{Map, Value, json};

pub const TRANSFORMS: [&str; 8] = [
    "normal",
    "90",
    "180",
    "270",
    "flipped",
    "flipped-90",
    "flipped-180",
    "flipped-270",
];

const ROTATED_TRANSFORMS: [&str; 4] = ["90", "270", "flipped-90", "flipped-270"];

pub const SCALE_MIN: f64 = 0.5;
pub const SCALE_MAX: f64 = 3.0;
pub const SCALE_STEP: f64 = 0.05;

/// The same scale presets hyprmon offers, including its DPI-friendly values.
pub const COMMON_SCALES: [f64; 15] = [
    0.50, 0.75, 0.90, 1.00, 1.10, 1.25, 1.33, 1.50, 1.66, 1.75, 2.00, 2.25, 2.50, 2.75, 3.00,
];

/// A value in a profile or state file that cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidValue(pub String);

impl fmt::Display for InvalidValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidValue {}

fn invalid(what: &str, value: &Value) -> InvalidValue {
    InvalidValue(format!("invalid {what} {}", py_repr(value)))
}

/// A missing key, reported like Python's `KeyError`.
fn missing(key: &str) -> InvalidValue {
    InvalidValue(crate::text::repr(key))
}

/// Python's `repr()` of a float: the shortest round-trip digits, with an
/// exponent below 1e-4 and from 1e16 (`1e-05`, `1e+16`), `.0` otherwise.
pub fn py_float_repr(value: f64) -> String {
    if !value.is_finite() {
        return if value.is_nan() {
            "nan".to_string()
        } else if value > 0.0 {
            "inf".to_string()
        } else {
            "-inf".to_string()
        };
    }
    let text = format!("{value:?}");
    match text.split_once('e') {
        Some((mantissa, exponent)) => {
            let (sign, digits) = match exponent.strip_prefix('-') {
                Some(digits) => ('-', digits),
                None => ('+', exponent),
            };
            format!("{mantissa}e{sign}{digits:0>2}")
        }
        None => text,
    }
}

/// Python's `repr()` of a decoded JSON value, used in messages.
pub fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Number(number) if number.is_f64() => py_float_repr(number.as_f64().unwrap_or(0.0)),
        Value::Number(number) => number.to_string(),
        Value::String(text) => crate::text::repr(text),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(py_repr).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(key, value)| format!("{}: {}", crate::text::repr(key), py_repr(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

// ----------------------------------------------------------------------
// Number formatting (matches what the Python version printed)
// ----------------------------------------------------------------------

/// Round to `digits` decimals, correctly rounded with ties to even (like
/// Python's `round(value, digits)`).
pub fn round_to(value: f64, digits: usize) -> f64 {
    if !value.is_finite() {
        return value;
    }
    format!("{value:.digits$}").parse().unwrap_or(value)
}

fn strip_zeros(text: String) -> String {
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    }
}

/// Python's `format(value, "g")`: 6 significant digits, no trailing zeros.
pub fn fmt_g(value: f64) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() { "-0" } else { "0" }.to_string();
    }
    if !value.is_finite() {
        return if value.is_nan() {
            "nan".to_string()
        } else if value > 0.0 {
            "inf".to_string()
        } else {
            "-inf".to_string()
        };
    }
    let sci = format!("{value:.5e}");
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    if (-4..6).contains(&exponent) {
        let decimals = (5 - exponent).max(0) as usize;
        strip_zeros(format!("{value:.decimals$}"))
    } else {
        let mantissa = strip_zeros(mantissa.to_string());
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", exponent.abs())
    }
}

/// Format a float without trailing zeros (`1.0` → `1`), six decimals at most.
pub fn format_number(value: f64) -> String {
    let text = strip_zeros(format!("{value:.6}"));
    if text.is_empty() || text == "-0" {
        "0".to_string()
    } else {
        text
    }
}

/// Clamp a scale factor to the range wlr-randr handles well.
pub fn clamp_scale(value: f64) -> f64 {
    let value = if value.is_nan() { 1.0 } else { value };
    round_to(value.clamp(SCALE_MIN, SCALE_MAX), 2)
}

/// Move a scale factor by `steps` increments of 0.05.
pub fn step_scale(value: f64, steps: i32) -> f64 {
    clamp_scale(value + SCALE_STEP * f64::from(steps))
}

/// Format a refresh rate for display.
pub fn format_refresh(value: f64) -> String {
    if (value - value.round_ties_even()).abs() < 0.005 {
        format!("{value:.0}")
    } else {
        strip_zeros(format!("{value:.2}"))
    }
}

/// Format a refresh rate for the wlr-randr command line.
pub fn refresh_arg(value: f64) -> String {
    format_number(value)
}

// ----------------------------------------------------------------------
// Strict readers for profile / state JSON (hand-editable, so untrusted)
// ----------------------------------------------------------------------

#[derive(Default, Clone, Copy)]
struct Bounds {
    positive: bool,
    non_negative: bool,
}

/// A finite number. Strings holding a number are accepted, booleans are not.
fn number(value: &Value, what: &str, bounds: Bounds) -> Result<f64, InvalidValue> {
    let parsed = match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    };
    match parsed {
        Some(number)
            if number.is_finite()
                && !(bounds.positive && number <= 0.0)
                && !(bounds.non_negative && number < 0.0) =>
        {
            Ok(number)
        }
        _ => Err(invalid(what, value)),
    }
}

/// Larger integers are rejected: positions and sizes stay far away from
/// overflowing the layout arithmetic.
const MAX_INTEGER: u64 = 9_000_000_000_000_000;

fn integer(value: &Value, what: &str, positive: bool) -> Result<i64, InvalidValue> {
    if let Some(exact) = value.as_i64() {
        if (positive && exact <= 0) || exact.unsigned_abs() > MAX_INTEGER {
            return Err(invalid(what, value));
        }
        return Ok(exact);
    }
    let parsed = number(
        value,
        what,
        Bounds {
            positive,
            non_negative: false,
        },
    )?;
    if parsed.fract() != 0.0 || parsed.abs() > MAX_INTEGER as f64 {
        return Err(invalid(what, value));
    }
    Ok(parsed as i64)
}

/// A JSON boolean; 0/1 are accepted, strings such as "false" are not.
fn flag(value: &Value, what: &str) -> Result<bool, InvalidValue> {
    match value {
        Value::Bool(flag) => Ok(*flag),
        Value::Number(number) => match number.as_u64() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(invalid(what, value)),
        },
        _ => Err(invalid(what, value)),
    }
}

/// Python truthiness of a JSON value.
pub(crate) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// `str(value or "")` for loosely typed JSON fields.
pub(crate) fn loose_str(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(other) if truthy(other) => other.to_string(),
        _ => String::new(),
    }
}

/// `int(value or 0)` for loosely typed JSON fields (wlr-randr output).
fn loose_int(value: Option<&Value>, what: &str) -> Result<i64, String> {
    let Some(value) = value.filter(|value| truthy(value)) else {
        return Ok(0);
    };
    let parsed = match value {
        Value::Bool(flag) => Some(i64::from(*flag)),
        Value::Number(number) => number.as_i64().or_else(|| {
            number
                .as_f64()
                .filter(|n| n.is_finite() && n.abs() <= MAX_INTEGER as f64)
                .map(|n| n.trunc() as i64)
        }),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    };
    // Bounded like the strict reader, so layout arithmetic cannot overflow.
    parsed
        .filter(|number| number.unsigned_abs() <= MAX_INTEGER)
        .ok_or_else(|| format!("invalid {what} {}", py_repr(value)))
}

fn loose_float(value: Option<&Value>, default: f64, what: &str) -> Result<f64, String> {
    let Some(value) = value.filter(|value| truthy(value)) else {
        return Ok(default);
    };
    let parsed = match value {
        Value::Bool(flag) => Some(f64::from(u8::from(*flag))),
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    };
    parsed
        .filter(|n| n.is_finite())
        .ok_or_else(|| format!("invalid {what} {value}"))
}

fn loose_bool(value: Option<&Value>, default: bool) -> bool {
    value.map_or(default, truthy)
}

/// True if applying `states` would leave no output enabled.
///
/// `current` maps every connected output name to its enabled state now;
/// outputs without a state keep that value.
pub fn disables_everything(current: &[(String, bool)], states: &[OutputState]) -> bool {
    if current.is_empty() && states.is_empty() {
        return false;
    }
    let mut result: HashMap<&str, bool> = current
        .iter()
        .map(|(name, enabled)| (name.as_str(), *enabled))
        .collect();
    for state in states {
        result.insert(state.name.as_str(), state.enabled);
    }
    !result.values().any(|enabled| *enabled)
}

/// The saved state for an output: by connector name, else by EDID id.
///
/// The EDID fallback (for connectors renamed after a replug) is only used
/// when it is unambiguous: exactly one saved state and exactly one connected
/// output carry that id. Identical monitors often share an EDID serial, and
/// guessing would give both the same settings.
pub fn match_state<'a>(
    saved: &'a [OutputState],
    name: &str,
    hardware_id: &str,
    connected_hardware: &[String],
) -> Option<&'a OutputState> {
    if let Some(state) = saved.iter().find(|state| state.name == name) {
        return Some(state);
    }
    if hardware_id.is_empty() {
        return None;
    }
    if connected_hardware
        .iter()
        .filter(|item| *item == hardware_id)
        .count()
        != 1
    {
        return None;
    }
    let mut candidates = saved
        .iter()
        .filter(|state| state.hardware_id == hardware_id);
    match (candidates.next(), candidates.next()) {
        (Some(state), None) => Some(state),
        _ => None,
    }
}

/// Comparable mode identity that ignores the current/preferred flags:
/// width, height and the refresh rate in millihertz.
pub type ModeKey = (i64, i64, i64);

/// A display mode reported by wlr-randr.
#[derive(Debug, Clone, PartialEq)]
pub struct Mode {
    pub width: i64,
    pub height: i64,
    pub refresh: f64,
    pub preferred: bool,
    pub current: bool,
}

impl Mode {
    pub fn new(width: i64, height: i64, refresh: f64) -> Self {
        Self {
            width,
            height,
            refresh,
            preferred: false,
            current: false,
        }
    }

    pub fn size_text(&self) -> String {
        format!("{}x{}", self.width, self.height)
    }

    pub fn label(&self) -> String {
        format!("{} @ {} Hz", self.size_text(), format_refresh(self.refresh))
    }

    /// The `--mode` value for wlr-randr. Virtual outputs report a refresh
    /// rate of 0, which wlr-randr rejects, so it is left out for them.
    pub fn cli_value(&self) -> String {
        if self.refresh > 0.0 {
            format!("{}@{}", self.size_text(), refresh_arg(self.refresh))
        } else {
            self.size_text()
        }
    }

    pub fn key(&self) -> ModeKey {
        let millihertz = (round_to(self.refresh, 3) * 1000.0).round();
        (self.width, self.height, millihertz as i64)
    }

    pub fn to_dict(&self) -> Value {
        json!({
            "width": self.width,
            "height": self.height,
            "refresh": round_to(self.refresh, 6),
        })
    }

    /// Strict reader for profile / state files.
    pub fn from_dict(data: &Map<String, Value>) -> Result<Self, InvalidValue> {
        let field = |key: &str| data.get(key).ok_or_else(|| missing(key));
        Ok(Self::new(
            integer(field("width")?, "mode width", true)?,
            integer(field("height")?, "mode height", true)?,
            // Virtual/headless outputs may report a refresh rate of 0.
            number(
                field("refresh")?,
                "refresh rate",
                Bounds {
                    positive: false,
                    non_negative: true,
                },
            )?,
        ))
    }

    /// Lenient reader for wlr-randr's JSON.
    pub fn from_json(data: &Value) -> Result<Self, String> {
        let object = data
            .as_object()
            .ok_or_else(|| format!("invalid mode {data}"))?;
        let required = |key: &str| {
            object
                .get(key)
                .ok_or_else(|| format!("mode is missing {key:?}"))
        };
        let width = required("width")?;
        let height = required("height")?;
        let refresh = required("refresh")?;
        Ok(Self {
            width: loose_int(Some(width), "mode width")?,
            height: loose_int(Some(height), "mode height")?,
            refresh: loose_float(Some(refresh), 0.0, "refresh rate")?,
            preferred: loose_bool(object.get("preferred"), false),
            current: loose_bool(object.get("current"), false),
        })
    }
}

/// A wlroots output and its current settings.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub name: String,
    pub description: String,
    pub make: String,
    pub model: String,
    pub serial: String,
    pub physical_width: i64,
    pub physical_height: i64,
    pub enabled: bool,
    pub modes: Vec<Mode>,
    pub mode: Option<Mode>,
    pub x: i64,
    pub y: i64,
    pub transform: String,
    pub scale: f64,
    pub adaptive_sync: bool,
    // Compositor-specific settings. `None` means "unknown / not managed",
    // so nothing is sent to the compositor for that setting.
    pub hdr: Option<bool>,
    pub bit_depth: Option<i64>,
    pub color_profile: Option<String>,
}

impl Output {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            make: String::new(),
            model: String::new(),
            serial: String::new(),
            physical_width: 0,
            physical_height: 0,
            enabled: true,
            modes: Vec::new(),
            mode: None,
            x: 0,
            y: 0,
            transform: "normal".to_string(),
            scale: 1.0,
            adaptive_sync: false,
            hdr: None,
            bit_depth: None,
            color_profile: None,
        }
    }

    pub fn title(&self) -> String {
        let maker_model = [self.make.as_str(), self.model.as_str()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if !maker_model.is_empty() {
            maker_model
        } else if !self.description.is_empty() {
            self.description.clone()
        } else {
            self.name.clone()
        }
    }

    /// A best-effort stable identity built from EDID fields.
    pub fn hardware_id(&self) -> String {
        [
            self.make.as_str(),
            self.model.as_str(),
            self.serial.as_str(),
        ]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/")
    }

    /// `make model serial` as used by sway and niri output matching.
    ///
    /// Empty unless all three EDID fields are known, because a partial
    /// identifier would not match anything in those compositors.
    pub fn identifier(&self) -> String {
        if !self.make.is_empty() && !self.model.is_empty() && !self.serial.is_empty() {
            format!("{} {} {}", self.make, self.model, self.serial)
        } else {
            String::new()
        }
    }

    pub fn is_rotated(&self) -> bool {
        ROTATED_TRANSFORMS.contains(&self.transform.as_str())
    }

    fn logical_size(&self) -> (i64, i64) {
        let Some(mode) = &self.mode else {
            return (0, 0);
        };
        let scale = self.scale.max(0.01);
        let mut width = mode.width as f64 / scale;
        let mut height = mode.height as f64 / scale;
        if self.is_rotated() {
            std::mem::swap(&mut width, &mut height);
        }
        (
            (width.round_ties_even() as i64).max(1),
            (height.round_ties_even() as i64).max(1),
        )
    }

    pub fn logical_width(&self) -> i64 {
        self.logical_size().0
    }

    pub fn logical_height(&self) -> i64 {
        self.logical_size().1
    }

    pub fn state(&self) -> OutputState {
        OutputState {
            name: self.name.clone(),
            enabled: self.enabled,
            mode: self.mode.clone(),
            x: self.x,
            y: self.y,
            scale: self.scale,
            transform: self.transform.clone(),
            adaptive_sync: self.adaptive_sync,
            hardware_id: self.hardware_id(),
            description: self.description.clone(),
            hdr: self.hdr,
            bit_depth: self.bit_depth,
            color_profile: self.color_profile.clone(),
        }
    }

    /// Unique modes in a stable, useful order: current first, then preferred,
    /// then by size and refresh rate.
    pub fn mode_options(&self) -> Vec<Mode> {
        let mut sorted = self.modes.clone();
        sorted.sort_by(|a, b| {
            (!a.current)
                .cmp(&!b.current)
                .then((!a.preferred).cmp(&!b.preferred))
                .then(
                    (i128::from(b.width) * i128::from(b.height))
                        .cmp(&(i128::from(a.width) * i128::from(a.height))),
                )
                .then(b.refresh.total_cmp(&a.refresh))
                .then(a.width.cmp(&b.width))
        });
        let mut seen = HashSet::new();
        sorted.retain(|mode| seen.insert(mode.key()));
        sorted
    }

    /// Lenient reader for one entry of `wlr-randr --json`.
    pub fn from_json(data: &Value) -> Result<Self, String> {
        let object = data
            .as_object()
            .ok_or_else(|| format!("invalid output {data}"))?;
        let name = match object.get("name") {
            Some(Value::String(name)) => name.clone(),
            Some(Value::Null) | None => return Err("an output has no name".to_string()),
            Some(other) => other.to_string(),
        };
        let modes = match object.get("modes") {
            Some(Value::Array(items)) => items
                .iter()
                .map(Mode::from_json)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("{name}: {error}"))?,
            _ => Vec::new(),
        };
        let mode = modes
            .iter()
            .find(|mode| mode.current)
            .or_else(|| modes.iter().find(|mode| mode.preferred))
            .or_else(|| modes.first())
            .cloned();
        let empty = Map::new();
        let physical = object
            .get("physical_size")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let position = object
            .get("position")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let context = |error: String| format!("{name}: {error}");
        let transform = loose_str(object.get("transform"));
        Ok(Self {
            description: loose_str(object.get("description")),
            make: loose_str(object.get("make")),
            model: loose_str(object.get("model")),
            serial: loose_str(object.get("serial")),
            physical_width: loose_int(physical.get("width"), "width").map_err(context)?,
            physical_height: loose_int(physical.get("height"), "height").map_err(context)?,
            enabled: loose_bool(object.get("enabled"), true),
            modes,
            mode,
            x: loose_int(position.get("x"), "x position").map_err(context)?,
            y: loose_int(position.get("y"), "y position").map_err(context)?,
            transform: if transform.is_empty() {
                "normal".to_string()
            } else {
                transform
            },
            scale: loose_float(object.get("scale"), 1.0, "scale").map_err(context)?,
            adaptive_sync: loose_bool(object.get("adaptive_sync"), false),
            hdr: None,
            bit_depth: None,
            color_profile: None,
            name,
        })
    }
}

/// The settings that can be applied to an output.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputState {
    pub name: String,
    pub enabled: bool,
    pub mode: Option<Mode>,
    pub x: i64,
    pub y: i64,
    pub scale: f64,
    pub transform: String,
    pub adaptive_sync: bool,
    pub hardware_id: String,
    pub description: String,
    pub hdr: Option<bool>,
    pub bit_depth: Option<i64>,
    pub color_profile: Option<String>,
}

impl OutputState {
    /// Compositor-specific settings that are set (not `None`).
    pub fn extras(&self) -> Map<String, Value> {
        let mut data = Map::new();
        if let Some(hdr) = self.hdr {
            data.insert("hdr".into(), Value::Bool(hdr));
        }
        if let Some(depth) = self.bit_depth {
            data.insert("bit_depth".into(), Value::from(depth));
        }
        if let Some(profile) = &self.color_profile {
            data.insert("color_profile".into(), Value::String(profile.clone()));
        }
        data
    }

    pub fn to_dict(&self) -> Map<String, Value> {
        let mut data = Map::new();
        data.insert("enabled".into(), Value::Bool(self.enabled));
        data.insert(
            "mode".into(),
            self.mode.as_ref().map_or(Value::Null, Mode::to_dict),
        );
        data.insert("position".into(), json!({"x": self.x, "y": self.y}));
        data.insert("scale".into(), json!(round_to(self.scale, 6)));
        data.insert("transform".into(), Value::String(self.transform.clone()));
        data.insert("adaptive_sync".into(), Value::Bool(self.adaptive_sync));
        data.extend(self.extras());
        if !self.hardware_id.is_empty() {
            data.insert(
                "hardware_id".into(),
                Value::String(self.hardware_id.clone()),
            );
        }
        if !self.description.is_empty() {
            data.insert(
                "description".into(),
                Value::String(self.description.clone()),
            );
        }
        data
    }

    /// Strict reader for profile / state files.
    pub fn from_dict(name: &str, data: &Value) -> Result<Self, InvalidValue> {
        let data = data
            .as_object()
            .ok_or_else(|| invalid("output settings", data))?;
        let empty = Map::new();
        let position = match data.get("position") {
            Some(value) if truthy(value) => value
                .as_object()
                .ok_or_else(|| invalid("position", value))?,
            _ => &empty,
        };
        let transform = match data.get("transform") {
            None => "normal".to_string(),
            // Profiles are plain JSON; never pass unknown strings on to the
            // compositor (they end up in commands and config files).
            Some(Value::String(text)) if TRANSFORMS.contains(&text.as_str()) => text.clone(),
            Some(other) => return Err(invalid("transform", other)),
        };
        let color_profile = match data.get("color_profile") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) => Some(text.clone()),
            Some(other) => return Err(invalid("color profile", other)),
        };
        let coordinate = |key: &str, what: &str| -> Result<i64, InvalidValue> {
            let value = position.get(key).or_else(|| data.get(key));
            match value {
                Some(value) if truthy(value) => integer(value, what, false),
                _ => Ok(0),
            }
        };
        let optional = |key: &str| data.get(key).filter(|value| !value.is_null());
        let mode = match data.get("mode") {
            Some(Value::Object(mode)) => Some(Mode::from_dict(mode)?),
            _ => None,
        };
        Ok(Self {
            name: name.to_string(),
            enabled: flag(data.get("enabled").unwrap_or(&Value::Bool(true)), "enabled")?,
            mode,
            x: coordinate("x", "x position")?,
            y: coordinate("y", "y position")?,
            scale: number(
                data.get("scale").unwrap_or(&json!(1.0)),
                "scale",
                Bounds {
                    positive: true,
                    non_negative: false,
                },
            )?,
            transform,
            adaptive_sync: flag(
                data.get("adaptive_sync").unwrap_or(&Value::Bool(false)),
                "adaptive_sync",
            )?,
            hardware_id: loose_str(data.get("hardware_id")),
            description: loose_str(data.get("description")),
            hdr: optional("hdr")
                .map(|value| flag(value, "hdr"))
                .transpose()?,
            bit_depth: optional("bit_depth")
                .map(|value| integer(value, "bit depth", true))
                .transpose()?,
            color_profile,
        })
    }

    /// Compare everything that affects the on-screen layout.
    pub fn matches_config(&self, other: &OutputState) -> bool {
        self.enabled == other.enabled
            && self.mode.as_ref().map(Mode::key) == other.mode.as_ref().map(Mode::key)
            && (self.x, self.y) == (other.x, other.y)
            && (self.scale - other.scale).abs() < 0.0001
            && self.transform == other.transform
            && self.adaptive_sync == other.adaptive_sync
            && same_if_known(&self.hdr, &other.hdr)
            && same_if_known(&self.bit_depth, &other.bit_depth)
            && same_if_known(&self.color_profile, &other.color_profile)
    }

    /// Copy this state onto an output.
    ///
    /// Compositor-specific settings are only copied when this state manages
    /// them, so older profiles do not reset HDR or color settings. With
    /// `restore` they are copied verbatim (used to undo edits).
    pub fn apply_to(&self, output: &mut Output, restore: bool) {
        output.enabled = self.enabled;
        output.x = self.x;
        output.y = self.y;
        output.scale = self.scale;
        output.transform = self.transform.clone();
        output.adaptive_sync = self.adaptive_sync;
        if restore || self.hdr.is_some() {
            output.hdr = self.hdr;
        }
        if restore || self.bit_depth.is_some() {
            output.bit_depth = self.bit_depth;
        }
        if restore || self.color_profile.is_some() {
            output.color_profile = self.color_profile.clone();
        }
        if let Some(mode) = &self.mode {
            let matching = output
                .modes
                .iter()
                .find(|item| item.key() == mode.key())
                .cloned();
            output.mode = Some(matching.unwrap_or_else(|| mode.clone()));
        }
    }
}

/// Compare optional settings, treating `None` (unknown) as a wildcard.
fn same_if_known<T: PartialEq>(first: &Option<T>, second: &Option<T>) -> bool {
    match (first, second) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}

/// The scale presets, plus `current` when it is not one of them.
pub fn common_scales(current: Option<f64>) -> Vec<f64> {
    let mut scales = COMMON_SCALES.to_vec();
    if let Some(current) = current
        && current.is_finite()
        && scales.iter().all(|item| (current - item).abs() > 0.0001)
    {
        scales.push(round_to(current, 3));
    }
    scales.sort_by(f64::total_cmp);
    scales.dedup();
    scales
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const FIXTURE: &str = include_str!("../tests/fixtures/outputs.json");

    pub(crate) fn load_outputs() -> Vec<Output> {
        let data: Value = serde_json::from_str(FIXTURE).unwrap();
        data.as_array()
            .unwrap()
            .iter()
            .map(|item| Output::from_json(item).unwrap())
            .collect()
    }

    fn with(base: &Map<String, Value>, key: &str, value: Value) -> Value {
        let mut data = base.clone();
        data.insert(key.to_string(), value);
        Value::Object(data)
    }

    #[test]
    fn parse_outputs() {
        let outputs = load_outputs();
        let (hdmi, edp) = (&outputs[0], &outputs[1]);
        assert_eq!(hdmi.name, "HDMI-A-1");
        assert!(hdmi.enabled);
        let mode = hdmi.mode.as_ref().unwrap();
        assert_eq!(mode.size_text(), "1920x1080");
        assert_eq!(mode.refresh, 100.0);
        assert_eq!(hdmi.x, 0);
        assert_eq!(edp.scale, 1.2);
        assert!(edp.adaptive_sync);
        assert_eq!(edp.logical_width(), 1600);
        assert_eq!(edp.logical_height(), 900);
    }

    #[test]
    fn malformed_wlr_randr_entries_are_errors_not_panics() {
        assert!(Output::from_json(&json!({"modes": []})).is_err());
        assert!(Output::from_json(&json!("HDMI-A-1")).is_err());
        assert!(Output::from_json(&json!({"name": "X", "modes": [{"width": 1}]})).is_err());
        let output = Output::from_json(&json!({"name": "X", "position": [], "scale": 0})).unwrap();
        assert_eq!(
            (output.x, output.scale, output.transform.as_str()),
            (0, 1.0, "normal")
        );
        // Absurd numbers are refused instead of overflowing the layout maths.
        let far = json!({"name": "X", "position": {"x": i64::MAX}});
        assert!(Output::from_json(&far).is_err());
        let side = 4_294_967_296_i64;
        let huge = json!({"name": "X", "modes": [{"width": side, "height": side, "refresh": 60}]});
        let mut big = Output::from_json(&huge).unwrap();
        big.modes.push(Mode::new(1, 1, 60.0));
        assert_eq!(big.mode_options()[1].width, 1);
    }

    #[test]
    fn hardware_id_uses_edid_fields() {
        let hdmi = &load_outputs()[0];
        assert_eq!(hdmi.hardware_id(), "Microstep/MSI MP275/PC3M394200857");
        assert!(hdmi.description.contains("Microstep"));
    }

    #[test]
    fn rotated_logical_size() {
        let mut output = load_outputs().remove(1);
        output.transform = "90".into();
        assert_eq!(output.logical_width(), 900);
        assert_eq!(output.logical_height(), 1600);
    }

    #[test]
    fn mode_labels_and_cli_values() {
        let mode = Mode::new(1920, 1080, 59.939999);
        assert_eq!(mode.label(), "1920x1080 @ 59.94 Hz");
        assert_eq!(mode.cli_value(), "1920x1080@59.939999");
        assert_eq!(Mode::new(1920, 1080, 0.0).cli_value(), "1920x1080");
        assert_eq!(format_refresh(100.0), "100");
        assert_eq!(refresh_arg(100.0), "100");
        assert_eq!(format_refresh(59.996), "60");
        assert_eq!(format_refresh(143.912), "143.91");
    }

    #[test]
    fn number_formatting_matches_python() {
        assert_eq!(fmt_g(1.0), "1");
        assert_eq!(fmt_g(1.25), "1.25");
        assert_eq!(fmt_g(0.5), "0.5");
        assert_eq!(fmt_g(1.333333333), "1.33333");
        assert_eq!(fmt_g(1234567.0), "1.23457e+06");
        assert_eq!(fmt_g(0.00001), "1e-05");
        assert_eq!(format_number(1.0), "1");
        assert_eq!(format_number(1.25), "1.25");
        assert_eq!(format_number(0.0), "0");
        assert_eq!(round_to(2.675, 2), 2.67);
        assert_eq!(round_to(1.234, 2), 1.23);
    }

    #[test]
    fn messages_quote_values_like_python() {
        assert_eq!(py_repr(&json!("bogus")), "'bogus'");
        assert_eq!(py_repr(&json!(true)), "True");
        assert_eq!(py_repr(&Value::Null), "None");
        assert_eq!(py_repr(&json!(1.5)), "1.5");
        assert_eq!(py_repr(&json!(100.0)), "100.0");
        assert_eq!(py_repr(&json!(1e16)), "1e+16");
        assert_eq!(py_repr(&json!(0.00001)), "1e-05");
        assert_eq!(py_repr(&json!(7)), "7");
        assert_eq!(py_repr(&json!([1, "a"])), "[1, 'a']");
        assert_eq!(py_repr(&json!({"x": false})), "{'x': False}");
        let error = OutputState::from_dict("X", &json!({"transform": "bogus"})).unwrap_err();
        assert_eq!(error.0, "invalid transform 'bogus'");
        let error = OutputState::from_dict("X", &json!({"mode": {"height": 1}})).unwrap_err();
        assert_eq!(error.0, "'width'");
    }

    #[test]
    fn mode_options_prefers_current_then_preferred() {
        let mut output = load_outputs().remove(0);
        output.modes.push(Mode::new(1920, 1080, 100.0));
        let modes = output.mode_options();
        assert_eq!(modes[0].refresh, 100.0);
        let keys: HashSet<_> = modes.iter().map(Mode::key).collect();
        assert_eq!(keys.len(), modes.len());
    }

    #[test]
    fn state_round_trip() {
        let output = load_outputs().remove(0);
        let state = output.state();
        let raw = Value::Object(state.to_dict());
        let restored = OutputState::from_dict(&output.name, &raw).unwrap();
        assert_eq!(Value::Object(restored.to_dict()), raw);
        assert_eq!(restored.mode.unwrap().key(), state.mode.unwrap().key());
        assert_eq!(restored.hardware_id, output.hardware_id());
    }

    #[test]
    fn state_apply_to_output_uses_matching_mode() {
        let mut output = load_outputs().remove(0);
        let mut state = output.state();
        state.mode = Some(Mode::new(1280, 720, 60.0));
        state.x = 100;
        state.scale = 1.5;
        state.apply_to(&mut output, false);
        assert_eq!(output.x, 100);
        assert_eq!(output.scale, 1.5);
        assert_eq!(output.mode.unwrap().key(), (1280, 720, 60000));
    }

    #[test]
    fn state_matches_config_ignores_identity_fields() {
        let output = load_outputs().remove(0);
        let first = output.state();
        let mut second = output.state();
        second.hardware_id = "different".into();
        second.description = "different".into();
        assert!(first.matches_config(&second));
        second.x += 1;
        assert!(!first.matches_config(&second));
    }

    #[test]
    fn common_scales_match_hyprmon_presets() {
        assert_eq!(common_scales(None), COMMON_SCALES.to_vec());
        let scales = common_scales(Some(1.33));
        assert!(scales.contains(&1.33) && scales.contains(&1.0));
        assert_eq!(scales.len(), COMMON_SCALES.len());
        let custom = common_scales(Some(1.4));
        assert_eq!(custom.len(), COMMON_SCALES.len() + 1);
        assert!(custom.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn clamp_and_step_scale() {
        assert_eq!(clamp_scale(0.1), SCALE_MIN);
        assert_eq!(clamp_scale(9.0), SCALE_MAX);
        assert_eq!(clamp_scale(1.234), 1.23);
        assert_eq!(step_scale(1.0, 1), 1.05);
        assert_eq!(step_scale(2.98, 1), SCALE_MAX);
        assert_eq!(step_scale(0.52, -1), SCALE_MIN);
    }

    #[test]
    fn compositor_settings_are_optional_in_state_dicts() {
        let mut output = load_outputs().remove(0);
        let raw = output.state().to_dict();
        assert!(
            !raw.contains_key("hdr")
                && !raw.contains_key("bit_depth")
                && !raw.contains_key("color_profile")
        );
        output.hdr = Some(false);
        output.bit_depth = Some(10);
        output.color_profile = Some("icc:/x.icc".into());
        let raw = output.state().to_dict();
        assert_eq!(raw["hdr"], json!(false));
        assert_eq!(raw["bit_depth"], json!(10));
        let restored = OutputState::from_dict(&output.name, &Value::Object(raw)).unwrap();
        assert_eq!(
            Value::Object(restored.extras()),
            json!({"hdr": false, "bit_depth": 10, "color_profile": "icc:/x.icc"})
        );
    }

    #[test]
    fn apply_to_keeps_unmanaged_settings_unless_restoring() {
        let mut output = load_outputs().remove(0);
        output.hdr = Some(true);
        let old = OutputState::from_dict(&output.name, &json!({"scale": 2.0})).unwrap();
        old.apply_to(&mut output, false);
        assert_eq!(output.scale, 2.0);
        assert_eq!(output.hdr, Some(true));
        old.apply_to(&mut output, true);
        assert_eq!(output.hdr, None);
    }

    #[test]
    fn identifier_needs_all_edid_fields() {
        let outputs = load_outputs();
        assert_eq!(outputs[0].identifier(), "Microstep MSI MP275 PC3M394200857");
        assert_eq!(outputs[1].identifier(), "");
    }

    #[test]
    fn state_values_are_validated_strictly() {
        let base = load_outputs()[0].state().to_dict();
        let bad = [
            ("scale", json!("nan")),
            ("scale", json!("inf")),
            ("scale", json!(0)),
            ("scale", Value::Null),
            ("position", json!({"x": "inf", "y": 0})),
            ("position", json!({"x": 1.5, "y": 0})),
            ("position", json!("0,0")),
            ("hdr", json!("false")),
            ("enabled", json!("yes")),
            ("enabled", Value::Null),
            ("adaptive_sync", json!("off")),
            ("bit_depth", json!(true)),
            ("bit_depth", json!(0)),
            ("color_profile", json!(8)),
            ("transform", json!("upside-down")),
            ("transform", Value::Null),
            (
                "mode",
                json!({"width": 1920, "height": 1080, "refresh": "nan"}),
            ),
            ("mode", json!({"width": 0, "height": 1080, "refresh": 60})),
            ("mode", json!({"width": 1920, "height": 1080})),
            ("position", json!({"x": i64::MAX, "y": 0})),
            ("position", json!({"x": 0, "y": i64::MIN})),
            ("position", json!({"x": u64::MAX, "y": 0})),
            ("bit_depth", json!(9_100_000_000_000_000_i64)),
        ];
        for (key, value) in bad {
            let data = with(&base, key, value.clone());
            assert!(
                OutputState::from_dict("HDMI-A-1", &data).is_err(),
                "{key}={value} was accepted"
            );
        }

        // JSON integers 0/1 are fine for flags, and virtual outputs may have a
        // refresh rate of 0.
        let mut data = base.clone();
        data.insert("hdr".into(), json!(1));
        data.insert("enabled".into(), json!(1));
        data.insert(
            "mode".into(),
            json!({"width": 1920, "height": 1080, "refresh": 0}),
        );
        let state = OutputState::from_dict("HEADLESS-1", &Value::Object(data)).unwrap();
        assert_eq!(state.hdr, Some(true));
        assert!(state.enabled);
    }

    #[test]
    fn match_state_never_guesses_between_twins() {
        let outputs = load_outputs();
        let shared = outputs[0].hardware_id();
        let mut first = outputs[0].state();
        let mut second = outputs[1].state();
        first.name = "DP-1".into();
        second.name = "DP-2".into();
        second.hardware_id = shared.clone(); // identical monitors
        let saved = vec![first.clone(), second.clone()];
        let only_first = vec![first];

        // Unique on both sides: the renamed connector is found by EDID.
        assert!(match_state(&only_first, "DP-9", &shared, std::slice::from_ref(&shared)).is_some());
        // Two saved twins: no guess.
        assert!(match_state(&saved, "DP-9", &shared, std::slice::from_ref(&shared)).is_none());
        // Two connected twins: no guess either.
        assert!(
            match_state(
                &only_first,
                "DP-9",
                &shared,
                &[shared.clone(), shared.clone()]
            )
            .is_none()
        );
        // The connector name always wins.
        assert_eq!(
            match_state(&saved, "DP-2", &shared, &[shared.clone(), shared.clone()]),
            Some(&second)
        );
    }

    #[test]
    fn disables_everything_checks_the_result() {
        let outputs = load_outputs();
        let mut state = outputs[0].state();
        state.enabled = false;
        let on = |name: &str, enabled: bool| (name.to_string(), enabled);
        assert!(disables_everything(
            &[on("HDMI-A-1", true)],
            &[state.clone()]
        ));
        assert!(!disables_everything(
            &[on("HDMI-A-1", true), on("eDP-1", true)],
            &[state.clone()]
        ));
        let other = outputs[1].state();
        assert!(!disables_everything(
            &[on("HDMI-A-1", true), on("eDP-1", false)],
            &[state, other]
        ));
        assert!(!disables_everything(&[], &[]));
    }
}
