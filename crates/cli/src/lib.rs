//! Command bindings are composed from the same canonical operation declarations.

use clap::{Arg, Command};
use okf_jawn_contract::metadata::operations;
use okf_jawn_contract::labels::operator_alias;

/// Construct command parsing and help without contacting a server.
#[must_use]
pub fn command() -> Command {
    let mut command = Command::new("okf-jawn")
        .version(env!("CARGO_PKG_VERSION"))
        .about("A document workspace for people and their AI tools")
        .arg_required_else_help(true)
        .arg(Arg::new("server").long("server").env("OKF_JAWN_URL")
            .default_value("http://127.0.0.1:7711").global(true))
        .arg(Arg::new("json").long("json").help("JSON request, @file, or - for stdin")
            .default_value("{}").global(true));
    for operation in operations() {
        let mut child = Command::new(operation.id).about(operation.description);
        if !operation.alias.is_empty() { child = child.visible_alias(operation.alias); }
        if let Some(alias) = operator_alias(operation.id) {
            if alias != operation.alias { child = child.visible_alias(alias); }
        }
        command = command.subcommand(child);
    }
    command
}
