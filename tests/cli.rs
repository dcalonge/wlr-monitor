//! Runs the real binary against a fake `wlr-randr` script.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

const OUTPUTS: &str = include_str!("fixtures/outputs.json");

struct Env {
    dir: TempDir,
}

impl Env {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("outputs.json"), OUTPUTS).unwrap();
        let fake = dir.path().join("wlr-randr");
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = --json ]; then cat '{dir}/outputs.json'; else echo \"$@\" >> '{dir}/calls'; fi\n",
            dir = dir.path().display()
        );
        fs::write(&fake, script).unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_wlr-monitor"))
            .args(["--compositor", "wlroots", "--wlr-randr"])
            .arg(self.path().join("wlr-randr"))
            .arg("--config")
            .arg(self.path())
            .args(args)
            .env("XDG_STATE_HOME", self.path())
            .output()
            .unwrap()
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.path().join("calls")).unwrap_or_default()
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn prints_outputs() {
    let out = Env::new().run(&["--outputs"]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(
        text.contains("HDMI-A-1") && text.contains("1920x1080@100"),
        "{text}"
    );
    assert!(text.contains("eDP-1") && text.contains("1.2"), "{text}");
}

#[test]
fn applies_a_saved_profile_and_marks_it_active() {
    let env = Env::new();
    fs::write(
        env.path().join("profiles.json"),
        r#"{"version": 2, "order": ["work"], "profiles": {"work": {"outputs": {
            "HDMI-A-1": {"enabled": true, "position": {"x": 0, "y": 0}, "scale": 2.0},
            "DP-9": {"enabled": false}
        }}}}"#,
    )
    .unwrap();

    let out = env.run(&["--profile", "work"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "Applied HDMI-A-1.\n");
    let err = stderr(&out);
    assert!(
        err.contains("Left unchanged (not in profile): eDP-1"),
        "{err}"
    );
    assert!(err.contains("Skipped (not connected): DP-9"), "{err}");
    assert!(
        env.calls().contains("--output HDMI-A-1 --on"),
        "{}",
        env.calls()
    );
    assert!(env.calls().contains("--scale 2"), "{}", env.calls());

    // The fake compositor state did not change, so the profile is not active.
    assert_eq!(stdout(&env.run(&["--list-profiles"])), "work\n");
}

#[test]
fn refuses_to_turn_everything_off() {
    let env = Env::new();
    fs::write(
        env.path().join("profiles.json"),
        r#"{"version": 2, "order": ["dark"], "profiles": {"dark": {"outputs": {
            "HDMI-A-1": {"enabled": false}, "eDP-1": {"enabled": false}}}}}"#,
    )
    .unwrap();
    let out = env.run(&["--profile", "dark"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("turn off every connected monitor"));
    assert_eq!(env.calls(), "");
}

#[test]
fn reports_unknown_profile_and_bad_usage() {
    let env = Env::new();
    let out = env.run(&["--profile", "nope"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("no profile named \"nope\" (available: none)"));

    let out = env.run(&["--outputs", "--list-profiles"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn prints_compositor_config() {
    let env = Env::new();
    let include = env.path().join("wlr-monitor.conf");
    let out = Command::new(env!("CARGO_BIN_EXE_wlr-monitor"))
        .args(["--compositor", "sway", "--wlr-randr"])
        .arg(env.path().join("wlr-randr"))
        .arg("--include")
        .arg(&include)
        .arg("--print-config")
        .env("XDG_STATE_HOME", env.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out)
            .contains("output \"eDP-1\" enable mode 1920x1080@60.000Hz pos 1920 0 scale 1.2")
    );
    assert!(!include.exists());
}
