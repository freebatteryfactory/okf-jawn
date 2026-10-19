//! The binary's command line, parsed with `clap`.
//!
//! ```text
//! okf-jawn-server [serve]
//! okf-jawn-server backup --output <directory> [--tenant <tenant id>]
//! okf-jawn-server restore --replacement <installation archive> [<workspace archive>...]
//! ```
//!
//! No argument means `serve`: the CLI's relay, and a desktop launcher, start the server with
//! none. Every setting is an environment variable, never a flag, so the service and the offline
//! commands of one deployment read the same environment. A usage error is clap's, with its exit
//! status 2; `--help` and `--version` are clap's answers with status 0.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Arg, ArgAction, value_parser};

use crate::offline::{BackupArgs, RestoreArgs};

/// What the binary is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Run the service (`serve`, or no argument).
    Serve,
    /// Write a backup set (`backup`).
    Backup(BackupArgs),
    /// Replace the installation from a backup set (`restore --replacement`).
    Restore(RestoreArgs),
}

/// Parse `args`, the program name first.
///
/// # Errors
/// Returns clap's error: a usage error (exit status 2), or the help or version text it shows
/// instead of running (exit status 0).
pub fn parse<I, T>(args: I) -> Result<Command, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let matches = command().try_get_matches_from(args)?;
    match matches.subcommand() {
        None | Some(("serve", _)) => Ok(Command::Serve),
        Some(("backup", backup)) => Ok(Command::Backup(BackupArgs {
            output: backup
                .get_one::<PathBuf>("output")
                .cloned()
                .unwrap_or_default(),
            tenant: backup.get_one::<String>("tenant").cloned(),
        })),
        Some(("restore", restore)) => Ok(Command::Restore(RestoreArgs {
            installation: restore
                .get_one::<PathBuf>("replacement")
                .cloned()
                .unwrap_or_default(),
            workspaces: restore
                .get_many::<PathBuf>("workspaces")
                .map(|paths| paths.cloned().collect())
                .unwrap_or_default(),
        })),
        Some((name, _)) => Err(clap::Error::raw(
            clap::error::ErrorKind::InvalidSubcommand,
            format!("unknown command {name}"),
        )),
    }
}

/// The command definition.
#[must_use]
pub fn command() -> clap::Command {
    clap::Command::new("okf-jawn-server")
        .version(env!("CARGO_PKG_VERSION"))
        .about("The okf-jawn service, and its offline backup and restore")
        .after_help(
            "Settings are environment variables (OKF_*, WORKOS_*), never arguments. With no \
             command the service runs.",
        )
        .subcommand(clap::Command::new("serve").about("Run the service (the default)"))
        .subcommand(
            clap::Command::new("backup")
                .about(
                    "Write the installation archive and every workspace archive, with no \
                     service on the data directory",
                )
                .arg(
                    Arg::new("output")
                        .long("output")
                        .value_name("DIRECTORY")
                        .help("A new or empty directory, outside the data directory")
                        .required(true)
                        .action(ArgAction::Set)
                        .value_parser(value_parser!(PathBuf)),
                )
                .arg(
                    Arg::new("tenant")
                        .long("tenant")
                        .value_name("TENANT")
                        .help("Hosted only: the one tenant to back up")
                        .action(ArgAction::Set),
                ),
        )
        .subcommand(
            clap::Command::new("restore")
                .about(
                    "Replace the installation from a backup set, with no service on the data \
                     directory",
                )
                .arg(
                    Arg::new("replacement")
                        .long("replacement")
                        .value_name("INSTALLATION_ARCHIVE")
                        .help("The installation archive, restored first")
                        .required(true)
                        .action(ArgAction::Set)
                        .value_parser(value_parser!(PathBuf)),
                )
                .arg(
                    Arg::new("workspaces")
                        .value_name("WORKSPACE_ARCHIVE")
                        .help("Each workspace archive, restored after it")
                        .num_args(0..)
                        .action(ArgAction::Append)
                        .value_parser(value_parser!(PathBuf)),
                ),
        )
}
