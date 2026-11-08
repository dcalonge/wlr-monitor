use super::*;
use crate::model::tests::load_outputs;
use crate::testutil::{FakeWlr, TempDir, quote as shell_quote};

/// A temporary HOME / XDG layout with fake IPC tools on PATH. Every tool
/// call is logged; `commands()` returns them (program basename first).
struct Env {
    dir: TempDir,
    vars: HashMap<String, String>,
}

impl Env {
    fn new(tools: &[&str]) -> Self {
        let dir = TempDir::new();
        fs::create_dir_all(dir.join("bin")).unwrap();
        let vars = HashMap::from([
            ("HOME".to_string(), dir.join("home").display().to_string()),
            (
                "XDG_CONFIG_HOME".to_string(),
                dir.join("home/.config").display().to_string(),
            ),
            (
                "XDG_STATE_HOME".to_string(),
                dir.join("state").display().to_string(),
            ),
            ("PATH".to_string(), dir.join("bin").display().to_string()),
        ]);
        let env = Self { dir, vars };
        for tool in tools {
            env.tool(tool, &[]);
        }
        env
    }

    fn config(&self) -> PathBuf {
        self.dir.join("home/.config")
    }

    /// A fake tool that logs its arguments and prints the first response
    /// whose key appears in its command line. PATH only holds the fake
    /// tools, so the script sticks to shell builtins.
    fn tool(&self, name: &str, responses: &[(&str, &str)]) {
        let log = shell_quote(&self.dir.join("log").display().to_string());
        let mut body = format!(
            "{{ printf '%s\\037' \"${{0##*/}}\"; for a in \"$@\"; do printf '%s\\037' \"$a\"; done; printf '\\n'; }} >> {log}\ncase \"$*\" in\n"
        );
        for (key, response) in responses {
            body.push_str(&format!(
                "  *{}*) printf '%s' {} ;;\n",
                key.replace('-', "\\-"),
                shell_quote(response)
            ));
        }
        body.push_str("esac\nexit 0");
        let path = self.dir.join("bin").join(name);
        let _ = fs::remove_file(&path);
        self.dir.script(&format!("bin/{name}"), &body);
    }

    fn commands(&self) -> Vec<Vec<String>> {
        let Ok(text) = fs::read_to_string(self.dir.join("log")) else {
            return Vec::new();
        };
        text.lines()
            .map(|line| {
                line.split('\u{1f}')
                    .filter(|part| !part.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .collect()
    }

    fn compositor(&self, kind: Kind) -> Compositor {
        Compositor::new(kind, Some(self.vars.clone()), None, true)
    }

    fn with_include(&self, kind: Kind, include: &Path) -> Compositor {
        Compositor::new(kind, Some(self.vars.clone()), Some(include), true)
    }

    fn without_include(&self, kind: Kind) -> Compositor {
        Compositor::new(kind, Some(self.vars.clone()), None, false)
    }
}

fn backend(wlr: &FakeWlr, compositor: Compositor) -> Backend {
    Backend::new(Box::new(wlr.clone()), compositor)
}

fn render(compositor: &Compositor, output: &Output) -> Result<Vec<String>, CompositorError> {
    compositor.render_output(
        output,
        &compositor.render_context(
            std::slice::from_ref(output),
            compositor.preserved_options().unwrap(),
        ),
    )
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

fn warnings(results: &[ApplyResult]) -> Vec<String> {
    results
        .iter()
        .filter(|result| !result.warning.is_empty())
        .map(|result| result.warning.clone())
        .collect()
}

fn twins() -> Vec<Output> {
    let mut outputs = load_outputs();
    let (make, model, serial) = (
        outputs[0].make.clone(),
        outputs[0].model.clone(),
        outputs[0].serial.clone(),
    );
    outputs[1].make = make;
    outputs[1].model = model;
    outputs[1].serial = serial;
    outputs
}

// ----------------------------------------------------------------------
// Detection
// ----------------------------------------------------------------------

#[test]
fn detect_compositor_from_environment() {
    let cases = [
        (
            vec![("MANGO_INSTANCE_SIGNATURE", "/run/mango.sock")],
            Kind::Mango,
        ),
        (vec![("SWAYSOCK", "/run/sway.sock")], Kind::Sway),
        (vec![("NIRI_SOCKET", "/run/niri.sock")], Kind::Niri),
        (vec![("XDG_CURRENT_DESKTOP", "sway:wlroots")], Kind::Sway),
        (vec![("XDG_CURRENT_DESKTOP", "mango")], Kind::Mango),
        (vec![], Kind::Wlroots),
    ];
    for (vars, expected) in cases {
        let env: HashMap<String, String> = vars
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            detect_compositor(None, Some(env), None, true).unwrap().kind,
            expected
        );
    }
}

#[test]
fn detect_compositor_honours_explicit_name() {
    let env = HashMap::from([("SWAYSOCK".to_string(), "x".to_string())]);
    let niri = detect_compositor(Some("niri"), Some(env.clone()), None, true).unwrap();
    assert_eq!(niri.kind, Kind::Niri);
    let auto = detect_compositor(Some("auto"), Some(env), None, true).unwrap();
    assert_eq!(auto.kind, Kind::Sway);
    let error = detect_compositor(Some("kwin"), Some(HashMap::new()), None, true).unwrap_err();
    assert_eq!(error, "Unknown compositor 'kwin'.");
}

#[test]
fn generic_wlroots_has_no_include_or_extras() {
    let env = Env::new(&[]);
    let mut compositor = env.compositor(Kind::Wlroots);
    assert_eq!(compositor.include_path(), None);
    assert_eq!(compositor.include_hint(), "");
    assert_eq!(compositor.live_query(), None);
    let mut outputs = load_outputs();
    outputs[0].hdr = Some(true);
    compositor.enrich(&mut outputs, None);
    assert_eq!(outputs[0].hdr, None);
}

// ----------------------------------------------------------------------
// Rendering
// ----------------------------------------------------------------------

#[test]
fn mango_rules_match_edid_and_fall_back_to_name() {
    let env = Env::new(&[]);
    let mango = env.compositor(Kind::Mango);
    let mut outputs = load_outputs();
    outputs[0].hdr = Some(true);
    outputs[0].color_profile = Some("icc:/icc/display.icc".into());
    outputs[1].transform = "flipped-90".into();
    assert_eq!(
        render(&mango, &outputs[0]).unwrap(),
        [
            "monitor_rule=make:Microstep,model:MSI MP275,serial:PC3M394200857,\
          width:1920,height:1080,refresh:100,x:0,y:0,scale:1,rr:0,vrr:0,\
          hdr:1,icc:/icc/display.icc"
        ]
    );
    // eDP-1 has no serial, so it is matched by an anchored connector regex.
    assert_eq!(
        render(&mango, &outputs[1]).unwrap(),
        [
            "monitor_rule=name:^eDP-1$,width:1920,height:1080,refresh:60,\
          x:1920,y:0,scale:1.2,rr:5,vrr:1"
        ]
    );
    outputs[1].enabled = false;
    assert_eq!(
        render(&mango, &outputs[1]).unwrap(),
        ["monitor_rule=name:^eDP-1$,disable:1"]
    );

    let mut special = load_outputs().remove(1);
    special.name = "DP-1.2(a)".into();
    assert!(
        render(&mango, &special).unwrap()[0].starts_with("monitor_rule=name:^DP-1\\.2\\(a\\)$,")
    );
}

#[test]
fn mango_rejects_values_it_cannot_parse() {
    let env = Env::new(&[]);
    let mango = env.compositor(Kind::Mango);
    let mut hdmi = load_outputs().remove(0);
    hdmi.model = "Model, with comma".into();
    assert!(render(&mango, &hdmi).unwrap()[0].starts_with("monitor_rule=name:^HDMI-A-1$,"));
    assert!(!mango.color_profile_supported("icc:/a,b.icc"));
    hdmi.color_profile = Some(format!("icc:/{}", "x".repeat(600)));
    assert!(
        render(&mango, &hdmi)
            .unwrap_err()
            .message
            .contains("line limit")
    );
    // The limit is in bytes: 250 two-byte characters are too long.
    hdmi.color_profile = Some(format!("icc:/{}", "é".repeat(250)));
    assert!(render(&mango, &hdmi).is_err());
}

#[test]
fn sway_output_lines() {
    let env = Env::new(&[]);
    let sway = env.compositor(Kind::Sway);
    let mut outputs = load_outputs();
    outputs[0].hdr = Some(false);
    outputs[0].bit_depth = Some(10);
    outputs[0].color_profile = Some("icc:/my \"icc\".icc".into());
    assert_eq!(
        render(&sway, &outputs[0]).unwrap(),
        ["output \"Microstep MSI MP275 PC3M394200857\" enable \
          mode 1920x1080@100.000Hz pos 0 0 scale 1 transform normal \
          adaptive_sync off render_bit_depth 10 hdr off \
          color_profile icc \"/my \\\"icc\\\".icc\""]
    );
    outputs[1].enabled = false;
    assert_eq!(
        render(&sway, &outputs[1]).unwrap(),
        ["output \"eDP-1\" disable"]
    );
}

#[test]
fn niri_output_blocks() {
    let env = Env::new(&[]);
    let niri = env.compositor(Kind::Niri);
    let mut outputs = load_outputs();
    assert_eq!(
        render(&niri, &outputs[1]).unwrap(),
        [
            "output \"eDP-1\" {",
            "    mode \"1920x1080@60.000\"",
            "    scale 1.2",
            "    transform \"normal\"",
            "    position x=1920 y=0",
            "    variable-refresh-rate",
            "}",
        ]
    );
    outputs[0].enabled = false;
    assert_eq!(
        render(&niri, &outputs[0]).unwrap(),
        [
            "output \"Microstep MSI MP275 PC3M394200857\" {",
            "    off",
            "}"
        ]
    );
}

#[test]
fn unprintable_edid_strings_never_reach_the_config() {
    let env = Env::new(&[]);
    let sway = env.compositor(Kind::Sway);
    let mut hdmi = load_outputs().remove(0);
    hdmi.model = "Evil\ninclude /tmp/x".into();
    assert!(render(&sway, &hdmi).unwrap()[0].starts_with("output \"HDMI-A-1\" "));
    let text = sway
        .render_include(std::slice::from_ref(&hdmi), Some("work\nexec rm -rf ~"))
        .unwrap();
    assert!(!text.contains("\nexec"));
    assert!(text.contains("# Profile: workexec rm -rf ~"));

    hdmi.name = "HDMI\n1".into();
    let mango = env.compositor(Kind::Mango);
    assert!(render(&mango, &hdmi).is_err());
}

#[test]
fn include_text_has_header_note_and_trailing_newline() {
    let env = Env::new(&[]);
    let niri = env.compositor(Kind::Niri);
    let text = niri.render_include(&load_outputs(), None).unwrap();
    assert!(text.starts_with(&format!("// {HEADER}\n// niri uses the first matching")));
    assert!(text.ends_with("}\n") && !text.ends_with("\n\n"));
    assert!(!text.contains("Profile:"));
}

// ----------------------------------------------------------------------
// Include file handling
// ----------------------------------------------------------------------

#[test]
fn write_include_and_state_sidecar() {
    let env = Env::new(&[]);
    let mut sway = env.compositor(Kind::Sway);
    let mut outputs = load_outputs();
    outputs[0].color_profile = Some("srgb".into());
    outputs[0].bit_depth = Some(10);
    let path = sway.write_include(&outputs, Some("work")).unwrap().unwrap();
    assert_eq!(path, env.config().join("sway/wlr-monitor.conf"));
    let text = read(&path);
    assert!(text.starts_with("# Generated by wlr-monitor"));
    assert!(text.contains("# Profile: work"));
    assert!(text.contains("color_profile srgb"));

    let state: Value = serde_json::from_str(&read(&sway.state_path())).unwrap();
    assert_eq!(state["profile"], "work");
    assert_eq!(state["include"], path.display().to_string());
    assert_eq!(state["outputs"]["HDMI-A-1"]["serial"], "PC3M394200857");

    // Settings sway cannot report come back from the sidecar.
    let mut fresh = load_outputs();
    sway.enrich(&mut fresh, None);
    assert_eq!(fresh[0].color_profile.as_deref(), Some("srgb"));
    assert_eq!(fresh[0].bit_depth, Some(10));
    assert_eq!(fresh[1].color_profile, None);
}

#[test]
fn unchanged_include_is_not_rewritten() {
    let env = Env::new(&[]);
    let sway = env.compositor(Kind::Sway);
    let path = sway.write_include(&load_outputs(), None).unwrap().unwrap();
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    thread::sleep(Duration::from_millis(20));
    sway.write_include(&load_outputs(), None).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
}

#[test]
fn write_include_refuses_symlinks() {
    let env = Env::new(&[]);
    let mango = env.compositor(Kind::Mango);
    let path = mango.include_path().unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let store_file = env.dir.join("store-file");
    fs::write(&store_file, "").unwrap();
    std::os::unix::fs::symlink(&store_file, &path).unwrap();
    let error = mango.write_include(&load_outputs(), None).unwrap_err();
    assert!(error.message.contains("symlink"));
    assert!(!error.state_file);
    assert_eq!(read(&store_file), "");
}

#[test]
fn write_include_can_be_disabled() {
    let env = Env::new(&[]);
    let niri = env.without_include(Kind::Niri);
    assert_eq!(niri.write_include(&load_outputs(), None).unwrap(), None);
    assert!(!env.config().join("niri/wlr-monitor.kdl").exists());
}

#[test]
fn include_path_override() {
    let env = Env::new(&[]);
    let custom = env.dir.join("custom.conf");
    let sway = env.with_include(Kind::Sway, &custom);
    assert_eq!(sway.include_path(), Some(custom.clone()));
    sway.write_include(&load_outputs(), None).unwrap();
    assert!(custom.exists());
}

#[test]
fn include_detection_for_each_compositor() {
    let env = Env::new(&[]);
    let config = env.config();

    let mango = env.compositor(Kind::Mango);
    fs::create_dir_all(config.join("mango")).unwrap();
    fs::write(config.join("mango/config.conf"), "source=./binds.conf\n").unwrap();
    assert!(!mango.include_is_sourced());
    assert!(
        mango
            .include_hint()
            .contains("source_optional=~/.config/mango/wlr-monitor.conf")
    );
    assert!(
        mango
            .include_hint()
            .contains("to ~/.config/mango/config.conf")
    );
    // A nested include one level deep is followed, with ~ expanded.
    fs::write(
        config.join("mango/binds.conf"),
        "source_optional=~/.config/mango/wlr-monitor.conf\n",
    )
    .unwrap();
    assert!(mango.include_is_sourced());
    assert_eq!(mango.include_hint(), "");
    fs::write(config.join("mango/binds.conf"), "# source_optional=x\n").unwrap();
    assert!(!mango.include_is_sourced());

    let sway = env.compositor(Kind::Sway);
    fs::create_dir_all(config.join("sway")).unwrap();
    fs::write(
        config.join("sway/config"),
        format!("include {}/sway/*.conf\n", config.display()),
    )
    .unwrap();
    assert!(!sway.include_is_sourced()); // the glob matches nothing yet
    sway.write_include(&load_outputs(), None).unwrap();
    assert!(sway.include_is_sourced());

    let niri = env.compositor(Kind::Niri);
    fs::create_dir_all(config.join("niri")).unwrap();
    fs::write(
        config.join("niri/config.kdl"),
        "// include \"wlr-monitor.kdl\"\ninput {}\n",
    )
    .unwrap();
    assert!(!niri.include_is_sourced());
    assert_eq!(niri.include_line(), "include \"wlr-monitor.kdl\"");
    fs::write(
        config.join("niri/config.kdl"),
        "include optional=true \"wlr-monitor.kdl\"\n",
    )
    .unwrap();
    assert!(niri.include_is_sourced());
}

#[test]
fn sway_include_with_variables_and_quotes() {
    let env = Env::new(&[]);
    let config = env.config();
    let sway = env.compositor(Kind::Sway);
    fs::create_dir_all(config.join("sway")).unwrap();
    fs::write(
        config.join("sway/config"),
        "  include '$HOME/.config/sway/wlr-monitor.conf'\n",
    )
    .unwrap();
    assert!(sway.include_is_sourced());
    fs::write(
        config.join("sway/config"),
        "includes ~/.config/sway/wlr-monitor.conf\n",
    )
    .unwrap();
    assert!(!sway.include_is_sourced());
}

#[test]
fn niri_include_line_outside_the_config_dir_is_absolute() {
    let env = Env::new(&[]);
    let custom = env.dir.join("custom dir/out.kdl");
    let niri = env.with_include(Kind::Niri, &custom);
    assert_eq!(
        niri.include_line(),
        format!("include \"{}\"", custom.display())
    );
}

// ----------------------------------------------------------------------
// Applying
// ----------------------------------------------------------------------

#[test]
fn sway_applies_with_swaymsg_and_persists() {
    let env = Env::new(&["swaymsg"]);
    let outputs = load_outputs();
    let wlr = FakeWlr::new(outputs.clone());
    let mut states: Vec<_> = outputs.iter().map(Output::state).collect();
    states[0].enabled = false;
    states[1].adaptive_sync = true;
    let results = backend(&wlr, env.compositor(Kind::Sway))
        .apply_states(&states, Some("desk"))
        .unwrap();

    assert!(results.iter().all(|result| result.ok));
    let applies: Vec<_> = env
        .commands()
        .into_iter()
        .filter(|cmd| cmd.contains(&"--".to_string()))
        .collect();
    // Enabled outputs are configured before others are disabled.
    assert!(
        applies[0]
            .last()
            .unwrap()
            .starts_with("output \"eDP-1\" enable")
    );
    assert!(applies[0].last().unwrap().contains("adaptive_sync on"));
    assert_eq!(applies[1].last().unwrap(), "output \"HDMI-A-1\" disable");
    let include = read(&env.config().join("sway/wlr-monitor.conf"));
    assert!(include.contains("# Profile: desk"));
    assert!(include.contains("output \"Microstep MSI MP275 PC3M394200857\" disable"));
    assert!(wlr.applied().is_empty());
}

#[test]
fn sway_reads_hdr_and_vrr_from_ipc() {
    let env = Env::new(&[]);
    let payload = r#"[{"name": "HDMI-A-1", "hdr": true, "adaptive_sync_status": "enabled"}]"#;
    env.tool("swaymsg", &[("get_outputs", payload)]);
    let mut sway = env.compositor(Kind::Sway);
    let mut outputs = load_outputs();
    let live = sway.query_live();
    sway.enrich(&mut outputs, live.as_ref());
    assert_eq!(outputs[0].hdr, Some(true));
    assert!(outputs[0].adaptive_sync);
    assert_eq!(outputs[1].hdr, None);
    assert_eq!(env.commands()[0], ["swaymsg", "-t", "get_outputs", "-r"]);
}

#[test]
fn backend_queries_the_compositor_while_wlr_randr_runs() {
    let env = Env::new(&[]);
    let marker = env.dir.join("queried");
    let payload = r#"[{"name": "HDMI-A-1", "hdr": true, "adaptive_sync_status": "enabled"}]"#;
    env.dir.script(
        "bin/swaymsg",
        &format!(
            ": > {}\nprintf '%s' {}",
            shell_quote(&marker.display().to_string()),
            shell_quote(payload)
        ),
    );
    let mut wlr = FakeWlr::new(load_outputs());
    // Run one after the other, the query would only start after this.
    wlr.wait_for = Some(marker);
    let outputs = backend(&wlr, env.compositor(Kind::Sway))
        .read_outputs()
        .unwrap();
    assert_eq!(outputs[0].hdr, Some(true));
    assert!(outputs[0].adaptive_sync);
}

#[test]
fn backend_reads_without_an_ipc_tool() {
    let env = Env::new(&[]);
    let sway = env.compositor(Kind::Sway);
    assert_eq!(sway.live_query(), None);
    let outputs = backend(&FakeWlr::new(load_outputs()), sway)
        .read_outputs()
        .unwrap();
    let names: Vec<_> = outputs.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["HDMI-A-1", "eDP-1"]);
}

#[test]
fn niri_applies_each_setting_with_niri_msg() {
    let env = Env::new(&["niri"]);
    let mut outputs = load_outputs();
    outputs[1].x = -1600;
    let results = backend(&FakeWlr::new(outputs.clone()), env.compositor(Kind::Niri))
        .apply_states(&[outputs[1].state()], None)
        .unwrap();
    assert_eq!(results.iter().map(|r| r.ok).collect::<Vec<_>>(), [true]);
    let tail: Vec<Vec<String>> = env
        .commands()
        .into_iter()
        .filter(|cmd| cmd.get(1..3) == Some(&["msg".to_string(), "output".to_string()]))
        .map(|cmd| cmd[2..].to_vec())
        .collect();
    assert_eq!(
        tail,
        [
            vec!["output", "eDP-1", "on"],
            vec!["output", "eDP-1", "mode", "1920x1080@60.000"],
            vec!["output", "eDP-1", "scale", "1.2"],
            vec!["output", "eDP-1", "transform", "normal"],
            vec!["output", "eDP-1", "position", "set", "--", "-1600", "0"],
            vec!["output", "eDP-1", "vrr", "on"],
        ]
    );
    assert!(env.config().join("niri/wlr-monitor.kdl").exists());
}

#[test]
fn niri_stops_an_output_at_its_first_failing_command() {
    let env = Env::new(&[]);
    env.dir.script(
        "bin/niri",
        "case \"$*\" in *scale*) echo 'bad scale' >&2; exit 1;; esac",
    );
    let outputs = load_outputs();
    let results = backend(&FakeWlr::new(outputs.clone()), env.compositor(Kind::Niri))
        .apply_states(&[outputs[1].state()], None)
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(!results[0].ok);
    assert_eq!(results[0].detail(), "bad scale");
    // A failed apply does not write the include.
    assert!(!env.config().join("niri/wlr-monitor.kdl").exists());
}

#[test]
fn mango_reloads_config_when_the_include_is_sourced() {
    let env = Env::new(&["mmsg"]);
    fs::create_dir_all(env.config().join("mango")).unwrap();
    fs::write(
        env.config().join("mango/config.conf"),
        "source_optional=./wlr-monitor.conf\n",
    )
    .unwrap();
    let outputs = load_outputs();
    let wlr = FakeWlr::new(outputs.clone());
    let mut state = outputs[0].state();
    state.adaptive_sync = true;
    let results = backend(&wlr, env.compositor(Kind::Mango))
        .apply_states(&[state], Some("gaming"))
        .unwrap();

    assert_eq!(results.iter().map(|r| r.ok).collect::<Vec<_>>(), [true]);
    assert!(wlr.applied().is_empty()); // the rule reload applies everything
    let dispatches: Vec<_> = env
        .commands()
        .into_iter()
        .filter(|cmd| cmd.get(1).map(String::as_str) == Some("dispatch"))
        .map(|cmd| cmd[1..].to_vec())
        .collect();
    assert_eq!(dispatches, [vec!["dispatch", "reload_config"]]);
    let include = read(&env.config().join("mango/wlr-monitor.conf"));
    assert!(include.contains("serial:PC3M394200857,"));
    assert!(include.contains(",vrr:1"));
    assert!(include.contains("name:^eDP-1$")); // untouched outputs keep their rule
}

#[test]
fn mango_reload_errors_are_reported() {
    let env = Env::new(&[]);
    env.tool("mmsg", &[("reload_config", "{\"error\":\"bad\"}")]);
    fs::create_dir_all(env.config().join("mango")).unwrap();
    fs::write(
        env.config().join("mango/config.conf"),
        format!("source={}/mango/wlr-monitor.conf\n", env.config().display()),
    )
    .unwrap();
    let mango = env.compositor(Kind::Mango);
    assert!(mango.include_is_sourced());
    let outputs = load_outputs();
    let results = backend(&FakeWlr::new(outputs.clone()), mango)
        .apply_states(&[outputs[0].state()], None)
        .unwrap();
    assert_eq!(results.iter().map(|r| r.ok).collect::<Vec<_>>(), [false]);
}

#[test]
fn mango_without_include_uses_wlr_randr_and_warns_about_vrr() {
    let env = Env::new(&["mmsg"]);
    let outputs = load_outputs();
    let wlr = FakeWlr::new(outputs.clone());
    let mut state = outputs[0].state();
    state.adaptive_sync = true;
    state.hdr = Some(true);
    let results = backend(&wlr, env.compositor(Kind::Mango))
        .apply_states(&[state], None)
        .unwrap();

    assert_eq!(wlr.applied(), ["HDMI-A-1"]);
    let commands: Vec<Vec<String>> = env
        .commands()
        .into_iter()
        .map(|cmd| cmd[1..].to_vec())
        .collect();
    assert!(commands.contains(&vec![
        "dispatch".to_string(),
        "togglehdr,on,HDMI-A-1".to_string()
    ]));
    let warnings = warnings(&results);
    assert!(warnings[0].contains("source_optional=~/.config/mango/wlr-monitor.conf"));
    // The include is still written so it can be sourced later.
    assert!(env.config().join("mango/wlr-monitor.conf").exists());
}

#[test]
fn mango_sourced_without_mmsg_relies_on_auto_reload() {
    let env = Env::new(&[]);
    fs::create_dir_all(env.config().join("mango")).unwrap();
    fs::write(
        env.config().join("mango/config.conf"),
        "source=./wlr-monitor.conf\n",
    )
    .unwrap();
    let outputs = load_outputs();
    let results = backend(&FakeWlr::new(outputs.clone()), env.compositor(Kind::Mango))
        .apply_states(&[outputs[0].state()], None)
        .unwrap();
    assert!(results.iter().all(|r| r.ok));
    assert!(warnings(&results)[0].contains("auto_reload_config"));
}

#[test]
fn missing_ipc_tool_falls_back_to_wlr_randr() {
    let env = Env::new(&[]);
    let outputs = load_outputs();
    let wlr = FakeWlr::new(outputs.clone());
    let results = backend(&wlr, env.compositor(Kind::Sway))
        .apply_states(&[outputs[0].state()], None)
        .unwrap();
    assert!(results.iter().all(|r| r.ok));
    assert_eq!(wlr.applied(), ["HDMI-A-1"]);
    assert!(
        warnings(&results)
            .iter()
            .any(|w| w.contains("swaymsg was not found"))
    );
    assert!(env.config().join("sway/wlr-monitor.conf").exists());
}

#[test]
fn include_write_failure_is_a_warning_after_live_apply() {
    let env = Env::new(&["swaymsg"]);
    let outputs = load_outputs();
    let sway = env.compositor(Kind::Sway);
    let path = sway.include_path().unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(env.dir.join("elsewhere"), &path).unwrap();
    let results = backend(&FakeWlr::new(outputs.clone()), sway)
        .apply_states(&[outputs[0].state()], None)
        .unwrap();
    assert!(results.iter().all(|r| r.ok));
    assert!(warnings(&results).iter().any(|w| w.contains("symlink")));
}

#[test]
fn backend_write_include_uses_live_layout() {
    let env = Env::new(&[]);
    let mut backend = backend(&FakeWlr::new(load_outputs()), env.compositor(Kind::Niri));
    let path = backend.write_include(Some("init")).unwrap().unwrap();
    assert!(read(&path).contains("// Profile: init"));
    let mode = fs::metadata(&path).unwrap().permissions();
    assert!(!mode.readonly());
}

// ----------------------------------------------------------------------
// Retention, identities, preserved options, validation
// ----------------------------------------------------------------------

#[test]
fn disconnected_outputs_keep_their_rules() {
    let env = Env::new(&[]);
    let mango = env.compositor(Kind::Mango);
    let mut docked = load_outputs();
    docked[0].x = 1600; // external monitor right of the laptop
    mango.write_include(&docked, None).unwrap();

    // Undocked: only eDP-1 is connected, the user moves it and applies.
    let mut laptop = vec![load_outputs().remove(1)];
    laptop[0].x = 0;
    mango.write_include(&laptop, None).unwrap();
    let text = read(&env.config().join("mango/wlr-monitor.conf"));
    assert!(text.contains("name:^eDP-1$,width:1920,height:1080,refresh:60,x:0,"));
    assert!(text.contains("serial:PC3M394200857,width:1920,height:1080,refresh:100,x:1600,"));

    // The retained monitor is reconnected on another connector: no duplicate.
    let mut replugged = load_outputs();
    replugged[0].name = "DP-7".into();
    mango.write_include(&replugged, None).unwrap();
    let text = read(&env.config().join("mango/wlr-monitor.conf"));
    assert_eq!(text.matches("serial:PC3M394200857").count(), 1);
}

#[test]
fn identical_monitors_fall_back_to_connector_names() {
    let env = Env::new(&[]);
    for kind in [Kind::Sway, Kind::Niri, Kind::Mango] {
        let text = env.compositor(kind).render_include(&twins(), None).unwrap();
        assert!(!text.contains("PC3M394200857"), "{}", kind.name());
        assert!(text.contains("HDMI-A-1") && text.contains("eDP-1"));
    }
}

#[test]
fn mango_keeps_hand_added_rule_options() {
    let env = Env::new(&[]);
    let mango = env.compositor(Kind::Mango);
    let mut outputs = load_outputs();
    let path = mango.write_include(&outputs, None).unwrap().unwrap();
    let lines: Vec<String> = read(&path)
        .lines()
        .map(|line| {
            if line.contains("PC3M394200857") {
                format!("{line},hdr_max_lum:616,primary:1")
            } else {
                line.to_string()
            }
        })
        .collect();
    fs::write(&path, lines.join("\n") + "\n").unwrap();
    outputs[0].x = 64;
    mango.write_include(&outputs, None).unwrap();
    let text = read(&path);
    let line = text
        .lines()
        .find(|line| line.contains("PC3M394200857"))
        .unwrap();
    assert!(line.ends_with(",hdr_max_lum:616,primary:1"));
    assert!(line.contains(",x:64,"));
    assert_eq!(line.matches("vrr:").count(), 1);
}

#[test]
fn niri_keeps_hand_added_nodes_and_on_demand_vrr() {
    let env = Env::new(&[]);
    let payload = r#"{"eDP-1": {"name": "eDP-1", "vrr_enabled": false}}"#;
    env.tool("niri", &[("--json", payload)]);
    let mut niri = env.compositor(Kind::Niri);
    let outputs = load_outputs();
    let path = niri.write_include(&outputs, None).unwrap().unwrap();
    let text = read(&path).replace(
        "    position x=1920 y=0\n    variable-refresh-rate\n",
        "    position x=1920 y=0\n    variable-refresh-rate on-demand=true\n    \
         focus-at-startup\n    layout {\n        gaps 4\n    }\n",
    );
    fs::write(&path, text).unwrap();

    let preserved = niri.preserved_options().unwrap();
    assert_eq!(
        preserved["eDP-1"],
        [
            "    variable-refresh-rate on-demand=true",
            "    focus-at-startup",
            "    layout {",
            "        gaps 4",
            "    }",
        ]
    );

    // niri reports vrr_enabled=false while no VRR window is focused.
    let mut fresh = load_outputs();
    let live = niri.query_live();
    niri.enrich(&mut fresh, live.as_ref());
    assert!(fresh[1].adaptive_sync);

    backend(&FakeWlr::new(fresh.clone()), niri.clone())
        .apply_states(&[fresh[1].state()], None)
        .unwrap();
    let commands: Vec<Vec<String>> = env
        .commands()
        .into_iter()
        .map(|cmd| cmd.get(2..).map(<[String]>::to_vec).unwrap_or_default())
        .collect();
    assert!(
        commands.contains(
            &["output", "eDP-1", "vrr", "on", "--on-demand"]
                .map(String::from)
                .to_vec()
        )
    );
    let text = read(&path);
    let block = text.split("output \"eDP-1\" {").nth(1).unwrap();
    assert!(block.contains("variable-refresh-rate on-demand=true"));
    assert!(!block.contains("\n    variable-refresh-rate\n"));
    assert!(block.contains("gaps 4"));

    // Turning VRR off in the TUI drops the on-demand line too.
    fresh[1].adaptive_sync = false;
    niri.write_include(&fresh, None).unwrap();
    let text = read(&path);
    assert!(
        !text
            .split("\"eDP-1\"")
            .nth(1)
            .unwrap()
            .contains("variable-refresh-rate")
    );
}

#[test]
fn invalid_profile_values_never_reach_commands() {
    let env = Env::new(&[]);
    let mut state = load_outputs()[0].state();
    state.transform = "normal; exec rm -rf ~".into();
    for kind in [Kind::Sway, Kind::Niri] {
        let error = env.compositor(kind).check_state(&state).unwrap_err();
        assert!(error.message.contains("invalid transform"));
    }
    assert!(env.compositor(Kind::Sway).sway_output_args(&state).is_err());

    let mut good = load_outputs()[0].state();
    good.color_profile = Some("srgb; exec evil".into());
    assert!(
        !env.compositor(Kind::Sway)
            .sway_output_args(&good)
            .unwrap()
            .contains("exec")
    );
}

#[test]
fn capabilities_follow_the_compositor_version() {
    let env = Env::new(&[]);
    let old_sway = r#"[{"name": "HDMI-A-1", "adaptive_sync_status": "disabled"}]"#;
    env.tool("swaymsg", &[("get_outputs", old_sway)]);
    let mut sway = env.compositor(Kind::Sway);
    let live = sway.query_live();
    sway.enrich(&mut load_outputs(), live.as_ref());
    assert!(!sway.capabilities.hdr);
    assert!(sway.capabilities.color_presets.is_empty());
    assert!(sway.capabilities.icc);
    assert!(Kind::Sway.default_capabilities().hdr);

    let old_niri = r#"{"eDP-1": {"name": "eDP-1", "vrr_enabled": true}}"#;
    env.tool("niri", &[("--json", old_niri)]);
    let mut niri = env.compositor(Kind::Niri);
    let live = niri.query_live();
    niri.enrich(&mut load_outputs(), live.as_ref());
    assert!(niri.capabilities.bit_depths.is_empty());
}

#[test]
fn mango_reads_hdr_from_ipc() {
    let env = Env::new(&[]);
    let payload = r#"{"monitors": [{"name": "eDP-1", "is_hdr": 1}]}"#;
    env.tool("mmsg", &[("all-monitors", payload)]);
    let mut mango = env.compositor(Kind::Mango);
    let mut outputs = load_outputs();
    let live = mango.query_live();
    mango.enrich(&mut outputs, live.as_ref());
    assert_eq!(outputs[1].hdr, Some(true));
    assert_eq!(outputs[0].hdr, None);
}

#[test]
fn mango_paths_follow_mango_not_xdg() {
    let mut env = Env::new(&[]);
    env.vars.insert(
        "XDG_CONFIG_HOME".into(),
        env.dir.join("elsewhere").display().to_string(),
    );
    let mango = env.compositor(Kind::Mango);
    let mango_dir = env.dir.join("home/.config/mango");
    assert_eq!(
        mango.include_path(),
        Some(mango_dir.join("wlr-monitor.conf"))
    );
    fs::create_dir_all(mango_dir.join("conf.d")).unwrap();
    fs::write(
        mango_dir.join("config.conf"),
        "source=./conf.d/monitors.conf\n",
    )
    .unwrap();
    // "./" is relative to ~/.config/mango, not to the including file.
    fs::write(
        mango_dir.join("conf.d/monitors.conf"),
        "source_optional=./wlr-monitor.conf\n",
    )
    .unwrap();
    assert!(mango.include_is_sourced());
}

#[test]
fn niri_config_never_gets_max_bpc_the_compositor_lacks() {
    let env = Env::new(&[]);
    let mut niri = env.compositor(Kind::Niri);
    let mut hdmi = load_outputs().remove(0);
    hdmi.bit_depth = Some(10);
    assert!(
        render(&niri, &hdmi)
            .unwrap()
            .contains(&"    max-bpc 10".to_string())
    );
    // niri <= 25.11: the live command and the config agree.
    niri.capabilities.bit_depths.clear();
    assert!(
        !render(&niri, &hdmi)
            .unwrap()
            .iter()
            .any(|line| line.contains("max-bpc"))
    );
    let commands = niri.niri_output_commands(&hdmi.state(), false).unwrap();
    assert!(!commands.iter().any(|cmd| cmd[0] == "max-bpc"));
}

#[test]
fn deleting_the_include_forgets_remembered_state() {
    let env = Env::new(&[]);
    let mut mango = env.compositor(Kind::Mango);
    let mut outputs = load_outputs();
    outputs[0].color_profile = Some("icc:/x.icc".into());
    let path = mango.write_include(&outputs, None).unwrap().unwrap();
    fs::remove_file(&path).unwrap();
    assert!(mango.read_include_state().is_empty());
    mango
        .write_include(&[load_outputs().remove(1)], None)
        .unwrap();
    assert!(!read(&path).contains("PC3M394200857")); // the disconnected monitor is gone
    let mut fresh = load_outputs();
    mango.enrich(&mut fresh, None);
    assert_eq!(fresh[0].color_profile, None);
}

#[test]
fn custom_include_paths_get_their_own_state() {
    let env = Env::new(&[]);
    let default = env.compositor(Kind::Sway);
    let other = env.dir.join("other.conf");
    let custom = env.with_include(Kind::Sway, &other);
    assert_ne!(default.state_path(), custom.state_path());
    let digest = &sha256::hex_digest(other.display().to_string().as_bytes())[..12];
    assert!(
        custom
            .state_path()
            .ends_with(format!("wlr-monitor/sway-{digest}-layout.json"))
    );
    default.write_include(&load_outputs(), None).unwrap();
    // Nothing from the default include leaks into the custom one.
    custom
        .write_include(&[load_outputs().remove(1)], None)
        .unwrap();
    assert!(!read(&other).contains("PC3M394200857"));
    let retained: Vec<_> = custom
        .retained_outputs(&[])
        .into_iter()
        .map(|o| o.name)
        .collect();
    assert_eq!(retained, ["eDP-1"]);
}

#[test]
fn niri_validates_every_output_before_running_anything() {
    let env = Env::new(&["niri"]);
    let outputs = load_outputs();
    let good = outputs[0].state();
    let mut bad = outputs[1].state();
    bad.transform = "sideways".into();
    let error = env
        .compositor(Kind::Niri)
        .apply(&FakeWlr::new(outputs.clone()), &[good, bad], &outputs, None)
        .unwrap_err();
    assert!(error.to_string().contains("invalid transform"));
    assert!(env.commands().is_empty());
}

#[test]
fn commands_run_with_the_compositor_environment() {
    let mut env = Env::new(&[]);
    env.vars
        .insert("SWAYSOCK".into(), "/run/user/1000/sway-test.sock".into());
    let seen = env.dir.join("seen");
    env.dir.script(
        "bin/swaymsg",
        &format!(
            "echo \"$SWAYSOCK\" >> {}",
            shell_quote(&seen.display().to_string())
        ),
    );
    let outputs = load_outputs();
    env.compositor(Kind::Sway)
        .apply(
            &FakeWlr::new(outputs.clone()),
            &[outputs[0].state()],
            &[],
            None,
        )
        .unwrap();
    let text = read(&seen);
    assert!(!text.is_empty());
    assert!(
        text.lines()
            .all(|line| line == "/run/user/1000/sway-test.sock")
    );
}

#[test]
fn mango_without_include_explains_no_include() {
    let env = Env::new(&["mmsg"]);
    let outputs = load_outputs();
    let mut state = outputs[0].state();
    state.adaptive_sync = true;
    let results = backend(
        &FakeWlr::new(outputs.clone()),
        env.without_include(Kind::Mango),
    )
    .apply_states(&[state], None)
    .unwrap();
    let warnings = warnings(&results);
    assert!(warnings.iter().any(|w| w.contains("--no-include")));
    assert!(!warnings.iter().any(|w| w.contains("source_optional")));
}

#[test]
fn mango_reloads_even_if_only_the_state_file_fails() {
    let env = Env::new(&["mmsg"]);
    fs::create_dir_all(env.config().join("mango")).unwrap();
    fs::write(
        env.config().join("mango/config.conf"),
        "source_optional=./wlr-monitor.conf\n",
    )
    .unwrap();
    // A file where the state directory should be makes the sidecar unwritable.
    fs::write(env.dir.join("state"), "").unwrap();
    let outputs = load_outputs();
    let results = backend(&FakeWlr::new(outputs.clone()), env.compositor(Kind::Mango))
        .apply_states(&[outputs[0].state()], None)
        .unwrap();
    assert!(results.iter().all(|r| r.ok));
    assert!(
        warnings(&results)
            .iter()
            .any(|w| w.contains("will not be remembered"))
    );
    let commands: Vec<Vec<String>> = env
        .commands()
        .into_iter()
        .map(|cmd| cmd[1..].to_vec())
        .collect();
    assert!(commands.contains(&vec!["dispatch".to_string(), "reload_config".to_string()]));
    assert!(env.config().join("mango/wlr-monitor.conf").exists());
}

#[test]
fn drop_unsupported_clears_foreign_profile_settings() {
    let env = Env::new(&[]);
    let mut output = load_outputs().remove(0);
    output.hdr = Some(true);
    output.bit_depth = Some(10);
    output.color_profile = Some("srgb".into());
    env.compositor(Kind::Wlroots)
        .drop_unsupported([&mut output]);
    assert_eq!(
        (output.hdr, output.bit_depth, output.color_profile),
        (None, None, None)
    );
}

#[test]
fn unplugged_twin_keeps_its_own_rule() {
    let env = Env::new(&[]);
    let sway = env.compositor(Kind::Sway);
    sway.write_include(&twins(), None).unwrap();
    sway.write_include(&twins()[..1], None).unwrap(); // unplug the second monitor
    let text = read(&sway.include_path().unwrap());
    let targets: Vec<&str> = text
        .lines()
        .filter(|line| line.starts_with("output"))
        .map(|line| line.split(" enable").next().unwrap())
        .collect();
    // Both rules stay, matched by connector, so the twin is not merged in.
    assert_eq!(targets, ["output \"HDMI-A-1\"", "output \"eDP-1\""]);
}

#[test]
fn moved_unique_monitor_is_not_duplicated() {
    let env = Env::new(&[]);
    let sway = env.compositor(Kind::Sway);
    sway.write_include(&load_outputs(), None).unwrap();
    let mut moved = load_outputs();
    moved[0].name = "DP-7".into(); // same monitor, new connector
    sway.write_include(&moved, None).unwrap();
    assert_eq!(
        read(&sway.include_path().unwrap())
            .matches("PC3M394200857")
            .count(),
        1
    );
}

#[test]
fn sidecar_lookup_skips_ambiguous_twins() {
    let env = Env::new(&[]);
    let mut sway = env.compositor(Kind::Sway);
    let mut outputs = twins();
    outputs[0].color_profile = Some("srgb".into());
    sway.write_include(&outputs, None).unwrap();
    let mut renamed = twins();
    renamed[0].name = "DP-1".into();
    renamed[1].name = "DP-2".into();
    sway.enrich(&mut renamed, None);
    // Neither twin can be identified by EDID, so neither gets srgb.
    assert_eq!(renamed[0].color_profile, None);
    assert_eq!(renamed[1].color_profile, None);
}

#[test]
fn sidecar_lookup_follows_a_unique_monitor_to_a_new_connector() {
    let env = Env::new(&[]);
    let mut sway = env.compositor(Kind::Sway);
    let mut outputs = load_outputs();
    outputs[0].color_profile = Some("srgb".into());
    sway.write_include(&outputs, None).unwrap();
    let mut renamed = load_outputs();
    renamed[0].name = "DP-7".into();
    sway.enrich(&mut renamed, None);
    assert_eq!(renamed[0].color_profile.as_deref(), Some("srgb"));
}

#[test]
fn no_include_ignores_old_state() {
    let env = Env::new(&[]);
    let mut outputs = load_outputs();
    outputs[0].color_profile = Some("srgb".into());
    env.compositor(Kind::Sway)
        .write_include(&outputs, None)
        .unwrap();
    let mut fresh = load_outputs();
    env.without_include(Kind::Sway).enrich(&mut fresh, None);
    assert_eq!(fresh[0].color_profile, None);
}

#[test]
fn icc_profiles_must_be_absolute() {
    let env = Env::new(&[]);
    for kind in [Kind::Sway, Kind::Mango] {
        let compositor = env.compositor(kind);
        assert!(compositor.color_profile_supported("icc:/usr/share/x.icc"));
        assert!(!compositor.color_profile_supported("icc:~/x.icc"));
        assert!(!compositor.color_profile_supported("icc:x.icc"));
    }
    assert!(
        !env.compositor(Kind::Niri)
            .color_profile_supported("icc:/usr/share/x.icc")
    );
    assert!(
        env.compositor(Kind::Sway)
            .color_profile_supported("gamma22")
    );
    assert!(!env.compositor(Kind::Mango).color_profile_supported("srgb"));
}

#[test]
fn relative_include_paths_are_made_absolute() {
    let env = Env::new(&[]);
    let sway = env.with_include(Kind::Sway, Path::new("outputs.conf"));
    let expected = std::env::current_dir().unwrap().join("outputs.conf");
    assert_eq!(sway.include_path(), Some(expected.clone()));
    assert_eq!(
        sway.include_line(),
        format!("include {}", expected.display())
    );
    let home = env.with_include(Kind::Sway, Path::new("~/x/../out.conf"));
    assert_eq!(home.include_path(), Some(env.dir.join("home/out.conf")));
    assert_eq!(home.include_line(), "include ~/out.conf");
}

#[test]
fn state_file_warning_does_not_claim_an_apply() {
    let env = Env::new(&[]);
    fs::write(env.dir.join("state"), "").unwrap(); // makes the state dir unwritable
    let error = env
        .compositor(Kind::Niri)
        .write_include(&load_outputs(), None)
        .unwrap_err();
    assert!(error.state_file);
    assert!(error.message.contains("include was written"));
    assert!(!error.message.contains("applied"));
}

#[test]
fn color_profile_labels() {
    assert_eq!(color_profile_label(None), "default");
    assert_eq!(color_profile_label(Some("icc:/x.icc")), "ICC /x.icc");
    assert_eq!(color_profile_label(Some("srgb")), "srgb");
}

#[test]
fn includes_that_are_not_utf8_are_never_rewritten() {
    let env = Env::new(&[]);
    for kind in [Kind::Mango, Kind::Niri] {
        let compositor = env.compositor(kind);
        let path = compositor
            .write_include(&load_outputs(), None)
            .unwrap()
            .unwrap();
        let mut bytes = fs::read(&path).unwrap();
        bytes.extend_from_slice(b"# caf\xe9\n");
        fs::write(&path, &bytes).unwrap();
        let error = compositor.write_include(&load_outputs(), None).unwrap_err();
        assert!(
            error.message.contains("not valid UTF-8"),
            "{}",
            error.message
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            bytes,
            "{} include kept",
            kind.name()
        );
    }
}

#[test]
fn niri_applies_live_even_if_its_include_is_not_utf8() {
    let env = Env::new(&["niri"]);
    let niri = env.compositor(Kind::Niri);
    let path = niri.write_include(&load_outputs(), None).unwrap().unwrap();
    fs::write(&path, b"// caf\xe9\n").unwrap();
    let outputs = load_outputs();
    let results = backend(&FakeWlr::new(outputs.clone()), niri)
        .apply_states(&[outputs[1].state()], None)
        .unwrap();
    assert!(results.iter().all(|r| r.ok));
    assert!(
        warnings(&results)
            .iter()
            .any(|w| w.contains("not valid UTF-8"))
    );
    assert!(
        env.commands()
            .iter()
            .any(|cmd| cmd.ends_with(&["on".to_string()]))
    );
    assert_eq!(fs::read(&path).unwrap(), b"// caf\xe9\n");
}

#[test]
fn tools_inherit_the_process_environment_by_default() {
    let compositor = Compositor::new(Kind::Sway, None, None, true);
    assert!(compositor.child_env().is_none());
    let env = Env::new(&[]);
    assert!(env.compositor(Kind::Sway).child_env().is_some());
}

#[test]
fn sourced_detection_follows_symlinked_config_dirs() {
    // ~/.config/mango is a stow-style link into a dotfiles checkout that
    // sources the include through `./..`.
    let env = Env::new(&[]);
    let dotfiles = env.dir.join("dotfiles");
    fs::create_dir_all(dotfiles.join("mango")).unwrap();
    fs::create_dir_all(dotfiles.join("shared")).unwrap();
    fs::create_dir_all(env.dir.join("home/.config")).unwrap();
    std::os::unix::fs::symlink(dotfiles.join("mango"), env.dir.join("home/.config/mango")).unwrap();
    fs::write(
        dotfiles.join("mango/config.conf"),
        "source=./../shared/wlr-monitor.conf\n",
    )
    .unwrap();
    let mango = env.with_include(Kind::Mango, &dotfiles.join("shared/wlr-monitor.conf"));
    assert!(mango.include_is_sourced());

    // `..` after a link that leaves the config dir is not the include.
    let env = Env::new(&[]);
    let mango_dir = env.dir.join("home/.config/mango");
    fs::create_dir_all(&mango_dir).unwrap();
    fs::create_dir_all(env.dir.join("other")).unwrap();
    std::os::unix::fs::symlink(env.dir.join("other"), mango_dir.join("lnk")).unwrap();
    fs::write(
        mango_dir.join("config.conf"),
        "source=~/.config/mango/lnk/../wlr-monitor.conf\n",
    )
    .unwrap();
    assert!(!env.compositor(Kind::Mango).include_is_sourced());
}

#[test]
fn env_paths_are_normalised_like_pathlib() {
    let mut env = Env::new(&[]);
    let config = env.dir.join("home/.config").display().to_string();
    env.vars
        .insert("XDG_CONFIG_HOME".into(), format!("{config}//./"));
    let sway = env.compositor(Kind::Sway);
    assert_eq!(
        sway.include_path(),
        Some(env.config().join("sway/wlr-monitor.conf"))
    );
}
