//! Sandbox-origin capabilities (`SandboxCapabilityStore`) over SQLite, keyed by token hash.
//!
//! The store receives only `core::sandbox::token_hash` of a token, never the token or a URL,
//! and resolves a hash only while its expiry lies ahead.

use okf_jawn_contract::error::ApiError;
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::sandbox::{SandboxCapabilityStore, SandboxMint, SandboxResolved};
use okf_jawn_core::storage::StorageScope;
use rusqlite::{OptionalExtension, params};

use crate::confirmations::canonical_expiry;
use crate::db::{Db, conflict, scope_key, sql};
use crate::seam;

/// `SandboxCapabilityStore` over the records database.
#[derive(Debug, Clone)]
pub struct SqliteSandbox {
    db: Db,
}

/// tenant, workspace, item, revision, object, media type: one resolved binding.
type BindingColumns = (String, String, String, String, String, String);

impl SqliteSandbox {
    pub(crate) const fn new(db: Db) -> Self {
        Self { db }
    }
}

impl SandboxCapabilityStore for SqliteSandbox {
    fn mint<'a>(
        &'a self,
        scope: &'a StorageScope,
        hash: [u8; 32],
        mint: SandboxMint,
    ) -> PortFuture<'a, ()> {
        let (tenant, workspace) = scope_key(scope);
        Box::pin(self.db.transaction(move |transaction| {
            let expires_at = canonical_expiry(&mint.expires_at)?;
            let inserted = transaction
                .execute(
                    "INSERT INTO sandbox_capabilities (token_hash, tenant_id, workspace_id,
                         item_id, revision, object, media_type, expires_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) ON CONFLICT DO NOTHING",
                    params![
                        hash.as_slice(),
                        tenant,
                        workspace,
                        mint.item_id.0.to_string(),
                        mint.revision.as_str(),
                        mint.object.as_str(),
                        mint.media_type,
                        expires_at.as_str()
                    ],
                )
                .map_err(|error| sql(&error))?;
            if inserted == 0 {
                return Err(conflict("a capability with this token hash already exists"));
            }
            Ok(())
        }))
    }

    fn resolve(&self, hash: [u8; 32]) -> PortFuture<'_, Option<SandboxResolved>> {
        Box::pin(self.db.call(move |connection| {
            let found: Option<BindingColumns> = connection
                .query_row(
                    "SELECT tenant_id, workspace_id, item_id, revision, object, media_type
                     FROM sandbox_capabilities WHERE token_hash = ?1 AND expires_at > ?2",
                    params![hash.as_slice(), seam::now()?.as_str()],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    },
                )
                .optional()
                .map_err(|error| sql(&error))?;
            found
                .map(|(tenant, workspace, item, revision, object, media_type)| {
                    Ok::<_, ApiError>(SandboxResolved {
                        scope: StorageScope {
                            tenant_id: from_text!(tenant.as_str())?,
                            workspace_id: from_text!(workspace.as_str())?,
                        },
                        item_id: from_text!(item.as_str())?,
                        revision: from_text!(revision.as_str())?,
                        object: from_text!(object.as_str())?,
                        media_type,
                    })
                })
                .transpose()
        }))
    }
}
