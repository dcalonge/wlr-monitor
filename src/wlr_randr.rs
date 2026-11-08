//! Reading and applying output settings through the wlr-randr command line.

use anyhow::{Context, Result};

use crate::apply::{Applied, ApplyResult, apply_order};
use crate::exec::{APPLY_TIMEOUT, READ_TIMEOUT, run_tool};
use crate::output::{Output, OutputState, fmt_num};

#[derive(Debug, Clone)]
pub struct WlrRandr {
    binary: String,
}

impl WlrRandr {
    /// `binary` overrides `$WLR_RANDR`, which overrides the `PATH` lookup.
    pub fn new(binary: Option<&str>) -> Self {
        let binary = binary
            .map(str::to_string)
            .or_else(|| std::env::var("WLR_RANDR").ok())
            .filter(|binary| !binary.is_empty())
            .unwrap_or_else(|| "wlr-randr".to_string());
        Self { binary }
    }

    pub fn read(&self) -> Result<Vec<Output>> {
        let json = run_tool(&self.binary, &["--json"], READ_TIMEOUT)?;
        serde_json::from_str(&json).context("wlr-randr returned unexpected JSON")
    }

    pub fn apply(&self, states: &[OutputState]) -> Applied {
        let results = apply_order(states)
            .into_iter()
            .map(|state| {
                let args = Self::args_for(state);
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                ApplyResult::new(
                    &state.name,
                    run_tool(&self.binary, &args, APPLY_TIMEOUT).map(drop),
                )
            })
            .collect();
        Applied {
            results,
            warnings: Vec::new(),
        }
    }

    fn args_for(state: &OutputState) -> Vec<String> {
        let mut args = vec!["--output".to_string(), state.name.clone()];
        if !state.enabled {
            args.push("--off".into());
            return args;
        }
        args.push("--on".into());
        if let Some(mode) = &state.mode {
            args.extend(["--mode".into(), mode.wlr_randr_arg()]);
        }
        args.extend([
            "--pos".into(),
            format!("{},{}", state.position.x, state.position.y),
            "--scale".into(),
            fmt_num(state.scale, 6),
            "--transform".into(),
            state.transform.as_str().into(),
            "--adaptive-sync".into(),
            if state.adaptive_sync {
                "enabled"
            } else {
                "disabled"
            }
            .into(),
        ]);
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::tests::script;
    use crate::output::tests::{FIXTURE, fixture_outputs};

    fn value_after<'a>(args: &'a [String], flag: &str) -> &'a str {
        let index = args.iter().position(|arg| arg == flag).unwrap();
        &args[index + 1]
    }

    #[test]
    fn builds_arguments_for_enabled_and_disabled_outputs() {
        let mut state = fixture_outputs()[0].state();
        let args = WlrRandr::args_for(&state);
        assert_eq!(args[..3], ["--output", "HDMI-A-1", "--on"]);
        assert_eq!(value_after(&args, "--mode"), "1920x1080@100");
        assert_eq!(value_after(&args, "--pos"), "0,0");
        assert_eq!(value_after(&args, "--scale"), "1");
        assert_eq!(value_after(&args, "--adaptive-sync"), "disabled");

        state.enabled = false;
        assert_eq!(
            WlrRandr::args_for(&state),
            ["--output", "HDMI-A-1", "--off"]
        );
    }

    #[test]
    fn reads_json_and_reports_failures() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("outputs.json");
        std::fs::write(&fixture, FIXTURE).unwrap();
        let good = script(
            dir.path(),
            "good",
            &format!("[ \"$1\" = --json ] && cat {}", fixture.display()),
        );
        let names: Vec<_> = WlrRandr::new(Some(&good))
            .read()
            .unwrap()
            .into_iter()
            .map(|o| o.name)
            .collect();
        assert_eq!(names, ["HDMI-A-1", "eDP-1"]);

        let bad = script(dir.path(), "bad", "echo not json");
        assert!(
            WlrRandr::new(Some(&bad))
                .read()
                .unwrap_err()
                .to_string()
                .contains("unexpected JSON")
        );
    }

    #[test]
    fn applies_enabled_outputs_first_and_collects_failures() {
        let dir = tempfile::tempdir().unwrap();
        let tool = script(
            dir.path(),
            "wlr",
            "case \"$*\" in *eDP-1*) echo 'bad mode' >&2; exit 1;; esac",
        );
        let mut states: Vec<_> = fixture_outputs().iter().map(Output::state).collect();
        states[0].enabled = false;
        let applied = WlrRandr::new(Some(&tool)).apply(&states);
        let names: Vec<_> = applied.results.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["eDP-1", "HDMI-A-1"]);
        assert_eq!(applied.failures().count(), 1);
        assert!(
            applied.results[0]
                .error
                .as_ref()
                .unwrap()
                .contains("bad mode")
        );
    }
}
