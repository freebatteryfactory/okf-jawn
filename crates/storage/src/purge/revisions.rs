//! Lifecycle ruling Q1: count distinct revisions named by durable records, never walk Git.
//! Only declared revision fields participate; hashes or revision-looking free text do not.
//! The count is captured before rows are removed. Workspace invalidation itself still covers
//! every revision, whether or not any retained record names it.

use okf_jawn_contract::error::ApiError;
use rusqlite::{Connection, params};

use crate::db::{internal, sql};

/// Distinct revision identities named by this workspace's records or citations of it.
pub(super) fn named(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
) -> Result<usize, ApiError> {
    let count: i64 = connection
        .query_row(
            "WITH revisions(revision) AS (
                SELECT old_revision FROM revision_map WHERE tenant_id=?1 AND workspace_id=?2
                UNION SELECT replacement FROM revision_map WHERE tenant_id=?1 AND workspace_id=?2
                UNION SELECT commit_id FROM commit_warnings WHERE tenant_id=?1 AND workspace_id=?2
                UNION SELECT revision FROM derived_objects WHERE tenant_id=?1 AND workspace_id=?2
                UNION SELECT revision FROM confirmations WHERE tenant_id=?1 AND workspace_id=?2
                UNION SELECT revision FROM sandbox_capabilities WHERE tenant_id=?1 AND workspace_id=?2
                UNION SELECT revision FROM history_exports WHERE tenant_id=?1 AND workspace_id=?2
                UNION SELECT json_extract(record, '$.base_revision') FROM drafts
                    WHERE tenant_id=?1 AND workspace_id=?2
                UNION SELECT json_extract(result, '$.base_revision') FROM draft_mutations
                    WHERE tenant_id=?1 AND workspace_id=?2
                UNION SELECT json_extract(result, '$.head') FROM workspace_mutations
                    WHERE tenant_id=?1 AND workspace_id=?2 AND action='change'
                UNION SELECT json_extract(record, '$.base_revision') FROM proposals
                    WHERE tenant_id=?1 AND workspace_id=?2
                UNION SELECT json_extract(record, '$.proposal_revision') FROM proposals
                    WHERE tenant_id=?1 AND workspace_id=?2
                UNION SELECT json_extract(spec, '$.base_revision') FROM jobs
                    WHERE tenant_id=?1 AND workspace_id=?2
                UNION SELECT json_extract(spec, '$.revision') FROM jobs
                    WHERE tenant_id=?1 AND workspace_id=?2
                UNION SELECT json_extract(record, '$.revision') FROM jobs
                    WHERE tenant_id=?1 AND workspace_id=?2
                UNION SELECT json_extract(record, '$.revision') FROM events
                    WHERE tenant_id=?1 AND workspace_id=?2
                UNION SELECT json_extract(source.value, '$.revision')
                    FROM receipts, json_each(receipts.record, '$.sources') AS source
                    WHERE receipts.tenant_id=?1
                        AND json_extract(source.value, '$.workspace_id')=?2
                UNION SELECT json_extract(record, '$.source.revision') FROM reviews
                    WHERE tenant_id=?1 AND json_extract(record, '$.source.workspace_id')=?2
                UNION SELECT json_extract(record, '$.source.revision') FROM comments
                    WHERE tenant_id=?1 AND json_extract(record, '$.source.workspace_id')=?2
            ) SELECT count(*) FROM revisions WHERE revision IS NOT NULL",
            params![tenant, workspace],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    usize::try_from(count).map_err(|_| internal("a purge revision count is negative"))
}
