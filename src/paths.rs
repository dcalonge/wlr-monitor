//! Home / XDG directories, timestamps and safe file writes.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(|value| clean(Path::new(&value)))
}

/// `$HOME`, falling back to the password database.
pub fn home_dir() -> PathBuf {
    env_path("HOME")
        .or_else(std::env::home_dir)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn config_home() -> PathBuf {
    env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home_dir().join(".config"))
}

pub fn state_home() -> PathBuf {
    env_path("XDG_STATE_HOME").unwrap_or_else(|| home_dir().join(".local").join("state"))
}

/// Seconds since the Unix epoch.
pub fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The current UTC time as ISO 8601 with seconds, e.g.
/// `2026-01-01T09:00:00+00:00`.
pub fn now_iso() -> String {
    iso_from_unix(unix_time())
}

pub fn iso_from_unix(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}+00:00",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Lexically normalise a path (`a/./b/../c` → `a/c`), like `os.path.normpath`.
pub fn normpath(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut result = PathBuf::new();
    let mut depth = 0usize;
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if depth > 0 {
                    result.pop();
                    depth -= 1;
                } else if !result.has_root() {
                    result.push("..");
                }
            }
            Component::Normal(part) => {
                result.push(part);
                depth += 1;
            }
            other => result.push(other.as_os_str()),
        }
    }
    if result.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        result
    }
}

/// `os.path.abspath`: absolute and normalised, without touching the disk.
pub fn abspath(path: &Path) -> PathBuf {
    if path.is_absolute() {
        normpath(path)
    } else {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        normpath(&cwd.join(path))
    }
}

/// `os.path.realpath` (non-strict): every symlink is resolved before the
/// next `..` is applied, dangling links are followed to their target, and
/// whatever does not exist is kept as written.
pub fn realpath(path: &Path) -> PathBuf {
    use std::collections::HashMap;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    /// posixpath.join for byte strings.
    fn join(base: &[u8], name: &[u8]) -> Vec<u8> {
        if name.starts_with(b"/") {
            return name.to_vec();
        }
        let mut joined = base.to_vec();
        if !joined.is_empty() && !joined.ends_with(b"/") {
            joined.push(b'/');
        }
        joined.extend_from_slice(name);
        joined
    }

    /// posixpath.split for byte strings.
    fn split(path: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let index = path.iter().rposition(|b| *b == b'/').map_or(0, |i| i + 1);
        let (mut head, tail) = (path[..index].to_vec(), path[index..].to_vec());
        if !head.is_empty() && head.iter().any(|b| *b != b'/') {
            while head.ends_with(b"/") {
                head.pop();
            }
        }
        (head, tail)
    }

    fn as_path(bytes: &[u8]) -> PathBuf {
        PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec()))
    }

    /// posixpath._joinrealpath; the bool is false when a symlink loop was found.
    fn join_realpath(
        mut path: Vec<u8>,
        rest: &[u8],
        seen: &mut HashMap<Vec<u8>, Option<Vec<u8>>>,
    ) -> (Vec<u8>, bool) {
        let mut rest = rest;
        if let Some(stripped) = rest.strip_prefix(b"/") {
            rest = stripped;
            path = b"/".to_vec();
        }
        while !rest.is_empty() {
            let (name, remaining) = match rest.iter().position(|b| *b == b'/') {
                Some(index) => (&rest[..index], &rest[index + 1..]),
                None => (rest, &rest[rest.len()..]),
            };
            rest = remaining;
            if name.is_empty() || name == b"." {
                continue;
            }
            if name == b".." {
                if path.is_empty() {
                    path = b"..".to_vec();
                } else {
                    let (head, tail) = split(&path);
                    path = if tail == b".." {
                        join(&join(&head, b".."), b"..")
                    } else {
                        head
                    };
                }
                continue;
            }
            let newpath = join(&path, name);
            let is_link = fs::symlink_metadata(as_path(&newpath))
                .is_ok_and(|meta| meta.file_type().is_symlink());
            if !is_link {
                path = newpath;
                continue;
            }
            if let Some(known) = seen.get(&newpath) {
                match known {
                    Some(resolved) => {
                        path = resolved.clone();
                        continue;
                    }
                    // A symlink loop: give up on resolving the rest.
                    None => return (join(&newpath, rest), false),
                }
            }
            seen.insert(newpath.clone(), None);
            let Ok(target) = fs::read_link(as_path(&newpath)) else {
                path = newpath;
                continue;
            };
            let (resolved, ok) = join_realpath(path, target.as_os_str().as_bytes(), seen);
            if !ok {
                return (join(&resolved, rest), false);
            }
            path = resolved;
            seen.insert(newpath, Some(path.clone()));
        }
        (path, true)
    }

    let mut seen = HashMap::new();
    let (resolved, _) = join_realpath(Vec::new(), path.as_os_str().as_bytes(), &mut seen);
    abspath(&as_path(&resolved))
}

/// Normalise a path the way `pathlib.Path` does when it is built: repeated
/// separators, `.` components and a trailing slash go away; `..` stays.
pub fn clean(path: &Path) -> PathBuf {
    use std::path::Component;
    let components: Vec<Component> = path.components().collect();
    if components.len() <= 1 {
        return components.iter().collect();
    }
    components
        .into_iter()
        .filter(|component| *component != Component::CurDir)
        .collect()
}

/// Expand a leading `~` or `~/` with `home`.
pub fn expanduser(raw: &str, home: &Path) -> PathBuf {
    expanduser_path(Path::new(raw), home)
}

/// Like `expanduser`, for paths that may not be valid UTF-8.
pub fn expanduser_path(path: &Path, home: &Path) -> PathBuf {
    use std::path::Component;
    let mut components = path.components();
    match components.next() {
        Some(Component::Normal(first)) if first == "~" => {
            let rest = components.as_path();
            if rest.as_os_str().is_empty() {
                home.to_path_buf()
            } else {
                home.join(rest)
            }
        }
        _ => path.to_path_buf(),
    }
}

/// `pathlib.Path.expanduser`: `~` is `home`, `~name` is that user's home
/// from the password database (left as is when the user is unknown).
pub fn expanduser_users(path: &Path, home: &Path) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    use std::path::Component;
    let mut components = path.components();
    let Some(Component::Normal(first)) = components.next() else {
        return path.to_path_buf();
    };
    let Some(user) = first.as_bytes().strip_prefix(b"~") else {
        return path.to_path_buf();
    };
    let user_home = if user.is_empty() {
        Some(home.to_path_buf())
    } else {
        user_home(std::ffi::OsStr::from_bytes(user))
    };
    match user_home {
        Some(base) => {
            let rest = components.as_path();
            if rest.as_os_str().is_empty() {
                base
            } else {
                base.join(rest)
            }
        }
        None => path.to_path_buf(),
    }
}

/// A user's home directory from the password database.
fn user_home(name: &std::ffi::OsStr) -> Option<PathBuf> {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let name = std::ffi::CString::new(name.as_bytes()).ok()?;
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    // SAFETY: an all-zero passwd is a valid value for getpwnam_r to fill in.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: every pointer is valid for the call; the buffer outlives it.
    let status = unsafe {
        libc::getpwnam_r(
            name.as_ptr(),
            &mut entry,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() || entry.pw_dir.is_null() {
        return None;
    }
    // SAFETY: pw_dir points into `buffer`, NUL-terminated by getpwnam_r.
    let dir = unsafe { std::ffi::CStr::from_ptr(entry.pw_dir) };
    Some(PathBuf::from(std::ffi::OsString::from_vec(
        dir.to_bytes().to_vec(),
    )))
}

/// The process environment, leaving out variables that are not UTF-8.
pub fn utf8_vars() -> std::collections::HashMap<String, String> {
    std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

/// `os.path.expandvars` with an explicit environment: `$NAME` and `${NAME}`
/// are replaced when set, anything else is left as it is.
pub fn expandvars(raw: &str, env: &std::collections::HashMap<String, String>) -> String {
    let mut result = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(index) = rest.find('$') {
        result.push_str(&rest[..index]);
        let after = &rest[index + 1..];
        if let Some(braced) = after.strip_prefix('{') {
            if let Some(end) = braced.find('}') {
                let name = &braced[..end];
                match env.get(name) {
                    Some(value) if !name.is_empty() => result.push_str(value),
                    _ => result.push_str(&rest[index..index + 1 + 1 + end + 1]),
                }
                rest = &braced[end + 1..];
                continue;
            }
            result.push('$');
            rest = after;
            continue;
        }
        let length = after
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '_'))
            .map_or(after.len(), |(position, _)| position);
        let name = &after[..length];
        match env.get(name) {
            Some(value) if !name.is_empty() => result.push_str(value),
            _ => {
                result.push('$');
                result.push_str(name);
            }
        }
        rest = &after[length..];
    }
    result.push_str(rest);
    result
}

fn has_magic(text: &str) -> bool {
    text.contains(['*', '?', '['])
}

/// Shell-style wildcard match of one path component (`*`, `?`, `[...]`),
/// with the same bracket rules as Python's `fnmatch.translate`.
pub fn fnmatch(name: &str, pattern: &str) -> bool {
    let name: Vec<char> = name.chars().collect();
    let pattern: Vec<char> = pattern.chars().collect();

    /// The set for a `[` at `pattern[start - 1]`: (members, negated, index
    /// after `]`), or None when there is no closing `]` (a literal `[`).
    fn bracket(pattern: &[char], start: usize) -> Option<(&[char], bool, usize)> {
        let mut end = start;
        if pattern.get(end) == Some(&'!') {
            end += 1;
        }
        // A `]` right after `[` or `[!` is a member, not the end.
        if pattern.get(end) == Some(&']') {
            end += 1;
        }
        while end < pattern.len() && pattern[end] != ']' {
            end += 1;
        }
        if end >= pattern.len() {
            return None;
        }
        let mut set = &pattern[start..end];
        let negated = set.first() == Some(&'!');
        if negated {
            set = &set[1..];
        }
        Some((set, negated, end + 1))
    }

    fn in_set(set: &[char], character: char) -> bool {
        let mut index = 0;
        while index < set.len() {
            if index + 2 < set.len() && set[index + 1] == '-' {
                if set[index] <= character && character <= set[index + 2] {
                    return true;
                }
                index += 3;
            } else {
                if set[index] == character {
                    return true;
                }
                index += 1;
            }
        }
        false
    }

    fn matches(name: &[char], pattern: &[char]) -> bool {
        let Some(&first) = pattern.first() else {
            return name.is_empty();
        };
        match first {
            '*' => (0..=name.len()).any(|skip| matches(&name[skip..], &pattern[1..])),
            '?' => !name.is_empty() && matches(&name[1..], &pattern[1..]),
            '[' => match bracket(pattern, 1) {
                None => name.first() == Some(&'[') && matches(&name[1..], &pattern[1..]),
                Some((set, negated, after)) => {
                    let Some(&character) = name.first() else {
                        return false;
                    };
                    // `[!]` (an empty negated set) matches any character.
                    let hit = if set.is_empty() {
                        negated
                    } else {
                        in_set(set, character) != negated
                    };
                    hit && matches(&name[1..], &pattern[after..])
                }
            },
            literal => name.first() == Some(&literal) && matches(&name[1..], &pattern[1..]),
        }
    }
    matches(&name, &pattern)
}

/// `glob.glob` for absolute or relative patterns (no `**` recursion).
/// Like Python, wildcards do not match names starting with a dot unless the
/// pattern does, a pattern without wildcards only matches if it exists, and
/// a trailing `/` only matches directories.
pub fn glob(pattern: &str) -> Vec<PathBuf> {
    if let Some(stripped) = pattern.strip_suffix('/')
        && !stripped.is_empty()
        && !stripped.ends_with('/')
    {
        return glob(stripped)
            .into_iter()
            .filter(|path| path.is_dir())
            .map(|path| {
                let mut text = path.into_os_string();
                text.push("/");
                PathBuf::from(text)
            })
            .collect();
    }
    if !has_magic(pattern) {
        let path = PathBuf::from(pattern);
        return if path.symlink_metadata().is_ok() {
            vec![path]
        } else {
            Vec::new()
        };
    }
    let path = Path::new(pattern);
    let mut current: Vec<PathBuf> = vec![PathBuf::new()];
    for component in path.components() {
        let part = component.as_os_str().to_string_lossy();
        let mut next = Vec::new();
        for base in &current {
            if !has_magic(&part) {
                let candidate = base.join(component.as_os_str());
                if candidate.symlink_metadata().is_ok() || candidate.as_os_str() == "/" {
                    next.push(candidate);
                }
                continue;
            }
            let dir = if base.as_os_str().is_empty() {
                Path::new(".")
            } else {
                base.as_path()
            };
            let Ok(entries) = fs::read_dir(dir) else {
                continue;
            };
            let mut names: Vec<std::ffi::OsString> = entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name())
                .filter(|name| {
                    let text = name.to_string_lossy();
                    (!text.starts_with('.') || part.starts_with('.')) && fnmatch(&text, &part)
                })
                .collect();
            names.sort();
            next.extend(names.into_iter().map(|name| base.join(name)));
        }
        current = next;
        if current.is_empty() {
            break;
        }
    }
    current
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// `error`, naming the path it is about like Python's `OSError` does.
pub fn with_path(error: io::Error, path: &Path) -> io::Error {
    io::Error::new(error.kind(), format!("{error}: '{}'", path.display()))
}

/// pathlib's `mkdir(parents=True, exist_ok=True)` (default mode); an
/// error names the directory that could not be created.
pub fn mkdir_parents(path: &Path) -> io::Result<()> {
    let exist_ok = |error: io::Error| {
        if path.is_dir() {
            Ok(())
        } else {
            Err(with_path(error, path))
        }
    };
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                Some(parent) if parent != path => {
                    mkdir_parents(parent)?;
                    fs::create_dir(path).or_else(exist_ok)
                }
                _ => Err(with_path(error, path)),
            }
        }
        Err(error) => exist_ok(error),
    }
}

/// Write `data` to `path` atomically: a private temporary file in the same
/// directory, flushed to disk, then renamed over the target.
pub fn atomic_write(path: &Path, data: &[u8], mode: u32) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map_or_else(|| "file".into(), |name| name.to_string_lossy());
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    let mut attempt = 0;
    let (temp, mut file) = loop {
        let candidate = dir.join(format!(
            ".{name}.{}.{nanos}.{}.tmp",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&candidate)
        {
            Ok(file) => break (candidate, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempt < 100 => {
                attempt += 1;
            }
            Err(error) => return Err(with_path(error, &candidate)),
        }
    };
    let result = file
        .write_all(data)
        .and_then(|()| file.sync_all())
        .and_then(|()| {
            fs::set_permissions(&temp, fs::Permissions::from_mode(mode))
                .map_err(|error| with_path(error, &temp))
        })
        .and_then(|()| {
            fs::rename(&temp, path).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("{error}: '{}' -> '{}'", temp.display(), path.display()),
                )
            })
        });
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn path_helpers_match_python() {
        assert_eq!(normpath(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
        assert_eq!(normpath(Path::new("a/../../b")), PathBuf::from("../b"));
        assert_eq!(normpath(Path::new("/../x")), PathBuf::from("/x"));
        assert_eq!(
            expanduser("~/x", Path::new("/home/u")),
            PathBuf::from("/home/u/x")
        );
        assert_eq!(
            expanduser("~user/x", Path::new("/home/u")),
            PathBuf::from("~user/x")
        );
        let env = std::collections::HashMap::from([("HOME".to_string(), "/h".to_string())]);
        assert_eq!(
            expandvars("$HOME/a ${HOME}/b $NOPE ${NOPE} $", &env),
            "/h/a /h/b $NOPE ${NOPE} $"
        );
        // Names are ASCII, like Python's re.ASCII `\w`.
        assert_eq!(expandvars("$HOMEé", &env), "/hé");
        assert_eq!(clean(Path::new("p.json/")), PathBuf::from("p.json"));
        assert_eq!(clean(Path::new("./bad.json")), PathBuf::from("bad.json"));
        assert_eq!(
            clean(Path::new("sub//./p.json")),
            PathBuf::from("sub/p.json")
        );
        assert_eq!(clean(Path::new("/x/../y/")), PathBuf::from("/x/../y"));
        assert_eq!(clean(Path::new(".")), PathBuf::from("."));
        let home = Path::new("/home/u");
        assert_eq!(
            expanduser_users(Path::new("~/p.json"), home),
            home.join("p.json")
        );
        assert_eq!(expanduser_users(Path::new("~"), home), home);
        let unknown = Path::new("~no-such-user-wlrm/x");
        assert_eq!(expanduser_users(unknown, home), unknown);
        let root = expanduser_users(Path::new("~root/x"), home);
        assert!(root.is_absolute() && root.ends_with("x"), "{root:?}");
    }

    #[test]
    fn fnmatch_and_glob() {
        assert!(fnmatch("monitors.conf", "*.conf"));
        assert!(fnmatch("a1", "a[0-9]"));
        assert!(!fnmatch("ab", "a[!b]"));
        assert!(fnmatch("abc", "a?c"));
        assert!(!fnmatch("abc", "a?"));
        // Bracket rules of fnmatch.translate.
        assert!(fnmatch("]", "[]]"));
        assert!(fnmatch("a", "[!]x]"));
        assert!(!fnmatch("x", "[!]x]"));
        assert!(!fnmatch("]", "[!]x]"));
        assert!(fnmatch("[!]", "[!]"));
        assert!(!fnmatch("a", "[!]"));
        assert!(fnmatch("[", "["));
        assert!(fnmatch("-", "[a-]"));

        let dir = TempDir::new();
        fs::create_dir(dir.join("config.d")).unwrap();
        fs::create_dir(dir.join("config.d/sub.conf")).unwrap();
        for name in ["b.conf", "a.conf", ".hidden.conf", "notes.txt"] {
            fs::write(dir.join("config.d").join(name), "").unwrap();
        }
        let pattern = format!("{}/config.d/*.conf", dir.path().display());
        let found = glob(&pattern);
        assert_eq!(
            found,
            [
                dir.join("config.d/a.conf"),
                dir.join("config.d/b.conf"),
                dir.join("config.d/sub.conf")
            ]
        );
        // A trailing slash only matches directories.
        let dirs = glob(&format!("{pattern}/"));
        assert_eq!(
            dirs,
            [PathBuf::from(format!(
                "{}/",
                dir.join("config.d/sub.conf").display()
            ))]
        );
        let exact = format!("{}/config.d/notes.txt", dir.path().display());
        assert_eq!(glob(&exact), [PathBuf::from(&exact)]);
        assert!(glob(&format!("{}/missing/*", dir.path().display())).is_empty());
    }

    #[test]
    fn realpath_resolves_existing_parts() {
        let dir = TempDir::new();
        fs::create_dir(dir.join("real")).unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).unwrap();
        assert_eq!(
            realpath(&dir.join("link/missing.conf")),
            dir.join("real/missing.conf")
        );
    }

    #[test]
    fn realpath_follows_links_before_parent_dirs_like_python() {
        let dir = TempDir::new();
        fs::create_dir_all(dir.join("dotfiles/sway")).unwrap();
        fs::create_dir_all(dir.join("home/.config")).unwrap();
        std::os::unix::fs::symlink(dir.join("dotfiles/sway"), dir.join("home/.config/sway"))
            .unwrap();
        // `..` applies to the link's target, not to the link's name.
        assert_eq!(
            realpath(&dir.join("home/.config/sway/../common/x.conf")),
            dir.join("dotfiles/common/x.conf")
        );
        // Dangling links are followed to where they point.
        std::os::unix::fs::symlink(dir.join("nowhere/target.conf"), dir.join("dangling.conf"))
            .unwrap();
        assert_eq!(
            realpath(&dir.join("dangling.conf")),
            dir.join("nowhere/target.conf")
        );
        // Relative links resolve against the link's directory; loops stop.
        std::os::unix::fs::symlink("../dotfiles", dir.join("home/dots")).unwrap();
        assert_eq!(
            realpath(&dir.join("home/dots/sway")),
            dir.join("dotfiles/sway")
        );
        std::os::unix::fs::symlink("loop", dir.join("loop")).unwrap();
        assert_eq!(realpath(&dir.join("loop/x")), dir.join("loop/x"));
    }

    #[test]
    fn iso_timestamps() {
        assert_eq!(iso_from_unix(0), "1970-01-01T00:00:00+00:00");
        assert_eq!(iso_from_unix(1_767_258_000), "2026-01-01T09:00:00+00:00");
        assert_eq!(iso_from_unix(951_782_400), "2000-02-29T00:00:00+00:00");
    }

    #[test]
    fn atomic_write_replaces_and_sets_mode() {
        let dir = TempDir::new();
        let path = dir.join("file.json");
        atomic_write(&path, b"one", 0o600).unwrap();
        atomic_write(&path, b"two", 0o644).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644);
        let leftovers: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(leftovers.len(), 1);
    }

    #[test]
    fn mkdir_parents_names_the_directory_that_failed() {
        let dir = TempDir::new();
        mkdir_parents(&dir.join("a/b/c")).unwrap();
        assert!(dir.join("a/b/c").is_dir());
        // Existing directories are fine.
        mkdir_parents(&dir.join("a/b")).unwrap();
        // A file in the way: the error says which path.
        fs::write(dir.join("a/file"), "").unwrap();
        let error = mkdir_parents(&dir.join("a/file/sway")).unwrap_err();
        let message = error.to_string();
        assert!(
            message.ends_with(&format!("'{}'", dir.join("a/file/sway").display())),
            "{message}"
        );
        let error = mkdir_parents(&dir.join("a/file")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert!(
            error
                .to_string()
                .ends_with(&format!("'{}'", dir.join("a/file").display()))
        );
    }
}
