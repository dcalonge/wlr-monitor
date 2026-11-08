//! Named layouts, stored as one small JSON file.
//!
//! A profile that cannot be parsed (say, an unknown transform in a
//! hand-edited file) does not make the file unusable: it is skipped, reported
//! by `problems()`, and written back untouched so editing other profiles
//! never loses it.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fsutil::{write_atomic, xdg_dir};
use crate::output::{Output, OutputState, match_state};

const SCHEMA_VERSION: u32 = 2;

/// Check and clean up a profile name.
pub fn validate_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() {
        bail!("Profile name must not be empty.");
    }
    // Names are written into compositor config comments, so no newlines.
    if name
        .chars()
        .any(|c| c == '/' || c == '\\' || c.is_control())
    {
        bail!("Profile names cannot contain slashes or control characters.");
    }
    Ok(name)
}

/// The saved settings of every output in a layout.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub name: String,
    pub outputs: Vec<OutputState>,
}

/// Which outputs a profile applied to.
#[derive(Debug, Default)]
pub struct Matched {
    /// Connected outputs that got saved settings.
    pub applied: Vec<String>,
    /// Connected outputs the profile knows nothing about.
    pub not_in_profile: Vec<String>,
    /// Saved outputs that are not connected.
    pub not_connected: Vec<String>,
}

impl Profile {
    /// Copy the saved settings onto the connected outputs they match.
    ///
    /// Outputs match by connector name, else by EDID id (see `match_state`).
    /// Settings the compositor cannot apply still need `drop_unsupported`.
    pub fn apply_to(&self, outputs: &mut [Output]) -> Matched {
        let connected: Vec<OutputState> = outputs.iter().map(Output::state).collect();
        let mut matched = Matched::default();
        let mut used = HashSet::new();
        for (output, current) in outputs.iter_mut().zip(&connected) {
            match match_state(&self.outputs, current, &connected) {
                Some(saved) => {
                    saved.apply_to(output, false);
                    used.insert(saved.name.as_str());
                    matched.applied.push(output.name.clone());
                }
                None => matched.not_in_profile.push(output.name.clone()),
            }
        }
        matched.not_connected = self
            .outputs
            .iter()
            .filter(|saved| !used.contains(saved.name.as_str()))
            .filter(|saved| !connected.iter().any(|current| current.name == saved.name))
            .map(|saved| saved.name.clone())
            .collect();
        matched
    }

    fn parse(name: &str, raw: &Value) -> Result<Self> {
        #[derive(Deserialize)]
        struct Stored {
            outputs: BTreeMap<String, OutputState>,
        }
        let stored =
            Stored::deserialize(raw).with_context(|| format!("Profile {name:?} is invalid"))?;
        let mut outputs = Vec::new();
        for (output, mut state) in stored.outputs {
            state
                .validate()
                .with_context(|| format!("Profile {name:?} has an invalid entry for {output}"))?;
            state.name = output;
            outputs.push(state);
        }
        Ok(Self {
            name: name.to_string(),
            outputs,
        })
    }
}

/// The file's content. Profiles stay raw until they are needed.
#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    order: Vec<String>,
    #[serde(default)]
    profiles: BTreeMap<String, Value>,
}

impl File {
    /// Valid profile names: the stored order first, then the rest sorted.
    fn names(&self) -> Vec<String> {
        let valid = |name: &String| Profile::parse(name, &self.profiles[name]).is_ok();
        let mut names: Vec<String> = Vec::new();
        for name in &self.order {
            if self.profiles.contains_key(name) && valid(name) && !names.contains(name) {
                names.push(name.clone());
            }
        }
        let rest = self
            .profiles
            .keys()
            .filter(|name| !names.contains(name) && valid(name));
        let rest: Vec<String> = rest.cloned().collect();
        names.extend(rest);
        names
    }

    fn get(&self, name: &str) -> Result<Option<Profile>> {
        self.profiles
            .get(name)
            .map(|raw| Profile::parse(name, raw))
            .transpose()
    }
}

#[derive(Debug, Clone)]
pub struct ProfileStore {
    pub path: PathBuf,
}

impl ProfileStore {
    /// `path` is a file; the default is `profiles.json` in the config dir.
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path: path.unwrap_or_else(|| {
                xdg_dir("XDG_CONFIG_HOME", ".config").join("wlr-monitor/profiles.json")
            }),
        }
    }

    /// Valid profile names in display order.
    pub fn names(&self) -> Result<Vec<String>> {
        Ok(self.read()?.names())
    }

    /// One message per profile that could not be read.
    pub fn problems(&self) -> Result<Vec<String>> {
        let file = self.read()?;
        Ok(file
            .profiles
            .iter()
            .filter_map(|(name, raw)| Profile::parse(name, raw).err())
            .map(|error| format!("{error:#}"))
            .collect())
    }

    /// True for any stored profile, including one that cannot be read.
    pub fn exists(&self, name: &str) -> Result<bool> {
        Ok(self.read()?.profiles.contains_key(name))
    }

    pub fn get(&self, name: &str) -> Result<Option<Profile>> {
        self.read()?.get(name)
    }

    pub fn save(&self, name: &str, states: &[OutputState]) -> Result<()> {
        let name = validate_name(name)?;
        let mut file = self.read()?;
        let outputs: BTreeMap<_, _> = states.iter().map(|state| (&state.name, state)).collect();
        // Saving over a broken profile replaces it.
        file.profiles
            .insert(name.to_string(), serde_json::json!({ "outputs": outputs }));
        if !file.order.iter().any(|item| item == name) {
            file.order.push(name.to_string());
        }
        self.write(&file)
    }

    /// Returns false if there is no such profile.
    pub fn delete(&self, name: &str) -> Result<bool> {
        let mut file = self.read()?;
        if file.profiles.remove(name).is_none() {
            return Ok(false);
        }
        file.order.retain(|item| item != name);
        self.write(&file)?;
        Ok(true)
    }

    /// Returns false if there is no such profile.
    pub fn rename(&self, old: &str, new: &str) -> Result<bool> {
        let new = validate_name(new)?;
        let mut file = self.read()?;
        if file.get(old)?.is_none() {
            return Ok(false);
        }
        if new == old {
            return Ok(true);
        }
        if file.profiles.contains_key(new) {
            bail!("A profile named {new:?} already exists.");
        }
        let raw = file.profiles.remove(old).unwrap_or_default();
        file.profiles.insert(new.to_string(), raw);
        for item in file.order.iter_mut().filter(|item| *item == old) {
            *item = new.to_string();
        }
        self.write(&file)?;
        Ok(true)
    }

    /// Move a profile `offset` places in the display order. Returns whether
    /// anything moved.
    pub fn move_by(&self, name: &str, offset: isize) -> Result<bool> {
        let mut file = self.read()?;
        let mut order = file.names();
        let Some(index) = order.iter().position(|item| item == name) else {
            return Ok(false);
        };
        let target = index.saturating_add_signed(offset).min(order.len() - 1);
        if target == index {
            return Ok(false);
        }
        let item = order.remove(index);
        order.insert(target, item);
        // Broken profiles keep their place at the end.
        let broken = file.profiles.keys().filter(|name| !order.contains(name));
        order.extend(broken.cloned().collect::<Vec<_>>());
        file.order = order;
        self.write(&file)?;
        Ok(true)
    }

    /// The name of the profile that matches `states`.
    ///
    /// Outputs match by connector name first and by EDID hardware id as a
    /// fallback, which keeps profiles usable when the kernel reassigns
    /// connector names after a replug.
    pub fn find_active(&self, states: &[OutputState]) -> Result<Option<String>> {
        let file = self.read()?;
        for name in file.names() {
            let Some(profile) = file.get(&name)? else {
                continue;
            };
            let matches = profile.outputs.len() == states.len()
                && states.iter().all(|current| {
                    match_state(&profile.outputs, current, states)
                        .is_some_and(|saved| saved.matches_config(current))
                });
            if matches {
                return Ok(Some(name));
            }
        }
        Ok(None)
    }

    fn read(&self) -> Result<File> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(File::default()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Could not read {}", self.path.display()));
            }
        };
        serde_json::from_str(&text)
            .with_context(|| format!("Could not parse {}", self.path.display()))
    }

    fn write(&self, file: &File) -> Result<()> {
        // Keep order entries that still exist; broken profiles are kept as is.
        let order = file
            .order
            .iter()
            .filter(|name| file.profiles.contains_key(*name))
            .collect::<Vec<_>>();
        let json = serde_json::json!({
            "version": SCHEMA_VERSION,
            "order": order,
            "profiles": file.profiles,
        });
        let mut text = serde_json::to_string_pretty(&json)?;
        text.push('\n');
        let context = || format!("Could not write {}", self.path.display());
        // Only a directory we create is made private.
        if let Some(dir) = self.path.parent().filter(|dir| !dir.exists()) {
            fs::create_dir_all(dir).with_context(context)?;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).with_context(context)?;
        }
        write_atomic(&self.path, text.as_bytes(), 0o600).with_context(context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::tests::fixture_outputs;

    fn store() -> (tempfile::TempDir, ProfileStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(Some(dir.path().join("wlr-monitor/profiles.json")));
        (dir, store)
    }

    fn states() -> Vec<OutputState> {
        fixture_outputs()
            .iter()
            .map(|output| output.state())
            .collect()
    }

    #[test]
    fn saves_renames_orders_and_deletes() {
        let (_dir, store) = store();
        store.save(" work ", &states()).unwrap();
        store.save("home", &states()).unwrap();
        assert_eq!(store.names().unwrap(), ["work", "home"]);
        let saved = store.get("work").unwrap().unwrap().outputs;
        assert!(
            saved
                .iter()
                .zip(states())
                .all(|(a, b)| a.matches_config(&b))
        );

        assert!(store.move_by("home", -1).unwrap());
        assert!(store.rename("home", "tv").unwrap());
        assert_eq!(store.names().unwrap(), ["tv", "work"]);
        assert!(store.rename("tv", "work").is_err());

        assert_eq!(store.find_active(&states()).unwrap().as_deref(), Some("tv"));
        assert!(store.delete("tv").unwrap());
        assert!(!store.delete("tv").unwrap());
    }

    #[test]
    fn rejects_bad_names() {
        for name in ["", "  ", "a/b", "a\nb"] {
            assert!(validate_name(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn keeps_broken_profiles_untouched() {
        let (_dir, store) = store();
        fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        let broken = r#"{"profiles": {"bad": {"outputs": {"X": {"transform": "sideways"}}}}}"#;
        fs::write(&store.path, broken).unwrap();
        assert!(store.names().unwrap().is_empty());
        assert!(store.exists("bad").unwrap());
        assert!(store.problems().unwrap()[0].contains("bad"));

        store.save("good", &states()).unwrap();
        let text = fs::read_to_string(&store.path).unwrap();
        assert!(text.contains("sideways"));
        assert_eq!(store.names().unwrap(), ["good"]);
    }
}
