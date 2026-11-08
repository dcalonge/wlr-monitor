//! End-to-end checks of the installed binary's command line.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

const FIXTURE: &str = include_str!("fixtures/outputs.json");

struct Sandbox(PathBuf);

impl Sandbox {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("wlr-monitor-it-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("outputs.json"), FIXTURE).unwrap();
        let script = dir.join("bin/wlr-randr");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nwhile IFS= read -r line; do printf '%s\\n' \"$line\"; done < '{}'\n",
                dir.join("outputs.json").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self(dir)
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_wlr-monitor"))
            .args(args)
            .env_clear()
            .env("HOME", self.0.join("home"))
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("XDG_STATE_HOME", self.0.join("state"))
            .env("PATH", self.0.join("bin"))
            .env("WLR_RANDR", self.0.join("bin/wlr-randr"))
            // A variable that is not UTF-8 must not break anything.
            .env("BROKEN", OsStr::from_bytes(b"caf\xe9"))
            .output()
            .unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn one_shot_commands_work_end_to_end() {
    let sandbox = Sandbox::new("oneshot");
    let outputs = sandbox.run(&["--outputs"]);
    assert!(outputs.status.success(), "{}", text(&outputs.stderr));
    assert!(text(&outputs.stdout).contains("HDMI-A-1       yes  1920x1080@100"));

    let listed = sandbox.run(&["--list-profiles"]);
    assert!(listed.status.success());
    assert_eq!(text(&listed.stdout), "");

    let version = sandbox.run(&["--version"]);
    assert_eq!(
        text(&version.stdout),
        format!("wlr-monitor {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_and_errors_are_plain_text_when_piped() {
    let sandbox = Sandbox::new("plain");
    let help = sandbox.run(&["--help"]);
    assert!(help.status.success());
    assert!(text(&help.stdout).contains("--print-config"));
    assert!(!help.stdout.contains(&0x1b));
    let error = sandbox.run(&["--bogus"]);
    assert_eq!(error.status.code(), Some(2));
    assert!(!error.stderr.contains(&0x1b));
}

#[test]
fn the_tui_needs_a_terminal() {
    let sandbox = Sandbox::new("tui");
    // No terminal on stdin/stdout: a clean error, not a hang or a panic.
    let output = sandbox.run(&[]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stderr));
    assert!(text(&output.stderr).starts_with("wlr-monitor: "));
}
