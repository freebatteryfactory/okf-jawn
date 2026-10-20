//! A restore captures its job claim once; every publication checks that same live claim.
use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::identity::MutationId;
use okf_jawn_core::storage::StorageScope;
use rusqlite::{Connection, OptionalExtension, params};

use crate::db::{conflict, sql};

/// The immutable claim captured before reading an archive. Jobless imports stay unfenced.
#[derive(Clone)]
pub(crate) struct Fence {
    job: String,
    token: Option<String>,
}

impl Fence {
    pub(crate) fn capture(
        connection: &Connection,
        scope: &StorageScope,
        mutation: MutationId,
    ) -> Result<Option<Self>, ApiError> {
        connection
            .query_row(
                "SELECT job_id, lease_token FROM jobs
             WHERE tenant_id=?1 AND workspace_id=?2 AND mutation_id=?3",
                params![
                    scope.tenant_id.as_str(),
                    scope.workspace_id.0.to_string(),
                    mutation.0.to_string()
                ],
                |row| {
                    Ok(Self {
                        job: row.get(0)?,
                        token: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(|error| sql(&error))
    }

    pub(crate) fn check(&self, connection: &Connection) -> Result<(), ApiError> {
        let live: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM jobs WHERE job_id=?1 AND state='running'
             AND lease_token=?2 AND lease_expires_ms>?3)",
                params![self.job, self.token, crate::seam::now_ms()?],
                |row| row.get(0),
            )
            .map_err(|error| sql(&error))?;
        if live {
            Ok(())
        } else {
            Err(conflict("the restore no longer holds its current lease"))
        }
    }
}
