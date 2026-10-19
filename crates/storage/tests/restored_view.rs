//! A View restored into a workspace of another identity validates and opens there with
//! `get_view` (the construction plan's carried item "Restored View bindings", met by core M1.6
//! ruling E1): a View's file names no workspace, so the archive's tree, restored as it is, binds
//! every View to the workspace that holds it, at the restored head and at every revision of its
//! history, and the View's content digest is the archive's, so a review of it keeps its
//! coverage. Composed as the product composes it: `ApplicationService` over every storage
//! adapter of a real data directory, called as dispatch calls a handler.
#![cfg(feature = "runtime")]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{
    At, ItemId, JobId, MutationId, Revision, TenantId, WorkspaceId, WorkspacePath,
};
use okf_jawn_contract::item::{GetItemRequest, ItemKind};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::views::{GetViewRequest, ResolveViewRequest};
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::application::{ApplicationConfig, ApplicationService, Ports};
use okf_jawn_core::context::{Attempt, OperationContext, TenantGrant, WorkspaceGrant};
use okf_jawn_core::conversion::{
    Conversion, ConversionInput, Converter, ConverterLimits, PageCount,
};
use okf_jawn_core::jobs::{JobQueue, JobScope};
use okf_jawn_core::ports::{Application, PortFuture};
use okf_jawn_core::storage::{
    Backups, BlobStore, CandidateCheck, CommitChanges, LocalSource, NewWorkspace, Provenance,
    StorageScope, TreeEdit, VersionStore, WorkspaceCatalog,
};
use okf_jawn_core::views::validate::validate;
use okf_jawn_storage::Storage;
use serde_json::json;
use uuid::Uuid;

use check::{TestResult, some};

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// Grants every permission on every workspace and on the tenant.
struct OpenAccess;

/// A `JobQueue` no job reaches in this test.
struct Quiet;

/// No conversion runs in this test.
struct NoConverter;

/// Accepts every candidate.
struct Accept;

const EVERY: [Permission; 6] = [
    Permission::Read,
    Permission::Write,
    Permission::Propose,
    Permission::Approve,
    Permission::Review,
    Permission::Admin,
];

impl AccessControl for OpenAccess {
    fn authorize<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
        _permission: Permission,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(async move { Ok(open_grant(principal, workspace)) })
    }

    fn authorize_tenant<'a>(
        &'a self,
        principal: &'a Principal,
        _permission: Permission,
    ) -> PortFuture<'a, TenantGrant> {
        Box::pin(async move {
            Ok(TenantGrant {
                tenant_id: principal.tenant_id.clone(),
                permissions: EVERY.to_vec(),
            })
        })
    }

    fn grants<'a>(&'a self, _principal: &'a Principal) -> PortFuture<'a, Vec<WorkspaceGrant>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn grant_creator<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(async move { Ok(open_grant(principal, workspace)) })
    }

    fn editors<'a>(&'a self, _scope: &'a StorageScope) -> PortFuture<'a, Vec<String>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

impl JobQueue for Quiet {
    fn enqueue(&self, _scope: JobScope, _job: JobId) -> PortFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

impl Converter for NoConverter {
    fn page_count<'a>(
        &'a self,
        _source: &'a LocalSource,
        _file_name: &'a str,
    ) -> PortFuture<'a, PageCount> {
        Box::pin(async { Err(no_conversion()) })
    }

    fn convert(&self, _input: ConversionInput) -> PortFuture<'_, Conversion> {
        Box::pin(async { Err(no_conversion()) })
    }

    fn limits(&self) -> ConverterLimits {
        ConverterLimits {
            window_pages: 1,
            memory_limit_bytes: 1,
        }
    }
}

impl CandidateCheck for Accept {
    fn check(
        &self,
        _root: &std::path::Path,
        _rewritten: &[WorkspacePath],
    ) -> Result<Vec<okf_jawn_contract::common::Warning>, ApiError> {
        Ok(Vec::new())
    }
}

fn open_grant(principal: &Principal, workspace: WorkspaceId) -> WorkspaceGrant {
    WorkspaceGrant {
        scope: StorageScope {
            tenant_id: principal.tenant_id.clone(),
            workspace_id: workspace,
        },
        permissions: EVERY.to_vec(),
    }
}

fn no_conversion() -> ApiError {
    ApiError::new(ErrorCode::Internal, "no conversion runs in this test")
}

fn tenant() -> Fallible<TenantId> {
    Ok(TenantId::try_from("local".to_owned())?)
}

const fn mutation(number: u128) -> MutationId {
    MutationId(Uuid::from_u128(number))
}

const fn item(number: u128) -> ItemId {
    ItemId(Uuid::from_u128(number))
}

fn ana() -> Provenance {
    Provenance {
        subject: "ana".to_owned(),
        route: AccessRoute::LocalOwner,
        client_id: None,
    }
}

/// `ApplicationService` over every adapter of `storage`.
fn service(storage: &Storage) -> Fallible<ApplicationService> {
    let ports = Ports {
        access: Arc::new(OpenAccess),
        mutations: Arc::new(storage.mutations()),
        catalog: Arc::new(storage.catalog()),
        blobs: Arc::new(storage.blobs()),
        backups: Arc::new(storage.backups()),
        versions: Arc::new(storage.versions()),
        records: Arc::new(storage.records()),
        queue: Arc::new(Quiet),
        search: Arc::new(storage.search()),
        converter: Arc::new(NoConverter),
        proposals: Arc::new(storage.proposals()),
        uploads: Arc::new(storage.uploads()),
        events: Arc::new(storage.events()),
        credentials: Arc::new(storage.credentials()),
        drafts: Arc::new(storage.drafts()),
        confirmations: Arc::new(storage.confirmations()),
        sandbox: Arc::new(storage.sandbox()),
        readiness: Arc::new(storage.readiness()),
    };
    let config = ApplicationConfig::new(
        "https://sandbox.example.test".to_owned(),
        Duration::from_secs(300),
    );
    Ok(ApplicationService::new(ports, config)?)
}

/// Ana reading `scope` with every permission, as dispatch hands a handler its context.
fn reading(scope: &StorageScope, operation: OperationName) -> Fallible<OperationContext> {
    Ok(OperationContext {
        principal: Principal {
            subject: "ana".to_owned(),
            tenant_id: tenant()?,
            route: AccessRoute::LocalOwner,
            client_id: None,
            delegation: None,
        },
        session_id: None,
        operation,
        tenant: None,
        grants: vec![WorkspaceGrant {
            scope: scope.clone(),
            permissions: EVERY.to_vec(),
        }],
        mutation: None,
        attempt: Attempt::First,
    })
}

/// A blank workspace of `storage`, named from `number`.
async fn create(storage: &Storage, number: u128, name: &str) -> Fallible<(StorageScope, Revision)> {
    let created = storage
        .catalog()
        .create(
            &tenant()?,
            mutation(number),
            NewWorkspace {
                name: name.to_owned(),
                description: "kept".to_owned(),
                creator: ana(),
            },
        )
        .await?;
    Ok((
        StorageScope {
            tenant_id: tenant()?,
            workspace_id: created.id,
        },
        created.head,
    ))
}

/// Create one item in one commit on `head`.
async fn create_item(
    storage: &Storage,
    scope: &StorageScope,
    number: u128,
    head: &Revision,
    edit: TreeEdit,
) -> Fallible<Revision> {
    Ok(storage
        .versions()
        .commit(
            scope,
            CommitChanges {
                mutation_id: mutation(number),
                expected_head: head.clone(),
                author: ana(),
                message: "Seed".to_owned(),
                edits: vec![edit],
            },
            Arc::new(Accept),
        )
        .await?
        .revision)
}

/// A View of the note `note` at `revision`: an excerpt of it and its source line.
fn view_body(note: ItemId, revision: &Revision) -> Fallible<String> {
    let block = json!({
        "schema_version": 1,
        "mode": "pinned",
        "grammar": "json_render",
        "bindings": [{
            "name": "venue",
            "source": {
                "item_id": note,
                "path": "notes/venue.md",
                "revision": revision,
                "selection": { "kind": "all" }
            },
            "units": {},
            "transforms": []
        }],
        "spec": {
            "root": "root",
            "elements": {
                "root": { "type": "Stack", "props": {}, "children": ["excerpt", "sources"] },
                "excerpt": { "type": "SourceExcerpt", "props": { "binding": "venue" }, "children": [] },
                "sources": { "type": "SourceList", "props": { "bindings": ["venue"] }, "children": [] }
            }
        }
    });
    Ok(format!(
        "Where the workshop meets.\n\n```json okf-jawn-view\n{}\n```\n",
        serde_json::to_string_pretty(&block)?
    ))
}

/// In `origin`, a workspace holding the note at the revision answered first, then a View citing
/// it, committed at the revision answered second.
async fn seeded(origin: &Storage) -> Fallible<(StorageScope, Revision, Revision)> {
    let (source, blank) = create(origin, 1, "Origin").await?;
    let note = item(10);
    let view = item(11);
    let noted = create_item(
        origin,
        &source,
        2,
        &blank,
        TreeEdit::CreateItem {
            item_id: note,
            path: WorkspacePath::try_from("notes/venue.md".to_owned())?,
            title: Some("Venue".to_owned()),
            type_name: "Note".to_owned(),
            kind: ItemKind::Note,
            body: "# Venue\nThe hall.\n".to_owned(),
            properties: BTreeMap::new(),
        },
    )
    .await?;
    let archived = create_item(
        origin,
        &source,
        3,
        &noted,
        TreeEdit::CreateItem {
            item_id: view,
            path: WorkspacePath::try_from("views/venue.md".to_owned())?,
            title: Some("Venue".to_owned()),
            type_name: "Dashboard".to_owned(),
            kind: ItemKind::View,
            body: view_body(note, &noted)?,
            properties: BTreeMap::new(),
        },
    )
    .await?;
    Ok((source, noted, archived))
}

/// `source`'s archive imported into a blank workspace of a second installation, with that
/// installation's directory, storage and workspace.
async fn imported(
    origin: &Storage,
    source: &StorageScope,
) -> Fallible<(tempfile::TempDir, Storage, StorageScope)> {
    let archive = origin
        .backups()
        .write_workspace_archive(source, mutation(4))
        .await?
        .object;
    let bytes = std::fs::read(
        origin
            .data()
            .objects_path()
            .join("local")
            .join(archive.digest.as_str().get(..2).unwrap_or_default())
            .join(archive.digest.as_str()),
    )?;
    let away = tempfile::tempdir()?;
    let copy = Storage::open(away.path())?;
    let (target, _) = create(&copy, 5, "Copy").await?;
    let uploaded = copy
        .blobs()
        .put(
            &target,
            Box::pin(std::io::Cursor::new(bytes)),
            1 << 30,
            None,
        )
        .await?;
    copy.backups()
        .restore_import(
            &target,
            uploaded.digest,
            mutation(6),
            vec!["ana".to_owned()],
        )
        .await?;
    Ok((away, copy, target))
}

#[tokio::test]
async fn backup_a_view_restored_into_another_workspace_validates_and_opens_there() -> TestResult {
    let home = tempfile::tempdir()?;
    let origin = Storage::open(home.path())?;
    let (source, noted, archived) = seeded(&origin).await?;
    let view = item(11);
    let before = service(&origin)?
        .get_item(
            &reading(&source, OperationName::GetItem)?,
            GetItemRequest {
                workspace_id: source.workspace_id,
                item_id: view,
                at: At::Latest,
            },
        )
        .await?;

    // The archive, imported into a blank workspace of another installation (SPEC section 12).
    let (_away, copy, target) = imported(&origin, &source).await?;
    assert_ne!(target.workspace_id, source.workspace_id);
    let copied = service(&copy)?;
    let opened = copied
        .get_view(
            &reading(&target, OperationName::GetView)?,
            GetViewRequest {
                workspace_id: target.workspace_id,
                item_id: view,
                at: At::Latest,
            },
        )
        .await?;
    assert_eq!(opened.description, "Where the workshop meets.");
    let binding = some(opened.bindings.first(), "the binding")?;
    assert_eq!(binding.source.workspace_id, target.workspace_id);
    assert_eq!(binding.source.revision, noted);
    opened.require_own_workspace(target.workspace_id, "")?;
    validate(&opened, target.workspace_id)?.require_drawable()?;
    let resolved = copied
        .resolve_view(
            &reading(&target, OperationName::ResolveView)?,
            ResolveViewRequest {
                workspace_id: target.workspace_id,
                item_id: view,
                at: At::Latest,
                refresh_live: false,
            },
        )
        .await?;
    let resolved_binding = some(resolved.resolved_bindings.first(), "the resolved binding")?;
    assert_eq!(resolved_binding.source.workspace_id, target.workspace_id);
    assert_eq!(resolved.charts, Vec::new());
    // At the archive's own head, before the restore commit, the View opens there too.
    let historical = copied
        .get_view(
            &reading(&target, OperationName::GetView)?,
            GetViewRequest {
                workspace_id: target.workspace_id,
                item_id: view,
                at: At::Revision { revision: archived },
            },
        )
        .await?;
    assert_eq!(
        some(historical.bindings.first(), "the binding")?
            .source
            .workspace_id,
        target.workspace_id
    );
    // The file is the archive's: its content digest is unchanged.
    let after = copied
        .get_item(
            &reading(&target, OperationName::GetItem)?,
            GetItemRequest {
                workspace_id: target.workspace_id,
                item_id: view,
                at: At::Latest,
            },
        )
        .await?;
    assert_eq!(after.content_digest, before.content_digest);
    assert_eq!(after.body, before.body);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
