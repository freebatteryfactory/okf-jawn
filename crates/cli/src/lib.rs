//! The `okf-jawn` command line: one subcommand per canonical operation.
//!
//! The executable is a client of the running service. It opens no store and holds no data
//! directory: an operation is one authenticated HTTP request. Subcommands are built from the
//! canonical operation table, so a new operation is a new command without a second schema.
//!
//! No credential is an argument. The bearer token of an operation is read from the
//! environment ([`TOKEN_VARIABLE`]).

use clap::{Arg, Command};
use okf_jawn_contract::metadata::operations;

pub use crate::credential::Credential;
pub use crate::endpoint::Endpoint;
pub use crate::run::{Streams, report, run};

/// The local service address the CLI, the workspace UI and the examples share (SPEC section 2).
pub const DEFAULT_SERVER: &str = "http://127.0.0.1:7711";

/// The environment variable that carries the bearer token.
pub const TOKEN_VARIABLE: &str = "OKF_JAWN_TOKEN";

/// Construct command parsing and help without contacting a server.
#[must_use]
pub fn command() -> Command {
    let mut command = Command::new("okf-jawn")
        .version(env!("CARGO_PKG_VERSION"))
        .about("A document workspace for people and their AI tools")
        .after_help(
            "The bearer token is read from the OKF_JAWN_TOKEN environment variable and is \
             never an argument.",
        )
        .arg_required_else_help(true)
        .subcommand_required(true)
        .arg(
            Arg::new("server")
                .long("server")
                .env("OKF_JAWN_URL")
                .value_name("URL")
                .help("The service address: HTTPS, or HTTP for a loopback address")
                .default_value(DEFAULT_SERVER)
                .global(true),
        );
    for operation in operations() {
        let mut child = Command::new(operation.id).about(operation.description).arg(
            Arg::new("json")
                .long("json")
                .value_name("JSON")
                .help("The operation's JSON input: inline, @file, or - for standard input")
                .default_value("{}"),
        );
        if !operation.alias.is_empty() {
            child = child.visible_alias(operation.alias);
        }
        if !operation.operator_alias.is_empty() && operation.operator_alias != operation.alias {
            child = child.visible_alias(operation.operator_alias);
        }
        command = command.subcommand(child);
    }
    command
}

mod credential;
mod endpoint;
mod request;
mod run;
