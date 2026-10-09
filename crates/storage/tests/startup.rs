//! Startup of a data directory: format version, migrations and the single-writer lock.
//!
//! `migrations_*` tests are the receipt of `storage-format-version-and-migrations`;
//! `single_writer_*` tests are the receipt of `storage-single-writer-lock`.
#![cfg(feature = "runtime")]

use std::fs::File;
use std::path::Path;
use std::process::Command;

use check::{TestResult, err_of, some};
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_core::mutations::{MutationKey, MutationStore};
use okf_jawn_storage::Storage;
use okf_jawn_storage::data::{DataDir, FORMAT_FILE, LOCK_FILE};
use okf_jawn_storage::format::migrate_database;
use okf_jawn_storage::schema;
use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};

/// The database file the `migrate_database` tests migrate inside a locked directory.
const DATABASE: &str = "records.sqlite";

/// Environment variable naming the data directory the child process probes.
const PROBE_DIR: &str = "OKF_JAWN_LOCK_PROBE_DIR";
/// Environment variable naming what the child must observe: `refused` or `opened`.
const PROBE_EXPECT: &str = "OKF_JAWN_LOCK_PROBE_EXPECT";

fn user_version(path: &Path) -> Result<i64, Box<dyn std::error::Error>> {
    let connection = Connection::open(path)?;
    Ok(connection.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

fn set_user_version(path: &Path, version: i64) -> TestResult {
    let connection = Connection::open(path)?;
    connection.pragma_update(None, "user_version", version)?;
    Ok(())
}

fn steps(sql: &'static [&'static str]) -> Migrations<'static> {
    Migrations::new(sql.iter().map(|step| M::up(step)).collect())
}

/// Run this test binary again, as a second process, on one test that probes the lock.
fn probe_in_child(directory: &Path, expect: &str) -> TestResult {
    let output = Command::new(std::env::current_exe()?)
        .args(["--exact", "lock_probe_child", "--test-threads", "1"])
        .env(PROBE_DIR, directory)
        .env(PROBE_EXPECT, expect)
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "the child process did not observe `{expect}`: {stdout}"
    );
    assert!(
        stdout.contains("1 passed"),
        "the child ran no probe: {stdout}"
    );
    Ok(())
}

#[test]
fn migrations_bring_a_new_directory_to_the_latest_schema() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.data().records_path();
    let index = storage.data().index_path();
    drop(storage);
    assert_eq!(user_version(&records)?, 1);
    assert_eq!(user_version(&index)?, 1);
    let marker: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
        directory.path().join(FORMAT_FILE),
    )?)?;
    assert_eq!(
        marker,
        serde_json::json!({ "format": "okf-jawn-data", "version": 1 })
    );
    assert!(
        !directory.path().join("backups").exists(),
        "a new directory needs no pre-migration backup"
    );
    // Opening again finds nothing pending.
    drop(Storage::open(directory.path())?);
    assert_eq!(user_version(&records)?, 1);
    Ok(())
}

#[test]
fn migrations_refuse_a_newer_records_database_and_leave_it_unchanged() -> TestResult {
    let directory = tempfile::tempdir()?;
    let records = {
        let storage = Storage::open(directory.path())?;
        storage.data().records_path()
    };
    set_user_version(&records, 99)?;
    let before = std::fs::read(&records)?;
    let error = err_of(Storage::open(directory.path()))?;
    assert_eq!(error.code, ErrorCode::Unsupported, "{}", error.message);
    assert!(
        error.message.contains("left unchanged"),
        "{}",
        error.message
    );
    assert_eq!(
        std::fs::read(&records)?,
        before,
        "the newer database was modified"
    );
    assert_eq!(user_version(&records)?, 99);
    Ok(())
}

#[test]
fn migrations_refuse_a_newer_index_database_before_migrating_the_records() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (records, index) = {
        let storage = Storage::open(directory.path())?;
        (storage.data().records_path(), storage.data().index_path())
    };
    set_user_version(&index, 7)?;
    set_user_version(&records, 0)?;
    let error = err_of(Storage::open(directory.path()))?;
    assert_eq!(error.code, ErrorCode::Unsupported, "{}", error.message);
    // The records database, which this build could have migrated, was not touched either.
    assert_eq!(user_version(&records)?, 0);
    Ok(())
}

#[test]
fn migrations_refuse_a_newer_format_version_and_leave_the_directory_unchanged() -> TestResult {
    let directory = tempfile::tempdir()?;
    let marker = directory.path().join(FORMAT_FILE);
    std::fs::write(&marker, r#"{"format":"okf-jawn-data","version":2}"#)?;
    let error = err_of(Storage::open(directory.path()))?;
    assert_eq!(error.code, ErrorCode::Unsupported, "{}", error.message);
    assert_eq!(
        std::fs::read_to_string(&marker)?,
        r#"{"format":"okf-jawn-data","version":2}"#
    );
    assert!(!directory.path().join("records.sqlite").exists());
    assert!(!directory.path().join("index.sqlite").exists());
    assert!(!directory.path().join("staging").exists());
    Ok(())
}

#[test]
fn migrations_back_up_then_upgrade_then_verify() -> TestResult {
    let directory = tempfile::tempdir()?;
    let data = DataDir::open(directory.path())?;
    let backups = data.pre_migration_path();
    let first = migrate_database(
        &data,
        DATABASE,
        &steps(&["CREATE TABLE notes (text TEXT);"]),
    )?;
    assert_eq!((first.from, first.to), (0, 1));
    assert!(first.backup.is_none(), "a new database needs no backup");
    first
        .connection
        .execute("INSERT INTO notes (text) VALUES ('kept')", [])?;
    drop(first);

    let upgraded = migrate_database(
        &data,
        DATABASE,
        &steps(&[
            "CREATE TABLE notes (text TEXT);",
            "ALTER TABLE notes ADD COLUMN author TEXT;",
        ]),
    )?;
    assert_eq!((upgraded.from, upgraded.to), (1, 2));
    let backup = some(upgraded.backup.clone(), "the pre-migration backup")?;
    assert!(backup.starts_with(&backups));
    let copy = Connection::open(&backup)?;
    let kept: String = copy.query_row("SELECT text FROM notes", [], |row| row.get(0))?;
    assert_eq!(kept, "kept");
    assert_eq!(
        copy.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
        1
    );
    let author: Option<String> =
        upgraded
            .connection
            .query_row("SELECT author FROM notes", [], |row| row.get(0))?;
    assert_eq!(author, None);
    Ok(())
}

#[test]
fn migrations_refuse_a_newer_database_without_writing_it() -> TestResult {
    let directory = tempfile::tempdir()?;
    let data = DataDir::open(directory.path())?;
    let database = data.root().join(DATABASE);
    let backups = data.pre_migration_path();
    let two = steps(&["CREATE TABLE a (x TEXT);", "CREATE TABLE b (x TEXT);"]);
    drop(migrate_database(&data, DATABASE, &two)?);
    let before = std::fs::read(&database)?;
    let error = err_of(migrate_database(
        &data,
        DATABASE,
        &steps(&["CREATE TABLE a (x TEXT);"]),
    ))?;
    assert_eq!(error.code, ErrorCode::Unsupported, "{}", error.message);
    assert_eq!(std::fs::read(&database)?, before);
    assert!(!backups.exists(), "a refused database is not backed up");
    Ok(())
}

#[test]
fn migrations_roll_back_a_failing_step_and_keep_the_backup() -> TestResult {
    let directory = tempfile::tempdir()?;
    let data = DataDir::open(directory.path())?;
    let database = data.root().join(DATABASE);
    let backups = data.pre_migration_path();
    let first = migrate_database(
        &data,
        DATABASE,
        &steps(&["CREATE TABLE notes (text TEXT);"]),
    )?;
    first
        .connection
        .execute("INSERT INTO notes (text) VALUES ('kept')", [])?;
    drop(first);
    let error = err_of(migrate_database(
        &data,
        DATABASE,
        &steps(&[
            "CREATE TABLE notes (text TEXT);",
            "ALTER TABLE missing ADD COLUMN x;",
        ]),
    ))?;
    assert_eq!(error.code, ErrorCode::Internal, "{}", error.message);
    assert_eq!(user_version(&database)?, 1);
    let connection = Connection::open(&database)?;
    let kept: String = connection.query_row("SELECT text FROM notes", [], |row| row.get(0))?;
    assert_eq!(kept, "kept");
    assert_eq!(
        std::fs::read_dir(&backups)?.count(),
        1,
        "the backup is kept"
    );
    Ok(())
}

#[test]
fn migrations_released_schemas_validate() -> TestResult {
    schema::RECORDS.validate()?;
    schema::INDEX.validate()?;
    Ok(())
}

#[test]
fn single_writer_lock_refuses_a_second_open_in_this_process() -> TestResult {
    let directory = tempfile::tempdir()?;
    let first = Storage::open(directory.path())?;
    let error = err_of(Storage::open(directory.path()))?;
    assert_eq!(error.code, ErrorCode::Unavailable);
    assert!(
        error.message.contains("another okf-jawn process"),
        "{}",
        error.message
    );
    drop(first);
    drop(Storage::open(directory.path())?);
    Ok(())
}

#[test]
fn single_writer_lock_is_taken_before_any_store_opens() -> TestResult {
    let directory = tempfile::tempdir()?;
    let holder = File::create(directory.path().join(LOCK_FILE))?;
    holder.try_lock()?;
    let error = err_of(Storage::open(directory.path()))?;
    assert_eq!(error.code, ErrorCode::Unavailable);
    for written in [
        "records.sqlite",
        "index.sqlite",
        FORMAT_FILE,
        "staging",
        "objects",
    ] {
        assert!(
            !directory.path().join(written).exists(),
            "{written} was created without the lock"
        );
    }
    Ok(())
}

#[test]
fn single_writer_lock_refuses_a_second_process_until_the_first_exits() -> TestResult {
    let directory = tempfile::tempdir()?;
    let first = Storage::open(directory.path())?;
    probe_in_child(directory.path(), "refused")?;
    drop(first);
    probe_in_child(directory.path(), "opened")?;
    Ok(())
}

#[test]
fn startup_removes_what_a_crash_left_in_staging() -> TestResult {
    let directory = tempfile::tempdir()?;
    drop(Storage::open(directory.path())?);
    let left = directory
        .path()
        .join("staging")
        .join("local")
        .join("00000000-0000-4000-8000-000000000001")
        .join("00000000-0000-4000-8000-000000000002");
    std::fs::create_dir_all(left.join("notes"))?;
    std::fs::write(left.join("notes").join("x.md"), "left by a crash")?;
    let storage = Storage::open(directory.path())?;
    let staging = storage.data().staging_path();
    assert!(staging.is_dir(), "staging exists after startup");
    assert!(!left.exists(), "a crash's staging directory was removed");
    assert_eq!(
        std::fs::read_dir(&staging)?.count(),
        0,
        "nothing a crash left survives startup"
    );
    Ok(())
}

/// A second open of `directory` must be refused because the lock is still held.
fn refused_while_held(directory: &Path) -> TestResult {
    let error = err_of(Storage::open(directory))?;
    assert_eq!(error.code, ErrorCode::Unavailable, "{}", error.message);
    assert!(
        error.message.contains("another okf-jawn process"),
        "{}",
        error.message
    );
    Ok(())
}

#[tokio::test]
async fn single_writer_lock_lives_as_long_as_any_store_it_handed_out() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let ledger = storage.mutations();
    drop(storage);
    refused_while_held(directory.path())?;
    let key = MutationKey {
        tenant_id: serde_json::from_value(serde_json::json!("local"))?,
        subject: "owner".to_owned(),
        client_id: None,
        operation: okf_jawn_contract::metadata::OperationName::CreateWorkspace,
        key: serde_json::from_value(serde_json::json!("00000000-0000-4000-8000-000000000001"))?,
    };
    ledger
        .begin(
            &key,
            &okf_jawn_contract::identity::Digest::try_from("a".repeat(64))?,
        )
        .await?;
    drop(ledger);
    // The last store dropped, so the lock is free again.
    let storage = Storage::open(directory.path())?;
    // A Git store and the blob store each hold the lock on their own as well.
    let versions = storage.versions();
    let blobs = storage.blobs();
    drop(storage);
    refused_while_held(directory.path())?;
    drop(versions);
    refused_while_held(directory.path())?;
    drop(blobs);
    drop(Storage::open(directory.path())?);
    Ok(())
}

/// The child half of the cross-process test: does nothing unless the parent names a directory.
#[test]
fn lock_probe_child() -> TestResult {
    let (Ok(directory), Ok(expect)) = (std::env::var(PROBE_DIR), std::env::var(PROBE_EXPECT))
    else {
        return Ok(());
    };
    let opened = Storage::open(Path::new(&directory));
    match expect.as_str() {
        "refused" => {
            let error = err_of(opened)?;
            assert_eq!(error.code, ErrorCode::Unavailable, "{}", error.message);
            assert!(error.message.contains("another okf-jawn process"));
        }
        _ => drop(opened?),
    }
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
