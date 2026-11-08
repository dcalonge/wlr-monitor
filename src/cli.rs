//! Command line arguments.

use std::path::PathBuf;

use clap::{ArgAction, ArgGroup, Parser};

use crate::compositor::Kind;

#[derive(Parser, Debug)]
#[command(
    about,
    version,
    disable_version_flag = true,
    group(ArgGroup::new("action").multiple(false).args([
        "profile",
        "profiles",
        "list_profiles",
        "active_profile",
        "delete_profile",
        "outputs",
        "print_config",
        "init_include",
    ]))
)]
pub struct Args {
    /// Show the version number and exit
    #[arg(short = 'v', long, action = ArgAction::Version)]
    version: Option<bool>,

    /// Path to the wlr-randr executable (default: $WLR_RANDR or PATH)
    #[arg(long, value_name = "PATH")]
    pub wlr_randr: Option<String>,

    /// Profile file or directory (default: $XDG_CONFIG_HOME/wlr-monitor/profiles.json)
    #[arg(long, visible_alias = "cfg", value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Compositor integration to use (default: detect from the environment)
    #[arg(long, value_enum)]
    pub compositor: Option<Kind>,

    /// Generated compositor config file (default: ~/.config/<compositor>/wlr-monitor.{conf,kdl})
    #[arg(long, value_name = "PATH")]
    pub include: Option<PathBuf>,

    /// Do not write the compositor config file when applying
    #[arg(long)]
    pub no_include: bool,

    /// Apply a saved profile and exit
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Open the profile browser
    #[arg(long)]
    pub profiles: bool,

    /// List saved profile names and exit
    #[arg(long)]
    pub list_profiles: bool,

    /// Print the name of the active profile and exit
    #[arg(long)]
    pub active_profile: bool,

    /// Delete a saved profile and exit
    #[arg(long, value_name = "NAME")]
    pub delete_profile: Option<String>,

    /// Print the current outputs and exit
    #[arg(long)]
    pub outputs: bool,

    /// Print the compositor config for the current layout, or for a saved profile, and exit
    #[arg(long, value_name = "PROFILE", num_args = 0..=1, default_missing_value = "")]
    pub print_config: Option<String>,

    /// Write the compositor config file from the current layout and print the line to add to
    /// your compositor config
    #[arg(long)]
    pub init_include: bool,

    /// With --list-profiles or --outputs, print JSON
    #[arg(long)]
    pub json: bool,

    /// Same as --profiles
    #[arg(value_parser = ["profiles"])]
    command: Option<String>,
}

impl Args {
    pub fn profiles_only(&self) -> bool {
        self.profiles || self.command.is_some()
    }

    /// The profile file: `--config` may name the file or its directory.
    pub fn profile_path(&self) -> Option<PathBuf> {
        let path = self.config.as_ref()?;
        let is_file = path.extension().is_some_and(|ext| ext == "json") || path.is_file();
        Some(if is_file && !path.is_dir() {
            path.clone()
        } else {
            path.join("profiles.json")
        })
    }
}
