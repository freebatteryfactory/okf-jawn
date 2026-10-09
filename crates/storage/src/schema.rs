//! SQLite schemas, as `rusqlite_migration` steps; a step is never edited once released.
//!
//! The records database holds application records that cannot be rebuilt. The index database
//! holds only what can be rebuilt from committed Git trees, so rebuilding it can never touch a
//! job, a review or a receipt: they are not in that file.

use rusqlite_migration::{M, Migrations};

/// Every released step of the records database, oldest first.
pub const RECORDS: Migrations<'static> = Migrations::from_slice(RECORDS_STEPS);

/// Every released step of the index database, oldest first.
pub const INDEX: Migrations<'static> = Migrations::from_slice(INDEX_STEPS);

const RECORDS_STEPS: &[M<'static>] = &[M::up(RECORDS_V1).foreign_key_check()];

const INDEX_STEPS: &[M<'static>] = &[M::up(INDEX_V1)];

/// Records schema, version 1. Each table that a repeated `MutationId` must not grow carries a
/// unique `mutation_id` column.
const RECORDS_V1: &str = r"
CREATE TABLE mutations (
    tenant_id TEXT NOT NULL,
    subject TEXT NOT NULL,
    client_key TEXT NOT NULL,
    operation TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    mutation_id TEXT NOT NULL UNIQUE,
    digest TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('leased', 'completed', 'released')),
    token INTEGER NOT NULL,
    lease_expires_ms INTEGER NOT NULL,
    response TEXT,
    finished_ms INTEGER,
    PRIMARY KEY (tenant_id, subject, client_key, operation, idempotency_key)
) STRICT;

CREATE TABLE workspaces (
    tenant_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    mutation_id TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    created_at TEXT NOT NULL,
    archived_at TEXT,
    PRIMARY KEY (tenant_id, workspace_id)
) STRICT;

CREATE TABLE workspace_mutations (
    mutation_id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    action TEXT NOT NULL,
    result TEXT NOT NULL
) STRICT;

CREATE TABLE grants (
    tenant_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    subject TEXT NOT NULL,
    permissions TEXT NOT NULL,
    PRIMARY KEY (tenant_id, workspace_id, subject)
) STRICT;

CREATE TABLE jobs (
    job_id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    workspace_id TEXT,
    mutation_id TEXT NOT NULL UNIQUE,
    initiator TEXT NOT NULL,
    spec TEXT NOT NULL,
    record TEXT NOT NULL,
    state TEXT NOT NULL,
    retryable INTEGER NOT NULL DEFAULT 0,
    lease_token TEXT,
    lease_expires_ms INTEGER,
    lease_expires_at TEXT
) STRICT;
CREATE INDEX jobs_by_scope ON jobs (tenant_id, workspace_id);
CREATE INDEX jobs_by_state ON jobs (state);

CREATE TABLE job_controls (
    mutation_id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL REFERENCES jobs (job_id),
    action TEXT NOT NULL CHECK (action IN ('cancel', 'retry'))
) STRICT;

CREATE TABLE job_outputs (
    job_id TEXT NOT NULL REFERENCES jobs (job_id),
    digest TEXT NOT NULL,
    PRIMARY KEY (job_id, digest)
) STRICT;

CREATE TABLE artifacts (
    artifact_id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    workspace_id TEXT,
    mutation_id TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    digest TEXT NOT NULL,
    size INTEGER NOT NULL,
    media_type TEXT NOT NULL,
    job_id TEXT NOT NULL
) STRICT;

CREATE TABLE purges (
    purge_id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    target_key TEXT NOT NULL,
    state TEXT NOT NULL,
    mutation_id TEXT NOT NULL UNIQUE,
    record TEXT NOT NULL
) STRICT;
CREATE UNIQUE INDEX purges_unfinished_by_target ON purges (tenant_id, target_key)
    WHERE state <> 'completed';

CREATE TABLE revision_map (
    tenant_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    old_revision TEXT NOT NULL,
    purge_id TEXT NOT NULL,
    replacement TEXT,
    PRIMARY KEY (tenant_id, workspace_id, old_revision)
) STRICT;

CREATE TABLE derived_objects (
    tenant_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    revision TEXT NOT NULL,
    digest TEXT NOT NULL,
    kind TEXT NOT NULL,
    media_type TEXT NOT NULL,
    PRIMARY KEY (tenant_id, workspace_id, item_id, revision, digest)
) STRICT;

CREATE TABLE reviews (
    review_id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    mutation_id TEXT NOT NULL UNIQUE,
    record TEXT NOT NULL
) STRICT;
CREATE INDEX reviews_by_item ON reviews (tenant_id, workspace_id, item_id);

CREATE TABLE receipts (
    receipt_id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    mutation_id TEXT UNIQUE,
    record TEXT NOT NULL
) STRICT;

CREATE TABLE drafts (
    tenant_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    editor TEXT NOT NULL,
    saved_at TEXT NOT NULL,
    record TEXT NOT NULL,
    body TEXT NOT NULL,
    properties TEXT NOT NULL,
    PRIMARY KEY (tenant_id, workspace_id, item_id, editor)
) STRICT;

CREATE TABLE draft_mutations (
    mutation_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('save', 'discard')),
    tenant_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    result TEXT NOT NULL,
    PRIMARY KEY (mutation_id, item_id, action)
) STRICT;

CREATE TABLE confirmations (
    confirmation_id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    mutation_id TEXT NOT NULL UNIQUE,
    action TEXT NOT NULL,
    target TEXT NOT NULL,
    revision TEXT NOT NULL,
    content_digest TEXT NOT NULL,
    session_id TEXT NOT NULL,
    subject TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    consumed_by TEXT
) STRICT;

CREATE TABLE sandbox_capabilities (
    token_hash BLOB PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    revision TEXT NOT NULL,
    object TEXT NOT NULL,
    media_type TEXT NOT NULL,
    expires_at TEXT NOT NULL
) STRICT;

CREATE TABLE proposals (
    proposal_id TEXT NOT NULL,
    tenant_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    mutation_id TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL,
    record TEXT NOT NULL,
    PRIMARY KEY (tenant_id, workspace_id, proposal_id)
) STRICT;

CREATE TABLE comments (
    comment_id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    proposal_id TEXT NOT NULL,
    mutation_id TEXT NOT NULL UNIQUE,
    record TEXT NOT NULL
) STRICT;

CREATE TABLE uploads (
    upload_id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    mutation_id TEXT NOT NULL UNIQUE,
    filename TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    expected_size INTEGER NOT NULL,
    expected_sha256 TEXT,
    supplied_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    received_bytes INTEGER NOT NULL,
    object_digest TEXT,
    object_size INTEGER,
    consumed_by TEXT
) STRICT;

CREATE TABLE events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    tenant_id TEXT NOT NULL,
    workspace_id TEXT,
    replay_key TEXT UNIQUE,
    record TEXT NOT NULL
) STRICT;
CREATE INDEX events_by_log ON events (tenant_id, workspace_id, seq);

CREATE TABLE installation (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    subject TEXT NOT NULL,
    created_at TEXT NOT NULL
) STRICT;

CREATE TABLE connectors (
    connector_id TEXT PRIMARY KEY,
    mutation_id TEXT NOT NULL UNIQUE,
    secret_hash BLOB NOT NULL UNIQUE,
    revoked INTEGER NOT NULL DEFAULT 0,
    record TEXT NOT NULL
) STRICT;

CREATE TABLE sessions (
    session_id TEXT PRIMARY KEY,
    principal TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    revoked INTEGER NOT NULL DEFAULT 0
) STRICT;
";

/// Index schema, version 1: one copy of each indexed revision's searchable projection.
const INDEX_V1: &str = r"
CREATE TABLE indexed_revisions (
    tenant_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    revision TEXT NOT NULL,
    PRIMARY KEY (tenant_id, workspace_id, revision)
) STRICT;

CREATE TABLE index_items (
    entry INTEGER PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    revision TEXT NOT NULL,
    item_id TEXT NOT NULL,
    path TEXT NOT NULL,
    archived INTEGER NOT NULL,
    extraction_status TEXT,
    summary TEXT NOT NULL,
    UNIQUE (tenant_id, workspace_id, revision, item_id)
) STRICT;

CREATE VIRTUAL TABLE index_text USING fts5 (title, description, body, tokenize = 'unicode61');

CREATE TABLE index_links (
    tenant_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    revision TEXT NOT NULL,
    source_item TEXT NOT NULL,
    source_path TEXT NOT NULL,
    target_path TEXT NOT NULL,
    target_item TEXT,
    label TEXT NOT NULL
) STRICT;
CREATE INDEX index_links_by_source ON index_links (tenant_id, workspace_id, revision, source_item);
CREATE INDEX index_links_by_target ON index_links (tenant_id, workspace_id, revision, target_item);
";
