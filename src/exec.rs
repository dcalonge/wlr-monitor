//! Running external tools with a timeout.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use wait_timeout::ChildExt;

pub const READ_TIMEOUT: Duration = Duration::from_secs(10);
pub const APPLY_TIMEOUT: Duration = Duration::from_secs(15);

/// Run `program` with `args` and return its stdout.
///
/// Errors carry a readable message: the tool is missing, timed out, or
/// exited with a failure (its own error text is the detail).
pub fn run_tool(program: &str, args: &[&str], timeout: Duration) -> Result<String> {
    let tool = Path::new(program)
        .file_name()
        .map_or(program.into(), |name| name.to_string_lossy());
    let mut command = Command::new(program);
    command.args(args);
    let (success, stdout, stderr) = match run(&mut command, timeout) {
        Ok(done) => done,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            bail!("{tool} was not found. Install it and make sure it is on PATH.")
        }
        Err(error) if error.kind() == io::ErrorKind::TimedOut => bail!("{tool} timed out."),
        Err(error) => return Err(anyhow!("{tool} could not be run: {error}")),
    };
    if success {
        return Ok(stdout);
    }
    let detail = [stderr.trim(), stdout.trim()]
        .into_iter()
        .find(|text| !text.is_empty())
        .unwrap_or("unknown error");
    bail!("{tool} failed: {detail}")
}

/// The full path of `program`: as given when it has a slash, else the first
/// match on `PATH`.
pub fn which(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        return Path::new(program).is_file().then(|| program.into());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// Returns whether the command succeeded, with its stdout and stderr.
fn run(command: &mut Command, timeout: Duration) -> io::Result<(bool, String, String)> {
    let mut child = spawn(command)?;
    // Drain both pipes while waiting, so a chatty tool cannot block on a full pipe.
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let Some(status) = child.wait_timeout(timeout)? else {
        child.kill()?;
        child.wait()?;
        return Err(io::ErrorKind::TimedOut.into());
    };
    let join = |handle: JoinHandle<String>| handle.join().unwrap_or_default();
    Ok((status.success(), join(stdout), join(stderr)))
}

/// Spawns `command` with null stdin and piped stdout and stderr.
fn spawn(command: &mut Command) -> io::Result<Child> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Another thread's fork can hold the write handle of a file we just made,
    // so our exec briefly sees ETXTBSY. It clears at once.
    let mut tries = 0;
    loop {
        match command.spawn() {
            Err(error) if error.kind() == io::ErrorKind::ExecutableFileBusy && tries < 100 => {
                tries += 1;
                thread::sleep(Duration::from_millis(1));
            }
            result => return result,
        }
    }
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Write an executable shell script into `dir` and return its path.
    pub fn script(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn reports_output_failures_and_missing_tools() {
        let dir = tempfile::tempdir().unwrap();
        let ok = script(dir.path(), "ok", "echo hi");
        assert_eq!(run_tool(&ok, &[], READ_TIMEOUT).unwrap(), "hi\n");

        let bad = script(dir.path(), "bad", "echo oops >&2; exit 3");
        let error = run_tool(&bad, &[], READ_TIMEOUT).unwrap_err();
        assert_eq!(error.to_string(), "bad failed: oops");

        let error = run_tool("/nonexistent/tool", &[], READ_TIMEOUT).unwrap_err();
        assert!(error.to_string().contains("tool was not found"));

        let slow = script(dir.path(), "slow", "sleep 5");
        let error = run_tool(&slow, &[], Duration::from_millis(100)).unwrap_err();
        assert_eq!(error.to_string(), "slow timed out.");
    }
}
