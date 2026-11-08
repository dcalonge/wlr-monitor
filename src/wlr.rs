//! Thin wrapper around the wlr-randr command line tool.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::model::{Output, OutputState, format_number};
use crate::process::{self, RunError};

/// wlr-randr cannot be run or reports an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WlrRandrError(pub String);

impl fmt::Display for WlrRandrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WlrRandrError {}

const NOT_FOUND: &str = "wlr-randr was not found. Install wlr-randr and make sure it is on PATH.";

/// The outcome of one configuration command.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ApplyResult {
    pub name: String,
    pub command: Vec<String>,
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
    pub error: String,
    pub warning: String,
}

impl ApplyResult {
    pub fn success(name: impl Into<String>, command: Vec<String>) -> Self {
        Self {
            name: name.into(),
            command,
            ok: true,
            ..Self::default()
        }
    }

    pub fn failure(
        name: impl Into<String>,
        command: Vec<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            command,
            ok: false,
            error: error.into(),
            ..Self::default()
        }
    }

    pub fn detail(&self) -> String {
        if !self.error.is_empty() {
            self.error.clone()
        } else if !self.stderr.trim().is_empty() {
            self.stderr.trim().to_string()
        } else {
            self.stdout.trim().to_string()
        }
    }
}

/// Read and apply output state through wlr-randr.
#[derive(Debug, Clone)]
pub struct WlrRandr {
    pub binary: String,
}

impl Default for WlrRandr {
    fn default() -> Self {
        Self::new(None)
    }
}

impl WlrRandr {
    pub fn new(binary: Option<&str>) -> Self {
        let binary = binary
            .filter(|binary| !binary.is_empty())
            .map(str::to_string)
            .or_else(|| std::env::var("WLR_RANDR").ok().filter(|v| !v.is_empty()))
            .or_else(|| {
                // A path that is not UTF-8 is left to the exec-time PATH lookup.
                process::which("wlr-randr", None)
                    .and_then(|path| path.into_os_string().into_string().ok())
            })
            .unwrap_or_else(|| "wlr-randr".to_string());
        Self { binary }
    }

    pub fn ensure_available(&self) -> Result<(), WlrRandrError> {
        if process::which(&self.binary, None).is_none() && !Path::new(&self.binary).exists() {
            return Err(WlrRandrError(NOT_FOUND.to_string()));
        }
        Ok(())
    }

    pub fn read_outputs(&self) -> Result<Vec<Output>, WlrRandrError> {
        self.ensure_available()?;
        let stdout = self.run(&[self.binary.as_str(), "--json"])?;
        parse_outputs(&stdout)
    }

    pub fn command_for(&self, state: &OutputState) -> Vec<String> {
        let mut command = vec![
            self.binary.clone(),
            "--output".to_string(),
            state.name.clone(),
        ];
        if !state.enabled {
            command.push("--off".to_string());
            return command;
        }
        command.push("--on".to_string());
        if let Some(mode) = &state.mode {
            command.extend(["--mode".to_string(), mode.cli_value()]);
        }
        command.extend([
            "--pos".to_string(),
            format!("{},{}", state.x, state.y),
            "--scale".to_string(),
            format_number(state.scale),
            "--transform".to_string(),
            state.transform.clone(),
            "--adaptive-sync".to_string(),
            if state.adaptive_sync {
                "enabled"
            } else {
                "disabled"
            }
            .to_string(),
        ]);
        command
    }

    pub fn apply_states(&self, states: &[OutputState]) -> Result<Vec<ApplyResult>, WlrRandrError> {
        self.ensure_available()?;
        Ok(apply_order(states)
            .iter()
            .map(|state| {
                run_apply_command(
                    &state.name,
                    self.command_for(state),
                    Duration::from_secs(15),
                    &[],
                    None,
                )
            })
            .collect())
    }

    fn run(&self, command: &[&str]) -> Result<String, WlrRandrError> {
        match process::run(command, Duration::from_secs(10), None) {
            Err(RunError::NotFound(_)) => Err(WlrRandrError(NOT_FOUND.to_string())),
            Err(RunError::TimedOut) => Err(WlrRandrError("wlr-randr timed out.".to_string())),
            Err(RunError::Other(error)) => Err(WlrRandrError(format!(
                "wlr-randr could not be run: {error}"
            ))),
            Ok(done) if !done.success() => {
                let detail = [done.stderr.trim(), done.stdout.trim()]
                    .into_iter()
                    .find(|text| !text.is_empty())
                    .unwrap_or("unknown error")
                    .to_string();
                Err(WlrRandrError(format!("wlr-randr failed: {detail}")))
            }
            Ok(done) => Ok(done.stdout),
        }
    }
}

/// Something that reads and applies output state through the
/// wlr-output-management protocol. `WlrRandr` in production; tests use fakes.
pub trait Wlr: Send + Sync {
    fn read_outputs(&self) -> Result<Vec<Output>, WlrRandrError>;
    fn apply_states(&self, states: &[OutputState]) -> Result<Vec<ApplyResult>, WlrRandrError>;
}

impl Wlr for WlrRandr {
    fn read_outputs(&self) -> Result<Vec<Output>, WlrRandrError> {
        WlrRandr::read_outputs(self)
    }

    fn apply_states(&self, states: &[OutputState]) -> Result<Vec<ApplyResult>, WlrRandrError> {
        WlrRandr::apply_states(self, states)
    }
}

/// Parse the output of `wlr-randr --json`.
pub fn parse_outputs(stdout: &str) -> Result<Vec<Output>, WlrRandrError> {
    let data: Value = serde_json::from_str(stdout)
        .map_err(|error| WlrRandrError(format!("wlr-randr returned invalid JSON: {error}")))?;
    let Value::Array(items) = data else {
        return Err(WlrRandrError(
            "wlr-randr returned an unexpected JSON document.".to_string(),
        ));
    };
    items
        .iter()
        .map(|item| {
            Output::from_json(item).map_err(|error| {
                WlrRandrError(format!("wlr-randr returned an unexpected output: {error}"))
            })
        })
        .collect()
}

/// Enable outputs before disabling others, so a session never has none.
pub fn apply_order(states: &[OutputState]) -> Vec<OutputState> {
    let (enabled, disabled): (Vec<_>, Vec<_>) =
        states.iter().cloned().partition(|state| state.enabled);
    enabled.into_iter().chain(disabled).collect()
}

/// Run one configuration command and wrap the outcome in an ApplyResult.
///
/// `error_markers` catches tools that report failures on stdout while still
/// exiting with status 0 (mango's `mmsg` does this). `env` defaults to the
/// current process environment.
pub fn run_apply_command(
    name: &str,
    command: Vec<String>,
    timeout: Duration,
    error_markers: &[&str],
    env: Option<&HashMap<String, String>>,
) -> ApplyResult {
    let tool = command
        .first()
        .map(|program| {
            PathBuf::from(program).file_name().map_or_else(
                || program.clone(),
                |name| name.to_string_lossy().into_owned(),
            )
        })
        .unwrap_or_else(|| "command".to_string());
    match process::run(&command, timeout, env) {
        Err(RunError::TimedOut) => ApplyResult::failure(name, command, format!("{tool} timed out")),
        Err(RunError::NotFound(error) | RunError::Other(error)) => {
            let error = format!("{tool}: {error}");
            ApplyResult::failure(name, command, error)
        }
        Ok(done) => {
            let output = format!("{}\n{}", done.stdout, done.stderr);
            let ok = done.success() && !error_markers.iter().any(|marker| output.contains(marker));
            ApplyResult {
                name: name.to_string(),
                command,
                ok,
                stdout: done.stdout,
                stderr: done.stderr,
                ..ApplyResult::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Mode;
    use crate::model::tests::{FIXTURE, load_outputs};
    use crate::testutil::{TempDir, quote};

    fn state(name: &str, enabled: bool, mode: Option<Mode>) -> OutputState {
        OutputState {
            name: name.into(),
            enabled,
            mode,
            x: 0,
            y: 0,
            scale: 1.0,
            transform: "normal".into(),
            adaptive_sync: false,
            hardware_id: String::new(),
            description: String::new(),
            hdr: None,
            bit_depth: None,
            color_profile: None,
        }
    }

    fn value_after<'a>(command: &'a [String], flag: &str) -> &'a str {
        let index = command.iter().position(|item| item == flag).unwrap();
        &command[index + 1]
    }

    #[test]
    fn command_for_enabled_output() {
        let backend = WlrRandr::new(Some("/usr/bin/wlr-randr"));
        let command = backend.command_for(&load_outputs()[0].state());
        assert_eq!(command[..3], ["/usr/bin/wlr-randr", "--output", "HDMI-A-1"]);
        assert!(command.contains(&"--on".to_string()));
        assert_eq!(value_after(&command, "--mode"), "1920x1080@100");
        assert_eq!(value_after(&command, "--pos"), "0,0");
        assert_eq!(value_after(&command, "--scale"), "1");
        assert_eq!(value_after(&command, "--transform"), "normal");
        assert_eq!(value_after(&command, "--adaptive-sync"), "disabled");
    }

    #[test]
    fn command_for_disabled_output_only_turns_it_off() {
        let backend = WlrRandr::new(Some("wlr-randr"));
        let mut state = load_outputs()[0].state();
        state.enabled = false;
        assert_eq!(
            backend.command_for(&state),
            ["wlr-randr", "--output", "HDMI-A-1", "--off"]
        );
    }

    #[test]
    fn read_outputs_runs_wlr_randr_json() {
        let dir = TempDir::new();
        dir.join("outputs.json");
        std::fs::write(dir.join("outputs.json"), FIXTURE).unwrap();
        let script = dir.script(
            "wlr-randr",
            &format!(
                "[ \"$1\" = --json ] || exit 9\ncat {}",
                quote(&dir.join("outputs.json").to_string_lossy())
            ),
        );
        let outputs = WlrRandr::new(script.to_str()).read_outputs().unwrap();
        let names: Vec<_> = outputs.iter().map(|output| output.name.as_str()).collect();
        assert_eq!(names, ["HDMI-A-1", "eDP-1"]);
    }

    #[test]
    fn read_outputs_reports_bad_json_and_failures() {
        let dir = TempDir::new();
        let bad = dir.script("bad", "echo not json");
        let error = WlrRandr::new(bad.to_str()).read_outputs().unwrap_err();
        assert!(error.0.contains("invalid JSON"), "{error}");

        let failing = dir.script("failing", "echo 'no wayland' >&2; exit 1");
        let error = WlrRandr::new(failing.to_str()).read_outputs().unwrap_err();
        assert_eq!(error.0, "wlr-randr failed: no wayland");

        let missing = WlrRandr::new(Some("/nonexistent/wlr-randr"));
        assert!(missing.read_outputs().unwrap_err().0.contains("not found"));

        assert!(
            parse_outputs("{}")
                .unwrap_err()
                .0
                .contains("unexpected JSON")
        );
        assert!(
            parse_outputs("[{}]")
                .unwrap_err()
                .0
                .contains("unexpected output")
        );
    }

    #[test]
    fn apply_states_collects_failures_in_apply_order() {
        let dir = TempDir::new();
        let script = dir.script(
            "wlr-randr",
            "case \"$*\" in *eDP-1*) echo 'bad mode' >&2; exit 1;; esac",
        );
        let mut states: Vec<_> = load_outputs().iter().map(Output::state).collect();
        let results = WlrRandr::new(script.to_str())
            .apply_states(&states)
            .unwrap();
        assert_eq!(
            results.iter().map(|r| r.ok).collect::<Vec<_>>(),
            [true, false]
        );
        assert_eq!(results[1].detail(), "bad mode");

        // Outputs are enabled before others are turned off.
        states[0].enabled = false;
        let ordered = apply_order(&states);
        assert_eq!(ordered[0].name, "eDP-1");
        assert_eq!(ordered[1].name, "HDMI-A-1");
    }

    #[test]
    fn apply_command_reports_markers_and_missing_tools() {
        let dir = TempDir::new();
        let script = dir.script("mmsg", "echo 'Error: unknown dispatch'");
        let marked = run_apply_command(
            "config",
            vec![script.to_string_lossy().into_owned()],
            Duration::from_secs(5),
            &["Error:"],
            None,
        );
        assert!(!marked.ok);
        assert_eq!(marked.detail(), "Error: unknown dispatch");
        let missing = run_apply_command(
            "x",
            vec!["/nonexistent/swaymsg".into()],
            Duration::from_secs(5),
            &[],
            None,
        );
        assert!(!missing.ok);
        assert!(missing.error.starts_with("swaymsg: "));
    }

    #[test]
    fn state_with_no_mode_is_valid() {
        let state = state("DP-1", false, None);
        assert_eq!(state.to_dict()["mode"], Value::Null);
        let restored = OutputState::from_dict("DP-1", &Value::Object(state.to_dict())).unwrap();
        assert!(restored.mode.is_none());
    }

    #[test]
    fn mode_cli_value_keeps_precision() {
        let state = state("DP-1", true, Some(Mode::new(2560, 1440, 59.950001)));
        let command = WlrRandr::new(Some("wlr-randr")).command_for(&state);
        assert_eq!(value_after(&command, "--mode"), "2560x1440@59.950001");
    }
}
