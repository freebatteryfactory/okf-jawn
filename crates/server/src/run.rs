//! What the binary runs, from the process environment: the service until a shutdown signal, or
//! one offline command. The binary (`main.rs`) only parses its command line and maps the outcome
//! to an exit status; everything it runs is here, in the library, so it is tested in process.
//!
//! - **The service** reads its settings from the environment and this machine's places
//!   (`settings::Places::of_this_machine`), writes its diagnostics to stderr, and serves until a
//!   shutdown signal: Ctrl-C everywhere; on Unix `SIGTERM` too (`docker stop`); on Windows
//!   Ctrl-Break, closing the console and the system's shutdown too. Its stdout lines (the
//!   listening origins and, locally, the first launch link) are written as `okf-jawn-server:
//!   <key> <value>`; a stdout that cannot be written to (closed by a launcher that starts it
//!   detached) is tolerated, never fatal.
//! - **`backup`** prints each archive it wrote, one path a line; **`restore`** prints one JSON
//!   `RestoreReport` a line, one for each workspace.

use std::cell::RefCell;
use std::io::Write as _;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use tokio_util::sync::CancellationToken;

use crate::config::invalid;
use crate::offline::{BackupArgs, RestoreArgs, backup, restore};
use crate::settings::{Places, Settings, offline_settings};
use crate::startup;

/// Run the service from the process environment until a shutdown signal.
///
/// # Errors
/// Returns the first refusal of startup, or a listener's failure while serving.
pub fn serve_from_environment() -> Result<(), ApiError> {
    install_diagnostics();
    let settings =
        from_environment(|lookup| Settings::from_lookup(lookup, &Places::of_this_machine()))?;
    runtime()?.block_on(async {
        let stop = CancellationToken::new();
        let signalled = stop.clone();
        tokio::spawn(async move {
            shutdown_signal().await;
            signalled.cancel();
        });
        startup::serve_settings(settings, &print_line, stop).await
    })
}

/// Run `backup` from the process environment and print each archive it wrote.
///
/// # Errors
/// Returns the command's refusal.
pub fn backup_from_environment(args: &BackupArgs) -> Result<(), ApiError> {
    install_diagnostics();
    let (kind, data_dir) =
        from_environment(|lookup| offline_settings(lookup, &Places::of_this_machine()))?;
    let written = runtime()?.block_on(backup(kind, &data_dir, args))?;
    for path in written {
        print_line(&path.display().to_string());
    }
    Ok(())
}

/// Run `restore --replacement` from the process environment and print each workspace's report.
///
/// # Errors
/// Returns the command's refusal.
pub fn restore_from_environment(args: &RestoreArgs) -> Result<(), ApiError> {
    install_diagnostics();
    let (_kind, data_dir) =
        from_environment(|lookup| offline_settings(lookup, &Places::of_this_machine()))?;
    let reports = runtime()?.block_on(restore(&data_dir, args))?;
    for report in reports {
        let line = serde_json::to_string(&report).map_err(|_| {
            ApiError::new(ErrorCode::Internal, "a restore report could not be written")
        })?;
        print_line(&line);
    }
    Ok(())
}

/// Read only the settings the pure parser requests, preserving a present non-Unicode refusal.
/// The parser can inspect defaults, but no parsed configuration reaches IO after that refusal.
/// Explicit local mode still ignores hosted credentials, as the authentication parser specifies.
fn from_environment<T>(
    parse: impl FnOnce(&dyn Fn(&str) -> Option<String>) -> Result<T, ApiError>,
) -> Result<T, ApiError> {
    let refused = RefCell::new(None);
    let parsed = parse(&|name| match std::env::var(name) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            refused.borrow_mut().get_or_insert_with(|| name.to_owned());
            None
        }
    });
    match refused.into_inner() {
        Some(name) => Err(invalid(&name, "must contain valid Unicode")),
        None => parsed,
    }
}

/// Write one line to stdout; a stdout that cannot be written to is noted on stderr, never fatal.
pub fn print_line(line: &str) {
    if writeln!(std::io::stdout().lock(), "{line}").is_err() {
        tracing::warn!("standard output is closed; a line was not printed");
    }
}

/// Diagnostics on stderr, at level INFO, without colour; a subscriber already set is kept.
fn install_diagnostics() {
    let _installed = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}

/// A multi-threaded Tokio runtime.
fn runtime() -> Result<tokio::runtime::Runtime, ApiError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| ApiError::new(ErrorCode::Internal, "the async runtime could not start"))
}

/// The first shutdown signal this platform delivers. A signal that cannot be listened for is
/// skipped: the others still stop the service.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let terminate = async {
            match signal(SignalKind::terminate()) {
                Ok(mut terminate) => {
                    let _received = terminate.recv().await;
                }
                Err(_) => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _interrupted = tokio::signal::ctrl_c() => {}
            () = terminate => {}
        }
    }
    #[cfg(windows)]
    {
        use tokio::signal::windows::{ctrl_break, ctrl_close, ctrl_shutdown};
        let wait = |signal: std::io::Result<tokio::signal::windows::CtrlBreak>| async move {
            match signal {
                Ok(mut signal) => {
                    let _received = signal.recv().await;
                }
                Err(_) => std::future::pending::<()>().await,
            }
        };
        let close = async {
            match ctrl_close() {
                Ok(mut close) => {
                    let _received = close.recv().await;
                }
                Err(_) => std::future::pending::<()>().await,
            }
        };
        let shutdown = async {
            match ctrl_shutdown() {
                Ok(mut shutdown) => {
                    let _received = shutdown.recv().await;
                }
                Err(_) => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _interrupted = tokio::signal::ctrl_c() => {}
            () = wait(ctrl_break()) => {}
            () = close => {}
            () = shutdown => {}
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _interrupted = tokio::signal::ctrl_c().await;
    }
}
