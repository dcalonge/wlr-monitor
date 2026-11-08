//! Running external tools (wlr-randr, swaymsg, niri, mmsg) with timeouts.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// The outcome of a finished command.
#[derive(Debug, Clone, Default)]
pub struct Completed {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Completed {
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }
}

#[derive(Debug)]
pub enum RunError {
    /// The program does not exist (or is not executable).
    NotFound(io::Error),
    /// The program ran longer than the timeout and was killed.
    TimedOut,
    Other(io::Error),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(error) | Self::Other(error) => write!(f, "{error}"),
            Self::TimedOut => f.write_str("timed out"),
        }
    }
}

/// Python's `os.access(path, X_OK) and not isdir(path)`.
fn is_executable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    if !path.metadata().is_ok_and(|meta| !meta.is_dir()) {
        return false;
    }
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `path` is a valid NUL-terminated string for the whole call.
    unsafe { libc::access(path.as_ptr(), libc::X_OK) == 0 }
}

/// Like Python's `shutil.which`: a name with a slash is checked as is,
/// otherwise every directory in `path` (default: `$PATH`, or `/bin:/usr/bin`
/// when it is unset) is searched. An empty search path finds nothing.
pub fn which(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    if name.contains('/') {
        let candidate = PathBuf::from(name);
        return is_executable(&candidate).then_some(candidate);
    }
    let search: OsString = match path {
        Some(path) => path.to_os_string(),
        None => std::env::var_os("PATH").unwrap_or_else(|| OsString::from("/bin:/usr/bin")),
    };
    if search.is_empty() {
        return None;
    }
    std::env::split_paths(&search)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

fn drain<R: Read + Send + 'static>(
    reader: Option<R>,
    stream: usize,
    done: mpsc::Sender<(usize, String)>,
) {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut reader) = reader {
            let _ = reader.read_to_end(&mut bytes);
        }
        let _ = done.send((stream, String::from_utf8_lossy(&bytes).into_owned()));
    });
}

/// ETXTBSY: another thread forked while the executable was open for writing.
const TEXT_FILE_BUSY: i32 = 26;

fn spawn_retrying(process: &mut Command) -> io::Result<Child> {
    let mut attempts = 0;
    loop {
        match process.spawn() {
            Err(error) if error.raw_os_error() == Some(TEXT_FILE_BUSY) && attempts < 10 => {
                attempts += 1;
                thread::sleep(Duration::from_millis(5 * attempts));
            }
            result => return result,
        }
    }
}

fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Wait for both output streams to close, then for the exit status. The
/// pipes close the moment the tool exits, so this returns without polling
/// delays in the common case.
fn collect(
    child: &mut Child,
    done: &mpsc::Receiver<(usize, String)>,
    deadline: Instant,
) -> Result<Completed, RunError> {
    let mut streams: [Option<String>; 2] = [None, None];
    while streams.iter().any(Option::is_none) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match done.recv_timeout(remaining) {
            Ok((stream, text)) => streams[stream] = Some(text),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                kill(child);
                return Err(RunError::TimedOut);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().unwrap_or(-1),
            Ok(None) if Instant::now() >= deadline => {
                kill(child);
                return Err(RunError::TimedOut);
            }
            // Closed its output but still running (e.g. it forked).
            Ok(None) => thread::sleep(Duration::from_millis(1)),
            Err(error) => {
                kill(child);
                return Err(RunError::Other(error));
            }
        }
    };
    let [stdout, stderr] = streams;
    Ok(Completed {
        status: Some(status),
        stdout: stdout.unwrap_or_default(),
        stderr: stderr.unwrap_or_default(),
    })
}

/// Run `command` (program first), capturing its output.
///
/// `env` replaces the whole environment when given. Stdin is closed so a
/// tool can never wait for input from the terminal.
pub fn run<S: AsRef<OsStr>>(
    command: &[S],
    timeout: Duration,
    env: Option<&HashMap<String, String>>,
) -> Result<Completed, RunError> {
    let Some((program, args)) = command.split_first() else {
        return Err(RunError::NotFound(io::Error::new(
            io::ErrorKind::NotFound,
            "empty command",
        )));
    };
    let mut process = Command::new(program);
    process
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(env) = env {
        process.env_clear().envs(env);
    }
    let mut child = spawn_retrying(&mut process).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied => RunError::NotFound(error),
        _ => RunError::Other(error),
    })?;
    let deadline = Instant::now() + timeout;
    let (sender, receiver) = mpsc::channel();
    drain(child.stdout.take(), 0, sender.clone());
    drain(child.stderr.take(), 1, sender);
    collect(&mut child, &receiver, deadline)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_output_and_status() {
        let done = run(
            &["/bin/sh", "-c", "echo out; echo err >&2; exit 3"],
            Duration::from_secs(5),
            None,
        )
        .unwrap();
        assert_eq!(done.status, Some(3));
        assert_eq!(done.stdout, "out\n");
        assert_eq!(done.stderr, "err\n");
        assert!(!done.success());
    }

    #[test]
    fn missing_programs_and_timeouts_are_reported() {
        assert!(matches!(
            run(&["/nonexistent/tool"], Duration::from_secs(1), None),
            Err(RunError::NotFound(_))
        ));
        let started = Instant::now();
        // A shell builtin loop: the Nix build sandbox has /bin/sh but no sleep.
        assert!(matches!(
            run(
                &["/bin/sh", "-c", "while :; do :; done"],
                Duration::from_millis(100),
                None
            ),
            Err(RunError::TimedOut)
        ));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn env_replaces_the_environment() {
        let env = HashMap::from([("ONLY".to_string(), "this".to_string())]);
        let done = run(
            &["/bin/sh", "-c", "echo \"$ONLY:${HOME:-unset}\""],
            Duration::from_secs(5),
            Some(&env),
        )
        .unwrap();
        assert_eq!(done.stdout, "this:unset\n");
    }

    #[test]
    fn which_checks_paths_and_names() {
        let bin = Some(OsStr::new("/bin"));
        assert_eq!(which("sh", bin), Some(PathBuf::from("/bin/sh")));
        assert_eq!(which("/bin/sh", None), Some(PathBuf::from("/bin/sh")));
        assert_eq!(which("definitely-not-a-tool", bin), None);
        assert_eq!(which("", None), None);
        // An empty search path finds nothing (no lookup in the working dir).
        assert_eq!(which("sh", Some(OsStr::new(""))), None);
        // Non-UTF-8 entries do not hide the others.
        use std::os::unix::ffi::OsStrExt;
        let mixed = OsStr::from_bytes(b"/caf\xe9:/bin");
        assert_eq!(which("sh", Some(mixed)), Some(PathBuf::from("/bin/sh")));
        // Directories and non-executable files are not programs.
        assert_eq!(which("/bin", None), None);
        let dir = crate::testutil::TempDir::new();
        std::fs::write(dir.join("tool"), "").unwrap();
        assert_eq!(which("tool", Some(dir.path().as_os_str())), None);
    }
}
