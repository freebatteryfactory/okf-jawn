//! One classification of managed archives for import refusals and archive exclusions.
use std::collections::BTreeMap;

use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::identity::Digest;
use okf_jawn_core::storage::ManagedKind;
use rusqlite::{Connection, params};

use crate::db::{internal, sql};

/// All managed objects, or just one digest. Old snapshots may predate a source table.
pub(crate) fn objects(
    connection: &Connection,
    tenant: &str,
    digest: Option<&Digest>,
) -> Result<BTreeMap<Digest, ManagedKind>, ApiError> {
    let present = super::snapshot::tables(connection)?;
    let mut found = BTreeMap::new();
    for (table, kind, predicate) in [
        (
            "artifacts",
            "CASE kind WHEN 'workspace_backup' THEN 'workspace' ELSE 'installation' END",
            "kind IN ('workspace_backup','installation_backup')",
        ),
        (
            "backup_archives",
            "CASE WHEN workspace_id IS NULL THEN 'installation' ELSE 'workspace' END",
            "1",
        ),
        ("history_exports", "'history'", "1"),
    ] {
        if !present.iter().any(|name| name == table) {
            continue;
        }
        let query = format!(
            "SELECT digest, {kind} FROM {table}
             WHERE tenant_id=?1 AND {predicate} AND (?2 IS NULL OR digest=?2)"
        );
        let mut statement = connection.prepare(&query).map_err(|error| sql(&error))?;
        let rows = statement
            .query_map(params![tenant, digest.map(Digest::as_str)], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| sql(&error))?;
        for row in rows {
            let (digest, kind) = row.map_err(|error| sql(&error))?;
            let kind = match kind.as_str() {
                "workspace" => ManagedKind::WorkspaceArchive,
                "installation" => ManagedKind::InstallationArchive,
                "history" => ManagedKind::WrittenHistory,
                _ => return Err(internal("a managed archive has an unknown kind")),
            };
            found.entry(from_text!(digest.as_str())?).or_insert(kind);
        }
    }
    Ok(found)
}
