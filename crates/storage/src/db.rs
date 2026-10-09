//! One SQLite connection per database file, used from blocking tasks.
//!
//! The process holds the data directory's single-writer lock, so each database has exactly one
//! writer: one connection behind a mutex, and every call runs on Tokio's blocking pool. Rows that
//! mirror a contract type are kept as that type's JSON beside the key columns the store
//! searches and constrains on, so a unique column, not a lookup, enforces each `MutationId`.

use std::sync::{Arc, Mutex};

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::jobs::JobScope;
use okf_jawn_core::storage::StorageScope;
use rusqlite::{Connection, TransactionBehavior};

/// A shared connection to one database file.
#[derive(Debug, Clone)]
pub(crate) struct Db {
    connection: Arc<Mutex<Connection>>,
}

impl Db {
    /// Wrap an opened, migrated connection.
    pub(crate) fn new(connection: Connection) -> Self {
        Self {
            connection: Arc::new(Mutex::new(connection)),
        }
    }

    /// Run `work` with the connection on the blocking pool.
    pub(crate) async fn call<T, F>(&self, work: F) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, ApiError> + Send + 'static,
    {
        let connection = Arc::clone(&self.connection);
        tokio::task::spawn_blocking(move || {
            let mut guard = connection
                .lock()
                .map_err(|_| internal("a database connection was poisoned by a failed task"))?;
            work(&mut guard)
        })
        .await
        .map_err(|error| internal(format!("a database task stopped: {error}")))?
    }

    /// Run `work` inside one immediate transaction, committed when it returns `Ok`.
    pub(crate) async fn transaction<T, F>(&self, work: F) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, ApiError> + Send + 'static,
    {
        self.call(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| sql(&error))?;
            let value = work(&transaction)?;
            transaction.commit().map_err(|error| sql(&error))?;
            Ok(value)
        })
        .await
    }
}

/// A store fault.
pub(crate) fn internal(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Internal, message)
}

/// A SQLite failure as a store fault.
pub(crate) fn sql(error: &rusqlite::Error) -> ApiError {
    internal(format!("the record store failed: {error}"))
}

/// A JSON encoding failure of a stored row as a store fault.
pub(crate) fn json(error: &serde_json::Error) -> ApiError {
    internal(format!("a stored record did not encode or decode: {error}"))
}

/// A precondition that no longer holds.
pub(crate) fn conflict(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Conflict, message)
}

/// The usual refusal of a record that the scope does not hold.
pub(crate) fn not_found(what: &str) -> ApiError {
    ApiError::new(
        ErrorCode::NotFound,
        format!("No {what} with that identity here"),
    )
}

/// A `u64` as the `i64` SQLite stores.
pub(crate) fn to_i64(value: u64) -> Result<i64, ApiError> {
    i64::try_from(value).map_err(|_| internal("a byte count exceeds what the store records"))
}

/// A stored `i64` back as the `u64` it was written from.
pub(crate) fn to_u64(value: i64) -> Result<u64, ApiError> {
    u64::try_from(value).map_err(|_| internal("a stored count is negative"))
}

/// Decode a page cursor this store issued: the decimal sequence number to continue after.
pub(crate) fn cursor(value: Option<&str>) -> Result<Option<i64>, ApiError> {
    value
        .map(|text| {
            text.parse::<i64>().map_err(|_| {
                ApiError::new(
                    ErrorCode::InvalidInput,
                    "the page cursor is not one this store issued",
                )
                .with_field("/page/cursor")
            })
        })
        .transpose()
}

/// The page size a listing reads: at least one row, at most 500.
pub(crate) fn page_limit(limit: u16) -> i64 {
    i64::from(limit.clamp(1, 500))
}

/// The (tenant, workspace) key columns of a workspace record.
pub(crate) fn scope_key(scope: &StorageScope) -> (String, String) {
    (
        scope.tenant_id.as_str().to_owned(),
        scope.workspace_id.0.to_string(),
    )
}

/// The (tenant, workspace or none) key columns of a job or artifact record.
pub(crate) fn job_scope_key(scope: &JobScope) -> (String, Option<String>) {
    (
        scope.tenant().as_str().to_owned(),
        scope.workspace().map(|workspace| workspace.0.to_string()),
    )
}

/// Rows read one past the page limit, split into a page of records and the cursor (the last
/// row's sequence) that continues it.
pub(crate) fn paged(rows: Vec<(i64, String)>, limit: i64) -> (Vec<String>, Option<String>) {
    let wanted = usize::try_from(limit).unwrap_or(usize::MAX);
    let more = rows.len() > wanted;
    let page: Vec<(i64, String)> = rows.into_iter().take(wanted).collect();
    let next_cursor = if more {
        page.last().map(|(sequence, _)| sequence.to_string())
    } else {
        None
    };
    (
        page.into_iter().map(|(_, record)| record).collect(),
        next_cursor,
    )
}
