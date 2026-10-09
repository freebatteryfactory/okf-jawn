//! Compile-level and pure-function proofs for the store ports.
//!
//! No port has an implementation in this crate. Each `*_calls` function type-checks one call to
//! every method of a port through `&dyn`; it is never awaited, and its proof is that this file
//! compiles. The other tests run the pure helpers and the data flow between port types.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use okf_jawn_contract::access::{AccessRoute, Connector, IssuedConnector, Permission, Principal};
use okf_jawn_contract::common::{PageRange, PageRequest, TextRange, Warning};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::{Event, EventKind, Receipt};
use okf_jawn_contract::identity::{
    ArtifactId, Digest, ItemId, JobId, MutationId, ProposalId, PurgeId, Revision, TenantId,
    Timestamp, UploadId, WorkspaceId, WorkspacePath,
};
use okf_jawn_contract::import::{Job, JobKind, JobState};
use okf_jawn_contract::item::{Draft, ItemKind};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::proposal::{Change, Comment, Proposal, ProposalStatus};
use okf_jawn_contract::purge::PurgeTarget;
use okf_jawn_contract::read::AssetRole;
use okf_jawn_contract::review::{Confirmation, Review};
use okf_jawn_contract::search::{GetGraphResponse, LinkDirection};
use okf_jawn_contract::source::{SourceLocation, SourceLocator};
use okf_jawn_contract::transport::TRANSPORTS;
use okf_jawn_contract::workspace::{RestoreReport, Workspace};
use okf_jawn_core::confirmations::{ConfirmationConsume, ConfirmationCreate, ConfirmationStore};
use okf_jawn_core::conversion::{
    ConversionInput, ConversionSettings, ConversionStatus, ConvertedAsset, Converter,
    ConverterLimits, OcrPolicy, PixelSize, RetainedAsset,
};
use okf_jawn_core::credentials::{
    ConnectorIssue, CredentialStore, NewConnector, SessionRecord, secret_hash,
};
use okf_jawn_core::drafts::{DraftStore, DraftWrite};
use okf_jawn_core::events::{EventLog, EventQuery, EventScope, NewEvent};
use okf_jawn_core::jobs::{
    ArtifactKind, ArtifactRecord, ClaimedJob, DerivedObject, JobCompletion, JobHandler, JobLease,
    JobQueue, JobScope, JobSpec, NewArtifact, NewJob, NewPurge, RecordStore,
};
use okf_jawn_core::proposals::{CommentPage, ProposalFilter, ProposalStore};
use okf_jawn_core::sandbox::{SandboxCapabilityStore, SandboxMint, SandboxResolved, token_hash};
use okf_jawn_core::search::{GraphQuery, LinkQuery, SearchIndex, SearchQuery};
use okf_jawn_core::storage::{
    Backups, BlameQuery, BlobStore, ByteReader, CandidateChanges, CandidateCheck, ChangeContext,
    CommitChanges, Committed, DiffQuery, LocalSource, LogQuery, NewWorkspace, ObjectInfo, Page,
    Promotion, Provenance, Purger, StorageScope, TreeEdit, VersionStore, WorkspaceArchive,
    WorkspaceCatalog, WorkspaceUpdate, derive_item_id, derive_proposal_id, derive_purge_id,
    workspace_with_permissions,
};
use okf_jawn_core::uploads::{NewUpload, UploadRecord, UploadStore};
use serde_json::json;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

/// The application-side check: load the candidate with okf-core and judge it with okf-validator.
struct OkfConformance;

impl CandidateCheck for OkfConformance {
    fn check(&self, root: &Path) -> Result<Vec<Warning>, ApiError> {
        let bundle = okf_core::Bundle::load(root)
            .map_err(|error| ApiError::new(ErrorCode::InvalidInput, error.to_string()))?;
        let report = okf_validator::validate_bundle(&bundle);
        if !report.is_conformant() {
            return Err(ApiError::new(
                ErrorCode::InvalidInput,
                "The candidate tree is not a conformant OKF bundle",
            ));
        }
        Ok(report
            .diagnostics
            .iter()
            .map(|diagnostic| Warning {
                code: diagnostic.severity.as_str().to_owned(),
                message: diagnostic.message.clone(),
                location: diagnostic
                    .path
                    .as_ref()
                    .map(|path| path.display().to_string()),
            })
            .collect())
    }
}

/// Marks a `*_calls` proof as used without awaiting it: a function item has no runtime size.
fn type_checked<F>(proof: &F) -> bool {
    std::mem::size_of_val(proof) == 0
}

fn digest(fill: char) -> Result<Digest, Box<dyn Error>> {
    Ok(Digest::try_from(fill.to_string().repeat(64))?)
}

fn revision(fill: char) -> Result<Revision, Box<dyn Error>> {
    Ok(Revision::try_from(fill.to_string().repeat(40))?)
}

fn instant(spelling: &str) -> Result<Timestamp, Box<dyn Error>> {
    Ok(Timestamp::try_from(spelling.to_owned())?)
}

fn scope() -> Result<StorageScope, Box<dyn Error>> {
    Ok(StorageScope {
        tenant_id: TenantId::try_from("local".to_owned())?,
        workspace_id: WorkspaceId(Uuid::from_u128(1)),
    })
}

fn initiator() -> Provenance {
    Provenance {
        subject: "user_1".to_owned(),
        route: AccessRoute::LocalOwner,
        client_id: None,
    }
}

async fn version_reads(
    versions: &dyn VersionStore,
    scope: &StorageScope,
    item: ItemId,
    digest: &Digest,
) -> Result<Option<String>, ApiError> {
    let head = versions.head(scope).await?;
    let page = Page {
        cursor: None,
        limit: 50,
    };
    let listing = versions.list(scope, &head, None, page).await?;
    let document = versions.show(scope, &head, item).await?;
    versions
        .read_file(scope, &head, &document.summary.path)
        .await?;
    versions.rules(scope, &head).await?;
    versions.types(scope, &head).await?;
    versions
        .log(
            scope,
            LogQuery {
                tip: head.clone(),
                item_id: Some(item),
                page: Page {
                    cursor: listing.next_cursor,
                    limit: 20,
                },
            },
        )
        .await?;
    versions
        .diff(
            scope,
            DiffQuery {
                from: head.clone(),
                to: head.clone(),
                item_id: None,
            },
        )
        .await?;
    versions
        .blame(
            scope,
            BlameQuery {
                revision: head.clone(),
                item_id: item,
                lines: TextRange { start: 1, end: 5 },
            },
        )
        .await?;
    versions.correction(scope, &head, item, digest).await
}

async fn version_writes(
    versions: &dyn VersionStore,
    scope: &StorageScope,
    changes: CommitChanges,
    proposal_id: ProposalId,
) -> Result<Committed, ApiError> {
    let check: Arc<dyn CandidateCheck> = Arc::new(OkfConformance);
    let mutation_id = changes.mutation_id;
    let base = changes.expected_head.clone();
    let author = changes.author.clone();
    let edits = changes.edits.clone();
    if let Some(found) = versions.find_commit(scope, mutation_id, &base).await? {
        return Ok(Committed {
            revision: found,
            replayed: true,
            warnings: Vec::new(),
        });
    }
    let committed = versions.commit(scope, changes, Arc::clone(&check)).await?;
    let candidate = versions
        .create_candidate(
            scope,
            proposal_id,
            CandidateChanges {
                mutation_id,
                base,
                author: author.clone(),
                message: "Propose a change".to_owned(),
                edits,
            },
            check,
        )
        .await?;
    versions
        .promote_candidate(
            scope,
            Promotion {
                mutation_id,
                proposal_id,
                expected_head: committed.revision,
                candidate,
                approver: author,
                message: "Accept the proposal".to_owned(),
            },
        )
        .await
}

async fn blob_store_calls(
    blobs: &dyn BlobStore,
    scope: &StorageScope,
    body: ByteReader,
    expected: &Digest,
) -> Result<LocalSource, ApiError> {
    let stored = blobs.put(scope, body, 1024, Some(expected.clone())).await?;
    let read = blobs.open(scope, &stored.digest, 0, stored.size).await?;
    blobs.materialize(scope, &read.object.digest).await
}

async fn catalog_calls(
    catalog: &dyn WorkspaceCatalog,
    scope: &StorageScope,
    mutation_id: MutationId,
) -> Result<Vec<Workspace>, ApiError> {
    let created = catalog
        .create(
            &scope.tenant_id,
            mutation_id,
            NewWorkspace {
                name: "Team".to_owned(),
                description: "Shared notes".to_owned(),
                creator: initiator(),
            },
        )
        .await?;
    let opened = catalog.open(scope).await?;
    let updated = catalog
        .update(
            scope,
            mutation_id,
            WorkspaceUpdate {
                expected_head: opened.head,
                name: created.name,
                description: created.description,
                author: initiator(),
            },
        )
        .await?;
    catalog
        .archive(
            scope,
            mutation_id,
            WorkspaceArchive {
                expected_head: updated.head,
                author: initiator(),
            },
        )
        .await?;
    catalog.unarchive(scope, mutation_id, initiator()).await?;
    catalog.list(&scope.tenant_id, true).await
}

/// What the purge and backup handlers ask of storage.
async fn purge_and_backup_calls(
    purger: &dyn Purger,
    backups: &dyn Backups,
    scope: &StorageScope,
    mutation_id: MutationId,
    item: ItemId,
) -> Result<RestoreReport, ApiError> {
    let purge = derive_purge_id(mutation_id);
    purger.purge_item(scope, mutation_id, purge, item).await?;
    purger
        .purge_workspace(&scope.tenant_id, mutation_id, purge, scope.workspace_id)
        .await?;
    backups
        .write_installation_archive(&scope.tenant_id, mutation_id)
        .await?;
    let archive = backups.write_workspace_archive(scope, mutation_id).await?;
    backups
        .restore_import(
            scope,
            archive.digest,
            mutation_id,
            vec!["user_1".to_owned()],
        )
        .await
}

async fn converter_calls(
    converter: &dyn Converter,
    source: LocalSource,
    output_directory: PathBuf,
) -> Result<ConversionStatus, ApiError> {
    let pages = converter.page_count(&source, "report.pdf").await?;
    let ConverterLimits { window_pages, .. } = converter.limits();
    let window = pages.map(|count| PageRange {
        start: 1,
        end: count.min(window_pages),
    });
    let conversion = converter
        .convert(ConversionInput {
            source,
            file_name: "report.pdf".to_owned(),
            settings: ConversionSettings::default(),
            window,
            timeout: Duration::from_secs(120),
            output_directory,
        })
        .await?;
    Ok(conversion.status)
}

fn job_specs() -> Result<Vec<JobSpec>, Box<dyn Error>> {
    Ok(vec![
        JobSpec::Import {
            base_revision: revision('a')?,
            upload_ids: vec![UploadId(Uuid::from_u128(9))],
            destination: Some(WorkspacePath::try_from("inbox".to_owned())?),
            apply_naming_rules: true,
            settings: ConversionSettings::default(),
        },
        JobSpec::Redigest {
            item_id: ItemId(Uuid::from_u128(10)),
            base_revision: revision('a')?,
            settings: ConversionSettings::default(),
            pages: Some(vec![PageRange { start: 5, end: 8 }]),
        },
        JobSpec::ExportWorkspace {
            revision: revision('a')?,
            include_history: false,
        },
        JobSpec::BackupWorkspace,
        JobSpec::RestoreWorkspace {
            upload_id: UploadId(Uuid::from_u128(12)),
            archive: digest('c')?,
        },
        JobSpec::RebuildIndex,
        JobSpec::ExportView {
            item_id: ItemId(Uuid::from_u128(10)),
            revision: revision('a')?,
        },
        JobSpec::BackupInstallation,
        JobSpec::PurgeWorkspace {
            purge_id: PurgeId(Uuid::from_u128(13)),
            workspace_id: WorkspaceId(Uuid::from_u128(1)),
        },
        JobSpec::PurgeItem {
            purge_id: PurgeId(Uuid::from_u128(14)),
            workspace_id: WorkspaceId(Uuid::from_u128(1)),
            item_id: ItemId(Uuid::from_u128(10)),
        },
    ])
}

/// What an import handler does with nothing but the claim it was handed.
fn commit_for(claimed: &ClaimedJob, edits: Vec<TreeEdit>) -> Option<CommitChanges> {
    let JobSpec::Import { base_revision, .. } = &claimed.spec else {
        return None;
    };
    Some(CommitChanges {
        mutation_id: claimed.mutation_id,
        expected_head: base_revision.clone(),
        author: claimed.initiator.clone(),
        message: format!("Import {} source(s)", edits.len()),
        edits,
    })
}

async fn record_store_calls(
    records: &dyn RecordStore,
    scope: &StorageScope,
    new_job: NewJob,
    review: Review,
    receipt: Receipt,
) -> Result<Vec<Review>, ApiError> {
    let mutation_id = new_job.mutation_id;
    let job_scope = JobScope::Workspace(scope.clone());
    let job = records.create_job(&job_scope, new_job).await?;
    records.get_job(&job_scope, job.id).await?;
    let page = Page {
        cursor: None,
        limit: 50,
    };
    records.list_jobs(&job_scope, page).await?;
    if let Some(claimed) = records.claim_job(job.id).await? {
        records.update_progress(&claimed.lease, 40).await?;
        records
            .complete_job(JobCompletion {
                lease: claimed.lease.clone(),
                revision: None,
                item_ids: Vec::new(),
                artifact: None,
                restore: None,
                outputs: Vec::new(),
                warnings: Vec::new(),
            })
            .await?;
        records
            .fail_job(claimed.lease, "converter stopped".to_owned(), true)
            .await?;
    }
    records.cancel_job(&job_scope, mutation_id, job.id).await?;
    records.retry_job(&job_scope, mutation_id, job.id).await?;
    records.pending_jobs().await?;
    records.expire_leases().await?;
    let item_id = review.source.item_id;
    records.insert_review(scope, mutation_id, review).await?;
    let stored = records
        .insert_receipt(scope, Some(mutation_id), receipt)
        .await?;
    records.get_receipt(scope, stored.id).await?;
    records.list_reviews(scope, item_id).await
}

/// What a purge request, its tenant job and a View binding's dataset do with the records.
async fn purge_and_derived_calls(
    records: &dyn RecordStore,
    scope: &StorageScope,
    mutation_id: MutationId,
    target: PurgeTarget,
    dataset: DerivedObject,
) -> Result<Option<DerivedObject>, ApiError> {
    let tenant = &scope.tenant_id;
    let purge = records
        .create_purge(
            tenant,
            mutation_id,
            NewPurge {
                id: derive_purge_id(mutation_id),
                target,
                initiator: initiator(),
            },
        )
        .await?;
    let purge = records.get_purge(tenant, purge.id).await?;
    records.update_purge(tenant, purge).await?;
    let tenant_scope = JobScope::Tenant(tenant.clone());
    records
        .list_jobs(
            &tenant_scope,
            Page {
                cursor: None,
                limit: 20,
            },
        )
        .await?;
    records.revision_mapping(scope, &dataset.revision).await?;
    let recorded = records.record_derived_object(scope, dataset).await?;
    records
        .derived_object(
            scope,
            recorded.item_id,
            &recorded.revision,
            &recorded.digest,
        )
        .await
}

async fn job_runtime_calls(
    queue: &dyn JobQueue,
    handler: &dyn JobHandler,
    claimed: &ClaimedJob,
) -> Result<(), ApiError> {
    queue
        .enqueue(claimed.scope.clone(), claimed.lease.job_id)
        .await?;
    handler.handle(claimed).await
}

fn hex(bytes: &[u8]) -> Result<String, std::fmt::Error> {
    let mut text = String::new();
    for byte in bytes {
        write!(text, "{byte:02x}")?;
    }
    Ok(text)
}

/// The rule a resumed `create_connector` follows: an existing row gets a fresh secret.
async fn issue_connector(
    credentials: &dyn CredentialStore,
    mutation_id: MutationId,
    connector: NewConnector,
) -> Result<IssuedConnector, ApiError> {
    match credentials.create_connector(mutation_id, connector).await? {
        ConnectorIssue::Issued(issued) => Ok(issued),
        ConnectorIssue::Existing(existing) => {
            credentials
                .rotate_connector_secret(existing.connector_id)
                .await
        }
    }
}

async fn credential_store_calls(
    credentials: &dyn CredentialStore,
    session: SessionRecord,
    presented_secret: &str,
) -> Result<Option<Connector>, ApiError> {
    credentials.installation_identity().await?;
    let stored = credentials.insert_session(session).await?;
    credentials.get_session(&stored.session_id).await?;
    credentials.revoke_session(&stored.session_id).await?;
    for connector in credentials.list_connectors(true).await? {
        credentials.revoke_connector(connector.connector_id).await?;
    }
    credentials
        .lookup_connector(secret_hash(presented_secret))
        .await
}

async fn sandbox_calls(
    sandbox: &dyn SandboxCapabilityStore,
    scope: &StorageScope,
    mint: SandboxMint,
    token: &str,
) -> Result<Option<SandboxResolved>, ApiError> {
    sandbox.mint(scope, token_hash(token), mint).await?;
    sandbox.resolve(token_hash(token)).await
}

async fn confirmation_calls(
    confirmations: &dyn ConfirmationStore,
    scope: &StorageScope,
    mutation_id: MutationId,
    create: ConfirmationCreate,
) -> Result<Confirmation, ApiError> {
    let issued = confirmations
        .create(scope, mutation_id, create.clone())
        .await?;
    confirmations
        .consume(
            scope,
            mutation_id,
            ConfirmationConsume {
                confirmation_id: issued.id,
                action: create.action,
                target: create.target,
                revision: create.revision,
                content_digest: create.content_digest,
                session_id: create.session_id,
                subject: create.subject,
            },
        )
        .await
}

async fn draft_calls(
    drafts: &dyn DraftStore,
    scope: &StorageScope,
    mutation_id: MutationId,
    draft: DraftWrite,
) -> Result<Draft, ApiError> {
    let item = draft.item_id;
    let editor = draft.editor.clone();
    drafts.save(scope, mutation_id, draft).await?;
    drafts.get(scope, item, &editor).await?;
    drafts.list(scope, &editor).await?;
    drafts.discard(scope, mutation_id, item, &editor).await
}

async fn proposal_calls(
    proposals: &dyn ProposalStore,
    scope: &StorageScope,
    mutation_id: MutationId,
    proposal: Proposal,
    comment: Comment,
) -> Result<CommentPage, ApiError> {
    let stored = proposals.insert(scope, mutation_id, proposal).await?;
    let read = proposals.get(scope, stored.id).await?;
    proposals
        .list(
            scope,
            ProposalFilter {
                status: Some(ProposalStatus::Open),
                page: Page {
                    cursor: None,
                    limit: 20,
                },
            },
        )
        .await?;
    let updated = proposals.update(scope, read).await?;
    proposals
        .add_comment(scope, mutation_id, updated.id, comment)
        .await?;
    let page = Page {
        cursor: None,
        limit: 50,
    };
    proposals.list_comments(scope, updated.id, page).await
}

async fn upload_calls(
    uploads: &dyn UploadStore,
    scope: &StorageScope,
    mutation_id: MutationId,
    body: ByteReader,
    sha256: Digest,
) -> Result<UploadRecord, ApiError> {
    let slot = uploads
        .create(
            scope,
            mutation_id,
            NewUpload {
                filename: "report.pdf".to_owned(),
                relative_path: "finance/2026".to_owned(),
                expected_size: 3,
                expected_sha256: Some(sha256.clone()),
                supplied_by: initiator(),
            },
        )
        .await?;
    uploads.get(scope, slot.id).await?;
    uploads
        .put_content(scope, slot.id, body, slot.expected_size)
        .await?;
    let complete = uploads.complete(scope, slot.id, sha256).await?;
    uploads
        .consume(scope, complete.id, JobId(Uuid::from_u128(7)))
        .await
}

async fn event_calls(
    events: &dyn EventLog,
    scope: &StorageScope,
    mutation_id: MutationId,
    job_id: JobId,
) -> Result<Event, ApiError> {
    let progress = NewEvent {
        kind: EventKind::JobUpdated,
        revision: None,
        item_id: None,
        job_id: Some(job_id),
        connector_id: None,
        actor: None,
        operation: None,
    };
    let workspace = EventScope::Workspace(scope.clone());
    events.append(&workspace, None, progress.clone()).await?;
    let principal = Principal {
        subject: "user_1".to_owned(),
        tenant_id: scope.tenant_id.clone(),
        route: AccessRoute::McpDelegation,
        client_id: Some("connector".to_owned()),
        delegation: None,
    };
    let tenant = EventScope::Tenant(scope.tenant_id.clone());
    events
        .append(
            &tenant,
            None,
            NewEvent::permission_denied(&principal, OperationName::BackupInstallation),
        )
        .await?;
    events
        .list(
            &workspace,
            EventQuery {
                after: None,
                page: Page {
                    cursor: None,
                    limit: 100,
                },
            },
        )
        .await?;
    events.append(&workspace, Some(mutation_id), progress).await
}

async fn search_index_calls(
    index: &dyn SearchIndex,
    scope: &StorageScope,
    head: Revision,
    item: ItemId,
) -> Result<GetGraphResponse, ApiError> {
    index.index_revision(scope, head.clone()).await?;
    index
        .search(
            scope,
            SearchQuery {
                revision: head.clone(),
                text: "quarterly revenue".to_owned(),
                folder: None,
                include_archived: false,
                extraction: None,
                page: Page {
                    cursor: None,
                    limit: 20,
                },
            },
        )
        .await?;
    index
        .links(
            scope,
            LinkQuery {
                revision: head.clone(),
                item_id: item,
                direction: LinkDirection::Both,
                page: Page {
                    cursor: None,
                    limit: 20,
                },
            },
        )
        .await?;
    index.rebuild(scope, head.clone()).await?;
    index
        .graph(
            scope,
            GraphQuery {
                revision: head,
                folder: None,
                max_nodes: 200,
            },
        )
        .await
}

/// What an export handler and, later, a download or a restore do with an artifact.
async fn artifact_calls(
    records: &dyn RecordStore,
    blobs: &dyn BlobStore,
    backups: &dyn Backups,
    claimed: &ClaimedJob,
    stored: ObjectInfo,
) -> Result<ObjectInfo, ApiError> {
    let record = records
        .record_artifact(
            &claimed.scope,
            claimed.mutation_id,
            NewArtifact {
                kind: ArtifactKind::Export,
                object: stored,
                media_type: "application/zip".to_owned(),
                created_by_job: claimed.lease.job_id,
            },
        )
        .await?;
    records
        .complete_job(JobCompletion {
            lease: claimed.lease.clone(),
            revision: None,
            item_ids: Vec::new(),
            artifact: Some(record.id),
            restore: None,
            outputs: vec![record.object.digest.clone()],
            warnings: Vec::new(),
        })
        .await?;
    let found = records.get_artifact(&claimed.scope, record.id).await?;
    // A workspace artifact's bytes come from the blob store, an installation backup's from the
    // tenant's archive store.
    let read = match &found.scope {
        JobScope::Workspace(workspace) => {
            blobs
                .open(workspace, &found.object.digest, 0, found.object.size)
                .await?
        }
        JobScope::Tenant(tenant) => {
            backups
                .open_installation_archive(tenant, &found.object.digest, 0, found.object.size)
                .await?
        }
    };
    Ok(read.object)
}

#[test]
fn provenance_keeps_the_typed_route_and_client() -> TestResult {
    let principal = Principal {
        subject: "user_1".to_owned(),
        tenant_id: TenantId::try_from("local".to_owned())?,
        route: AccessRoute::McpDelegation,
        client_id: Some("client_1".to_owned()),
        delegation: None,
    };
    assert_eq!(
        Provenance::from_principal(&principal),
        Provenance {
            subject: "user_1".to_owned(),
            route: AccessRoute::McpDelegation,
            client_id: Some("client_1".to_owned()),
        }
    );
    Ok(())
}

#[test]
fn page_carries_the_wire_cursor_and_limit() {
    let page = Page::from(PageRequest {
        cursor: Some("next".to_owned()),
        limit: 25,
    });
    assert_eq!(
        page,
        Page {
            cursor: Some("next".to_owned()),
            limit: 25,
        }
    );
}

#[test]
fn derived_identities_are_stable_per_mutation() {
    let first = MutationId(Uuid::from_u128(1));
    let second = MutationId(Uuid::from_u128(2));
    assert_eq!(derive_item_id(first, 0), derive_item_id(first, 0));
    assert_ne!(derive_item_id(first, 0), derive_item_id(first, 1));
    assert_ne!(derive_item_id(first, 0), derive_item_id(second, 0));
    assert_eq!(derive_item_id(first, 0).0.get_version_num(), 8);
    assert_eq!(derive_proposal_id(first), derive_proposal_id(first));
    assert_ne!(derive_proposal_id(first), derive_proposal_id(second));
    assert_ne!(derive_proposal_id(first).0, derive_item_id(first, 0).0);
}

#[test]
fn object_info_is_digest_and_size_only() -> TestResult {
    let object = ObjectInfo {
        digest: digest('a')?,
        size: 3,
    };
    assert_eq!(object.size, 3);
    assert_eq!(object.digest, digest('a')?);
    assert!(type_checked(&blob_store_calls));
    Ok(())
}

#[test]
fn version_store_calls_type_check() {
    assert!(type_checked(&version_reads));
    assert!(type_checked(&version_writes));
}

#[test]
fn a_proposed_change_becomes_the_matching_tree_edit() -> TestResult {
    let new_id = ItemId(Uuid::from_u128(10));
    let existing = ItemId(Uuid::from_u128(11));
    let path = WorkspacePath::try_from("notes/plan.md".to_owned())?;
    let view = ChangeContext {
        new_item_id: new_id,
        new_item_kind: ItemKind::View,
        proposal_id: ProposalId(Uuid::from_u128(12)),
        proposer: initiator(),
    };
    let note = ChangeContext {
        new_item_kind: ItemKind::Note,
        ..view.clone()
    };
    let created = TreeEdit::from_change(
        Change::Create {
            path: path.clone(),
            type_name: "Note".to_owned(),
            body: "# Plan\n".to_owned(),
            properties: BTreeMap::new(),
        },
        &view,
    );
    assert!(matches!(
        created,
        TreeEdit::CreateItem { item_id, kind: ItemKind::View, title: None, path: created_path, .. }
            if item_id == new_id && created_path == path
    ));
    let archived = TreeEdit::from_change(Change::Archive { item_id: existing }, &note);
    assert!(matches!(
        archived,
        TreeEdit::SetStatus { item_id, status: None, archived: Some(true) } if item_id == existing
    ));
    let moved = TreeEdit::from_change(
        Change::Move {
            item_id: existing,
            destination: path.clone(),
        },
        &note,
    );
    assert!(matches!(
        moved,
        TreeEdit::MoveItem { item_id, destination } if item_id == existing && destination == path
    ));
    let edited = TreeEdit::from_change(
        Change::Edit {
            item_id: existing,
            body: "new".to_owned(),
            properties: BTreeMap::new(),
        },
        &note,
    );
    assert!(matches!(
        edited,
        TreeEdit::EditItem { item_id, body, .. } if item_id == existing && body == "new"
    ));
    let supplied = TreeEdit::from_change(
        Change::SupplyExtraction {
            item_id: existing,
            based_on: None,
            pages: Vec::new(),
            markdown: "# Supplied".to_owned(),
        },
        &note,
    );
    assert!(matches!(
        supplied,
        TreeEdit::SupplyExtraction { item_id, proposal_id, supplier, .. }
            if item_id == existing && proposal_id == note.proposal_id && supplier == initiator()
    ));
    Ok(())
}

#[test]
fn a_candidate_check_rejects_a_tree_it_cannot_load() -> TestResult {
    let check: Arc<dyn CandidateCheck> = Arc::new(OkfConformance);
    let error = check
        .check(Path::new("no-such-okf-jawn-candidate-directory"))
        .err()
        .ok_or("a missing candidate directory must be rejected")?;
    assert_eq!(error.code, ErrorCode::InvalidInput);
    Ok(())
}

#[test]
fn a_commit_names_its_mutation_author_base_and_edits() -> TestResult {
    let changes = CommitChanges {
        mutation_id: MutationId(Uuid::from_u128(5)),
        expected_head: revision('a')?,
        author: initiator(),
        message: "Remove a note".to_owned(),
        edits: vec![
            TreeEdit::DeleteItem {
                item_id: derive_item_id(MutationId(Uuid::from_u128(4)), 0),
            },
            TreeEdit::RestoreWorkspace {
                from: revision('b')?,
            },
        ],
    };
    assert_eq!(changes.expected_head, revision('a')?);
    assert_eq!(changes.author, initiator());
    assert_eq!(changes.edits.len(), 2);
    assert_eq!(scope()?.workspace_id, WorkspaceId(Uuid::from_u128(1)));
    Ok(())
}

#[test]
fn the_application_not_the_catalog_fills_permissions() -> TestResult {
    let bare = Workspace {
        id: WorkspaceId(Uuid::from_u128(1)),
        name: "Team".to_owned(),
        description: "Shared notes".to_owned(),
        head: revision('a')?,
        created_at: instant("2026-10-05T00:00:00.000Z")?,
        archived_at: None,
        permissions: Vec::new(),
    };
    let shown = workspace_with_permissions(bare, vec![Permission::Read]);
    assert_eq!(shown.permissions, vec![Permission::Read]);
    assert!(type_checked(&catalog_calls));
    assert!(type_checked(&purge_and_backup_calls));
    Ok(())
}

#[test]
fn conversion_settings_are_typed_and_reject_unknown_keys() -> TestResult {
    // The contract's settings (Stage 1b design section 3): page renders are on at 144 dpi by
    // default, and every field is required on the wire.
    let defaults = ConversionSettings::default();
    assert_eq!(defaults.ocr, OcrPolicy::Auto);
    assert!(defaults.table_structure);
    assert!(defaults.page_images);
    assert_eq!(defaults.page_image_dpi, 144);
    let parsed: ConversionSettings = serde_json::from_value(json!({
        "ocr": "force_full_page",
        "ocr_language": "de",
        "table_structure": true,
        "page_images": true,
        "page_image_dpi": 144
    }))?;
    assert_eq!(
        parsed,
        ConversionSettings {
            ocr: OcrPolicy::ForceFullPage,
            ocr_language: Some("de".to_owned()),
            ..ConversionSettings::default()
        }
    );
    assert!(
        serde_json::from_value::<ConversionSettings>(json!({"ocr": "force_full_page"})).is_err()
    );
    assert!(serde_json::from_value::<ConversionSettings>(json!({"quality": "high"})).is_err());
    assert!(type_checked(&converter_calls));
    Ok(())
}

#[test]
fn every_job_spec_reports_its_kind_and_survives_storage() -> TestResult {
    let specs = job_specs()?;
    let kinds: Vec<JobKind> = specs.iter().map(JobSpec::kind).collect();
    assert!(matches!(
        kinds.as_slice(),
        [
            JobKind::Import,
            JobKind::Redigest,
            JobKind::ExportWorkspace,
            JobKind::BackupWorkspace,
            JobKind::RestoreWorkspace,
            JobKind::RebuildIndex,
            JobKind::ExportView,
            JobKind::BackupInstallation,
            JobKind::PurgeWorkspace,
            JobKind::PurgeItem
        ]
    ));
    for spec in &specs {
        let stored = serde_json::to_value(spec)?;
        let loaded: JobSpec = serde_json::from_value(stored)?;
        assert_eq!(&loaded, spec);
    }
    assert_eq!(
        serde_json::to_value(JobSpec::RebuildIndex)?,
        json!({"kind": "rebuild_index"})
    );
    Ok(())
}

#[test]
fn a_commit_is_built_from_a_claimed_job_alone() -> TestResult {
    let scope = scope()?;
    let spec = JobSpec::Import {
        base_revision: revision('a')?,
        upload_ids: vec![UploadId(Uuid::from_u128(9))],
        destination: None,
        apply_naming_rules: true,
        settings: ConversionSettings::default(),
    };
    let mutation_id = MutationId(Uuid::from_u128(5));
    let job_id = JobId(Uuid::from_u128(7));
    let claimed = ClaimedJob {
        scope: JobScope::Workspace(scope.clone()),
        lease: JobLease {
            job_id,
            token: "claim-1".to_owned(),
            attempt: 1,
            expires_at: instant("2026-10-08T12:05:00.000Z")?,
        },
        job: Job {
            id: job_id,
            workspace_id: Some(scope.workspace_id),
            kind: spec.kind(),
            state: JobState::Running,
            progress: 0,
            attempt: 1,
            warnings: Vec::new(),
            error: None,
            revision: None,
            item_ids: Vec::new(),
            artifact: None,
            restore: None,
        },
        mutation_id,
        initiator: initiator(),
        spec,
    };
    let folder = WorkspacePath::try_from("inbox".to_owned())?;
    let commit = commit_for(&claimed, vec![TreeEdit::CreateFolder { folder }])
        .ok_or("an import job must yield a commit")?;
    assert_eq!(commit.mutation_id, mutation_id);
    assert_eq!(commit.expected_head, revision('a')?);
    assert_eq!(commit.author, initiator());
    assert_eq!(commit.message, "Import 1 source(s)");
    assert_eq!(commit.edits.len(), 1);
    assert_eq!(
        derive_item_id(claimed.mutation_id, 0),
        derive_item_id(commit.mutation_id, 0)
    );
    assert!(type_checked(&record_store_calls));
    assert!(type_checked(&job_runtime_calls));
    assert!(type_checked(&purge_and_derived_calls));
    Ok(())
}

#[test]
fn connector_secrets_are_hashed_with_sha256_in_one_place() -> TestResult {
    assert_eq!(
        hex(&secret_hash("abc"))?,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_ne!(secret_hash("abc"), secret_hash("abd"));
    assert!(type_checked(&issue_connector));
    assert!(type_checked(&credential_store_calls));
    Ok(())
}

#[test]
fn sandbox_tokens_are_hashed_before_they_reach_the_store() -> TestResult {
    assert_eq!(
        hex(&token_hash("abc"))?,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_ne!(token_hash("abc"), token_hash("abd"));
    let mint = SandboxMint {
        item_id: ItemId(Uuid::from_u128(10)),
        revision: revision('a')?,
        object: digest('b')?,
        media_type: "text/html".to_owned(),
        expires_at: "2026-10-05T00:05:00Z".to_owned(),
    };
    assert_eq!(mint.media_type, "text/html");
    assert!(type_checked(&sandbox_calls));
    Ok(())
}

#[test]
fn row_stores_take_core_types() -> TestResult {
    let draft = DraftWrite {
        item_id: ItemId(Uuid::from_u128(10)),
        editor: "user_1".to_owned(),
        base_revision: revision('a')?,
        body: "# Draft\n".to_owned(),
        properties: BTreeMap::new(),
        content_digest: digest('d')?,
    };
    assert_eq!(draft.content_digest, digest('d')?);
    let filter = ProposalFilter {
        status: None,
        page: Page::from(PageRequest {
            cursor: None,
            limit: 10,
        }),
    };
    assert_eq!(filter.page.limit, 10);
    assert!(type_checked(&confirmation_calls));
    assert!(type_checked(&draft_calls));
    assert!(type_checked(&proposal_calls));
    Ok(())
}

#[test]
fn an_upload_slot_keeps_what_the_request_supplied() -> TestResult {
    let slot = NewUpload {
        filename: "report.pdf".to_owned(),
        relative_path: "finance/2026".to_owned(),
        expected_size: 3,
        expected_sha256: Some(digest('e')?),
        supplied_by: initiator(),
    };
    assert_eq!(slot.filename, "report.pdf");
    assert_eq!(slot.relative_path, "finance/2026");
    assert_eq!(slot.supplied_by, initiator());
    assert!(type_checked(&upload_calls));
    assert!(type_checked(&event_calls));
    Ok(())
}

#[test]
fn a_search_query_names_exactly_one_revision() -> TestResult {
    let query = SearchQuery {
        revision: revision('a')?,
        text: "quarterly revenue".to_owned(),
        folder: Some(WorkspacePath::try_from("finance".to_owned())?),
        include_archived: false,
        extraction: None,
        page: Page {
            cursor: None,
            limit: 20,
        },
    };
    assert_eq!(query.revision, revision('a')?);
    assert_eq!(
        query.folder.as_ref().map(WorkspacePath::as_str),
        Some("finance")
    );
    assert!(type_checked(&search_index_calls));
    Ok(())
}

#[test]
fn a_page_of_comments_keeps_its_order_and_cursor() -> TestResult {
    let page = CommentPage {
        items: vec![Comment {
            id: "c1".to_owned(),
            author: "user_1".to_owned(),
            text: "Why this figure?".to_owned(),
            created_at: instant("2026-10-05T00:00:00.000Z")?,
            invalidated_by: None,
        }],
        next_cursor: Some("after-c1".to_owned()),
    };
    assert_eq!(
        page.items.first().map(|comment| comment.id.as_str()),
        Some("c1")
    );
    assert_eq!(page.next_cursor.as_deref(), Some("after-c1"));
    assert!(type_checked(&proposal_calls));
    Ok(())
}

#[test]
fn an_artifact_record_yields_the_wire_download_for_its_transport() -> TestResult {
    let workspace = WorkspaceId(Uuid::from_u128(1));
    let record = ArtifactRecord {
        id: ArtifactId(Uuid::from_u128(12)),
        scope: JobScope::Workspace(StorageScope {
            tenant_id: TenantId::try_from("local".to_owned())?,
            workspace_id: workspace,
        }),
        kind: ArtifactKind::WorkspaceBackup,
        object: ObjectInfo {
            digest: digest('c')?,
            size: 2048,
        },
        media_type: "application/zip".to_owned(),
        created_by_job: JobId(Uuid::from_u128(7)),
    };
    let transport = TRANSPORTS
        .iter()
        .find(|transport| transport.id == "download_artifact")
        .ok_or("the download_artifact transport is not declared")?;
    let expected_path = transport
        .path
        .replace("{workspace_id}", &workspace.0.to_string())
        .replace("{artifact_id}", &record.id.0.to_string());
    let download = record.download();
    assert_eq!(download.download_path, expected_path);
    assert_eq!(download.kind, ArtifactKind::WorkspaceBackup);
    assert_eq!(download.artifact_id, record.id);
    assert_eq!(download.sha256, digest('c')?);
    assert_eq!(download.size, "2048");
    assert_eq!(transport.response_media, record.media_type);
    assert!(type_checked(&artifact_calls));
    Ok(())
}

#[test]
fn a_converted_image_carries_its_size_and_its_own_caption() -> TestResult {
    let location = SourceLocation::Direct {
        locator: SourceLocator::Page { page_no: 2 },
    };
    let asset = ConvertedAsset {
        path: PathBuf::from("out/figure-1.png"),
        media_type: "image/png".to_owned(),
        role: AssetRole::Picture,
        location: location.clone(),
        pixel_size: Some(PixelSize {
            width: 640,
            height: 480,
        }),
        caption: Some("Revenue by quarter".to_owned()),
    };
    // The retained form is what ingest stores and core reads back: size and caption survive
    // the stored JSON, and an image without them stores neither key.
    let retained = RetainedAsset {
        digest: Digest::try_from("c".repeat(64))?,
        media_type: asset.media_type.clone(),
        role: asset.role,
        location,
        pixel_size: asset.pixel_size,
        caption: asset.caption.clone(),
    };
    let stored = serde_json::to_value(&retained)?;
    assert_eq!(
        stored.get("pixel_size"),
        Some(&json!({ "width": 640, "height": 480 }))
    );
    assert_eq!(stored.get("caption"), Some(&json!("Revenue by quarter")));
    assert_eq!(
        serde_json::from_value::<RetainedAsset>(stored.clone())?,
        retained
    );
    let bare = RetainedAsset {
        pixel_size: None,
        caption: None,
        ..retained
    };
    let bare_stored = serde_json::to_value(&bare)?;
    assert!(bare_stored.get("pixel_size").is_none(), "{bare_stored}");
    assert!(bare_stored.get("caption").is_none(), "{bare_stored}");
    assert_eq!(serde_json::from_value::<RetainedAsset>(bare_stored)?, bare);
    // The caption is the document's own text. No generated caption has a producer (Stage 1b
    // design section 3), so a stored image with a field beside them, such as the old origin
    // word, is refused rather than read.
    let mut with_origin = stored;
    with_origin
        .as_object_mut()
        .ok_or("a retained asset serializes as an object")?
        .insert("caption_origin".to_owned(), json!("generated"));
    let refused = serde_json::from_value::<RetainedAsset>(with_origin);
    assert!(refused.is_err(), "{refused:?}");
    Ok(())
}
