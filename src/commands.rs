//! The one-shot commands: everything except the TUI.

use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::json;

use crate::apply::Applied;
use crate::backend::Backend;
use crate::cli::Args;
use crate::output::{Output, OutputState, disables_everything, fmt_num};
use crate::profile::ProfileStore;

/// Run the command the arguments ask for, or `None` to open the TUI.
pub fn run(args: &Args, store: &ProfileStore, backend: &mut Backend) -> Option<Result<ExitCode>> {
    let done = |result: Result<()>| Some(result.map(|()| ExitCode::SUCCESS));
    if args.list_profiles {
        done(list_profiles(store, backend, args.json))
    } else if args.active_profile {
        done(print_active_profile(store, backend))
    } else if let Some(name) = &args.delete_profile {
        done(delete_profile(store, name))
    } else if args.outputs {
        done(print_outputs(backend, args.json))
    } else if let Some(name) = &args.profile {
        Some(apply_profile(backend, store, name))
    } else if let Some(name) = &args.print_config {
        done(print_config(
            backend,
            store,
            Some(name.as_str()).filter(|name| !name.is_empty()),
        ))
    } else if args.init_include {
        done(init_include(backend))
    } else {
        None
    }
}

fn states(outputs: &[Output]) -> Vec<OutputState> {
    outputs.iter().map(Output::state).collect()
}

fn active_profile(store: &ProfileStore, backend: &mut Backend) -> Result<Option<String>> {
    store.find_active(&states(&backend.read_outputs()?))
}

fn list_profiles(store: &ProfileStore, backend: &mut Backend, as_json: bool) -> Result<()> {
    let names = store.names()?;
    for problem in store.problems()? {
        eprintln!("wlr-monitor: skipped {problem}");
    }
    // Listing must still work when wlr-randr is unavailable.
    let active = active_profile(store, backend).ok().flatten();
    let mut out = io::stdout().lock();
    if as_json {
        let items: Vec<_> = names
            .iter()
            .map(|name| json!({ "name": name, "is_active": active.as_ref() == Some(name) }))
            .collect();
        writeln!(out, "{}", serde_json::to_string_pretty(&items)?)?;
        return Ok(());
    }
    for name in &names {
        let marker = if active.as_ref() == Some(name) {
            " *"
        } else {
            ""
        };
        writeln!(out, "{name}{marker}")?;
    }
    Ok(())
}

fn print_active_profile(store: &ProfileStore, backend: &mut Backend) -> Result<()> {
    let name = active_profile(store, backend)?;
    writeln!(
        io::stdout(),
        "{}",
        name.as_deref().unwrap_or("No active profile found")
    )?;
    Ok(())
}

fn delete_profile(store: &ProfileStore, name: &str) -> Result<()> {
    if !store.delete(name)? {
        bail!("no profile named {name:?}");
    }
    writeln!(io::stdout(), "Deleted profile {name:?}.")?;
    Ok(())
}

fn print_outputs(backend: &mut Backend, as_json: bool) -> Result<()> {
    #[derive(Serialize)]
    struct Row<'a> {
        name: &'a str,
        description: &'a str,
        #[serde(flatten)]
        state: &'a OutputState,
    }
    let outputs = backend.read_outputs()?;
    let mut out = io::stdout().lock();
    if as_json {
        let states = states(&outputs);
        let rows: Vec<_> = outputs
            .iter()
            .zip(&states)
            .map(|(output, state)| Row {
                name: &output.name,
                description: &output.description,
                state,
            })
            .collect();
        // Through a `Value`, so keys come out sorted.
        writeln!(
            out,
            "{}",
            serde_json::to_string_pretty(&serde_json::to_value(rows)?)?
        )?;
        return Ok(());
    }
    if outputs.is_empty() {
        writeln!(out, "No outputs found.")?;
        return Ok(());
    }
    writeln!(
        out,
        "{:<14} {:<4} {:<20} {:<12} {:<7} {:<12} {:<4} HDR",
        "NAME", "ON", "MODE", "POSITION", "SCALE", "ROTATION", "VRR"
    )?;
    for output in &outputs {
        let mode = output.mode.as_ref().map_or_else(
            || "-".to_string(),
            |mode| format!("{}@{}", mode.size_text(), fmt_num(mode.refresh, 2)),
        );
        let hdr = match output.hdr {
            None => "-",
            Some(true) => "on",
            Some(false) => "off",
        };
        writeln!(
            out,
            "{:<14} {:<4} {:<20} {:<12} {:<7} {:<12} {:<4} {}",
            output.name,
            if output.enabled { "yes" } else { "no" },
            mode,
            format!("{},{}", output.position.x, output.position.y),
            fmt_num(output.scale, 6),
            output.transform.as_str(),
            if output.adaptive_sync { "on" } else { "off" },
            hdr
        )?;
    }
    Ok(())
}

fn apply_profile(backend: &mut Backend, store: &ProfileStore, name: &str) -> Result<ExitCode> {
    let Some(profile) = store.get(name)? else {
        let names = store.names()?;
        let available = if names.is_empty() {
            "none".into()
        } else {
            names.join(", ")
        };
        bail!("no profile named {name:?} (available: {available})");
    };

    let mut outputs = backend.read_outputs()?;
    let live: Vec<(String, bool)> = outputs
        .iter()
        .map(|o| (o.name.clone(), o.enabled))
        .collect();
    let matched = profile.apply_to(&mut outputs);
    backend.compositor.drop_unsupported(&mut outputs);
    let states: Vec<OutputState> = outputs
        .iter()
        .filter(|output| matched.applied.contains(&output.name))
        .map(Output::state)
        .collect();
    if states.is_empty() {
        bail!("profile {name:?} does not match any connected output.");
    }
    if disables_everything(&live, &states) {
        bail!("refusing to apply {name:?}: it would turn off every connected monitor.");
    }

    let applied = backend.apply(&states, Some(name))?;
    print_applied(&applied);
    if !matched.not_in_profile.is_empty() {
        eprintln!(
            "Left unchanged (not in profile): {}",
            matched.not_in_profile.join(", ")
        );
    }
    if !matched.not_connected.is_empty() {
        eprintln!(
            "Skipped (not connected): {}",
            matched.not_connected.join(", ")
        );
    }
    Ok(if applied.succeeded() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn print_applied(applied: &Applied) {
    for warning in &applied.warnings {
        eprintln!("Warning: {warning}");
    }
    for result in &applied.results {
        match &result.error {
            Some(error) => eprintln!("Failed {}: {error}", result.name),
            None => println!("Applied {}.", result.name),
        }
    }
}

fn print_config(backend: &mut Backend, store: &ProfileStore, name: Option<&str>) -> Result<()> {
    if backend.compositor.include_path().is_none() {
        bail!(
            "the generic wlroots backend has no config format; use --compositor mango|sway|niri."
        );
    }
    let mut outputs = backend.read_outputs()?;
    if let Some(name) = name {
        let Some(profile) = store.get(name)? else {
            bail!("no profile named {name:?}");
        };
        profile.apply_to(&mut outputs);
        backend.compositor.drop_unsupported(&mut outputs);
    }
    // The same layout write_include() would write, disconnected outputs included.
    let layout = backend.compositor.layout_for_include(&outputs)?;
    let text = backend.compositor.render_include(&layout, name)?;
    io::stdout().write_all(text.as_bytes())?;
    Ok(())
}

fn init_include(backend: &mut Backend) -> Result<()> {
    if backend.compositor.include_path().is_none() {
        bail!("no compositor integration detected; use --compositor mango|sway|niri.");
    }
    if !backend.compositor.include_enabled {
        bail!("--init-include conflicts with --no-include.");
    }
    let Some(written) = backend.write_include(None)? else {
        return Ok(());
    };
    if let Some(warning) = &written.warning {
        eprintln!("Warning: {warning}");
    }
    let c = &backend.compositor;
    let mut out = io::stdout().lock();
    writeln!(out, "Wrote {}.", c.display_path(&written.path))?;
    if c.include_is_sourced() {
        writeln!(out, "Your compositor config already includes it.")?;
    } else {
        let place = c
            .main_config()
            .map_or_else(|| "your config".to_string(), |main| c.display_path(&main));
        writeln!(out, "Add this line to {place}:")?;
        writeln!(out, "  {}", c.include_line())?;
    }
    Ok(())
}
