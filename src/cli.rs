//! Command line entry point.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::{ArgAction, ArgGroup, Parser};
use serde_json::{Map, Value};

use crate::compositor::{ApplyError, Backend, detect_compositor};
use crate::model::{Output, OutputState, disables_everything, fmt_g, format_refresh, match_state};
use crate::paths;
use crate::profiles::{Profile, ProfileError, ProfileStore, to_pretty_json};
use crate::text::repr;
use crate::wlr::{Wlr, WlrRandr, WlrRandrError};

#[derive(Parser, Debug)]
#[command(
    name = "wlr-monitor",
    version = crate::VERSION,
    about = "A terminal monitor-layout editor for wlroots compositors.",
    disable_version_flag = true,
    infer_long_args = true,
    // Like argparse: an option given twice keeps the last value.
    args_override_self = true,
    group(
        ArgGroup::new("action")
            .multiple(false)
            .args([
                "profile",
                "profiles",
                "list_profiles",
                "active_profile",
                "delete_profile",
                "outputs",
                "print_config",
                "init_include",
            ])
    )
)]
pub struct Args {
    /// show program's version number and exit
    #[arg(short = 'v', long = "version", action = ArgAction::Version)]
    version: Option<bool>,

    /// path to the wlr-randr executable (default: $WLR_RANDR or PATH)
    #[arg(long = "wlr-randr", value_name = "PATH")]
    pub wlr_randr: Option<String>,

    /// profile file or directory (default: $XDG_CONFIG_HOME/wlr-monitor/profiles.json)
    #[arg(long = "config", visible_alias = "cfg", value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// compositor integration to use (default: detect from the environment)
    #[arg(
        long,
        default_value = "auto",
        value_parser = ["auto", "wlroots", "mango", "sway", "niri"]
    )]
    pub compositor: String,

    /// generated compositor config file (default: ~/.config/<compositor>/wlr-monitor.{conf,kdl})
    #[arg(long, value_name = "PATH")]
    pub include: Option<PathBuf>,

    /// do not write the compositor config file when applying
    #[arg(long = "no-include")]
    pub no_include: bool,

    /// apply a saved profile and exit
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// open the profile browser
    #[arg(long)]
    pub profiles: bool,

    /// list saved profile names and exit
    #[arg(long = "list-profiles")]
    pub list_profiles: bool,

    /// print the name of the active profile and exit
    #[arg(long = "active-profile")]
    pub active_profile: bool,

    /// delete a saved profile and exit
    #[arg(long = "delete-profile", value_name = "NAME")]
    pub delete_profile: Option<String>,

    /// print the current outputs and exit
    #[arg(long)]
    pub outputs: bool,

    /// print the compositor config for the current layout, or for a saved profile, and exit
    #[arg(
        long = "print-config",
        value_name = "PROFILE",
        num_args = 0..=1,
        default_missing_value = ""
    )]
    pub print_config: Option<String>,

    /// write the compositor config file from the current layout and print the line to add to
    /// your compositor config
    #[arg(long = "init-include")]
    pub init_include: bool,

    /// with --list-profiles or --outputs, print JSON
    #[arg(long)]
    pub json: bool,

    /// 'profiles' is an alias for --profiles
    #[arg(value_parser = ["profiles"])]
    pub command: Option<String>,
}

/// What the command line needs from the outside world, so tests can swap
/// wlr-randr, the environment and the TUI.
pub struct Runtime<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
    /// Whether `out` / `err` are terminals (colored usage errors and help).
    pub ansi_out: bool,
    pub ansi_err: bool,
    /// The environment for compositor detection; `None` uses the process's.
    pub env: Option<HashMap<String, String>>,
    pub make_wlr: &'a dyn Fn(Option<&str>) -> Box<dyn Wlr>,
    /// Runs the TUI: backend, profile store, profile browser only.
    pub run_tui: &'a mut dyn FnMut(Backend, ProfileStore, bool) -> io::Result<()>,
}

/// Errors that end a one-shot command with `wlr-monitor: <message>`.
enum Failure {
    Message(String),
    Io(io::Error),
}

impl From<WlrRandrError> for Failure {
    fn from(error: WlrRandrError) -> Self {
        Self::Message(error.0)
    }
}

impl From<ProfileError> for Failure {
    fn from(error: ProfileError) -> Self {
        Self::Message(error.0)
    }
}

impl From<ApplyError> for Failure {
    fn from(error: ApplyError) -> Self {
        Self::Message(error.to_string())
    }
}

impl From<crate::compositor::CompositorError> for Failure {
    fn from(error: crate::compositor::CompositorError) -> Self {
        Self::Message(error.message)
    }
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

type Outcome = Result<i32, Failure>;

/// Run the command line with `args` (program name first); returns the exit code.
pub fn run(args: &[OsString], rt: &mut Runtime) -> i32 {
    let args = match Args::try_parse_from(args) {
        Ok(args) => args,
        Err(error) => {
            let (stream, ansi): (&mut dyn Write, bool) = if error.use_stderr() {
                (rt.err, rt.ansi_err)
            } else {
                (rt.out, rt.ansi_out)
            };
            let rendered = error.render();
            let text = if ansi {
                rendered.ansi().to_string()
            } else {
                rendered.to_string()
            };
            let _ = stream.write_all(text.as_bytes());
            return error.exit_code();
        }
    };
    // Lookups only; the IPC tools inherit the real environment unless a
    // test supplied one.
    let env = rt.env.clone();
    let lookup = |key: &str| {
        env.as_ref()
            .and_then(|env| env.get(key))
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let (home, default_store) = match &env {
        None => (paths::home_dir(), ProfileStore::default_path()),
        Some(_) => {
            let home = lookup("HOME").unwrap_or_else(paths::home_dir);
            let config_home = lookup("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
            (home, config_home.join("wlr-monitor").join("profiles.json"))
        }
    };
    let store = ProfileStore::new(Some(
        config_path(args.config.as_deref(), &home).unwrap_or(default_store),
    ));
    let compositor = match detect_compositor(
        Some(&args.compositor),
        env,
        args.include.as_deref(),
        !args.no_include,
    ) {
        Ok(compositor) => compositor,
        Err(message) => {
            let _ = writeln!(rt.err, "wlr-monitor: error: {message}");
            return 2;
        }
    };
    let mut backend = Backend::new((rt.make_wlr)(args.wlr_randr.as_deref()), compositor);

    let outcome = dispatch(&args, &store, &mut backend, rt);
    let outcome = match outcome {
        Some(outcome) => outcome,
        None => {
            let profiles_only = args.profiles || args.command.as_deref() == Some("profiles");
            return match (rt.run_tui)(backend, store, profiles_only) {
                Ok(()) => 0,
                Err(error) => {
                    let _ = writeln!(rt.err, "wlr-monitor: {error}");
                    1
                }
            };
        }
    };
    match outcome {
        Ok(code) => code,
        Err(Failure::Message(message)) => {
            let _ = writeln!(rt.err, "wlr-monitor: {message}");
            1
        }
        Err(Failure::Io(error)) if error.kind() == io::ErrorKind::BrokenPipe => 0,
        Err(Failure::Io(error)) => {
            let _ = writeln!(rt.err, "wlr-monitor: {error}");
            1
        }
    }
}

/// The one-shot commands; `None` means "open the TUI".
fn dispatch(
    args: &Args,
    store: &ProfileStore,
    backend: &mut Backend,
    rt: &mut Runtime,
) -> Option<Outcome> {
    if args.list_profiles {
        return Some(list_profiles(store, backend, args.json, rt));
    }
    if args.active_profile {
        return Some(print_active_profile(store, backend, rt));
    }
    if let Some(name) = &args.delete_profile {
        return Some(delete_profile(store, name, rt));
    }
    if args.outputs {
        return Some(print_outputs(backend, args.json, rt));
    }
    if let Some(name) = &args.profile {
        return Some(apply_profile(backend, store, name, rt));
    }
    if let Some(name) = &args.print_config {
        let name = Some(name.as_str()).filter(|name| !name.is_empty());
        return Some(print_config(backend, store, name, rt));
    }
    if args.init_include {
        return Some(init_include(backend, rt));
    }
    None
}

/// Production runtime: real streams, wlr-randr and TUI.
pub fn main() -> i32 {
    use std::io::IsTerminal;
    let args: Vec<OsString> = std::env::args_os().collect();
    // Unlocked handles: the TUI (and a panic hook) must be able to write.
    let mut out = io::stdout();
    let mut err = io::stderr();
    let ansi_out = out.is_terminal();
    let ansi_err = err.is_terminal();
    let make_wlr = |binary: Option<&str>| -> Box<dyn Wlr> { Box::new(WlrRandr::new(binary)) };
    let mut run_tui = crate::app::run;
    let mut rt = Runtime {
        out: &mut out,
        err: &mut err,
        ansi_out,
        ansi_err,
        env: None,
        make_wlr: &make_wlr,
        run_tui: &mut run_tui,
    };
    let code = run(&args, &mut rt);
    let _ = rt.out.flush();
    code
}

/// Accept either a profiles.json file or a directory, like hyprmon's --cfg.
pub fn config_path(raw: Option<&Path>, home: &Path) -> Option<PathBuf> {
    let raw = raw?;
    let path = paths::clean(&paths::expanduser_users(raw, home));
    if path.is_dir() {
        return Some(path.join("profiles.json"));
    }
    if path.extension().is_some_and(|ext| ext == "json") || path.is_file() {
        return Some(path);
    }
    Some(path.join("profiles.json"))
}

fn states_of(outputs: &[Output]) -> Vec<OutputState> {
    outputs.iter().map(Output::state).collect()
}

fn active_profile(store: &ProfileStore, backend: &mut Backend) -> Result<Option<String>, Failure> {
    let outputs = backend.read_outputs()?;
    Ok(store.find_active(&states_of(&outputs))?)
}

fn list_profiles(
    store: &ProfileStore,
    backend: &mut Backend,
    as_json: bool,
    rt: &mut Runtime,
) -> Outcome {
    let names = store.names()?;
    for problem in store.problems()? {
        writeln!(rt.err, "wlr-monitor: skipped {problem}")?;
    }
    // Listing must still work when wlr-randr is unavailable.
    let active = active_profile(store, backend).ok().flatten();
    if as_json {
        let payload: Vec<Value> = names
            .iter()
            .map(|name| {
                let mut item = Map::new();
                item.insert("name".into(), Value::String(name.clone()));
                item.insert(
                    "is_active".into(),
                    Value::Bool(Some(name) == active.as_ref()),
                );
                Value::Object(item)
            })
            .collect();
        writeln!(rt.out, "{}", to_pretty_json(&Value::Array(payload)))?;
        return Ok(0);
    }
    // One name per line, with an asterisk for the active profile.
    for name in &names {
        if Some(name) == active.as_ref() {
            writeln!(rt.out, "{name} *")?;
        } else {
            writeln!(rt.out, "{name}")?;
        }
    }
    Ok(0)
}

fn print_active_profile(store: &ProfileStore, backend: &mut Backend, rt: &mut Runtime) -> Outcome {
    match active_profile(store, backend)? {
        Some(name) => writeln!(rt.out, "{name}")?,
        None => writeln!(rt.out, "No active profile found")?,
    }
    Ok(0)
}

fn delete_profile(store: &ProfileStore, name: &str, rt: &mut Runtime) -> Outcome {
    if store.delete(name)? {
        writeln!(rt.out, "Deleted profile {}.", repr(name))?;
        return Ok(0);
    }
    writeln!(rt.err, "wlr-monitor: no profile named {}", repr(name))?;
    Ok(1)
}

pub fn output_dict(output: &Output) -> Value {
    let mut result = output.state().to_dict();
    result.insert("name".into(), Value::String(output.name.clone()));
    result.insert(
        "description".into(),
        Value::String(output.description.clone()),
    );
    Value::Object(result)
}

fn print_outputs(backend: &mut Backend, as_json: bool, rt: &mut Runtime) -> Outcome {
    let outputs = backend.read_outputs()?;
    if as_json {
        let payload = Value::Array(outputs.iter().map(output_dict).collect());
        writeln!(rt.out, "{}", to_pretty_json(&payload))?;
        return Ok(0);
    }
    if outputs.is_empty() {
        writeln!(rt.out, "No outputs found.")?;
        return Ok(0);
    }
    writeln!(
        rt.out,
        "{:<14} {:<4} {:<20} {:<12} {:<7} {:<12} {:<4} HDR",
        "NAME", "ON", "MODE", "POSITION", "SCALE", "ROTATION", "VRR"
    )?;
    for output in &outputs {
        let mode = output.mode.as_ref().map_or_else(
            || "-".to_string(),
            |mode| format!("{}@{}", mode.size_text(), format_refresh(mode.refresh)),
        );
        let hdr = match output.hdr {
            None => "-",
            Some(true) => "on",
            Some(false) => "off",
        };
        writeln!(
            rt.out,
            "{:<14} {:<4} {:<20} {:<12} {:<7} {:<12} {:<4} {}",
            output.name,
            if output.enabled { "yes" } else { "no" },
            mode,
            format!("{},{}", output.x, output.y),
            fmt_g(output.scale),
            output.transform,
            if output.adaptive_sync { "on" } else { "off" },
            hdr
        )?;
    }
    Ok(0)
}

fn matching_state<'a>(
    profile: &'a Profile,
    output: &Output,
    outputs: &[Output],
) -> Option<&'a OutputState> {
    let connected: Vec<String> = outputs.iter().map(Output::hardware_id).collect();
    match_state(
        &profile.outputs,
        &output.name,
        &output.hardware_id(),
        &connected,
    )
}

fn apply_profile(
    backend: &mut Backend,
    store: &ProfileStore,
    name: &str,
    rt: &mut Runtime,
) -> Outcome {
    let Some(profile) = store.get(name)? else {
        let names = store.names()?;
        let available = if names.is_empty() {
            "none".to_string()
        } else {
            names.join(", ")
        };
        writeln!(
            rt.err,
            "wlr-monitor: no profile named {} (available: {available})",
            repr(name)
        )?;
        return Ok(1);
    };

    let mut outputs = backend.read_outputs()?;
    let live_enabled: Vec<(String, bool)> = outputs
        .iter()
        .map(|output| (output.name.clone(), output.enabled))
        .collect();
    let current_names: HashSet<String> = outputs.iter().map(|output| output.name.clone()).collect();
    let snapshot = outputs.clone();
    let mut states = Vec::new();
    let mut not_in_profile = Vec::new();
    let mut used: HashSet<String> = HashSet::new();
    for output in &mut outputs {
        let Some(state) = matching_state(&profile, output, &snapshot) else {
            not_in_profile.push(output.name.clone());
            continue;
        };
        used.insert(state.name.clone());
        state.apply_to(output, false);
        backend.compositor.drop_unsupported([&mut *output]);
        states.push(output.state());
    }

    if states.is_empty() {
        writeln!(
            rt.err,
            "wlr-monitor: profile {} does not match any connected output.",
            repr(name)
        )?;
        return Ok(1);
    }
    if disables_everything(&live_enabled, &states) {
        writeln!(
            rt.err,
            "wlr-monitor: refusing to apply {}: it would turn off every connected monitor.",
            repr(name)
        )?;
        return Ok(1);
    }

    let results = backend.apply_states(&states, Some(name))?;
    let mut warned = HashSet::new();
    let mut reported = HashSet::new();
    for result in &results {
        if !result.warning.is_empty() && warned.insert(result.warning.clone()) {
            writeln!(rt.err, "Warning: {}", result.warning)?;
        }
        if result.name == "config" {
            continue;
        }
        if !result.ok {
            writeln!(rt.err, "Failed {}: {}", result.name, result.detail())?;
        } else if !reported.contains(&result.name) {
            writeln!(rt.out, "Applied {}.", result.name)?;
        }
        reported.insert(result.name.clone());
    }
    if !not_in_profile.is_empty() {
        writeln!(
            rt.err,
            "Left unchanged (not in profile): {}",
            not_in_profile.join(", ")
        )?;
    }
    // Saved outputs matched to a renamed connector were applied, not skipped.
    let mut absent: Vec<&str> = profile
        .outputs
        .iter()
        .map(|state| state.name.as_str())
        .filter(|saved| !current_names.contains(*saved) && !used.contains(*saved))
        .collect();
    absent.sort_unstable();
    absent.dedup();
    if !absent.is_empty() {
        writeln!(rt.err, "Skipped (not connected): {}", absent.join(", "))?;
    }
    Ok(if results.iter().any(|result| !result.ok) {
        1
    } else {
        0
    })
}

fn print_config(
    backend: &mut Backend,
    store: &ProfileStore,
    name: Option<&str>,
    rt: &mut Runtime,
) -> Outcome {
    if !backend.compositor.supports_include() {
        writeln!(
            rt.err,
            "wlr-monitor: the generic wlroots backend has no config format; use --compositor \
             mango|sway|niri."
        )?;
        return Ok(1);
    }
    let mut outputs = backend.read_outputs()?;
    if let Some(name) = name {
        let Some(profile) = store.get(name)? else {
            writeln!(rt.err, "wlr-monitor: no profile named {}", repr(name))?;
            return Ok(1);
        };
        let snapshot = outputs.clone();
        for output in &mut outputs {
            if let Some(state) = matching_state(&profile, output, &snapshot) {
                state.apply_to(output, false);
            }
        }
        backend.compositor.drop_unsupported(outputs.iter_mut());
    }
    // The same layout write_include() would write, disconnected outputs included.
    let compositor = &backend.compositor;
    let layout = compositor.layout_for_include(&outputs)?;
    let text = compositor.render_include(&layout, name)?;
    rt.out.write_all(text.as_bytes())?;
    Ok(0)
}

fn init_include(backend: &mut Backend, rt: &mut Runtime) -> Outcome {
    if !backend.compositor.supports_include() {
        writeln!(
            rt.err,
            "wlr-monitor: no compositor integration detected; use --compositor mango|sway|niri."
        )?;
        return Ok(1);
    }
    if !backend.compositor.write_include_enabled {
        writeln!(
            rt.err,
            "wlr-monitor: --init-include conflicts with --no-include."
        )?;
        return Ok(1);
    }
    let path = match backend.write_include(None) {
        Ok(path) => path,
        Err(ApplyError::Compositor(error)) if error.state_file => {
            writeln!(rt.err, "Warning: {error}")?;
            backend.compositor.include_path()
        }
        Err(error) => return Err(error.into()),
    };
    let compositor = &backend.compositor;
    let Some(path) = path.or_else(|| compositor.include_path()) else {
        return Ok(1);
    };
    writeln!(rt.out, "Wrote {}.", compositor.display_path(&path))?;
    if compositor.include_is_sourced() {
        writeln!(rt.out, "Your compositor config already includes it.")?;
    } else {
        let place = compositor.main_config().map_or_else(
            || "your config".to_string(),
            |main| compositor.display_path(&main),
        );
        writeln!(rt.out, "Add this line to {place}:")?;
        writeln!(rt.out, "  {}", compositor.include_line())?;
    }
    Ok(0)
}

#[cfg(test)]
mod tests;
