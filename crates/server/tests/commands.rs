//! The binary's command line: no argument is `serve` (the relay and a launcher start the server
//! with none), `backup --output <dir> [--tenant <id>]`, and `restore --replacement <installation
//! archive> [<workspace archive>...]`; anything else is a usage error, exit status 2.
#![cfg(feature = "runtime")]

use std::path::PathBuf;

use okf_jawn_server::commands::{Command, parse};
use okf_jawn_server::offline::{BackupArgs, RestoreArgs};

use check::{TestResult, err_of};

fn args(words: &[&str]) -> Vec<String> {
    std::iter::once("okf-jawn-server")
        .chain(words.iter().copied())
        .map(str::to_owned)
        .collect()
}

#[test]
fn no_argument_and_serve_both_serve() -> TestResult {
    assert_eq!(parse(args(&[]))?, Command::Serve);
    assert_eq!(parse(args(&["serve"]))?, Command::Serve);
    Ok(())
}

#[test]
fn backup_takes_an_output_and_an_optional_tenant() -> TestResult {
    assert_eq!(
        parse(args(&["backup", "--output", "set"]))?,
        Command::Backup(BackupArgs {
            output: PathBuf::from("set"),
            tenant: None,
        })
    );
    assert_eq!(
        parse(args(&["backup", "--output", "set", "--tenant", "org_a"]))?,
        Command::Backup(BackupArgs {
            output: PathBuf::from("set"),
            tenant: Some("org_a".to_owned()),
        })
    );
    Ok(())
}

#[test]
fn restore_takes_the_installation_archive_and_the_workspace_archives() -> TestResult {
    assert_eq!(
        parse(args(&[
            "restore",
            "--replacement",
            "i.zip",
            "a.zip",
            "b.zip"
        ]))?,
        Command::Restore(RestoreArgs {
            installation: PathBuf::from("i.zip"),
            workspaces: vec![PathBuf::from("a.zip"), PathBuf::from("b.zip")],
        })
    );
    assert_eq!(
        parse(args(&["restore", "--replacement", "i.zip"]))?,
        Command::Restore(RestoreArgs {
            installation: PathBuf::from("i.zip"),
            workspaces: Vec::new(),
        })
    );
    Ok(())
}

#[test]
fn anything_else_is_a_usage_error_with_status_two() -> TestResult {
    for words in [
        &["backup"][..],
        &["backup", "--output"],
        &["backup", "--output", "a", "--output", "b"],
        &["restore"],
        &["restore", "a.zip"],
        &["unknown"],
        &["serve", "extra"],
        &["--output", "set"],
    ] {
        let error = err_of(parse(args(words)))?;
        assert_eq!(error.exit_code(), 2, "{words:?}");
    }
    Ok(())
}

#[test]
fn help_and_version_answer_without_serving() -> TestResult {
    for words in [&["--help"][..], &["--version"], &["backup", "--help"]] {
        let shown = err_of(parse(args(words)))?;
        assert_eq!(shown.exit_code(), 0, "{words:?}");
    }
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
