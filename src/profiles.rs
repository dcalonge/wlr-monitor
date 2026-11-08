//! Named monitor-layout profiles stored as JSON.

use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::model::{OutputState, loose_str, match_state};
use crate::paths;
use crate::text::{is_printable, repr};

pub const SCHEMA_VERSION: i64 = 2;

/// Names the profile browser uses for its own entries.
pub const RESERVED_NAMES: [&str; 2] = ["__save__", "__launch__"];

/// A profile file cannot be read or written, or a name is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileError(pub String);

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProfileError {}

/// Check and clean up a profile name.
pub fn validate_name(name: &str) -> Result<String, ProfileError> {
    let clean = name.trim();
    if clean.is_empty() {
        return Err(ProfileError("Profile name must not be empty.".into()));
    }
    if RESERVED_NAMES.contains(&clean) {
        return Err(ProfileError("That profile name is reserved.".into()));
    }
    // Names are written into compositor config comments, so no newlines.
    if clean
        .chars()
        .any(|c| c == '/' || c == '\\' || !is_printable(c))
    {
        return Err(ProfileError(
            "Profile names cannot contain slashes or control characters.".into(),
        ));
    }
    Ok(clean.to_string())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub name: String,
    /// Saved settings, one per output name, in insertion order.
    pub outputs: Vec<OutputState>,
    pub created_at: String,
    pub updated_at: String,
}

impl Profile {
    pub fn output(&self, name: &str) -> Option<&OutputState> {
        self.outputs.iter().find(|state| state.name == name)
    }

    pub fn to_dict(&self) -> Value {
        let outputs: Map<String, Value> = self
            .outputs
            .iter()
            .map(|state| (state.name.clone(), Value::Object(state.to_dict())))
            .collect();
        let mut data = Map::new();
        data.insert("outputs".into(), Value::Object(outputs));
        if !self.created_at.is_empty() {
            data.insert("created_at".into(), Value::String(self.created_at.clone()));
        }
        if !self.updated_at.is_empty() {
            data.insert("updated_at".into(), Value::String(self.updated_at.clone()));
        }
        Value::Object(data)
    }

    pub fn from_dict(name: &str, data: &Map<String, Value>) -> Result<Self, ProfileError> {
        let Some(Value::Object(raw_outputs)) = data.get("outputs") else {
            return Err(ProfileError(format!(
                "Profile {} has no outputs object.",
                repr(name)
            )));
        };
        let mut outputs = Vec::with_capacity(raw_outputs.len());
        for (output_name, raw_state) in raw_outputs {
            if !raw_state.is_object() {
                return Err(ProfileError(format!(
                    "Profile {} has an invalid output entry.",
                    repr(name)
                )));
            }
            let state = OutputState::from_dict(output_name, raw_state).map_err(|error| {
                ProfileError(format!(
                    "Profile {} has an invalid entry for {output_name}: {error}",
                    repr(name)
                ))
            })?;
            outputs.push(state);
        }
        Ok(Self {
            name: name.to_string(),
            outputs,
            created_at: loose_str(data.get("created_at")),
            updated_at: loose_str(data.get("updated_at")),
        })
    }
}

/// The parsed profile file: valid profiles, order and broken entries.
#[derive(Debug, Default)]
struct Snapshot {
    /// Valid profiles in file order.
    profiles: Vec<Profile>,
    order: Vec<String>,
    /// name, raw JSON value, error message; written back untouched.
    broken: Vec<(String, Value, String)>,
}

impl Snapshot {
    fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|profile| profile.name == name)
    }

    fn broken(&self, name: &str) -> Option<&str> {
        self.broken
            .iter()
            .find(|(broken, _, _)| broken == name)
            .map(|(_, _, message)| message.as_str())
    }

    fn contains(&self, name: &str) -> bool {
        self.profile(name).is_some() || self.broken(name).is_some()
    }

    /// Valid profile names: the stored order first, then the rest sorted.
    fn names(&self) -> Vec<String> {
        let mut result: Vec<String> = Vec::new();
        for name in &self.order {
            if self.profile(name).is_some() && !result.contains(name) {
                result.push(name.clone());
            }
        }
        let mut rest: Vec<String> = self
            .profiles
            .iter()
            .map(|profile| profile.name.clone())
            .filter(|name| !result.contains(name))
            .collect();
        rest.sort();
        result.extend(rest);
        result
    }
}

/// A small JSON file containing named profiles.
///
/// The file also keeps a display order so the profile browser can be
/// re-ordered, like hyprmon's profile menu.
///
/// A profile with invalid content (for example an unknown rotation value)
/// does not make the whole file unusable: it is left out of `load()` and
/// `names()`, reported by `problems()`, returns an error from `get()`, and
/// is written back unchanged so editing other profiles never loses it.
#[derive(Debug, Clone)]
pub struct ProfileStore {
    pub path: PathBuf,
}

impl Default for ProfileStore {
    fn default() -> Self {
        Self::new(None)
    }
}

impl ProfileStore {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path: path.unwrap_or_else(Self::default_path),
        }
    }

    pub fn default_path() -> PathBuf {
        paths::config_home()
            .join("wlr-monitor")
            .join("profiles.json")
    }

    /// Valid profiles in file order.
    pub fn load(&self) -> Result<Vec<Profile>, ProfileError> {
        Ok(self.read()?.profiles)
    }

    /// One message per profile that could not be read.
    pub fn problems(&self) -> Result<Vec<String>, ProfileError> {
        Ok(self
            .read()?
            .broken
            .into_iter()
            .map(|(_, _, message)| message)
            .collect())
    }

    /// True for any stored profile, including one that cannot be read.
    pub fn exists(&self, name: &str) -> Result<bool, ProfileError> {
        Ok(self.read()?.contains(name))
    }

    pub fn names(&self) -> Result<Vec<String>, ProfileError> {
        Ok(self.read()?.names())
    }

    pub fn get(&self, name: &str) -> Result<Option<Profile>, ProfileError> {
        let snapshot = self.read()?;
        if let Some(message) = snapshot.broken(name) {
            return Err(ProfileError(message.to_string()));
        }
        Ok(snapshot.profile(name).cloned())
    }

    pub fn save(&self, name: &str, states: &[OutputState]) -> Result<Profile, ProfileError> {
        let clean = validate_name(name)?;
        let mut snapshot = self.read()?;
        let now = paths::now_iso();
        let mut outputs: Vec<OutputState> = Vec::with_capacity(states.len());
        for state in states {
            match outputs.iter_mut().find(|item| item.name == state.name) {
                Some(existing) => *existing = state.clone(),
                None => outputs.push(state.clone()),
            }
        }
        let created_at = snapshot
            .profile(&clean)
            .map_or_else(|| now.clone(), |existing| existing.created_at.clone());
        let profile = Profile {
            name: clean.clone(),
            outputs,
            created_at,
            updated_at: now,
        };
        // Saving over a broken profile replaces it.
        snapshot.broken.retain(|(broken, _, _)| *broken != clean);
        match snapshot.profiles.iter_mut().find(|item| item.name == clean) {
            Some(existing) => *existing = profile.clone(),
            None => snapshot.profiles.push(profile.clone()),
        }
        if !snapshot.order.contains(&clean) {
            snapshot.order.push(clean);
        }
        self.write(&snapshot)?;
        Ok(profile)
    }

    pub fn delete(&self, name: &str) -> Result<bool, ProfileError> {
        let mut snapshot = self.read()?;
        if !snapshot.contains(name) {
            return Ok(false);
        }
        snapshot.profiles.retain(|profile| profile.name != name);
        snapshot.broken.retain(|(broken, _, _)| broken != name);
        snapshot.order.retain(|item| item != name);
        self.write(&snapshot)?;
        Ok(true)
    }

    pub fn rename(&self, old_name: &str, new_name: &str) -> Result<bool, ProfileError> {
        let clean = validate_name(new_name)?;
        let mut snapshot = self.read()?;
        if let Some(message) = snapshot.broken(old_name) {
            return Err(ProfileError(message.to_string()));
        }
        if snapshot.profile(old_name).is_none() {
            return Ok(false);
        }
        if clean == old_name {
            return Ok(true);
        }
        if snapshot.contains(&clean) {
            return Err(ProfileError(format!(
                "A profile named {} already exists.",
                repr(&clean)
            )));
        }
        let now = paths::now_iso();
        if let Some(profile) = snapshot
            .profiles
            .iter_mut()
            .find(|profile| profile.name == old_name)
        {
            profile.name = clean.clone();
            profile.updated_at = now;
        }
        for item in &mut snapshot.order {
            if item == old_name {
                *item = clean.clone();
            }
        }
        self.write(&snapshot)?;
        Ok(true)
    }

    /// Move a profile `offset` places in the display order.
    pub fn move_by(&self, name: &str, offset: i64) -> Result<bool, ProfileError> {
        let mut snapshot = self.read()?;
        if snapshot.profile(name).is_none() {
            return Ok(false);
        }
        let mut order = snapshot.names();
        let Some(index) = order.iter().position(|item| item == name) else {
            return Ok(false);
        };
        let last = order.len() as i64 - 1;
        let target = (index as i64 + offset).clamp(0, last.max(0)) as usize;
        if target == index {
            return Ok(false);
        }
        let item = order.remove(index);
        order.insert(target, item);
        // Broken profiles keep their place at the end of the order.
        let broken: Vec<String> = snapshot
            .order
            .iter()
            .filter(|item| snapshot.broken(item).is_some())
            .cloned()
            .collect();
        order.extend(broken);
        snapshot.order = order;
        self.write(&snapshot)?;
        Ok(true)
    }

    /// The name of the profile that matches these output states.
    ///
    /// Profiles are matched by connector name first and by EDID hardware id
    /// as a fallback, which keeps them usable when the kernel reassigns
    /// connector names after a replug.
    pub fn find_active(&self, states: &[OutputState]) -> Result<Option<String>, ProfileError> {
        let connected: Vec<String> = states
            .iter()
            .map(|state| state.hardware_id.clone())
            .collect();
        for profile in self.load()? {
            if profile.outputs.len() != states.len() {
                continue;
            }
            let matched = states.iter().all(|current| {
                match_state(
                    &profile.outputs,
                    &current.name,
                    &current.hardware_id,
                    &connected,
                )
                .is_some_and(|saved| saved.matches_config(current))
            });
            if matched {
                return Ok(Some(profile.name));
            }
        }
        Ok(None)
    }

    fn read(&self) -> Result<Snapshot, ProfileError> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Snapshot::default());
            }
            Err(error) => {
                return Err(ProfileError(format!(
                    "Could not read {}: {error}",
                    self.path.display()
                )));
            }
        };
        let data: Value = serde_json::from_str(&text).map_err(|error| {
            ProfileError(format!("Could not parse {}: {error}", self.path.display()))
        })?;
        let Value::Object(data) = data else {
            return Err(ProfileError(format!(
                "{} does not contain a JSON object.",
                self.path.display()
            )));
        };
        let empty = Value::Object(Map::new());
        let Value::Object(raw_profiles) = data.get("profiles").unwrap_or(&empty) else {
            return Err(ProfileError(format!(
                "{} has an invalid profiles object.",
                self.path.display()
            )));
        };

        let mut snapshot = Snapshot::default();
        for (name, raw) in raw_profiles {
            let parsed = match raw {
                Value::Object(object) => Profile::from_dict(name, object),
                _ => Err(ProfileError(format!(
                    "Profile {} is not an object.",
                    repr(name)
                ))),
            };
            match parsed {
                Ok(profile) => snapshot.profiles.push(profile),
                Err(error) => snapshot.broken.push((name.clone(), raw.clone(), error.0)),
            }
        }
        if let Some(Value::Array(items)) = data.get("order") {
            for item in items {
                let name = match item {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                };
                if !snapshot.order.contains(&name) {
                    snapshot.order.push(name);
                }
            }
        }
        Ok(snapshot)
    }

    fn write(&self, snapshot: &Snapshot) -> Result<(), ProfileError> {
        let mut profiles = Map::new();
        for (name, raw, _) in &snapshot.broken {
            profiles.insert(name.clone(), raw.clone());
        }
        for profile in &snapshot.profiles {
            profiles.insert(profile.name.clone(), profile.to_dict());
        }
        let order: Vec<Value> = snapshot
            .order
            .iter()
            .filter(|name| profiles.contains_key(*name))
            .map(|name| Value::String(name.clone()))
            .collect();
        let mut payload = Map::new();
        payload.insert("version".into(), Value::from(SCHEMA_VERSION));
        payload.insert("order".into(), Value::Array(order));
        payload.insert("profiles".into(), Value::Object(profiles));
        let mut text = to_pretty_json(&sorted(Value::Object(payload)));
        text.push('\n');

        let error = |error: io::Error| {
            ProfileError(format!("Could not write {}: {error}", self.path.display()))
        };
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            // Only the profile directory is private, and only when we create
            // it; its parents get the default mode.
            let existed = parent.is_dir();
            paths::mkdir_parents(parent).map_err(error)?;
            if !existed {
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(error)?;
            }
        }
        self.backup_existing().map_err(error)?;
        paths::atomic_write(&self.path, text.as_bytes(), 0o600).map_err(error)
    }

    /// Keep a timestamped copy before modifying an existing profile file.
    fn backup_existing(&self) -> io::Result<()> {
        let data = match fs::read(&self.path) {
            Ok(data) => data,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let file_name = self
            .path
            .file_name()
            .map_or_else(|| "profiles.json".into(), |name| name.to_string_lossy());
        let stamp = paths::unix_time();
        let mut backup = self.path.with_file_name(format!("{file_name}.bak.{stamp}"));
        let mut counter = 1;
        while backup.exists() {
            backup = self
                .path
                .with_file_name(format!("{file_name}.bak.{stamp}.{counter}"));
            counter += 1;
        }
        write_private(&backup, &data)
    }
}

fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(data)?;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    Ok(())
}

/// Sort object keys recursively (the files are written with sorted keys).
pub fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, sorted(value)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

/// JSON exactly like Python's `json.dumps(value, indent=2)`: two-space
/// indent, `,` and `: ` separators, ASCII only (`\uXXXX` escapes, DEL
/// included) and floats written like Python's `repr`.
pub fn to_pretty_json(value: &Value) -> String {
    fn string(out: &mut String, text: &str) {
        out.push('"');
        for character in text.chars() {
            match character {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{8}' => out.push_str("\\b"),
                '\u{c}' => out.push_str("\\f"),
                ' '..='~' => out.push(character),
                _ => {
                    let mut units = [0u16; 2];
                    for unit in character.encode_utf16(&mut units) {
                        out.push_str(&format!("\\u{unit:04x}"));
                    }
                }
            }
        }
        out.push('"');
    }

    fn write(out: &mut String, value: &Value, depth: usize) {
        let indent = |out: &mut String, depth: usize| {
            out.push('\n');
            out.push_str(&"  ".repeat(depth));
        };
        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
            Value::Number(number) if number.is_f64() => {
                out.push_str(&crate::model::py_float_repr(number.as_f64().unwrap_or(0.0)));
            }
            Value::Number(number) => out.push_str(&number.to_string()),
            Value::String(text) => string(out, text),
            Value::Array(items) if items.is_empty() => out.push_str("[]"),
            Value::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    indent(out, depth + 1);
                    write(out, item, depth + 1);
                }
                indent(out, depth);
                out.push(']');
            }
            Value::Object(map) if map.is_empty() => out.push_str("{}"),
            Value::Object(map) => {
                out.push('{');
                for (index, (key, item)) in map.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    indent(out, depth + 1);
                    string(out, key);
                    out.push_str(": ");
                    write(out, item, depth + 1);
                }
                indent(out, depth);
                out.push('}');
            }
        }
    }

    let mut out = String::new();
    write(&mut out, value, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Mode;
    use crate::testutil::TempDir;

    fn sample(name: &str, x: i64, scale: f64, hardware_id: &str) -> OutputState {
        OutputState {
            name: name.into(),
            enabled: true,
            mode: Some(Mode::new(1920, 1080, 100.0)),
            x,
            y: 0,
            scale,
            transform: "normal".into(),
            adaptive_sync: false,
            hardware_id: hardware_id.into(),
            description: format!("{name} description"),
            hdr: None,
            bit_depth: None,
            color_profile: None,
        }
    }

    fn state() -> OutputState {
        sample("HDMI-A-1", 0, 1.0, "")
    }

    fn store(dir: &TempDir) -> ProfileStore {
        ProfileStore::new(Some(dir.join("profiles.json")))
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn missing_file_has_no_profiles() {
        let dir = TempDir::new();
        let store = store(&dir);
        assert!(store.load().unwrap().is_empty());
        assert!(store.names().unwrap().is_empty());
    }

    #[test]
    fn profile_round_trip() {
        let dir = TempDir::new();
        let store = store(&dir);
        let state = sample("HDMI-A-1", 0, 1.0, "HW/1");
        store.save("work", std::slice::from_ref(&state)).unwrap();

        assert!(store.path.exists());
        assert_eq!(store.names().unwrap(), ["work"]);
        let profile = store.get("work").unwrap().unwrap();
        assert_eq!(
            profile.output("HDMI-A-1").unwrap().to_dict(),
            state.to_dict()
        );
        assert!(!profile.created_at.is_empty() && !profile.updated_at.is_empty());

        let raw = read_json(&store.path);
        assert_eq!(raw["version"], 2);
        assert_eq!(raw["order"], serde_json::json!(["work"]));
        let output = &raw["profiles"]["work"]["outputs"]["HDMI-A-1"];
        assert_eq!(output["mode"]["width"], 1920);
        assert_eq!(output["hardware_id"], "HW/1");

        // Keys are sorted and the file ends with a newline, like before.
        let text = fs::read_to_string(&store.path).unwrap();
        assert!(text.starts_with("{\n  \"order\": [\n    \"work\"\n  ],\n  \"profiles\""));
        assert!(text.ends_with("}\n"));
    }

    #[test]
    fn update_existing_profile_preserves_created_at() {
        let dir = TempDir::new();
        let store = store(&dir);
        store.save("work", &[state()]).unwrap();
        let first = store.get("work").unwrap().unwrap();
        store
            .save("work", &[sample("HDMI-A-1", 1920, 1.5, "")])
            .unwrap();
        let profile = store.get("work").unwrap().unwrap();
        let saved = profile.output("HDMI-A-1").unwrap();
        assert_eq!((saved.x, saved.scale), (1920, 1.5));
        assert_eq!(profile.created_at, first.created_at);
    }

    #[test]
    fn each_write_keeps_a_timestamped_backup() {
        let dir = TempDir::new();
        let store = store(&dir);
        store.save("work", &[state()]).unwrap();
        store
            .save("home", &[sample("HDMI-A-1", 100, 1.0, "")])
            .unwrap();
        store.save("docked", &[state()]).unwrap();
        let backups: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("profiles.json.bak.")
            })
            .collect();
        assert_eq!(backups.len(), 2);
        for backup in backups {
            let mode = backup.metadata().unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn delete_profile() {
        let dir = TempDir::new();
        let store = store(&dir);
        store.save("work", &[state()]).unwrap();
        assert!(store.delete("work").unwrap());
        assert!(!store.delete("work").unwrap());
        assert!(store.names().unwrap().is_empty());
    }

    #[test]
    fn empty_and_reserved_names_are_rejected() {
        let dir = TempDir::new();
        let store = store(&dir);
        assert!(store.save("   ", &[state()]).is_err());
        assert!(store.save("__save__", &[state()]).is_err());
        assert!(store.save("__launch__", &[state()]).is_err());
        assert_eq!(store.save("  work  ", &[state()]).unwrap().name, "work");
    }

    #[test]
    fn invalid_json_reports_error() {
        let dir = TempDir::new();
        let store = store(&dir);
        fs::write(&store.path, "{ nope").unwrap();
        assert!(store.load().unwrap_err().0.starts_with("Could not parse"));
        fs::write(&store.path, "[]").unwrap();
        assert!(
            store
                .load()
                .unwrap_err()
                .0
                .contains("does not contain a JSON object")
        );
        fs::write(&store.path, "{\"profiles\": []}").unwrap();
        assert!(
            store
                .load()
                .unwrap_err()
                .0
                .contains("invalid profiles object")
        );
    }

    #[test]
    fn profile_file_and_new_directory_are_private() {
        let dir = TempDir::new();
        let path = dir.join("new").join("profiles.json");
        ProfileStore::new(Some(path.clone()))
            .save("work", &[state()])
            .unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&dir.join("new")), 0o700);
        // Parents created on the way keep the default mode.
        let nested = dir.join("a/b/profiles.json");
        ProfileStore::new(Some(nested.clone()))
            .save("work", &[state()])
            .unwrap();
        assert_eq!(mode(&dir.join("a/b")), 0o700);
        assert_ne!(mode(&dir.join("a")), 0o700);
    }

    #[test]
    fn names_keep_insertion_order_and_move() {
        let dir = TempDir::new();
        let store = store(&dir);
        for name in ["work", "home", "docked"] {
            store.save(name, &[state()]).unwrap();
        }
        assert_eq!(store.names().unwrap(), ["work", "home", "docked"]);
        assert!(store.move_by("docked", -1).unwrap());
        assert_eq!(store.names().unwrap(), ["work", "docked", "home"]);
        assert!(store.move_by("docked", -1).unwrap());
        assert_eq!(store.names().unwrap(), ["docked", "work", "home"]);
        assert!(!store.move_by("docked", -1).unwrap());
        assert!(!store.move_by("missing", 1).unwrap());
    }

    #[test]
    fn names_without_an_order_are_sorted_and_duplicates_dropped() {
        let dir = TempDir::new();
        let store = store(&dir);
        store.save("b", &[state()]).unwrap();
        store.save("a", &[state()]).unwrap();
        let mut raw = read_json(&store.path);
        raw["order"] = serde_json::json!(["a", "a", "gone"]);
        fs::write(&store.path, raw.to_string()).unwrap();
        assert_eq!(store.names().unwrap(), ["a", "b"]);
        raw.as_object_mut().unwrap().remove("order");
        fs::write(&store.path, raw.to_string()).unwrap();
        assert_eq!(store.names().unwrap(), ["a", "b"]);
    }

    #[test]
    fn rename_profile() {
        let dir = TempDir::new();
        let store = store(&dir);
        store.save("work", &[state()]).unwrap();
        let created_at = store.get("work").unwrap().unwrap().created_at;
        assert!(store.rename("work", "office").unwrap());
        assert_eq!(store.names().unwrap(), ["office"]);
        assert_eq!(store.get("office").unwrap().unwrap().created_at, created_at);
        assert!(store.get("work").unwrap().is_none());
        assert!(!store.rename("missing", "other").unwrap());
        assert!(store.rename("office", "office").unwrap());
    }

    #[test]
    fn rename_rejects_duplicate_and_invalid_names() {
        let dir = TempDir::new();
        let store = store(&dir);
        store.save("work", &[state()]).unwrap();
        store.save("home", &[state()]).unwrap();
        assert!(store.rename("work", "home").is_err());
        assert!(store.rename("work", "  ").is_err());
    }

    #[test]
    fn find_active_by_connector_name() {
        let dir = TempDir::new();
        let store = store(&dir);
        store.save("work", &[state()]).unwrap();
        assert_eq!(
            store.find_active(&[state()]).unwrap().as_deref(),
            Some("work")
        );
        let changed = sample("HDMI-A-1", 1, 1.0, "");
        assert_eq!(store.find_active(&[changed]).unwrap(), None);
    }

    #[test]
    fn find_active_by_hardware_id_fallback() {
        let dir = TempDir::new();
        let store = store(&dir);
        store
            .save("work", &[sample("DP-1", 0, 1.0, "Microstep/MSI MP275/ABC")])
            .unwrap();
        let replugged = sample("DP-4", 0, 1.0, "Microstep/MSI MP275/ABC");
        assert_eq!(
            store.find_active(&[replugged]).unwrap().as_deref(),
            Some("work")
        );
    }

    #[test]
    fn find_active_requires_the_same_output_count() {
        let dir = TempDir::new();
        let store = store(&dir);
        store.save("work", &[state()]).unwrap();
        let second = sample("eDP-1", 0, 1.0, "");
        assert_eq!(store.find_active(&[state(), second]).unwrap(), None);
    }

    #[test]
    fn control_characters_are_rejected() {
        let dir = TempDir::new();
        let store = store(&dir);
        // Profile names end up in compositor config comments.
        let error = store.save("work\nexec evil", &[state()]).unwrap_err();
        assert!(error.0.contains("control characters"));
        store.save("work", &[state()]).unwrap();
        assert!(store.rename("work", "a\tb").is_err());
        assert!(store.rename("work", "a/b").is_err());
        assert!(store.rename("work", "a\u{2028}b").is_err());
        assert!(store.rename("work", "a\u{200b}b").is_err());
        assert!(store.rename("work", "café docked").unwrap());
        // Written like Python's json.dumps: ASCII only, read back unchanged.
        let text = fs::read_to_string(&store.path).unwrap();
        assert!(text.is_ascii() && text.contains("caf\\u00e9 docked"));
        assert_eq!(store.names().unwrap(), ["café docked"]);
        assert_eq!(
            to_pretty_json(&Value::String("😀\u{7f}\u{1}\n".into())),
            "\"\\ud83d\\ude00\\u007f\\u0001\\n\""
        );
        let floats = serde_json::json!({"a": 0.00001, "b": 1.0, "c": [], "d": {}, "e": 1e16});
        assert_eq!(
            to_pretty_json(&floats),
            "{\n  \"a\": 1e-05,\n  \"b\": 1.0,\n  \"c\": [],\n  \"d\": {},\n  \"e\": 1e+16\n}"
        );
    }

    #[test]
    fn compositor_settings_round_trip_and_match_loosely() {
        let dir = TempDir::new();
        let store = store(&dir);
        let mut hdr = state();
        hdr.hdr = Some(true);
        hdr.color_profile = Some("srgb".into());
        store.save("hdr", &[hdr]).unwrap();
        let profile = store.get("hdr").unwrap().unwrap();
        let saved = profile.output("HDMI-A-1").unwrap();
        assert_eq!(saved.hdr, Some(true));
        assert_eq!(saved.color_profile.as_deref(), Some("srgb"));
        assert_eq!(saved.bit_depth, None);

        // Live state without HDR information still matches the profile...
        assert_eq!(
            store.find_active(&[state()]).unwrap().as_deref(),
            Some("hdr")
        );
        // ...but a known, different HDR state does not.
        let mut live = state();
        live.hdr = Some(false);
        assert_eq!(store.find_active(&[live]).unwrap(), None);
    }

    #[test]
    fn one_broken_profile_does_not_hide_the_others() {
        let dir = TempDir::new();
        let store = store(&dir);
        store.save("work", &[state()]).unwrap();
        store
            .save("home", &[sample("HDMI-A-1", 100, 1.0, "")])
            .unwrap();
        let mut raw = read_json(&store.path);
        raw["profiles"]["work"]["outputs"]["HDMI-A-1"]["transform"] =
            Value::String("normal; exec x".into());
        fs::write(&store.path, raw.to_string()).unwrap();

        assert_eq!(store.names().unwrap(), ["home"]);
        let loaded: Vec<_> = store.load().unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(loaded, ["home"]);
        let problems = store.problems().unwrap();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("invalid transform"), "{}", problems[0]);
        assert!(
            store
                .get("work")
                .unwrap_err()
                .0
                .contains("invalid transform")
        );
        assert!(store.exists("work").unwrap());
        assert!(store.rename("work", "office").is_err());
        assert!(
            store
                .rename("home", "work")
                .unwrap_err()
                .0
                .contains("already exists")
        );

        // Editing other profiles keeps the broken one untouched.
        store
            .save("docked", &[sample("HDMI-A-1", 200, 1.0, "")])
            .unwrap();
        assert!(store.move_by("docked", -1).unwrap());
        let kept = read_json(&store.path);
        assert_eq!(kept["profiles"]["work"], raw["profiles"]["work"]);
        assert_eq!(kept["order"], serde_json::json!(["docked", "home", "work"]));

        // Saving over it, or deleting it, clears the problem.
        store.save("work", &[state()]).unwrap();
        assert!(store.problems().unwrap().is_empty());
        assert!(store.delete("work").unwrap());
        assert!(read_json(&store.path)["profiles"].get("work").is_none());
    }
}
