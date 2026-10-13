//! The one entry point: parse the arguments, then run an operation or the relay.
//!
//! Exit codes: 0 for success, help and version; 2 for a usage error, as clap reports it; 1 for
//! every other failure, which is written to the error stream in plain words with its error
//! code. An operation writes its response body to the output stream and nothing else. Whatever
//! the CLI itself writes to the error stream passes through [`Credential::redact`] first.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use clap::ArgMatches;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::metadata::operations;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use crate::credential::Credential;
use crate::endpoint::Endpoint;
use crate::relay::{HttpTransport, Limits, ProcessLauncher, Relay, StartPolicy, server_beside};
use crate::{RELAY_COMMAND, TOKEN_VARIABLE};

/// The standard streams of one run; tests pass in-memory ones.
#[derive(Debug)]
pub struct Streams<I, O, E> {
    /// Where an operation's `-` input and the relay's messages are read from.
    pub stdin: I,
    /// Where a response body or the relay's protocol messages are written.
    pub stdout: O,
    /// Where failures and diagnostics are written.
    pub stderr: E,
}

/// The exit code of a failure that is not a usage error.
const FAILURE: u8 = 1;

/// Run the command line `arguments` (the program name first) and return the exit code.
///
/// `environment` looks up one environment variable; the bearer token and the connector
/// credential are read through it and from nowhere else in the arguments.
pub async fn run<A, I, O, E>(
    arguments: A,
    environment: &dyn Fn(&str) -> Option<String>,
    streams: Streams<I, O, E>,
) -> u8
where
    A: IntoIterator,
    A::Item: Into<OsString> + Clone,
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
{
    let Streams {
        mut stdin,
        mut stdout,
        mut stderr,
    } = streams;
    let matches = match crate::command().try_get_matches_from(arguments) {
        Ok(matches) => matches,
        Err(error) if error.use_stderr() => {
            let _ = stderr.write_all(error.to_string().as_bytes()).await;
            let _ = stderr.flush().await;
            return u8::try_from(error.exit_code()).unwrap_or(FAILURE);
        }
        Err(error) => {
            let _ = stdout.write_all(error.to_string().as_bytes()).await;
            let _ = stdout.flush().await;
            return 0;
        }
    };
    let Some((name, selected)) = matches.subcommand() else {
        return fail(
            &mut stderr,
            None,
            &ApiError::new(ErrorCode::InvalidInput, "A command is required"),
        )
        .await;
    };
    if name == RELAY_COMMAND {
        return match relay(selected, environment) {
            Ok(relay) => {
                crate::relay::serve(
                    relay,
                    Streams {
                        stdin,
                        stdout,
                        stderr,
                    },
                )
                .await
            }
            Err(error) => fail(&mut stderr, None, &error).await,
        };
    }
    let credential = match Credential::from_environment(environment, TOKEN_VARIABLE) {
        Ok(credential) => credential,
        Err(error) => return fail(&mut stderr, None, &error).await,
    };
    match operate(name, selected, credential.as_ref(), &mut stdin, &mut stdout).await {
        Ok(()) => 0,
        Err(error) => fail(&mut stderr, credential.as_ref(), &error).await,
    }
}

/// A failure as the CLI prints it: its code and message, then any field, request and detail.
#[must_use]
pub fn report(error: &ApiError) -> String {
    let code = serde_json::to_value(&error.code)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{:?}", error.code));
    let mut text = format!("okf-jawn: {code}: {}\n", error.message);
    // Writing to a `String` cannot fail.
    if let Some(field) = &error.field {
        let _ = writeln!(text, "  field: {field}");
    }
    if let Some(request_id) = &error.request_id {
        let _ = writeln!(text, "  request: {request_id}");
    }
    if let Some(detail) = &error.detail
        && let Ok(detail) = serde_json::to_string(detail)
    {
        let _ = writeln!(text, "  detail: {detail}");
    }
    text
}

/// Write a failure to the error stream, without the credential, and return the failure code.
async fn fail<E: AsyncWrite + Unpin>(
    stderr: &mut E,
    credential: Option<&Credential>,
    error: &ApiError,
) -> u8 {
    let text = report(error);
    let text = credential.map_or_else(|| text.clone(), |credential| credential.redact(&text));
    let _ = stderr.write_all(text.as_bytes()).await;
    let _ = stderr.flush().await;
    FAILURE
}

/// Run the operation command `name`.
async fn operate<I: AsyncRead + Unpin, O: AsyncWrite + Unpin>(
    name: &str,
    selected: &ArgMatches,
    credential: Option<&Credential>,
    stdin: &mut I,
    stdout: &mut O,
) -> Result<(), ApiError> {
    let operation = operations()
        .into_iter()
        .find(|operation| operation.id == name)
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidInput,
                format!("{name} is not an operation"),
            )
        })?;
    let endpoint = endpoint(selected)?;
    let argument = selected
        .get_one::<String>("json")
        .map_or("{}", String::as_str);
    let input = crate::request::input(argument, stdin).await?;
    crate::request::call(&operation, &endpoint, credential, input, stdout).await
}

/// The checked `--server` address of the selected command.
fn endpoint(selected: &ArgMatches) -> Result<Endpoint, ApiError> {
    Endpoint::parse(
        selected
            .get_one::<String>("server")
            .map_or(crate::DEFAULT_SERVER, String::as_str),
    )
}

/// Assemble the production relay from the `mcp` command's arguments and the environment.
///
/// The credential is resolved here, before anything is read from standard input and before
/// the service is asked whether it is running.
fn relay(
    selected: &ArgMatches,
    environment: &dyn Fn(&str) -> Option<String>,
) -> Result<Relay<HttpTransport, ProcessLauncher>, ApiError> {
    let secret = match selected.get_one::<PathBuf>("token-file") {
        Some(path) => Credential::from_file(path)?,
        None => Credential::from_environment(environment, TOKEN_VARIABLE)?.ok_or_else(|| {
            ApiError::new(
                ErrorCode::Unauthenticated,
                "No connector credential: set OKF_JAWN_TOKEN, or name a file with --token-file",
            )
        })?,
    };
    let endpoint = endpoint(selected)?;
    let limits = Limits::default();
    let transport = HttpTransport::new(&endpoint, &secret, limits.answer_bytes)?;
    let program = selected
        .get_one::<PathBuf>("server-bin")
        .cloned()
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .map(|executable| server_beside(&executable))
        });
    Ok(Relay {
        transport: Arc::new(transport),
        launcher: ProcessLauncher::new(program, endpoint.bind_address()),
        loopback: endpoint.is_loopback(),
        start: StartPolicy::default(),
        limits,
        secret,
    })
}
