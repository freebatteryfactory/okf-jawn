//! Readiness of the stores this data directory holds (`ReadinessProbe`).
//!
//! It reports whether each store answers now, not whether any feature is qualified. The
//! sandbox origin is server configuration, so this probe leaves it absent for the server to
//! fill.

use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::health::{DependencyStatus, ReadinessResponse};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::readiness::ReadinessProbe;

use crate::Storage;

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
                let answer = std::fs::metadata(&directory)
                    .map_err(|error| crate::data::io_error("inspect", &directory, &error))
                    .and_then(|metadata| {
                        if metadata.is_dir() && !metadata.permissions().readonly() {
                            Ok(())
                        } else {
                            Err(crate::db::internal(format!(
                                "{} is not a writable directory",
                                directory.display()
                            )))
                        }
                    });
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

fn status(name: &str, answer: Result<(), ApiError>) -> DependencyStatus {
    DependencyStatus {
        name: name.to_owned(),
        ready: answer.is_ok(),
        message: answer.map_or_else(|error| error.message, |()| "ready".to_owned()),
    }
}
