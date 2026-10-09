//! The opened data directory and every store over it.
//!
//! Opening is the only way to obtain a store, so no store writes before the single-writer lock
//! is held, the format is checked and the databases are migrated. Every store keeps a share of
//! the lock (through its `Db`, `Repositories` or `LocalBlobs`), so dropping the `Storage` while
//! a store is still in use keeps the directory locked until that store drops too.

use std::path::Path;
use std::sync::Arc;

use okf_jawn_contract::error::{ApiError, ErrorCode};

use crate::access::LocalAccess;
use crate::blobs::LocalBlobs;
use crate::catalog::GitCatalog;
use crate::confirmations::SqliteConfirmations;
use crate::credentials::SqliteCredentials;
use crate::data::{DataDir, FormatState, INDEX_FILE, RECORDS_FILE};
use crate::db::Db;
use crate::drafts::SqliteDrafts;
use crate::events::SqliteEvents;
use crate::format::{migrate_database, refuse_newer};
use crate::git::GitVersions;
use crate::git::repo::Repositories;
use crate::mutations::SqliteMutations;
use crate::proposals::SqliteProposals;
use crate::readiness::StorageReadiness;
use crate::records::SqliteRecords;
use crate::sandbox::SqliteSandbox;
use crate::schema;
use crate::search::SqliteSearch;
use crate::uploads::SqliteUploads;

/// An opened data directory; every store it hands out shares its lock.
#[derive(Debug, Clone)]
pub struct Storage {
    pub(crate) data: Arc<DataDir>,
    pub(crate) records: Db,
    pub(crate) index: Db,
    pub(crate) blobs: LocalBlobs,
    pub(crate) repositories: Repositories,
}

impl Storage {
    /// Open the data directory at `root` for this process alone.
    ///
    /// # Errors
    /// Returns `Unavailable` when another process owns the directory or it cannot be prepared,
    /// `Unsupported` (changing nothing) when its data is newer than this build, and `Internal`
    /// when a migration fails or does not verify.
    pub fn open(root: &Path) -> Result<Self, ApiError> {
        let data = Arc::new(DataDir::open(root)?);
        for (path, migrations) in [
            (&data.records_path(), &schema::RECORDS),
            (&data.index_path(), &schema::INDEX),
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
        let records = migrate_database(&data, RECORDS_FILE, &schema::RECORDS)?;
        let index = migrate_database(&data, INDEX_FILE, &schema::INDEX)?;
        Ok(Self {
            records: Db::new(records.connection, Arc::clone(&data)),
            index: Db::new(index.connection, Arc::clone(&data)),
            blobs: LocalBlobs::open(Arc::clone(&data))?,
            repositories: Repositories::new(Arc::clone(&data)),
            data,
        })
    }

    /// The mutation ledger.
    #[must_use]
    pub fn mutations(&self) -> SqliteMutations {
        SqliteMutations::new(self.records.clone())
    }

    /// Jobs, artifacts, purges, the revision map, derived objects, reviews and receipts.
    #[must_use]
    pub fn records(&self) -> SqliteRecords {
        SqliteRecords::new(self.records.clone())
    }

    /// Per-editor drafts.
    #[must_use]
    pub fn drafts(&self) -> SqliteDrafts {
        SqliteDrafts::new(self.records.clone())
    }

    /// Session-bound confirmation challenges.
    #[must_use]
    pub fn confirmations(&self) -> SqliteConfirmations {
        SqliteConfirmations::new(self.records.clone())
    }

    /// Sandbox-origin capabilities.
    #[must_use]
    pub fn sandbox(&self) -> SqliteSandbox {
        SqliteSandbox::new(self.records.clone())
    }

    /// Proposals and their discussion.
    #[must_use]
    pub fn proposals(&self) -> SqliteProposals {
        SqliteProposals::new(self.records.clone())
    }

    /// Content-addressed bytes.
    #[must_use]
    pub fn blobs(&self) -> LocalBlobs {
        self.blobs.clone()
    }

    /// Upload slots.
    #[must_use]
    pub fn uploads(&self) -> SqliteUploads {
        SqliteUploads::new(
            self.records.clone(),
            self.blobs.clone(),
            self.data.root().join("uploads"),
        )
    }

    /// The workspace and tenant event logs.
    #[must_use]
    pub fn events(&self) -> SqliteEvents {
        SqliteEvents::new(self.records.clone())
    }

    /// Connectors, sessions and the installation identity.
    #[must_use]
    pub fn credentials(&self) -> SqliteCredentials {
        SqliteCredentials::new(self.records.clone())
    }

    /// The local access control.
    #[must_use]
    pub fn access(&self) -> LocalAccess {
        LocalAccess::new(self.records.clone())
    }

    /// Versioned workspace content.
    #[must_use]
    pub fn versions(&self) -> GitVersions {
        GitVersions::new(
            self.repositories.clone(),
            self.records.clone(),
            self.blobs.clone(),
        )
    }

    /// The workspace catalog.
    #[must_use]
    pub fn catalog(&self) -> GitCatalog {
        GitCatalog::new(self.records.clone(), self.repositories.clone())
    }

    /// The search, link and graph index.
    #[must_use]
    pub fn search(&self) -> SqliteSearch {
        SqliteSearch::new(self.index.clone(), self.repositories.clone())
    }

    /// Readiness of these stores.
    #[must_use]
    pub fn readiness(&self) -> StorageReadiness {
        StorageReadiness::new(self.clone())
    }

    /// The opened data directory.
    #[must_use]
    pub fn data(&self) -> &DataDir {
        &self.data
    }
}
