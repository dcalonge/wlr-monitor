use std::ffi::OsString;
use std::fs;

use super::*;
use crate::model::tests::load_outputs;
use crate::testutil::{Buffer, FakeWlr, TempDir};

/// A sandboxed HOME / XDG layout and captured streams.
struct Harness {
    dir: TempDir,
    env: HashMap<String, String>,
    out: Buffer,
    err: Buffer,
    tui: Vec<bool>,
}

impl Harness {
    fn new() -> Self {
        let dir = TempDir::new();
        fs::create_dir_all(dir.join("bin")).unwrap();
        let env = HashMap::from([
            ("HOME".to_string(), dir.join("home").display().to_string()),
            (
                "XDG_CONFIG_HOME".to_string(),
                dir.join("config").display().to_string(),
            ),
            (
                "XDG_STATE_HOME".to_string(),
                dir.join("state").display().to_string(),
            ),
            // No compositor tools: IPC is skipped, applies use wlr-randr.
            ("PATH".to_string(), dir.join("bin").display().to_string()),
        ]);
        Self {
            dir,
            env,
            out: Buffer::default(),
            err: Buffer::default(),
            tui: Vec::new(),
        }
    }

    fn profiles(&self) -> PathBuf {
        self.dir.join("profiles.json")
    }

    /// "work" matches the fixture, "home" has HDMI-A-1 moved to x=128.
    fn make_store(&self) -> ProfileStore {
        let store = ProfileStore::new(Some(self.profiles()));
        let outputs = load_outputs();
        store.save("work", &states_of(&outputs)).unwrap();
        let mut moved = states_of(&outputs);
        moved[0].x = 128;
        store.save("home", &moved).unwrap();
        store
    }

    fn run(&mut self, args: &[&str], wlr: &FakeWlr) -> i32 {
        let mut argv = vec![OsString::from("wlr-monitor")];
        argv.extend(args.iter().map(OsString::from));
        self.run_os(&argv, wlr)
    }

    fn run_os(&mut self, argv: &[OsString], wlr: &FakeWlr) -> i32 {
        let wlr = wlr.clone();
        let make_wlr = move |_: Option<&str>| -> Box<dyn Wlr> { Box::new(wlr.clone()) };
        let mut out = self.out.clone();
        let mut err = self.err.clone();
        let tui = &mut self.tui;
        let mut run_tui = |_backend: Backend, _store: ProfileStore, profiles_only: bool| {
            tui.push(profiles_only);
            Ok(())
        };
        let mut rt = Runtime {
            out: &mut out,
            err: &mut err,
            ansi_out: false,
            ansi_err: false,
            env: Some(self.env.clone()),
            make_wlr: &make_wlr,
            run_tui: &mut run_tui,
        };
        run(argv, &mut rt)
    }

    fn run_with_config(&mut self, args: &[&str], wlr: &FakeWlr) -> i32 {
        let config = self.profiles().display().to_string();
        let mut full = vec!["--config", config.as_str()];
        full.extend_from_slice(args);
        self.run(&full, wlr)
    }
}

fn fixture() -> FakeWlr {
    FakeWlr::exact(load_outputs())
}

#[test]
fn parser_accepts_hyprmon_compatible_flags() {
    let args = Args::try_parse_from([
        "wlr-monitor",
        "--cfg",
        "/tmp/cfg",
        "--list-profiles",
        "--json",
    ])
    .unwrap();
    assert_eq!(args.config, Some(PathBuf::from("/tmp/cfg")));
    assert!(args.list_profiles && args.json);
    let args = Args::try_parse_from(["wlr-monitor", "profiles"]).unwrap();
    assert_eq!(args.command.as_deref(), Some("profiles"));
    assert!(
        Args::try_parse_from(["wlr-monitor", "--active-profile"])
            .unwrap()
            .active_profile
    );
    let args = Args::try_parse_from(["wlr-monitor", "--print-config"]).unwrap();
    assert_eq!(args.print_config.as_deref(), Some(""));
    let args = Args::try_parse_from(["wlr-monitor", "--print-config", "work"]).unwrap();
    assert_eq!(args.print_config.as_deref(), Some("work"));
    // Actions are mutually exclusive, and unknown values are rejected.
    assert!(Args::try_parse_from(["wlr-monitor", "--outputs", "--list-profiles"]).is_err());
    assert!(Args::try_parse_from(["wlr-monitor", "--compositor", "kwin"]).is_err());
    assert!(Args::try_parse_from(["wlr-monitor", "other"]).is_err());
}

#[test]
fn version_and_usage_errors() {
    let mut harness = Harness::new();
    assert_eq!(harness.run(&["-v"], &fixture()), 0);
    assert_eq!(
        harness.out.take(),
        format!("wlr-monitor {}\n", crate::VERSION)
    );
    assert_eq!(harness.run(&["--bogus"], &fixture()), 2);
    let err = harness.err.take();
    assert!(err.contains("--bogus"));
    assert!(
        !err.contains('\u{1b}'),
        "no color codes when not a terminal: {err:?}"
    );
    assert_eq!(harness.run(&["--help"], &fixture()), 0);
    assert!(!harness.out.take().contains('\u{1b}'));
    assert!(harness.tui.is_empty());
}

#[test]
fn non_utf8_arguments_do_not_crash() {
    use std::os::unix::ffi::OsStringExt;
    let mut harness = Harness::new();
    harness.make_store();
    let mut config = harness.profiles().into_os_string().into_vec();
    config.truncate(config.len() - "profiles.json".len());
    config.extend_from_slice(b"\xff");
    let argv = [
        OsString::from("wlr-monitor"),
        OsString::from("--config"),
        OsString::from_vec(config),
        OsString::from("--list-profiles"),
    ];
    assert_eq!(harness.run_os(&argv, &fixture()), 0);
    assert_eq!(harness.out.take(), "");
}

#[test]
fn config_path_accepts_files_and_directories() {
    let dir = TempDir::new();
    let home = dir.join("home");
    assert_eq!(config_path(None, &home), None);
    let file = dir.join("profiles.json");
    assert_eq!(config_path(Some(&file), &home), Some(file.clone()));
    assert_eq!(
        config_path(Some(dir.path()), &home),
        Some(dir.join("profiles.json"))
    );
    let missing = dir.join("cfg");
    assert_eq!(
        config_path(Some(&missing), &home),
        Some(missing.join("profiles.json"))
    );
    let explicit = dir.join("custom");
    fs::write(&explicit, "{}").unwrap();
    assert_eq!(config_path(Some(&explicit), &home), Some(explicit));
    assert_eq!(
        config_path(Some(Path::new("~/p.json")), &home),
        Some(home.join("p.json"))
    );
}

#[test]
fn list_profiles_text_marks_active() {
    let mut harness = Harness::new();
    harness.make_store();
    assert_eq!(harness.run_with_config(&["--list-profiles"], &fixture()), 0);
    assert_eq!(harness.out.text(), "work *\nhome\n");
}

#[test]
fn list_profiles_json_matches_hyprmon_shape() {
    let mut harness = Harness::new();
    harness.make_store();
    assert_eq!(
        harness.run_with_config(&["--list-profiles", "--json"], &fixture()),
        0
    );
    let payload: Value = serde_json::from_str(&harness.out.text()).unwrap();
    assert_eq!(
        payload,
        serde_json::json!([
            {"name": "work", "is_active": true},
            {"name": "home", "is_active": false},
        ])
    );
}

#[test]
fn list_profiles_still_works_without_wlr_randr() {
    let mut harness = Harness::new();
    harness.make_store();
    let wlr = FakeWlr::failing("no backend");
    assert_eq!(harness.run_with_config(&["--list-profiles"], &wlr), 0);
    assert_eq!(harness.out.text(), "work\nhome\n");
}

#[test]
fn active_profile_output() {
    let mut harness = Harness::new();
    harness.make_store();
    assert_eq!(
        harness.run_with_config(&["--active-profile"], &fixture()),
        0
    );
    assert_eq!(harness.out.take(), "work\n");

    let mut moved = load_outputs();
    moved[0].x = 500;
    assert_eq!(
        harness.run_with_config(&["--active-profile"], &FakeWlr::exact(moved)),
        0
    );
    assert_eq!(harness.out.take(), "No active profile found\n");

    // Without wlr-randr there is no answer: an error, not a guess.
    assert_eq!(
        harness.run_with_config(&["--active-profile"], &FakeWlr::failing("nope")),
        1
    );
    assert_eq!(harness.err.take(), "wlr-monitor: nope\n");
}

#[test]
fn tui_launch_modes() {
    let mut harness = Harness::new();
    assert_eq!(harness.run_with_config(&["profiles"], &fixture()), 0);
    assert_eq!(harness.run_with_config(&["--profiles"], &fixture()), 0);
    assert_eq!(harness.run_with_config(&[], &fixture()), 0);
    assert_eq!(harness.tui, [true, true, false]);
}

#[test]
fn the_default_profile_file_follows_xdg_config_home() {
    let mut harness = Harness::new();
    let path = harness.dir.join("config/wlr-monitor/profiles.json");
    ProfileStore::new(Some(path))
        .save("xdg", &states_of(&load_outputs()))
        .unwrap();
    assert_eq!(harness.run(&["--list-profiles"], &fixture()), 0);
    assert_eq!(harness.out.take(), "xdg *\n");
    harness.env.remove("XDG_CONFIG_HOME");
    assert_eq!(harness.run(&["--list-profiles"], &fixture()), 0);
    assert_eq!(harness.out.take(), "");
}

#[test]
fn apply_profile_uses_hardware_id_fallback() {
    let mut harness = Harness::new();
    let store = ProfileStore::new(Some(harness.profiles()));
    let mut saved = load_outputs()[0].state();
    saved.name = "DP-9".into();
    saved.hardware_id = "Microstep/MSI MP275/PC3M394200857".into();
    store.save("replugged", &[saved]).unwrap();

    let wlr = FakeWlr::exact(load_outputs()[..1].to_vec());
    assert_eq!(
        harness.run_with_config(&["--profile", "replugged"], &wlr),
        0
    );
    assert_eq!(wlr.applied(), ["HDMI-A-1"]);
    assert!(harness.out.text().contains("Applied HDMI-A-1."));
    // DP-9 was applied to its new connector, so it is not reported as missing.
    assert!(
        !harness.err.text().contains("Skipped"),
        "{}",
        harness.err.text()
    );
}

#[test]
fn apply_profile_reports_unknown_and_partial_profiles() {
    let mut harness = Harness::new();
    harness.make_store();
    assert_eq!(
        harness.run_with_config(&["--profile", "nope"], &fixture()),
        1
    );
    assert_eq!(
        harness.err.take(),
        "wlr-monitor: no profile named 'nope' (available: work, home)\n"
    );

    // A profile for a monitor that is not connected, plus one that is left alone.
    let store = ProfileStore::new(Some(harness.profiles()));
    let mut gone = load_outputs()[0].state();
    gone.name = "DP-3".into();
    gone.hardware_id.clear();
    let edp = load_outputs()[1].state();
    store.save("mixed", &[gone, edp]).unwrap();
    let wlr = fixture();
    assert_eq!(harness.run_with_config(&["--profile", "mixed"], &wlr), 0);
    assert_eq!(wlr.applied(), ["eDP-1"]);
    let err = harness.err.take();
    assert!(
        err.contains("Left unchanged (not in profile): HDMI-A-1"),
        "{err}"
    );
    assert!(err.contains("Skipped (not connected): DP-3"), "{err}");

    let failing = FakeWlr {
        fail: vec!["eDP-1".into()],
        ..fixture()
    };
    assert_eq!(
        harness.run_with_config(&["--profile", "mixed"], &failing),
        1
    );
    assert!(harness.err.take().contains("Failed eDP-1: bad mode"));
}

#[test]
fn delete_profile_command() {
    let mut harness = Harness::new();
    let store = harness.make_store();
    assert_eq!(
        harness.run_with_config(&["--delete-profile", "work"], &fixture()),
        0
    );
    assert_eq!(store.names().unwrap(), ["home"]);
    assert_eq!(harness.out.take(), "Deleted profile 'work'.\n");
    assert_eq!(
        harness.run_with_config(&["--delete-profile", "work"], &fixture()),
        1
    );
    assert_eq!(harness.err.take(), "wlr-monitor: no profile named 'work'\n");
}

#[test]
fn outputs_json_includes_state() {
    let mut harness = Harness::new();
    assert_eq!(harness.run(&["--outputs", "--json"], &fixture()), 0);
    let payload: Value = serde_json::from_str(&harness.out.text()).unwrap();
    assert_eq!(payload[0]["name"], "HDMI-A-1");
    assert_eq!(payload[0]["position"], serde_json::json!({"x": 0, "y": 0}));
    assert!(payload[0]["hardware_id"].is_string());
    assert_eq!(payload[1]["description"], "BOE 0x0A1F (eDP-1)");
}

#[test]
fn outputs_table() {
    let mut harness = Harness::new();
    let mut outputs = load_outputs();
    outputs[0].hdr = Some(true);
    outputs[1].enabled = false;
    outputs[1].mode = None;
    assert_eq!(harness.run(&["--outputs"], &FakeWlr::exact(outputs)), 0);
    assert_eq!(
        harness.out.take(),
        "NAME           ON   MODE                 POSITION     SCALE   ROTATION     VRR  HDR\n\
         HDMI-A-1       yes  1920x1080@100        0,0          1       normal       off  -\n\
         eDP-1          no   -                    1920,0       1.2     normal       on   -\n"
    );
    assert_eq!(harness.run(&["--outputs"], &FakeWlr::exact(Vec::new())), 0);
    assert_eq!(harness.out.take(), "No outputs found.\n");
}

#[test]
fn print_config_renders_profile_for_compositor() {
    let mut harness = Harness::new();
    harness.make_store();
    let code = harness.run_with_config(
        &["--compositor", "mango", "--print-config", "home"],
        &fixture(),
    );
    assert_eq!(code, 0);
    let out = harness.out.text();
    assert!(out.contains("# Profile: home"));
    assert!(out.contains("monitor_rule=make:Microstep,model:MSI MP275,serial:PC3M394200857,"));
    assert!(out.contains("x:128,"));
}

#[test]
fn print_config_needs_a_compositor() {
    let mut harness = Harness::new();
    assert_eq!(harness.run(&["--print-config"], &fixture()), 1);
    assert!(harness.err.text().contains("--compositor"));
}

#[test]
fn init_include_writes_file_and_prints_line() {
    let mut harness = Harness::new();
    let include = harness.dir.join("out/outputs.conf");
    let include_arg = include.display().to_string();
    let code = harness.run(
        &[
            "--compositor",
            "sway",
            "--include",
            &include_arg,
            "--init-include",
        ],
        &fixture(),
    );
    assert_eq!(code, 0);
    assert!(
        fs::read_to_string(&include)
            .unwrap()
            .contains("output \"eDP-1\" enable")
    );
    let out = harness.out.text();
    assert!(out.contains(&format!("Wrote {}.", include.display())));
    assert!(out.contains(&format!("include {}", include.display())));

    assert_eq!(harness.run(&["--init-include"], &fixture()), 1);
    assert!(harness.err.take().contains("no compositor integration"));
    assert_eq!(
        harness.run(
            &["--compositor", "sway", "--no-include", "--init-include"],
            &fixture()
        ),
        1
    );
    assert!(harness.err.take().contains("conflicts with --no-include"));
}

#[test]
fn apply_profile_writes_compositor_include() {
    let mut harness = Harness::new();
    harness.make_store();
    let include = harness.dir.join("niri.kdl");
    // The generic backend applies through wlr-randr and writes nothing.
    assert_eq!(
        harness.run_with_config(&["--profile", "home"], &fixture()),
        0
    );
    assert!(!include.exists());

    // niri without `niri` on PATH falls back to wlr-randr and still persists.
    let include_arg = include.display().to_string();
    let code = harness.run_with_config(
        &[
            "--compositor",
            "niri",
            "--include",
            &include_arg,
            "--profile",
            "home",
        ],
        &FakeWlr::new(load_outputs()),
    );
    assert_eq!(code, 0);
    assert!(harness.err.text().contains("niri was not found"));
    assert!(
        fs::read_to_string(&include)
            .unwrap()
            .contains("position x=128 y=0")
    );
}

#[test]
fn print_config_matches_what_would_be_written() {
    let mut harness = Harness::new();
    let include = harness.dir.join("mango.conf");
    let include_arg = include.display().to_string();
    let args = ["--compositor", "mango", "--include", include_arg.as_str()];
    let mut init = args.to_vec();
    init.push("--init-include");
    assert_eq!(harness.run(&init, &fixture()), 0);
    harness.out.take();

    // Undocked: only eDP-1 is connected, but HDMI-A-1 keeps its rule.
    let mut print = args.to_vec();
    print.push("--print-config");
    let undocked = FakeWlr::exact(load_outputs()[1..].to_vec());
    assert_eq!(harness.run(&print, &undocked), 0);
    let printed = harness.out.take();
    assert!(printed.contains("serial:PC3M394200857"));
    assert!(printed.contains("name:^eDP-1$"));
}

#[test]
fn profile_settings_the_compositor_lacks_are_dropped() {
    let mut harness = Harness::new();
    let store = ProfileStore::new(Some(harness.profiles()));
    let mut state = load_outputs()[0].state();
    state.hdr = Some(true);
    state.color_profile = Some("srgb".into());
    store.save("hdr", &[state]).unwrap();
    let wlr = FakeWlr::exact(load_outputs()[..1].to_vec());
    assert_eq!(harness.run_with_config(&["--profile", "hdr"], &wlr), 0);
    let applied = wlr.applied_states();
    assert_eq!(applied[0].hdr, None);
    assert_eq!(applied[0].color_profile, None);
}

#[test]
fn list_profiles_reports_broken_ones() {
    let mut harness = Harness::new();
    harness.make_store();
    let path = harness.profiles();
    let mut raw: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    raw["profiles"]["home"]["outputs"]["HDMI-A-1"]["transform"] = Value::String("bogus".into());
    fs::write(&path, raw.to_string()).unwrap();
    assert_eq!(harness.run_with_config(&["--list-profiles"], &fixture()), 0);
    assert_eq!(harness.out.take(), "work *\n");
    assert!(harness.err.take().contains("invalid transform"));
    assert_eq!(
        harness.run_with_config(&["--profile", "home"], &fixture()),
        1
    );
    assert!(harness.err.take().contains("invalid transform"));
}

#[test]
fn cli_refuses_profiles_that_turn_everything_off() {
    let mut harness = Harness::new();
    let store = ProfileStore::new(Some(harness.profiles()));
    let mut dark = states_of(&load_outputs());
    for state in &mut dark {
        state.enabled = false;
    }
    store.save("dark", &dark).unwrap();
    let wlr = fixture();
    assert_eq!(harness.run_with_config(&["--profile", "dark"], &wlr), 1);
    assert!(wlr.applied().is_empty());
    assert!(harness.err.text().contains("every connected monitor"));
}

#[test]
fn unreadable_profile_files_are_reported() {
    let mut harness = Harness::new();
    fs::write(harness.profiles(), "{ nope").unwrap();
    assert_eq!(harness.run_with_config(&["--list-profiles"], &fixture()), 1);
    assert!(
        harness
            .err
            .text()
            .starts_with("wlr-monitor: Could not parse")
    );
}

#[test]
fn repeated_options_keep_the_last_value_like_argparse() {
    let mut harness = Harness::new();
    harness.make_store();
    let config = harness.profiles().display().to_string();
    let code = harness.run(
        &[
            "--config",
            "/nonexistent/p.json",
            "--config",
            &config,
            "--no-include",
            "--no-include",
            "--compositor",
            "sway",
            "--compositor",
            "wlroots",
            "--list-profiles",
            "--list-profiles",
        ],
        &fixture(),
    );
    assert_eq!(code, 0, "{}", harness.err.text());
    assert_eq!(harness.out.take(), "work *\nhome\n");
    let wlr = fixture();
    assert_eq!(
        harness.run(
            &["--config", &config, "--profile", "zz", "--profile", "home"],
            &wlr
        ),
        0
    );
    assert!(harness.out.take().contains("Applied HDMI-A-1."));
}

#[test]
fn config_paths_are_normalised_like_pathlib() {
    let mut harness = Harness::new();
    harness.make_store();
    let with_slash = format!("{}/", harness.profiles().display());
    assert_eq!(
        harness.run(&["--config", &with_slash, "--list-profiles"], &fixture()),
        0
    );
    assert_eq!(harness.out.take(), "work *\nhome\n");
    // Saving to a new path given with a trailing slash works too.
    let fresh = format!("{}/new.json/", harness.dir.path().display());
    let store = ProfileStore::new(config_path(Some(Path::new(&fresh)), Path::new("/")));
    store.save("x", &states_of(&load_outputs())).unwrap();
    assert!(harness.dir.join("new.json").is_file());
}
