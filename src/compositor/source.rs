//! Is the include loaded by the compositor's config, and what to add if not.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::{Compositor, Kind, quote};

impl Compositor {
    /// The line the user adds to their compositor config.
    pub fn include_line(&self) -> String {
        let Some(path) = self.include_path() else {
            return String::new();
        };
        let shown = self.display_path(path);
        match self.kind {
            Kind::Wlroots => String::new(),
            Kind::Sway => format!("include {shown}"),
            Kind::Mango => format!("source_optional={shown}"),
            Kind::Niri => {
                let beside_main = self
                    .main_config()
                    .is_some_and(|main| path.parent() == main.parent());
                match path.file_name() {
                    Some(name) if beside_main => format!("include \"{}\"", name.to_string_lossy()),
                    _ => format!("include {}", quote(&path.to_string_lossy())),
                }
            }
        }
    }

    fn main_config_paths(&self) -> Vec<PathBuf> {
        let home = &self.home;
        let config = &self.config_home;
        match self.kind {
            Kind::Wlroots => Vec::new(),
            Kind::Sway => vec![
                home.join(".sway/config"),
                config.join("sway/config"),
                home.join(".i3/config"),
                config.join("i3/config"),
                "/etc/sway/config".into(),
            ],
            Kind::Niri => {
                let from_env = std::env::var("NIRI_CONFIG")
                    .ok()
                    .filter(|path| !path.is_empty());
                from_env
                    .map(|path| PathBuf::from(self.expand_tilde(&path)))
                    .into_iter()
                    .chain([
                        config.join("niri/config.kdl"),
                        "/etc/niri/config.kdl".into(),
                    ])
                    .collect()
            }
            Kind::Mango => {
                let dir = self.mango_dir();
                vec![
                    dir.join("config.conf"),
                    dir.join("config.toml"),
                    "/etc/mango/config.conf".into(),
                    "/etc/mango/config.toml".into(),
                ]
            }
        }
    }

    pub fn main_config(&self) -> Option<PathBuf> {
        self.main_config_paths()
            .into_iter()
            .find(|path| path.exists())
    }

    /// Resolve a path from a config line: `~`, `$VARS` and relative paths.
    fn expand(&self, raw: &str, base: &Path) -> PathBuf {
        if self.kind == Kind::Mango {
            // Like mango's resolve_config_path(): "./x" is relative to the
            // config directory (not the including file), "~/x" to $HOME and
            // anything else is used as is.
            return match raw.strip_prefix("./") {
                Some(rest) => self.mango_dir().join(rest),
                None => self.expand_tilde(raw).into(),
            };
        }
        let home = || Some(self.home.to_string_lossy());
        let expanded =
            shellexpand::full_with_context_no_errors(raw, home, |var| std::env::var(var).ok());
        base.join(expanded.as_ref())
    }

    /// Files referenced by the include/source lines of a config file.
    fn include_refs(&self, text: &str, base: &Path) -> Vec<PathBuf> {
        let mut refs = Vec::new();
        for line in text.lines() {
            match self.kind {
                Kind::Wlroots => {}
                Kind::Sway => {
                    let Some(raw) = sway_include_arg(line) else {
                        continue;
                    };
                    let expanded = self.expand(raw.trim_matches(['\'', '"']), base);
                    let matches: Vec<PathBuf> = glob::glob(&expanded.to_string_lossy())
                        .map(|paths| paths.flatten().collect())
                        .unwrap_or_default();
                    if matches.is_empty() {
                        refs.push(expanded);
                    } else {
                        refs.extend(matches);
                    }
                }
                Kind::Niri => {
                    let stripped = line.trim();
                    if let Some(raw) =
                        niri_include_arg(stripped).filter(|_| !stripped.starts_with("//"))
                    {
                        refs.push(self.expand(raw, base));
                    }
                }
                Kind::Mango => {
                    if let Some(raw) = mango_source_arg(line) {
                        refs.push(self.expand(raw.trim_matches(['\'', '"']), base));
                    }
                }
            }
        }
        refs
    }

    /// Best-effort check that the main config includes our file.
    ///
    /// Follows one level of nested includes, which covers the common
    /// "config.conf sources monitors.conf" layout.
    pub fn include_is_sourced(&self) -> bool {
        let (Some(target), Some(main)) = (self.include_path(), self.main_config()) else {
            return false;
        };
        let real = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let wanted = real(target);
        let mut pending = vec![(main, 0)];
        let mut seen = HashSet::new();
        while let Some((config, depth)) = pending.pop() {
            if !seen.insert(real(&config)) {
                continue;
            }
            let Ok(bytes) = fs::read(&config) else {
                continue;
            };
            let base = config.parent().unwrap_or(Path::new(""));
            for reference in self.include_refs(&String::from_utf8_lossy(&bytes), base) {
                if real(&reference) == wanted {
                    return true;
                }
                if depth < 1 && reference.is_file() {
                    pending.push((reference, depth + 1));
                }
            }
        }
        false
    }

    /// A short setup hint, or an empty string when nothing is needed.
    pub fn include_hint(&self) -> String {
        if self.include_path().is_none() || !self.include_enabled || self.include_is_sourced() {
            return String::new();
        }
        let place = self.main_config().map_or_else(
            || "your config".to_string(),
            |main| self.display_path(&main),
        );
        format!(
            "Add `{}` to {place} to keep layouts after a restart.",
            self.include_line()
        )
    }
}

/// The argument of a sway `include` line.
fn sway_include_arg(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix("include")?;
    let arg = rest.strip_prefix(char::is_whitespace).map(str::trim)?;
    (!arg.is_empty()).then_some(arg)
}

/// The quoted path of a niri `include "…"` line.
fn niri_include_arg(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("include")?;
    if rest.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let body = &rest[rest.find('"')? + 1..];
    let mut escaped = false;
    body.char_indices().find_map(|(index, character)| {
        match character {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => return Some(&body[..index]),
            _ => {}
        }
        None
    })
}

/// The path of a mango `source=` or `source_optional=` line.
fn mango_source_arg(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix("source")?;
    let rest = rest.strip_prefix("_optional").unwrap_or(rest);
    let arg = rest.trim_start().strip_prefix('=')?.trim();
    (!arg.is_empty()).then_some(arg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compositor(kind: Kind, home: &Path) -> Compositor {
        Compositor::with_dirs(
            kind,
            None,
            true,
            home.into(),
            home.join(".config"),
            home.join(".state"),
        )
    }

    #[test]
    fn finds_include_through_one_nested_level() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".config/sway");
        fs::create_dir_all(&dir).unwrap();
        let c = compositor(Kind::Sway, home.path());
        assert!(!c.include_is_sourced());
        assert!(
            c.include_hint()
                .contains("include ~/.config/sway/wlr-monitor.conf")
        );

        fs::write(dir.join("config"), "include monitors.d/*\n").unwrap();
        fs::create_dir_all(dir.join("monitors.d")).unwrap();
        fs::write(
            dir.join("monitors.d/a.conf"),
            "include ~/.config/sway/wlr-monitor.conf\n",
        )
        .unwrap();
        fs::write(dir.join("wlr-monitor.conf"), "").unwrap();
        assert!(c.include_is_sourced());
        assert_eq!(c.include_hint(), "");
    }

    #[test]
    fn parses_source_lines() {
        assert_eq!(
            sway_include_arg("  include ~/x/*.conf "),
            Some("~/x/*.conf")
        );
        assert_eq!(sway_include_arg("includes foo"), None);
        assert_eq!(niri_include_arg(r#"include "a b.kdl""#), Some("a b.kdl"));
        assert_eq!(niri_include_arg(r#"includes "a""#), None);
        assert_eq!(
            mango_source_arg("source_optional = ./x.conf"),
            Some("./x.conf")
        );
        assert_eq!(mango_source_arg("sourcefoo=1"), None);
    }
}
