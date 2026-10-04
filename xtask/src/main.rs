//! Executable repository maintenance using the selected libraries, not generated-looking substitutes.

use std::error::Error;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "okf-jawn foundation tasks")]
struct Arguments {
    #[command(subcommand)]
    command: Task,
}
#[derive(Subcommand)]
enum Task {
    Generate {
        #[arg(long)]
        out: PathBuf,
    },
    SourcePolicy {
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(std::io::stderr().lock(), "{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let arguments = match Arguments::try_parse() {
        Ok(arguments) => arguments,
        Err(error) if error.use_stderr() => return Err(error.into()),
        Err(error) => {
            write!(std::io::stdout().lock(), "{error}")?;
            return Ok(());
        }
    };
    match arguments.command {
        Task::Generate { out } => {
            std::fs::create_dir_all(&out)?;
            api::generate(&out.join("api"))?;
            let path = out.join("cli");
            std::fs::create_dir_all(&path)?;
            for shell in [
                clap_complete::Shell::Bash,
                clap_complete::Shell::Zsh,
                clap_complete::Shell::Fish,
                clap_complete::Shell::PowerShell,
            ] {
                clap_complete::generate_to(shell, &mut okf_jawn_cli::command(), "okf-jawn", &path)?;
            }
            clap_mangen::generate_to(okf_jawn_cli::command(), &path)?;
            Ok(())
        }
        Task::SourcePolicy { root } => source_policy::check(&root),
    }
}

mod api;
mod fixtures;
mod output;
mod source_policy;
