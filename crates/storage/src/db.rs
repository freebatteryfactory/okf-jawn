//! One SQLite connection per database file, used from blocking tasks.
//!
//! The process holds the data directory's single-writer lock, so each database has exactly one
//! writer: one connection behind a mutex, and every call runs on Tokio's blocking pool. Rows that
//! mirror a contract type are kept as that type's JSON beside the key columns the store
//! searches and constrains on, so a unique column, not a lookup, enforces each `MutationId`.

use std::sync::{Arc, Mutex};

use okf_jawn_contract::error::{ApiError, ErrorCode};
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
