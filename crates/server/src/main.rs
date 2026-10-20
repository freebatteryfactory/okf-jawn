//! The `okf-jawn-server` binary: the service, and its offline `backup` and `restore` commands.
//!
//! It holds no behaviour: it parses the command line (`commands`), reads the environment, runs
//! the library's startup (`startup::serve`) or an offline command (`offline`), and turns the
//! outcome into an exit status: 0 for success, 1 for a refusal or a failure (one line on stderr,
//! `okf-jawn-server: <code>: <message>`), 2 for a usage error (clap's). A refusal never prints a
//! secret: settings errors name the setting, never its value.

use std::io::Write as _;
use std::process::ExitCode;

use okf_jawn_server::commands::{Command, parse};
use okf_jawn_server::run;

fn main() -> ExitCode {
    let command = match parse(std::env::args_os()) {
        Ok(command) => command,
        Err(error) => {
            // Usage errors, help and version are clap's to print, with clap's status.
            let _printed = error.print();
            return ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(2));
        }
    };
    let outcome = match command {
        Command::Serve => run::serve_from_environment(),
        Command::Backup(args) => run::backup_from_environment(&args),
        Command::Restore(args) => run::restore_from_environment(&args),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let code = serde_json::to_value(&error.code)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default();
            announce(&format!("{code}: {}", error.message));
            ExitCode::FAILURE
        }
    }
}

/// One diagnostic line; a closed stderr must not change the command's exit status.
fn announce(text: &str) {
    let _written = writeln!(std::io::stderr().lock(), "okf-jawn-server: {text}");
}
