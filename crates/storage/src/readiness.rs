//! Readiness of the stores this data directory holds (`ReadinessProbe`).
//!
//! It reports whether each store answers now, not whether any feature is qualified. The blob
//! and Git directories are probed by really writing, syncing and removing a small file in each,
//! so a read-only mount or a refusing access-control list reports not ready. The
//! sandbox origin is server configuration, so this probe leaves it absent for the server to
//! fill.

use std::io::Write as _;
use std::path::Path;

use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::health::{DependencyStatus, ReadinessResponse};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::readiness::ReadinessProbe;

use crate::Storage;
use crate::data::io_error;
use crate::seam;

/// `ReadinessProbe` over an opened data directory.
#[derive(Debug, Clone)]
pub struct StorageReadiness {
    storage: Storage,
}

impl StorageReadiness {
    pub(crate) const fn new(storage: Storage) -> Self {
        Self { storage }
    }
}

impl ReadinessProbe for StorageReadiness {
    fn probe(&self) -> PortFuture<'_, ReadinessResponse> {
        Box::pin(async move {
            let mut dependencies = Vec::new();
            for (name, database) in [
                ("records", &self.storage.records),
                ("index", &self.storage.index),
            ] {
                let answer = database
                    .call(|connection| {
                        connection
                            .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
                            .map_err(|error| crate::db::sql(&error))
                    })
                    .await;
                dependencies.push(status(
                    name,
                    answer.and_then(|check| {
                        if check == "ok" {
                            Ok(())
                        } else {
                            Err(crate::db::internal(check))
                        }
                    }),
                ));
            }
            for (name, directory) in [
                ("objects", self.storage.data.objects_path()),
                ("repositories", self.storage.data.repositories_path()),
            ] {
                let answer = tokio::task::spawn_blocking(move || writable(&directory))
                    .await
                    .map_err(|error| {
                        crate::db::internal(format!("a readiness task stopped: {error}"))
                    })
                    .and_then(|answer| answer);
                dependencies.push(status(name, answer));
            }
            Ok(ReadinessResponse {
                ready: dependencies.iter().all(|dependency| dependency.ready),
                dependencies,
                sandbox_origin: None,
            })
        })
    }
}

/// Whether a file can really be written in `directory`: create a new probe file, write it,
/// sync it and remove it. Permission bits are not consulted, because a read-only mount or an
/// access-control list can refuse a write the bits allow. The probe file is removed whatever
/// step fails after it was created.
fn writable(directory: &Path) -> Result<(), ApiError> {
    let probe = directory.join(format!(
        ".okf-jawn-ready-{}",
        seam::hex(&seam::random_bytes::<8>()?)
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(|error| io_error("create a file in", directory, &error))?;
    let written = file
        .write_all(b"ready")
        .and_then(|()| file.sync_all())
        .map_err(|error| io_error("write and sync a file in", directory, &error));
    drop(file);
    let removed = std::fs::remove_file(&probe)
        .map_err(|error| io_error("remove a file from", directory, &error));
    written.and(removed)
}

fn status(name: &str, answer: Result<(), ApiError>) -> DependencyStatus {
    DependencyStatus {
        name: name.to_owned(),
        ready: answer.is_ok(),
        message: answer.map_or_else(|error| error.message, |()| "ready".to_owned()),
    }
}
