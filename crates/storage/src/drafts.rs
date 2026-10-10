//! Per-editor drafts (`DraftStore`) over SQLite: one row per (workspace, item, editor).
//!
//! A save or a discard first writes its (mutation, item, action) row with the result it
//! returns; when that primary key already exists, the call changes nothing and returns the
//! stored result. Drafts live only in the records database: they are never indexed, exported
//! or committed by storage.

use std::collections::BTreeMap;

use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::identity::{ItemId, MutationId};
use okf_jawn_contract::item::{Draft, DraftContent};
use okf_jawn_core::drafts::{DraftStore, DraftWrite};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::StorageScope;
use rusqlite::{OptionalExtension, Transaction, params};

use crate::db::{Db, internal, json, not_found, scope_key, sql};
use crate::seam;

/// `DraftStore` over the records database.
#[derive(Debug, Clone)]
pub struct SqliteDrafts {
    db: Db,
}

impl SqliteDrafts {
    pub(crate) const fn new(db: Db) -> Self {
        Self { db }
    }
}

impl DraftStore for SqliteDrafts {
    fn save<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        draft: DraftWrite,
    ) -> PortFuture<'a, Draft> {
        let (tenant, workspace) = scope_key(scope);
        Box::pin(self.db.transaction(move |transaction| {
            let saved = Draft {
                item_id: draft.item_id,
                editor: draft.editor.clone(),
                base_revision: draft.base_revision.clone(),
                content_digest: draft.content_digest.clone(),
                saved_at: seam::now()?,
            };
            let record = serde_json::to_string(&saved).map_err(|error| json(&error))?;
            if let Some(prior) = remember(
                transaction,
                (&tenant, &workspace),
                mutation_id,
                draft.item_id,
                "save",
                &record,
            )? {
                return Ok(prior);
            }
            let properties =
                serde_json::to_string(&draft.properties).map_err(|error| json(&error))?;
            transaction
                .execute(
                    "INSERT INTO drafts (tenant_id, workspace_id, item_id, editor, saved_at,
                         record, body, properties)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT (tenant_id, workspace_id, item_id, editor) DO UPDATE SET
                         saved_at = excluded.saved_at, record = excluded.record,
                         body = excluded.body, properties = excluded.properties",
                    params![
                        tenant,
                        workspace,
                        draft.item_id.0.to_string(),
                        draft.editor,
                        saved.saved_at.as_str(),
                        record,
                        draft.body,
                        properties
                    ],
                )
                .map_err(|error| sql(&error))?;
            Ok(saved)
        }))
    }

    fn get<'a>(
        &'a self,
        scope: &'a StorageScope,
        item: ItemId,
        editor: &'a str,
    ) -> PortFuture<'a, Option<DraftContent>> {
        let (tenant, workspace) = scope_key(scope);
        let editor = editor.to_owned();
        Box::pin(self.db.call(move |connection| {
            let found: Option<(String, String, String)> = connection
                .query_row(
                    "SELECT record, body, properties FROM drafts
                     WHERE tenant_id = ?1 AND workspace_id = ?2 AND item_id = ?3 AND editor = ?4",
                    params![tenant, workspace, item.0.to_string(), editor],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(|error| sql(&error))?;
            found
                .map(|(record, body, properties)| {
                    let properties: BTreeMap<String, serde_json::Value> =
                        serde_json::from_str(&properties).map_err(|error| json(&error))?;
                    Ok(DraftContent {
                        draft: serde_json::from_str(&record).map_err(|error| json(&error))?,
                        body,
                        properties,
                    })
                })
                .transpose()
        }))
    }

    fn list<'a>(&'a self, scope: &'a StorageScope, editor: &'a str) -> PortFuture<'a, Vec<Draft>> {
        let (tenant, workspace) = scope_key(scope);
        let editor = editor.to_owned();
        Box::pin(self.db.call(move |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT record FROM drafts WHERE tenant_id = ?1 AND workspace_id = ?2
                       AND editor = ?3 ORDER BY saved_at DESC, rowid DESC",
                )
                .map_err(|error| sql(&error))?;
            let records = statement
                .query_map(params![tenant, workspace, editor], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|error| sql(&error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| sql(&error))?;
            records
                .iter()
                .map(|record| serde_json::from_str(record).map_err(|error| json(&error)))
                .collect()
        }))
    }

    fn discard<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        item: ItemId,
        editor: &'a str,
    ) -> PortFuture<'a, Draft> {
        let (tenant, workspace) = scope_key(scope);
        let editor = editor.to_owned();
        Box::pin(self.db.transaction(move |transaction| {
            let key = params![tenant, workspace, item.0.to_string(), editor];
            let current: Option<String> = transaction
                .query_row(
                    "SELECT record FROM drafts
                     WHERE tenant_id = ?1 AND workspace_id = ?2 AND item_id = ?3 AND editor = ?4",
                    key,
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| sql(&error))?;
            let Some(current) = current else {
                return prior(transaction, mutation_id, item, "discard")?
                    .ok_or_else(|| not_found("draft"));
            };
            if let Some(prior) = remember(
                transaction,
                (&tenant, &workspace),
                mutation_id,
                item,
                "discard",
                &current,
            )? {
                return Ok(prior);
            }
            transaction
                .execute(
                    "DELETE FROM drafts
                     WHERE tenant_id = ?1 AND workspace_id = ?2 AND item_id = ?3 AND editor = ?4",
                    key,
                )
                .map_err(|error| sql(&error))?;
            serde_json::from_str(&current).map_err(|error| json(&error))
        }))
    }
}

/// Record what this (mutation, item, action) returns; `Some` with the earlier result when the
/// key was already recorded, in which case the caller changes nothing.
fn remember(
    transaction: &Transaction<'_>,
    (tenant, workspace): (&str, &str),
    mutation_id: MutationId,
    item: ItemId,
    action: &str,
    result: &str,
) -> Result<Option<Draft>, ApiError> {
    let inserted = transaction
        .execute(
            "INSERT INTO draft_mutations (mutation_id, item_id, action, tenant_id, workspace_id,
                 result)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT DO NOTHING",
            params![
                mutation_id.0.to_string(),
                item.0.to_string(),
                action,
                tenant,
                workspace,
                result
            ],
        )
        .map_err(|error| sql(&error))?;
    if inserted == 1 {
        return Ok(None);
    }
    prior(transaction, mutation_id, item, action)?
        .map(Some)
        .ok_or_else(|| internal("a draft mutation row vanished"))
}

fn prior(
    transaction: &Transaction<'_>,
    mutation_id: MutationId,
    item: ItemId,
    action: &str,
) -> Result<Option<Draft>, ApiError> {
    let result: Option<String> = transaction
        .query_row(
            "SELECT result FROM draft_mutations
             WHERE mutation_id = ?1 AND item_id = ?2 AND action = ?3",
            params![mutation_id.0.to_string(), item.0.to_string(), action],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    result
        .map(|result| serde_json::from_str(&result).map_err(|error| json(&error)))
        .transpose()
}
