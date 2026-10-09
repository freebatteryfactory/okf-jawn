//! The opened data directory and every store over it.
//!
//! Opening is the only way to obtain a store, so no store writes before the single-writer lock
//! is held, the format is checked and the databases are migrated.

use std::path::Path;
use std::sync::Arc;

use okf_jawn_contract::error::{ApiError, ErrorCode};

use crate::data::{DataDir, FormatState};
use crate::db::Db;
use crate::format::{migrate_database, refuse_newer};
use crate::mutations::SqliteMutations;
use crate::schema;

/// An opened data directory; every store it hands out shares its lock.
#[derive(Debug, Clone)]
pub struct Storage {
    pub(crate) data: Arc<DataDir>,
    pub(crate) records: Db,
}

impl Storage {
    /// Open the data directory at `root` for this process alone.
    ///
    /// # Errors
    /// Returns `Unavailable` when another process owns the directory or it cannot be prepared,
    /// `Unsupported` (changing nothing) when its data is newer than this build, and `Internal`
    /// when a migration fails or does not verify.
    pub fn open(root: &Path) -> Result<Self, ApiError> {
        let data = DataDir::open(root)?;
        let records_path = data.records_path();
        let index_path = data.index_path();
        for (path, migrations) in [
            (&records_path, &schema::RECORDS),
            (&index_path, &schema::INDEX),
        ] {
            if path.exists() {
                refuse_newer(path, migrations)?;
            }
        }
        match data.format_state()? {
            FormatState::Absent => data.write_format()?,
            FormatState::Current => {}
            FormatState::Older(version) => {
                return Err(ApiError::new(
                    ErrorCode::Internal,
                    format!("no upgrade is defined from data format version {version}"),
                ));
            }
        }
        data.clear_staging()?;
        for directory in [data.objects_path(), data.repositories_path()] {
            std::fs::create_dir_all(&directory)
                .map_err(|error| crate::data::io_error("create", &directory, &error))?;
        }
        let backups = data.pre_migration_path();
        let records = migrate_database(&records_path, &schema::RECORDS, &backups)?;
        // TODO(search index): the index store keeps this connection once it lands.
        drop(migrate_database(&index_path, &schema::INDEX, &backups)?);
        Ok(Self {
            data: Arc::new(data),
            records: Db::new(records.connection),
        })
    }

    /// The mutation ledger.
    #[must_use]
    pub fn mutations(&self) -> SqliteMutations {
        SqliteMutations::new(self.records.clone())
    }

    /// The opened data directory.
    #[must_use]
    pub fn data(&self) -> &DataDir {
        &self.data
    }
}
