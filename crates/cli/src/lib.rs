//! The `okf-jawn` command line: one subcommand per canonical operation, and the `mcp --stdio` relay.
//!
//! The executable is a client of the running service. It opens no store and holds no data
//! directory: an operation is one authenticated HTTP request, and the relay forwards MCP
//! messages to the service's MCP endpoint. Subcommands are built from the canonical operation
//! table, so a new operation is a new command without a second schema.
//!
//! No credential is an argument. The bearer token of an operation and the connector credential
//! of the relay are read from the environment ([`TOKEN_VARIABLE`]); the relay also accepts a
//! file named by `--token-file`.

use clap::{Arg, ArgAction, ArgGroup, Command, value_parser};
use okf_jawn_contract::metadata::operations;

pub use crate::credential::Credential;
pub use crate::endpoint::Endpoint;
pub use crate::run::{Streams, report, run};

/// The local service address the CLI, the workspace UI and the examples share (SPEC section 2).
pub const DEFAULT_SERVER: &str = "http://127.0.0.1:7711";

/// The subcommand that runs the stdio relay.
pub const RELAY_COMMAND: &str = "mcp";

/// The environment variable that names the server executable the relay starts.
pub const SERVER_BIN_VARIABLE: &str = "OKF_JAWN_SERVER_BIN";

/// The environment variable that carries the bearer token or connector credential.
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
    command.subcommand(relay_command())
}

/// The `mcp` subcommand: the stdio relay of SPEC section 11.
fn relay_command() -> Command {
    Command::new(RELAY_COMMAND)
        .about(
            "Relay MCP messages between a local client's standard streams and the running \
             service, starting the local service when none is running",
        )
        .after_help(
            "The connector credential is read from the OKF_JAWN_TOKEN environment variable, \
             or from the first line of the file named by --token-file. It is never an argument.",
        )
        .arg(
            Arg::new("stdio")
                .long("stdio")
                .action(ArgAction::SetTrue)
                .help("Speak MCP over standard input and output, one JSON-RPC message per line"),
        )
        // The relay speaks over exactly one transport, and stdio is the only one.
        .group(ArgGroup::new("transport").arg("stdio").required(true))
        .arg(
            Arg::new("token-file")
                .long("token-file")
                .value_name("PATH")
                .value_parser(value_parser!(std::path::PathBuf))
                .help("Read the connector credential from the first line of this file"),
        )
        .arg(
            Arg::new("server-bin")
                .long("server-bin")
                .env(SERVER_BIN_VARIABLE)
                .value_name("PATH")
                .value_parser(value_parser!(std::path::PathBuf))
                .help(
                    "The server executable to start when no local service is running \
                     (default: okf-jawn-server beside this executable)",
                ),
        )
}

mod credential;
mod endpoint;
pub mod relay;
mod request;
mod run;
