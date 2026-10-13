//! The command surface against the canonical operation table.
//!
//! Every operation is a command under its canonical id with its agent alias and its operator
//! alias; nothing else is a command but the relay; no two names collide; and no command takes
//! a credential as an argument.

use std::collections::{BTreeMap, BTreeSet};

use clap::Command;
use okf_jawn_cli::{RELAY_COMMAND, command};
use okf_jawn_contract::metadata::operations;

use crate::check::{TestResult, some};

/// Words that name a secret. An argument, an alias, a value name or an environment variable
/// that holds one of them would put a credential on the command line.
const SECRET_WORDS: [&str; 8] = [
    "token",
    "secret",
    "password",
    "passwd",
    "credential",
    "bearer",
    "auth",
    "key",
];

/// The one argument that may name a secret word: it takes a path, not the credential.
const TOKEN_FILE: &str = "token-file";

/// Every name a subcommand answers to: its own and its aliases.
fn names(child: &Command) -> Vec<String> {
    std::iter::once(child.get_name())
        .chain(child.get_all_aliases())
        .map(str::to_owned)
        .collect()
}

#[test]
fn every_operation_is_a_command_under_its_canonical_id_with_its_aliases() -> TestResult {
    let root = command();
    let table = operations();
    assert!(
        table.len() > 70,
        "the table holds {} operations",
        table.len()
    );
    for operation in table {
        let child = some(root.find_subcommand(operation.id), operation.id)?;
        assert_eq!(child.get_name(), operation.id);
        let aliases: BTreeSet<&str> = child.get_all_aliases().collect();
        let expected: BTreeSet<&str> = [operation.alias, operation.operator_alias]
            .into_iter()
            .filter(|alias| !alias.is_empty())
            .collect();
        assert_eq!(aliases, expected, "{}", operation.id);
        assert!(
            child
                .get_arguments()
                .any(|argument| argument.get_id() == "json"),
            "{} takes no --json",
            operation.id
        );
        for name in std::iter::once(operation.id).chain(expected) {
            let matches = command().try_get_matches_from(["okf-jawn", name])?;
            assert_eq!(
                matches.subcommand_name(),
                Some(operation.id),
                "{name} runs {}",
                operation.id
            );
        }
    }
    Ok(())
}

#[test]
fn every_command_is_an_operation_or_the_relay() {
    let commands: BTreeSet<String> = command()
        .get_subcommands()
        .map(|child| child.get_name().to_owned())
        .collect();
    let expected: BTreeSet<String> = operations()
        .into_iter()
        .map(|operation| operation.id.to_owned())
        .chain([RELAY_COMMAND.to_owned()])
        .collect();
    assert_eq!(commands, expected);
}

#[test]
fn no_two_commands_or_aliases_collide() {
    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // clap adds `help` itself; a command or an alias of that name would shadow it.
    owners.insert("help".to_owned(), vec!["(clap's help)".to_owned()]);
    for child in command().get_subcommands() {
        for name in names(child) {
            owners
                .entry(name)
                .or_default()
                .push(child.get_name().to_owned());
        }
    }
    let collisions: Vec<_> = owners
        .iter()
        .filter(|(_, commands)| commands.len() > 1)
        .collect();
    assert!(collisions.is_empty(), "{collisions:?}");
    assert!(owners.len() > operations().len());
}

#[test]
fn no_command_takes_a_credential_as_an_argument() {
    let root = command();
    let mut seen = 0_usize;
    for owner in std::iter::once(&root).chain(root.get_subcommands()) {
        for argument in owner.get_arguments() {
            seen = seen.saturating_add(1);
            let id = argument.get_id().as_str();
            if id == TOKEN_FILE {
                assert_eq!(argument.get_long(), Some(TOKEN_FILE));
                assert_eq!(
                    argument.get_env(),
                    None,
                    "a credential path, not a credential"
                );
                continue;
            }
            let mut spellings = vec![id.to_owned()];
            spellings.extend(argument.get_long().map(str::to_owned));
            spellings.extend(
                argument
                    .get_all_aliases()
                    .into_iter()
                    .flatten()
                    .map(str::to_owned),
            );
            spellings.extend(
                argument
                    .get_value_names()
                    .into_iter()
                    .flatten()
                    .map(ToString::to_string),
            );
            spellings.extend(
                argument
                    .get_env()
                    .map(|name| name.to_string_lossy().into_owned()),
            );
            for spelling in spellings {
                let lowered = spelling.to_ascii_lowercase();
                for word in SECRET_WORDS {
                    assert!(
                        !lowered.contains(word),
                        "{}: argument {id} names a secret ({spelling})",
                        owner.get_name()
                    );
                }
            }
        }
    }
    assert!(seen > operations().len(), "walked {seen} arguments");
}
