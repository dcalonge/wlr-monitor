//! File helpers shared by the profile store and the compositor include.

use std::fs::{self, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Replace `path` with `data` in one step, creating parent directories.
pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> io::Result<()> {
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(data)?;
    file.as_file()
        .set_permissions(Permissions::from_mode(mode))?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// Like `write_atomic`, but leaves the file alone when it already has `data`.
pub fn write_if_changed(path: &Path, data: &[u8], mode: u32) -> io::Result<()> {
    if fs::read(path).is_ok_and(|current| current == data) {
        return Ok(());
    }
    write_atomic(path, data, mode)
}

pub fn home_dir() -> PathBuf {
    std::env::home_dir().unwrap_or_default()
}

/// An XDG base directory: `$var` when set, else `~/fallback`.
pub fn xdg_dir(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .filter(|value| !value.is_empty())
        .map_or_else(|| home_dir().join(fallback), PathBuf::from)
}
