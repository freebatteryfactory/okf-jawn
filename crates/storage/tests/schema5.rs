//! Lifecycle design section 9: schema-4 data upgrades without losing replay identity.
//! These are raw schema/migration checks, not evidence of a real cleanup sweep.
#![cfg(feature = "runtime")]

use check::{TestResult, err_of, some};
use okf_jawn_storage::{Storage, schema};
use rusqlite::{Connection, params};

/// A purged legacy row's state, digest, response, purge, completion clock, token and lease.
type TerminalLedger = (
    String,
    Option<String>,
    Option<String>,
    String,
    i64,
    i64,
    i64,
);
/// An unchanged control row's state, digest, response, purge and completion clock.
type PreservedLedger = (String, String, Option<String>, Option<String>, i64);
/// Synthetic migration rows: every old ledger state, scope controls, and an old pending put.
const LEGACY_ROWS: &str = r#"
INSERT INTO purges VALUES (
    '00000000-0000-0000-0000-0000000000aa', 'tenant-a',
    'workspace/00000000-0000-0000-0000-000000000010', 'running', 'purge-mutation',
    '{"id":"00000000-0000-0000-0000-0000000000aa","target":{"kind":"workspace","workspace_id":"00000000-0000-0000-0000-000000000010"},"state":"running","job_id":"00000000-0000-0000-0000-0000000000ab","requested_by":"fixture-editor","requested_at":"2026-10-19T00:00:00.000Z"}'
);
INSERT INTO mutations (tenant_id, subject, client_key, operation, idempotency_key,
    mutation_id, digest, state, token, lease_expires_ms, response, finished_ms, workspace_id)
VALUES
    ('tenant-a','editor','client','edit_item','completed','m-completed','erased-completed-request-digest','completed',7,44,'old-response',101,'00000000-0000-0000-0000-000000000010'),
    ('tenant-a','editor','client','edit_item','released','m-released','erased-released-request-digest','released',7,44,NULL,102,'00000000-0000-0000-0000-000000000010'),
    ('tenant-a','editor','client','edit_item','leased','m-leased','leased-request-digest','leased',7,44,NULL,NULL,'00000000-0000-0000-0000-000000000010'),
    ('tenant-a','editor','client','edit_item','live','m-live','live-digest','completed',7,44,'live-response',103,'00000000-0000-0000-0000-000000000020'),
    ('tenant-b','editor','client','edit_item','other-tenant','m-other','other-digest','completed',7,44,'other-response',104,'00000000-0000-0000-0000-000000000010'),
    ('tenant-a','editor','client','purge_workspace','purge-workspace','m-pw','purge-digest','completed',7,44,'purge-response',105,'00000000-0000-0000-0000-000000000010'),
    ('tenant-a','editor','client','purge_item','purge-item','m-pi','purge-digest','released',7,44,NULL,106,'00000000-0000-0000-0000-000000000010'),
    ('tenant-a','editor','client','create_workspace','tenant-row','m-tenant','tenant-digest','completed',7,44,'tenant-response',107,NULL);
INSERT INTO pending_puts VALUES (9, 'tenant-a', '00000000-0000-0000-0000-000000000020',
    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', '2026-10-19T00:00:00.000Z');
"#;

/// Only the released steps seed the old shape; never relabel a schema-5 database as version4.
fn at_schema_4(connection: &mut Connection) -> TestResult {
    schema::RECORDS.to_version(connection, 4)?;
    connection.execute_batch(LEGACY_ROWS)?;
    Ok(())
}

fn latest() -> Result<Connection, Box<dyn std::error::Error>> {
    let mut connection = Connection::open_in_memory()?;
    schema::RECORDS.to_latest(&mut connection)?;
    Ok(connection)
}

fn count(connection: &Connection, sql: &str) -> Result<i64, rusqlite::Error> {
    connection.query_row(sql, [], |row| row.get(0))
}

/// A CHECK violation must be the declared constraint, not a missing table or bad test SQL.
fn check_refuses(connection: &Connection, sql: &str) -> TestResult {
    let refused = err_of(connection.execute_batch(sql))?;
    assert_eq!(
        refused.sqlite_error().map(|failure| failure.extended_code),
        Some(rusqlite::ffi::SQLITE_CONSTRAINT_CHECK),
        "{refused}"
    );
    Ok(())
}

/// Literal live, tenant, purge-operation and unscoped controls stay unchanged by V5.
fn assert_live_and_exempt_rows(connection: &Connection) -> TestResult {
    for (key, state, digest, response, finished) in [
        (
            "live",
            "completed",
            "live-digest",
            Some("live-response"),
            103,
        ),
        (
            "other-tenant",
            "completed",
            "other-digest",
            Some("other-response"),
            104,
        ),
        (
            "purge-workspace",
            "completed",
            "purge-digest",
            Some("purge-response"),
            105,
        ),
        ("purge-item", "released", "purge-digest", None, 106),
        (
            "tenant-row",
            "completed",
            "tenant-digest",
            Some("tenant-response"),
            107,
        ),
    ] {
        let held: PreservedLedger = connection.query_row(
            "SELECT state,digest,response,purge_id,finished_ms FROM mutations WHERE idempotency_key=?1",
            [key], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
        )?;
        assert_eq!(
            held,
            (
                state.to_owned(),
                digest.to_owned(),
                response.map(str::to_owned),
                None,
                finished
            )
        );
    }
    Ok(())
}

/// V9.2/V5.5: only completed/released non-purge rows of the named tenant/workspace change.
#[test]
fn schema5_upgrades_legacy_rows_and_keeps_their_original_clocks_and_pending_puts() -> TestResult {
    let mut connection = Connection::open_in_memory()?;
    at_schema_4(&mut connection)?;
    schema::RECORDS.to_latest(&mut connection)?;
    assert_eq!(count(&connection, "PRAGMA user_version")?, 5);
    for (key, finished) in [("completed", 101), ("released", 102)] {
        let held: TerminalLedger = connection.query_row(
            "SELECT state,digest,response,purge_id,finished_ms,token,lease_expires_ms
                 FROM mutations WHERE idempotency_key = ?1",
            [key],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )?;
        assert_eq!(
            held,
            (
                "purged".to_owned(),
                None,
                None,
                "00000000-0000-0000-0000-0000000000aa".to_owned(),
                finished,
                7,
                44
            )
        );
    }
    assert_eq!(count(&connection, "SELECT count(*) FROM mutations")?, 8);
    assert_eq!(
        count(
            &connection,
            "SELECT count(*) FROM mutations WHERE state='leased' AND digest='leased-request-digest' AND finished_ms IS NULL"
        )?,
        1
    );
    assert_live_and_exempt_rows(&connection)?;
    assert_eq!(
        count(
            &connection,
            "SELECT count(*) FROM pending_puts WHERE put_id=9 AND mutation_id IS NULL AND tenant_id='tenant-a' AND workspace_id='00000000-0000-0000-0000-000000000020' AND digest='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' AND created_at='2026-10-19T00:00:00.000Z'"
        )?,
        1
    );
    assert_eq!(
        count(&connection, "SELECT count(*) FROM pragma_foreign_key_check")?,
        0
    );
    Ok(())
}

/// Owner ruling9: the one pre-V5 copy is still schema4 but carries no purged request digest.
#[test]
fn schema5_sanitizes_the_new_pre_migration_copy_and_keeps_the_backup_set() -> TestResult {
    let directory = tempfile::tempdir()?;
    let records = directory.path().join("records.sqlite");
    {
        let mut connection = Connection::open(&records)?;
        at_schema_4(&mut connection)?;
    }
    let storage = Storage::open(directory.path())?;
    let saved = std::fs::read_dir(storage.data().pre_migration_path())?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    let copies: Vec<_> = saved
        .iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("records-v4-"))
        })
        .collect();
    assert_eq!(copies.len(), 1, "one SQLite copy precedes the upgrade");
    let copy = Connection::open(some(copies.first(), "the pre-V5 copy")?)?;
    assert_eq!(count(&copy, "PRAGMA user_version")?, 4);
    assert_eq!(
        count(
            &copy,
            "SELECT count(*) FROM mutations WHERE idempotency_key IN ('completed','released') AND state='released' AND response IS NULL AND digest='' AND finished_ms IN (101,102)"
        )?,
        2
    );
    assert_eq!(
        count(
            &copy,
            "SELECT count(*) FROM mutations WHERE idempotency_key='live' AND digest='live-digest' AND response='live-response' AND finished_ms=103"
        )?,
        1
    );
    assert_eq!(
        count(
            &copy,
            "SELECT count(*) FROM mutations WHERE idempotency_key='leased' AND state='leased' AND digest='leased-request-digest'"
        )?,
        1
    );
    let raw_copy = std::fs::read(some(copies.first(), "the pre-V5 copy")?)?;
    for erased in [
        "erased-completed-request-digest",
        "erased-released-request-digest",
    ] {
        assert!(
            !raw_copy
                .windows(erased.len())
                .any(|bytes| bytes == erased.as_bytes()),
            "sanitation must not leave a removed request digest in the copy's free space"
        );
    }
    let sets: Vec<_> = saved.iter().filter(|path| path.is_dir()).collect();
    assert_eq!(sets.len(), 1);
    assert!(
        some(sets.first(), "the pre-migration set")?
            .join("installation/tenant-a.zip")
            .is_file()
    );
    let live = Connection::open(&records)?;
    assert_eq!(count(&live, "PRAGMA user_version")?, 5);
    assert_eq!(
        count(
            &live,
            "SELECT count(*) FROM mutations WHERE idempotency_key IN ('completed','released') AND state='purged' AND digest IS NULL AND response IS NULL AND purge_id='00000000-0000-0000-0000-0000000000aa'"
        )?,
        2
    );
    Ok(())
}

/// V5.8 and section9(a): a purged ledger row has neither digest nor response, and has a purge.
#[test]
fn schema5_mutations_enforce_all_terminal_state_checks() -> TestResult {
    let connection = latest()?;
    connection.execute_batch("INSERT INTO mutations (tenant_id,subject,client_key,operation,idempotency_key,mutation_id,digest,state,token,lease_expires_ms,purge_id) VALUES ('t','s','c','edit_item','k','m',NULL,'purged',1,2,'p')")?;
    for sql in [
        "UPDATE mutations SET digest='request'",
        "UPDATE mutations SET purge_id=NULL",
        "UPDATE mutations SET response='content'",
        "UPDATE mutations SET state='unknown'",
        "UPDATE mutations SET state='released',digest=NULL,purge_id=NULL",
        "UPDATE mutations SET state='released',digest='request'",
    ] {
        check_refuses(&connection, sql)?;
    }
    connection
        .execute_batch("UPDATE mutations SET state='leased',digest='request',purge_id=NULL")?;
    for state in ["completed", "released", "leased"] {
        connection.execute("UPDATE mutations SET state=?1", [state])?;
        check_refuses(&connection, "UPDATE mutations SET digest=NULL")?;
        check_refuses(&connection, "UPDATE mutations SET purge_id='p'")?;
    }
    Ok(())
}

/// Section9(b): only a tenant pending put must name a mutation; every declared index exists.
#[test]
fn schema5_pending_puts_accept_scoped_and_tracked_tenant_rows_with_both_indexes() -> TestResult {
    let connection = latest()?;
    connection.execute_batch("INSERT INTO pending_puts VALUES (1,'t','w','a','now',NULL),(2,'t',NULL,'b','now','m'),(3,'t','w','c','now','both')")?;
    check_refuses(
        &connection,
        "INSERT INTO pending_puts VALUES (4,'t',NULL,'d','now',NULL)",
    )?;
    assert_eq!(count(&connection, "SELECT count(*) FROM pending_puts")?, 3);
    assert_eq!(
        count(
            &connection,
            "SELECT count(*) FROM pragma_index_list('pending_puts') WHERE name='pending_puts_by_mutation' AND partial=1"
        )?,
        1
    );
    assert_eq!(
        count(
            &connection,
            "SELECT count(*) FROM pragma_index_info('pending_puts_by_mutation') WHERE (seqno=0 AND name='tenant_id') OR (seqno=1 AND name='mutation_id')"
        )?,
        2
    );
    assert_eq!(
        count(
            &connection,
            "SELECT count(*) FROM pragma_index_info('pending_puts_by_object') WHERE (seqno=0 AND name='tenant_id') OR (seqno=1 AND name='workspace_id') OR (seqno=2 AND name='digest')"
        )?,
        3
    );
    assert_eq!(
        count(
            &connection,
            "SELECT count(*) FROM pragma_index_info('mutations_by_workspace') WHERE (seqno=0 AND name='tenant_id') OR (seqno=1 AND name='workspace_id')"
        )?,
        2
    );
    Ok(())
}

/// Section9(c): upload, discard and original-create identities each prevent a second tombstone.
#[test]
fn schema5_discarded_uploads_keep_their_three_unique_identities() -> TestResult {
    let connection = latest()?;
    connection.execute_batch("INSERT INTO discarded_uploads VALUES ('u','t','w','discard','create','now','report.pdf','notes',12,NULL,'editor','before',5,NULL,NULL)")?;
    for (upload, discard, create) in [
        ("u", "d2", "c2"),
        ("u2", "discard", "c2"),
        ("u2", "d2", "create"),
    ] {
        let refused = err_of(connection.execute(
            "INSERT INTO discarded_uploads SELECT ?1,tenant_id,workspace_id,?2,?3,discarded_at,filename,relative_path,expected_size,expected_sha256,supplied_by,created_at,received_bytes,object_digest,object_size FROM discarded_uploads WHERE upload_id='u'",
            params![upload,discard,create],
        ))?;
        assert_eq!(
            refused.sqlite_error_code(),
            Some(rusqlite::ErrorCode::ConstraintViolation)
        );
    }
    assert_eq!(
        count(&connection, "SELECT count(*) FROM discarded_uploads")?,
        1
    );
    Ok(())
}

/// Section9(d)/V3.4: the constraint is per tenant and only applies to queued/running `CleanUp`.
#[test]
fn schema5_allows_one_active_cleanup_per_tenant_and_preserves_finished_jobs() -> TestResult {
    let connection = latest()?;
    connection.execute_batch("INSERT INTO jobs (job_id,tenant_id,mutation_id,initiator,spec,record,state) VALUES ('j1','a','m1','{}','{\"kind\":\"clean_up\"}','{}','queued'),('j2','b','m2','{}','{\"kind\":\"clean_up\"}','{}','running'),('j3','a','m3','{}','{\"kind\":\"clean_up\"}','{}','succeeded'),('j4','a','m4','{}','{\"kind\":\"rebuild_index\"}','{}','queued')")?;
    for state in ["queued", "running"] {
        let refused = err_of(connection.execute("INSERT INTO jobs (job_id,tenant_id,mutation_id,initiator,spec,record,state) VALUES ('j5','a','m5','{}','{\"kind\":\"clean_up\"}','{}',?1)", [state]))?;
        assert_eq!(
            refused.sqlite_error_code(),
            Some(rusqlite::ErrorCode::ConstraintViolation)
        );
    }
    connection.execute_batch("UPDATE jobs SET state='failed' WHERE job_id='j1'; INSERT INTO jobs (job_id,tenant_id,mutation_id,initiator,spec,record,state) VALUES ('j5','a','m5','{}','{\"kind\":\"clean_up\"}','{}','queued')")?;
    assert_eq!(count(&connection, "SELECT count(*) FROM jobs")?, 5);
    connection.execute_batch("INSERT INTO clean_up_requests VALUES ('a',2,1),('b',0,0)")?;
    for sql in [
        "UPDATE clean_up_requests SET requested=-1 WHERE tenant_id='a'",
        "UPDATE clean_up_requests SET started=-1 WHERE tenant_id='a'",
        "UPDATE clean_up_requests SET started=3 WHERE tenant_id='a'",
    ] {
        check_refuses(&connection, sql)?;
    }
    Ok(())
}

/// Section9(e): progress defaults to not removed, accepts only0/1, and keys a digest per sweep.
#[test]
fn schema5_sweep_progress_enforces_boolean_and_composite_identity() -> TestResult {
    let connection = latest()?;
    connection.execute_batch(
        "INSERT INTO sweep_removals (sweep,tenant_id,digest) VALUES ('s1','a','d'),('s2','a','d')",
    )?;
    assert_eq!(
        count(
            &connection,
            "SELECT count(*) FROM sweep_removals WHERE removed=0"
        )?,
        2
    );
    for value in [-1, 2] {
        check_refuses(
            &connection,
            &format!("UPDATE sweep_removals SET removed={value}"),
        )?;
    }
    connection.execute_batch("UPDATE sweep_removals SET removed=1 WHERE sweep='s1'")?;
    let refused =
        err_of(connection.execute_batch("INSERT INTO sweep_removals VALUES ('s1','b','d',0)"))?;
    assert_eq!(
        refused.sqlite_error_code(),
        Some(rusqlite::ErrorCode::ConstraintViolation)
    );
    assert_eq!(
        count(
            &connection,
            "SELECT count(*) FROM sweep_removals WHERE removed=1"
        )?,
        1
    );
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
