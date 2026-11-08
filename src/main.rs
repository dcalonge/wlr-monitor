mod app;
mod apply;
mod backend;
mod cli;
mod commands;
mod compositor;
mod dialog;
mod exec;
mod fsutil;
mod handler;
mod layout;
mod notification;
mod output;
mod profile;
mod theme;
mod tui;
mod ui;
mod wlr_randr;

use std::io::ErrorKind;
use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;

use app::App;
use backend::Backend;
use cli::Args;
use compositor::{Compositor, Kind};
use profile::ProfileStore;
use wlr_randr::WlrRandr;

fn main() -> ExitCode {
    match run(&Args::parse()) {
        Ok(code) => code,
        Err(error) => {
            // A closed pipe (`wlr-monitor --outputs | head`) is not an error.
            let closed_pipe = error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == ErrorKind::BrokenPipe);
            if closed_pipe {
                return ExitCode::SUCCESS;
            }
            eprintln!("wlr-monitor: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &Args) -> Result<ExitCode> {
    let kind = args
        .compositor
        .unwrap_or_else(|| Kind::detect(|key| std::env::var(key).ok()));
    let compositor = Compositor::new(kind, args.include.as_deref(), !args.no_include);
    let mut backend = Backend::new(WlrRandr::new(args.wlr_randr.as_deref()), compositor);
    let store = ProfileStore::new(args.profile_path());

    if let Some(result) = commands::run(args, &store, &mut backend) {
        return result;
    }
    tui::run(App::new(backend, store, args.profiles_only()))?;
    Ok(ExitCode::SUCCESS)
}
