//! Rebuild a private test database using genuinely released migration steps and its old columns.
//! This is fixture construction, never a production down-migration or a version relabel.

use std::path::Path;

use okf_jawn_storage::schema;
use rusqlite::Connection;

use crate::check::TestResult;

/// Create a new released schema, copy the fixture's matching old fields, then replace only
/// that private test database. Callers have closed its stores; immutable archive fixtures
/// never go through this helper. A value an old constraint cannot represent remains an error.
pub fn rebuild(records: &Path, version: usize) -> TestResult {
    let parent = records.parent().ok_or("test database has no parent")?;
    let temporary = tempfile::tempdir_in(parent)?;
    let released = temporary.path().join("released.sqlite");
    {
        let mut connection = Connection::open(&released)?;
        schema::RECORDS.to_version(&mut connection, version)?;
        connection.execute(
            "ATTACH DATABASE ?1 AS legacy",
            [records.to_str().ok_or("test database path is not UTF-8")?],
        )?;
        let tables = names(
            &connection,
            "SELECT name FROM main.sqlite_schema
             WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )?;
        let transaction = connection.transaction()?;
        // Table-name order can copy a child before its parent; enforce all keys at commit.
        transaction.pragma_update(None, "defer_foreign_keys", true)?;
        for table in &tables {
            let mut statement = transaction.prepare("SELECT name FROM pragma_table_info(?1)")?;
            let columns = statement
                .query_map([table], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            let columns = columns
                .iter()
                .map(|name| quoted(name))
                .collect::<Vec<_>>()
                .join(",");
            let table = quoted(table);
            transaction.execute(
                &format!(
                    "INSERT INTO main.{table} ({columns}) SELECT {columns} FROM legacy.{table}"
                ),
                [],
            )?;
        }
        // Preserve even an event sequence above its surviving rows, not just max(seq).
        transaction.execute_batch(
            "DELETE FROM main.sqlite_sequence;
             INSERT INTO main.sqlite_sequence (name,seq)
             SELECT name,seq FROM legacy.sqlite_sequence
             WHERE name IN (SELECT name FROM main.sqlite_schema WHERE type='table');",
        )?;
        transaction.commit()?;
        connection.execute_batch("DETACH DATABASE legacy;")?;
        let dangling: i64 =
            connection.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })?;
        assert_eq!(dangling, 0, "released fixture retains its foreign-key rows");
        let actual: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        assert_eq!(actual, i64::try_from(version)?);
    }
    std::fs::copy(&released, records)?;
    Ok(())
}

fn names(connection: &Connection, sql: &str) -> Result<Vec<String>, rusqlite::Error> {
    connection
        .prepare(sql)?
        .query_map([], |row| row.get(0))?
        .collect()
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
