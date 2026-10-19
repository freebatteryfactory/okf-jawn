//! Archives written by an earlier build, kept as fixtures: the restore of them on this build,
//! and the writer that makes them (SPEC section 12: a backup is independently recoverable).
//!
//! `tests/fixtures/archives/records-schema-<n>/` holds one workspace archive and one
//! installation archive that the build at records schema `n` wrote; its `README.md` says which
//! commit wrote them and what they hold. They are never written again: a later build has to
//! restore the bytes an earlier build wrote.
//!
//! The tests here restore them on this build, by replacement into an empty data directory and
//! by import into a blank workspace, and read back what the README says they hold. Every later
//! change to the records schema, or to anything else an archive stores, has to keep them green.
//! What the content is, is in this file's constants: the writer stores them and the tests
//! expect them, so a later schema's fixtures are checked by the same assertions.
//!
//! # Writing the fixtures of a schema
//!
//! Before a change to the records schema, or to anything else an archive stores, run the writer
//! on the build that is about to change (the commit before the change), from the repository
//! root, in PowerShell:
//!
//! ```text
//! $env:OKF_JAWN_FIXTURE_OUT = "$PWD\crates\storage\tests\fixtures\archives"
//! cargo test --locked --all-features -p okf-jawn-storage --test archive_fixtures -- --ignored --nocapture write_archive_fixtures
//! ```
//!
//! It writes `records-schema-<n>/workspace.zip` and `records-schema-<n>/installation.zip` under
//! that directory, `n` being the records `user_version` of the data directory it built, and
//! says on standard error what it wrote. It refuses to replace a fixture that exists. Without
//! the variable the writer writes nothing. Then add the directory's `README.md` and a restore
//! test for it beside the ones here, and make the schema change.
//!
//! Everything the writer stores is synthetic (`fixture-owner`, `fixtures.invalid`): a fixture
//! is a tracked file, and no tracked file holds a personal name, address or machine path.
#![cfg(feature = "runtime")]

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use okf_jawn_contract::access::{AccessRoute, Principal};
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::identity::{Digest, ItemId, Revision, TenantId, WorkspacePath};
use okf_jawn_contract::import::{JobKind, JobState};
use okf_jawn_contract::item::ItemKind;
use okf_jawn_contract::proposal::ProposalStatus;
use okf_jawn_contract::search::LinkDirection;
use okf_jawn_contract::workspace::ArtifactKind;
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::credentials::{CredentialStore, NewConnector};
use okf_jawn_core::drafts::{DraftStore, DraftWrite};
use okf_jawn_core::events::{EventLog, EventQuery, EventScope, NewEvent};
use okf_jawn_core::jobs::{
    DerivedKind, DerivedObject, JobCompletion, JobScope, JobSpec, NewArtifact, NewJob, RecordStore,
};
use okf_jawn_core::proposals::ProposalStore;
use okf_jawn_core::search::{LinkQuery, SearchIndex};
use okf_jawn_core::storage::{
    Backups, BlobStore, CandidateChanges, CandidateCheck, CommitChanges, NewWorkspace, Page,
    Provenance, REPLAY_WARNINGS_UNAVAILABLE, SourceCard, StorageScope, TreeEdit, VersionStore,
    WorkspaceCatalog,
};
use okf_jawn_core::uploads::{NewUpload, UploadStore};
use okf_jawn_storage::Storage;
use serde_json::json;

use check::{TestResult, err_of, some};

/// The identity numbered `n`, of the type the context names.
macro_rules! uuid {
    ($n:expr) => {
        serde_json::from_value(json!(uuid_text($n)))
    };
}

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// Accepts every candidate.
struct Accept;

/// What the writer built before it took the archives.
struct Seeded {
    storage: Storage,
    /// The workspace the workspace archive is of.
    workspace: StorageScope,
    /// A second workspace of the same tenant, which no fixture archive holds.
    second: StorageScope,
    /// The first workspace's head.
    head: Revision,
    /// The candidate of the first workspace's proposal.
    candidate: Revision,
}

/// The fixture archives of one schema and what they say about themselves.
struct Archived {
    /// The workspace archive's file.
    workspace: PathBuf,
    /// The installation archive's file.
    installation: PathBuf,
    /// The tenant and workspace the workspace archive's manifest names.
    scope: StorageScope,
    /// The commit its manifest gives for the accepted line.
    head: Revision,
    /// The installation identity the installation archive holds.
    identity: String,
}

/// The environment variable naming the directory the writer writes under.
const OUT: &str = "OKF_JAWN_FIXTURE_OUT";
/// The directory of the fixtures the last build at records schema 2 wrote.
const SCHEMA_2: &str = "records-schema-2";
/// The directory of the fixtures the last build at records schema 3 wrote.
const SCHEMA_3: &str = "records-schema-3";
/// The archives generated from the retained writer before records schema 5.
const SCHEMA_4: &str = "records-schema-4";
/// The tenant whose blank workspace the import test fills.
const IMPORT_TENANT: &str = "import-tenant";
/// The tenant of everything in the fixtures.
const TENANT: &str = "fixture-tenant";
/// The subject that created, wrote and reviewed everything.
const OWNER: &str = "fixture-owner";
/// A second editor, with a draft of its own; spelled as an address to show the only addresses
/// in a fixture are at `fixtures.invalid`.
const EDITOR: &str = "fixture-editor@fixtures.invalid";
/// The archived workspace's name and description.
const WORKSPACE: (&str, &str) = ("Fixture workspace", "Synthetic content for restore tests.");
/// The second workspace's name, which no row of the archived records may hold.
const SECOND: &str = "Second fixture workspace";
/// The notes at the head: item number, path and body. The harbor note links to the two others;
/// the budget note was written once and then edited.
const NOTES: [(u32, &str, &str); 3] = [
    (
        10,
        "notes/harbor.md",
        "The harbor plan.\n\nSee [budget](budget.md) and the [schedule](../plans/schedule.md).",
    ),
    (11, "notes/budget.md", "The budget for the harbor, revised."),
    (
        12,
        "plans/schedule.md",
        "The schedule follows the harbor plan.",
    ),
];
/// The budget note's body in the first commit, before its edit.
const BUDGET_FIRST: &str = "The budget for the harbor.";
/// The source card: item number, path, title and body.
const CARD: (u32, &str, &str, &str) = (
    13,
    "sources/report-pdf.md",
    "report.pdf",
    "Converted text of the fixture report.",
);
/// The bytes of the card's original object.
const ORIGINAL: &[u8] = b"fixture original bytes";
/// The bytes of the structured export and of the page render the card's conversion record names.
const DERIVATIVES: (&[u8], &[u8]) = (
    b"{\"fixture\": \"structured export\"}",
    b"fixture page render",
);
/// The bytes of the dataset derived from the card.
const DATASET: &[u8] = b"harbor,budget\n1,2\n";
/// The bytes of the retained export the finished job produced.
const EXPORT: &[u8] = b"fixture export bytes";
/// The bytes of the completed upload, and its file name.
const UPLOAD: (&[u8], &str) = (b"fixture upload bytes", "fixture-upload.txt");
/// The note only the proposal's candidate holds: item number, path and body.
const PROPOSED: (u32, &str, &str) = (14, "notes/idea.md", "A proposed idea.");
/// The proposal: its number, title and description.
const PROPOSAL: (u32, &str, &str) = (20, "Fixture proposal", "A proposal kept by the archive.");
/// The proposal's one comment: its identity and text.
const COMMENT: (&str, &str) = ("fixture-comment-1", "A fixture comment.");
/// The draft bodies of `OWNER` and of `EDITOR`, both of the harbor note.
const DRAFTS: (&str, &str) = (
    "The harbor plan, drafted by the owner.",
    "The harbor plan, drafted by the editor.",
);
/// The connector's label.
const CONNECTOR: &str = "Fixture connector";
/// The number of the review.
const REVIEW: u32 = 60;
/// The number of the receipt.
const RECEIPT: u32 = 61;
/// The mutations of the first workspace's commits, in order: the notes and the card, the budget
/// note's edit, the naming rules.
const COMMITS: [u32; 3] = [3, 4, 5];
/// A time for the records that carry one the caller gives.
const WHEN: &str = "2026-10-09T12:00:00.000Z";

impl CandidateCheck for Accept {
    fn check(&self, _root: &Path, _rewritten: &[WorkspacePath]) -> Result<Vec<Warning>, ApiError> {
        Ok(Vec::new())
    }
}

fn uuid_text(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

fn person(subject: &str) -> Provenance {
    Provenance {
        subject: subject.to_owned(),
        route: AccessRoute::LocalOwner,
        client_id: None,
    }
}

fn tenant() -> Fallible<TenantId> {
    Ok(TenantId::try_from(TENANT.to_owned())?)
}

fn note(item: u32, at: &str, body: &str) -> Fallible<TreeEdit> {
    Ok(TreeEdit::CreateItem {
        item_id: uuid!(item)?,
        path: WorkspacePath::try_from(at.to_owned())?,
        title: None,
        type_name: "Note".to_owned(),
        kind: ItemKind::Note,
        body: body.to_owned(),
        properties: BTreeMap::new(),
    })
}

/// The converter a conversion record and its card name.
fn converter() -> serde_json::Value {
    json!({
        "name": "docling",
        "version": "2.3.0",
        "packages": [],
        "settings": {
            "ocr": "auto",
            "table_structure": true,
            "page_images": true,
            "page_image_dpi": 144
        },
        "models": [{ "stage": "layout", "file": "layout.onnx", "size": "1", "sha256": "7".repeat(64) }]
    })
}

/// A conversion record naming the structured export `structured` and the page render `asset`.
fn conversion_record(structured: &Digest, asset: &Digest) -> Fallible<Vec<u8>> {
    Ok(serde_json::to_vec(&json!({
        "converter": converter(),
        "outcome": { "status": "completed" },
        "page_count": 1,
        "structured": [{ "digest": structured.as_str() }],
        "markdown": "8".repeat(64),
        "locations": [{ "lines": { "start": 0, "end": 0 }, "page_no": 1 }],
        "outline": [],
        "tables": [],
        "assets": [{
            "digest": asset.as_str(),
            "media_type": "image/png",
            "role": "page_render",
            "location": { "page_no": 1 }
        }],
        "warnings": []
    }))?)
}

/// The source card over `original` whose completed extraction names the conversion `record`.
fn card(original: &Digest, record: &Digest) -> Fallible<TreeEdit> {
    let (item, path, title, body) = CARD;
    Ok(TreeEdit::WriteSourceCard(Box::new(SourceCard {
        item_id: uuid!(item)?,
        path: WorkspacePath::try_from(path.to_owned())?,
        title: title.to_owned(),
        type_name: "Source".to_owned(),
        body: body.to_owned(),
        properties: BTreeMap::new(),
        appearance: serde_json::from_value(json!({
            "object": original.as_str(),
            "names": [{
                "filename": title,
                "folder": "",
                "observed_at": WHEN,
                "supplied_by": OWNER
            }],
            "media_type": "application/pdf",
            "size": ORIGINAL.len().to_string(),
            "metadata": {},
            "extraction": {
                "outcome": { "status": "completed" },
                "converter": converter(),
                "digest": record.as_str(),
                "page_count": 1,
                "text_origin": "converter",
                "corrected": false,
                "warnings": []
            }
        }))?,
    })))
}

/// The naming rules the first workspace keeps, as the contract spells them.
fn naming_rules() -> serde_json::Value {
    json!({
        "schema_version": 1,
        "rules": [{
            "glob": "notes/**",
            "case": "kebab",
            "separator": "-",
            "prefix": "",
            "date_order": "ymd",
            "date_format": "%Y-%m-%d",
            "strip_version_suffix": false,
            "extension_policy": "lowercase",
            "alias_preservation": true,
            "collision": "reject"
        }]
    })
}

async fn put(storage: &Storage, scope: &StorageScope, bytes: &[u8]) -> Fallible<Digest> {
    Ok(storage
        .blobs()
        .put(
            scope,
            Box::pin(std::io::Cursor::new(bytes.to_vec())),
            1 << 20,
            None,
        )
        .await?
        .digest)
}

async fn create(
    storage: &Storage,
    tenant: &str,
    mutation: u32,
    name: &str,
    description: &str,
) -> Fallible<(StorageScope, Revision)> {
    let tenant = TenantId::try_from(tenant.to_owned())?;
    let created = storage
        .catalog()
        .create(
            &tenant,
            uuid!(mutation)?,
            NewWorkspace {
                name: name.to_owned(),
                description: description.to_owned(),
                creator: person(OWNER),
            },
        )
        .await?;
    Ok((
        StorageScope {
            tenant_id: tenant,
            workspace_id: created.id,
        },
        created.head,
    ))
}

async fn commit(
    storage: &Storage,
    scope: &StorageScope,
    mutation: u32,
    head: &Revision,
    message: &str,
    edits: Vec<TreeEdit>,
) -> Fallible<Revision> {
    Ok(storage
        .versions()
        .commit(
            scope,
            CommitChanges {
                mutation_id: uuid!(mutation)?,
                expected_head: head.clone(),
                author: person(OWNER),
                message: message.to_owned(),
                edits,
            },
            Arc::new(Accept),
        )
        .await?
        .revision)
}

fn count(database: &Path, query: &str) -> Fallible<i64> {
    let connection = rusqlite::Connection::open(database)?;
    Ok(connection.query_row(query, [], |row| row.get(0))?)
}

/// The bytes of the entry `name` of the archive at `path`.
fn entry(path: &Path, name: &str) -> Fallible<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let mut bytes = Vec::new();
    archive.by_name(name)?.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// The two workspaces and the first one's history: three notes in two folders, a source card
/// over an original with a conversion record, an edit, the naming rules, and a proposal's
/// candidate.
async fn seed_history(directory: &Path) -> Fallible<Seeded> {
    let storage = Storage::open(directory)?;
    let (workspace, created) = create(&storage, TENANT, 1, WORKSPACE.0, WORKSPACE.1).await?;
    let (second, second_head) = create(&storage, TENANT, 2, SECOND, "").await?;
    let original = put(&storage, &workspace, ORIGINAL).await?;
    let structured = put(&storage, &workspace, DERIVATIVES.0).await?;
    let asset = put(&storage, &workspace, DERIVATIVES.1).await?;
    let record = put(
        &storage,
        &workspace,
        &conversion_record(&structured, &asset)?,
    )
    .await?;
    let mut edits = Vec::new();
    for (item, path, body) in NOTES {
        let body = if item == 11 { BUDGET_FIRST } else { body };
        edits.push(note(item, path, body)?);
    }
    edits.push(card(&original, &record)?);
    let [first, revise, rule] = COMMITS;
    let written = commit(&storage, &workspace, first, &created, "Seed", edits).await?;
    let &(budget, _, revised) = some(NOTES.get(1), "the budget note")?;
    let edit = TreeEdit::EditItem {
        item_id: uuid!(budget)?,
        body: revised.to_owned(),
        properties: BTreeMap::new(),
    };
    let edited = commit(&storage, &workspace, revise, &written, "Revise", vec![edit]).await?;
    let rules = TreeEdit::SetRules {
        rules: serde_json::from_value(naming_rules())?,
    };
    let head = commit(&storage, &workspace, rule, &edited, "Rules", vec![rules]).await?;
    let (proposal, _, _) = PROPOSAL;
    let (item, path, body) = PROPOSED;
    let candidate = storage
        .versions()
        .create_candidate(
            &workspace,
            uuid!(proposal)?,
            CandidateChanges {
                mutation_id: uuid!(6)?,
                base: head.clone(),
                author: person(EDITOR),
                message: "Propose".to_owned(),
                edits: vec![note(item, path, body)?],
            },
            Arc::new(Accept),
        )
        .await?
        .revision;
    let marked = "Content of the second workspace.";
    let elsewhere = vec![note(15, "notes/elsewhere.md", marked)?];
    commit(&storage, &second, 7, &second_head, "Seed", elsewhere).await?;
    Ok(Seeded {
        storage,
        workspace,
        second,
        head,
        candidate,
    })
}

/// The first workspace's drafts, derived dataset and completed upload.
async fn seed_content_records(seeded: &Seeded) -> TestResult {
    let (storage, workspace) = (&seeded.storage, &seeded.workspace);
    for (mutation, editor, body) in [(20_u32, OWNER, DRAFTS.0), (21, EDITOR, DRAFTS.1)] {
        storage
            .drafts()
            .save(
                workspace,
                uuid!(mutation)?,
                DraftWrite {
                    item_id: uuid!(10)?,
                    editor: editor.to_owned(),
                    base_revision: seeded.head.clone(),
                    body: body.to_owned(),
                    properties: BTreeMap::new(),
                    content_digest: Digest::try_from("e".repeat(64))?,
                },
            )
            .await?;
    }
    let dataset = put(storage, workspace, DATASET).await?;
    storage
        .records()
        .record_derived_object(
            workspace,
            DerivedObject {
                item_id: uuid!(CARD.0)?,
                revision: seeded.head.clone(),
                digest: dataset,
                kind: DerivedKind::Dataset,
                media_type: "text/csv".to_owned(),
            },
        )
        .await?;
    let (bytes, filename) = UPLOAD;
    let uploaded = put(storage, workspace, bytes).await?;
    let uploads = storage.uploads();
    let slot = uploads
        .create(
            workspace,
            uuid!(22)?,
            NewUpload {
                filename: filename.to_owned(),
                relative_path: String::new(),
                expected_size: u64::try_from(bytes.len())?,
                expected_sha256: None,
                supplied_by: person(OWNER),
            },
        )
        .await?;
    uploads
        .put_content(
            workspace,
            slot.id,
            Box::pin(std::io::Cursor::new(bytes.to_vec())),
            1 << 20,
        )
        .await?;
    uploads.complete(workspace, slot.id, uploaded).await?;
    Ok(())
}

/// A finished export job of the first workspace, with its retained export as its artifact and
/// its output.
async fn seed_job(seeded: &Seeded) -> TestResult {
    let (storage, workspace) = (&seeded.storage, &seeded.workspace);
    let scope = JobScope::Workspace(workspace.clone());
    let records = storage.records();
    let job = records
        .create_job(
            &scope,
            NewJob {
                mutation_id: uuid!(30)?,
                initiator: person(OWNER),
                spec: JobSpec::ExportWorkspace {
                    revision: seeded.head.clone(),
                    include_history: false,
                },
            },
        )
        .await?;
    let claimed = some(records.claim_job(job.id).await?, "the claimed job")?;
    let export = storage
        .blobs()
        .put(
            workspace,
            Box::pin(std::io::Cursor::new(EXPORT.to_vec())),
            1 << 20,
            None,
        )
        .await?;
    let artifact = records
        .record_artifact(
            &scope,
            uuid!(31)?,
            NewArtifact {
                kind: ArtifactKind::Export,
                object: export.clone(),
                media_type: "application/zip".to_owned(),
                created_by_job: job.id,
            },
        )
        .await?;
    records
        .complete_job(JobCompletion {
            lease: claimed.lease,
            revision: None,
            item_ids: Vec::new(),
            artifact: Some(artifact.id),
            restore: None,
            backup: None,
            clean_up: None,
            outputs: vec![export.digest],
            warnings: Vec::new(),
        })
        .await?;
    Ok(())
}

/// The first workspace's review and receipt of the harbor note, and an event about it.
async fn seed_review(seeded: &Seeded) -> TestResult {
    let (storage, workspace) = (&seeded.storage, &seeded.workspace);
    let (harbor, path, _) = some(NOTES.first(), "the harbor note")?;
    let source = json!({
        "workspace_id": workspace.workspace_id,
        "item_id": uuid_text(*harbor),
        "path": path,
        "revision": seeded.head,
        "selection": { "kind": "all" }
    });
    let review = serde_json::from_value(json!({
        "id": uuid_text(REVIEW),
        "source": source,
        "content_digest": "d".repeat(64),
        "reviewer_subject": OWNER,
        "reviewed_at": WHEN,
        "coverage": "current"
    }))?;
    let records = storage.records();
    records.insert_review(workspace, uuid!(40)?, review).await?;
    let receipt = serde_json::from_value(json!({
        "id": uuid_text(RECEIPT),
        "workspace_id": workspace.workspace_id,
        "operation_id": "get_item",
        "principal_subject": OWNER,
        "route": "local_owner",
        "sources": [source],
        "returned_at": WHEN,
        "audience": "human_display"
    }))?;
    records
        .insert_receipt(workspace, Some(uuid!(41)?), receipt)
        .await?;
    let changed = NewEvent {
        kind: EventKind::Changed,
        revision: Some(seeded.head.clone()),
        item_id: Some(uuid!(*harbor)?),
        job_id: None,
        connector_id: None,
        actor: None,
        operation: None,
        transport: None,
        proposal_id: None,
        count: None,
    };
    storage
        .events()
        .append(&EventScope::Workspace(workspace.clone()), None, changed)
        .await?;
    Ok(())
}

/// The first workspace's open proposal with its comment, and the second editor's grant.
async fn seed_proposal(seeded: &Seeded) -> TestResult {
    let (storage, workspace) = (&seeded.storage, &seeded.workspace);
    let (number, title, description) = PROPOSAL;
    let proposal = serde_json::from_value(json!({
        "id": uuid_text(number),
        "workspace_id": workspace.workspace_id,
        "base_revision": seeded.head,
        "proposal_revision": seeded.candidate,
        "content_digest": "d".repeat(64),
        "title": title,
        "description": description,
        "changes": [],
        "kind": "content",
        "status": "open",
        "created_by": EDITOR,
        "created_via": "mcp_delegation",
        "created_at": WHEN
    }))?;
    let proposals = storage.proposals();
    let open = proposals.insert(workspace, uuid!(42)?, proposal).await?;
    let comment = serde_json::from_value(json!({
        "id": COMMENT.0,
        "author": OWNER,
        "text": COMMENT.1,
        "created_at": WHEN
    }))?;
    proposals
        .add_comment(workspace, uuid!(43)?, open.id, comment)
        .await?;
    let editor = Principal {
        subject: EDITOR.to_owned(),
        tenant_id: tenant()?,
        route: AccessRoute::LocalOwner,
        client_id: None,
        delegation: None,
    };
    storage
        .access()
        .grant_creator(&editor, workspace.workspace_id)
        .await?;
    Ok(())
}

/// What only the installation archive carries: the installation's identity, a connector that
/// may read both workspaces, and a tenant event.
async fn seed_installation(seeded: &Seeded) -> TestResult {
    let storage = &seeded.storage;
    storage.credentials().installation_identity().await?;
    storage
        .credentials()
        .create_connector(
            uuid!(50)?,
            NewConnector {
                label: CONNECTOR.to_owned(),
                workspace_ids: vec![seeded.workspace.workspace_id, seeded.second.workspace_id],
                allow_propose: true,
            },
        )
        .await?;
    let signed_in = NewEvent {
        kind: EventKind::SignedIn,
        revision: None,
        item_id: None,
        job_id: None,
        connector_id: None,
        actor: None,
        operation: None,
        transport: None,
        proposal_id: None,
        count: None,
    };
    storage
        .events()
        .append(&EventScope::Tenant(tenant()?), None, signed_in)
        .await?;
    Ok(())
}

/// Take the two archives of `seeded` and write them under `target`; returns their paths, the
/// workspace archive first.
async fn take_archives(seeded: &Seeded, target: &Path) -> Fallible<(PathBuf, PathBuf)> {
    let (storage, backups) = (&seeded.storage, seeded.storage.backups());
    let paths = (
        target.join("workspace.zip"),
        target.join("installation.zip"),
    );
    for path in [&paths.0, &paths.1] {
        if path.exists() {
            return Err(format!(
                "{} exists: a fixture an earlier build wrote is never written again",
                path.display()
            )
            .into());
        }
    }
    std::fs::create_dir_all(target)?;
    let archive = backups
        .write_workspace_archive(&seeded.workspace, uuid!(70)?)
        .await?
        .object;
    let stored = storage
        .data()
        .objects_path()
        .join(TENANT)
        .join(archive.digest.as_str().get(..2).unwrap_or_default())
        .join(archive.digest.as_str());
    std::fs::copy(stored, &paths.0)?;
    let tenant = tenant()?;
    let object = backups
        .write_installation_archive(&tenant, uuid!(71)?)
        .await?;
    let mut read = backups
        .open_installation_archive(&tenant, &object.digest, 0, object.size)
        .await?;
    let mut bytes = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut read.body, &mut bytes).await?;
    std::fs::write(&paths.1, &bytes)?;
    Ok(paths)
}

/// Say on standard error what was written: the versions the README of a fixture directory
/// states.
fn describe(directory: &Path, schema: i64, paths: &(PathBuf, PathBuf)) -> TestResult {
    let layout = std::fs::read_to_string(directory.join("format.json"))?;
    let workspace: serde_json::Value = serde_json::from_slice(&entry(&paths.0, "manifest.json")?)?;
    let installation: serde_json::Value =
        serde_json::from_slice(&entry(&paths.1, "installation.json")?)?;
    let scratch = tempfile::tempdir()?;
    let archived = scratch.path().join("records.sqlite");
    std::fs::write(&archived, entry(&paths.0, "records.sqlite")?)?;
    eprintln!("records user_version of the data directory: {schema}");
    eprintln!("data directory format.json: {}", layout.trim());
    for (path, manifest) in [(&paths.0, &workspace), (&paths.1, &installation)] {
        eprintln!(
            "{}: {} bytes, format {}, version {}",
            path.display(),
            std::fs::metadata(path)?.len(),
            manifest.get("format").unwrap_or(&serde_json::Value::Null),
            manifest.get("version").unwrap_or(&serde_json::Value::Null),
        );
    }
    eprintln!(
        "records user_version inside the workspace archive: {}",
        count(&archived, "PRAGMA user_version")?
    );
    eprintln!(
        "workspace {} of tenant {}, references {}",
        workspace
            .get("workspace_id")
            .unwrap_or(&serde_json::Value::Null),
        workspace
            .get("tenant_id")
            .unwrap_or(&serde_json::Value::Null),
        workspace
            .get("references")
            .unwrap_or(&serde_json::Value::Null),
    );
    Ok(())
}

/// The writer (see this file's header): builds a data directory on this build and writes its
/// two archives under `OKF_JAWN_FIXTURE_OUT`, in `records-schema-<n>`. Without the variable it
/// writes nothing.
#[tokio::test]
#[ignore = "writes fixtures; run it by name with OKF_JAWN_FIXTURE_OUT set (see the file header)"]
async fn write_archive_fixtures() -> TestResult {
    let Some(out) = std::env::var_os(OUT) else {
        eprintln!("{OUT} is not set: no fixture was written");
        return Ok(());
    };
    let directory = tempfile::tempdir()?;
    let seeded = seed_history(directory.path()).await?;
    seed_content_records(&seeded).await?;
    seed_job(&seeded).await?;
    seed_review(&seeded).await?;
    seed_proposal(&seeded).await?;
    seed_installation(&seeded).await?;
    let schema = count(&seeded.storage.data().records_path(), "PRAGMA user_version")?;
    let target = PathBuf::from(out).join(format!("records-schema-{schema}"));
    let paths = take_archives(&seeded, &target).await?;
    let held = String::from_utf8_lossy(&entry(&paths.0, "records.sqlite")?).into_owned();
    assert!(
        !held.contains(SECOND),
        "the workspace archive holds nothing of the second workspace"
    );
    describe(directory.path(), schema, &paths)
}

/// The path of the fixture `name` in the fixture directory `schema`.
///
/// A fixture cannot be made again (the build that wrote it is gone from the tree), so a missing
/// one is reported by name, with what to do about it.
fn fixture(schema: &str, name: &str) -> Fallible<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/archives")
        .join(schema)
        .join(name);
    if path.is_file() {
        return Ok(path);
    }
    Err(format!(
        "the archive fixture {schema}/{name} is missing (looked for {}): an earlier build \
         wrote it and it cannot be written again, so restore the file from Git",
        path.display()
    )
    .into())
}

/// The two fixture archives of `schema` and what they say about themselves.
fn archived(schema: &str) -> Fallible<Archived> {
    let workspace = fixture(schema, "workspace.zip")?;
    let installation = fixture(schema, "installation.zip")?;
    let manifest: serde_json::Value = serde_json::from_slice(&entry(&workspace, "manifest.json")?)?;
    let field = |name: &str| some(manifest.get(name), name).cloned();
    let references: Vec<(String, String)> = serde_json::from_value(field("references")?)?;
    let head = some(
        references
            .iter()
            .find(|(name, _)| name == "refs/heads/main"),
        "the archive's head",
    )?;
    let level: serde_json::Value =
        serde_json::from_slice(&entry(&installation, "installation.json")?)?;
    let identity = some(
        level
            .pointer("/identity/0")
            .and_then(serde_json::Value::as_str),
        "the installation identity",
    )?;
    Ok(Archived {
        scope: StorageScope {
            tenant_id: serde_json::from_value(field("tenant_id")?)?,
            workspace_id: serde_json::from_value(field("workspace_id")?)?,
        },
        head: serde_json::from_value(json!(head.1))?,
        identity: identity.to_owned(),
        workspace,
        installation,
    })
}

/// The records `user_version` of the workspace archive at `path`, and how many tables named
/// `commit_warnings` its records hold.
fn archived_schema(path: &Path) -> Fallible<(i64, i64)> {
    let scratch = tempfile::tempdir()?;
    let records = scratch.path().join("records.sqlite");
    std::fs::write(&records, entry(path, "records.sqlite")?)?;
    Ok((
        count(&records, "PRAGMA user_version")?,
        count(
            &records,
            "SELECT count(*) FROM sqlite_schema WHERE name = 'commit_warnings'",
        )?,
    ))
}

/// The lower-case hexadecimal SHA-256 of `bytes`.
fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    use std::fmt::Write as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// The digest at the JSON Pointer `pointer` of `value`.
fn digest_at(value: &serde_json::Value, pointer: &str) -> Fallible<Digest> {
    let text = some(
        value.pointer(pointer).and_then(serde_json::Value::as_str),
        pointer,
    )?;
    Ok(Digest::try_from(text.to_owned())?)
}

/// The codes of `warnings`, in order.
fn codes(warnings: &[Warning]) -> Vec<&str> {
    warnings
        .iter()
        .map(|warning| warning.code.as_str())
        .collect()
}

/// A page of up to twenty, from the start.
const fn page() -> Page {
    Page {
        cursor: None,
        limit: 20,
    }
}

/// Every byte of the object `digest` of `scope`.
async fn read(storage: &Storage, scope: &StorageScope, digest: &Digest) -> Fallible<Vec<u8>> {
    let mut opened = storage.blobs().open(scope, digest, 0, u64::MAX).await?;
    let mut bytes = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut opened.body, &mut bytes).await?;
    Ok(bytes)
}

/// The notes as the archive's head holds them, the harbor note's two links, and the naming
/// rules.
async fn assert_notes(storage: &Storage, scope: &StorageScope, head: &Revision) -> TestResult {
    let versions = storage.versions();
    for (item, path, body) in NOTES {
        let shown = versions.show(scope, head, uuid!(item)?).await?;
        assert_eq!(shown.body, body, "the body of {path} at the head");
        assert_eq!(
            shown.summary.path,
            WorkspacePath::try_from(path.to_owned())?
        );
    }
    let search = storage.search();
    search.index_revision(scope, head.clone()).await?;
    let outgoing = search
        .links(
            scope,
            LinkQuery {
                revision: head.clone(),
                item_id: uuid!(10)?,
                direction: LinkDirection::Outgoing,
                page: page(),
            },
        )
        .await?;
    let targets: Vec<Option<ItemId>> = outgoing.links.iter().map(|link| link.to).collect();
    let (budget, schedule): (ItemId, ItemId) = (uuid!(11)?, uuid!(12)?);
    assert_eq!(targets.len(), 2, "the harbor note's two links: {targets:?}");
    assert!(
        targets.contains(&Some(budget)),
        "the link to the budget note in its own folder"
    );
    assert!(
        targets.contains(&Some(schedule)),
        "the link to the schedule in the other folder"
    );
    let rules = some(versions.rules(scope, head).await?, "the naming rules")?;
    let parsed = rules
        .parsed
        .map_err(|error| format!("the archive's naming rules do not read: {error:?}"))?;
    assert_eq!(serde_json::to_value(parsed)?, naming_rules());
    Ok(())
}

/// The source card, its original's bytes, what its conversion record names, and the dataset
/// derived from it.
async fn assert_card(storage: &Storage, scope: &StorageScope, head: &Revision) -> TestResult {
    let (item, _, title, body) = CARD;
    let shown = storage.versions().show(scope, head, uuid!(item)?).await?;
    assert_eq!(
        (shown.summary.title.as_str(), shown.body.as_str()),
        (title, body)
    );
    let appearance = serde_json::to_value(some(shown.source, "the card's appearance")?)?;
    let original = digest_at(&appearance, "/object")?;
    assert_eq!(original.as_str(), sha256(ORIGINAL));
    assert_eq!(
        read(storage, scope, &original).await?,
        ORIGINAL,
        "the original object reads with its bytes"
    );
    let record = digest_at(&appearance, "/extraction/digest")?;
    let record: serde_json::Value = serde_json::from_slice(&read(storage, scope, &record).await?)?;
    let structured = digest_at(&record, "/structured/0/digest")?;
    assert_eq!(read(storage, scope, &structured).await?, DERIVATIVES.0);
    let asset = digest_at(&record, "/assets/0/digest")?;
    assert_eq!(read(storage, scope, &asset).await?, DERIVATIVES.1);
    let dataset = Digest::try_from(sha256(DATASET))?;
    let derived = storage
        .records()
        .derived_object(scope, uuid!(item)?, head, &dataset)
        .await?;
    assert_eq!(
        some(derived, "the derived dataset's record")?.media_type,
        "text/csv"
    );
    assert_eq!(read(storage, scope, &dataset).await?, DATASET);
    Ok(())
}

/// The owner's draft, the review, the receipt, and the proposal with its candidate and its
/// comment, each naming the workspace it is now in.
async fn assert_discussion(storage: &Storage, scope: &StorageScope, head: &Revision) -> TestResult {
    let draft = storage.drafts().get(scope, uuid!(10)?, OWNER).await?;
    let draft = some(draft, "the owner's draft")?;
    assert_eq!(draft.body, DRAFTS.0);
    assert_eq!(&draft.draft.base_revision, head);
    let records = storage.records();
    let reviews = records.list_reviews(scope, uuid!(10)?).await?;
    let review = some(reviews.first(), "the review")?;
    assert_eq!(reviews.len(), 1);
    assert_eq!(
        serde_json::to_value(review.id)?,
        json!(uuid_text(REVIEW)),
        "the review keeps its identity"
    );
    assert_eq!(review.reviewer_subject, OWNER);
    assert_eq!(review.source.workspace_id, scope.workspace_id);
    assert_eq!(&review.source.revision, head);
    let receipt = records.get_receipt(scope, uuid!(RECEIPT)?).await?;
    assert_eq!(receipt.principal_subject, OWNER);
    assert_eq!(receipt.workspace_id, scope.workspace_id);
    let (number, title, description) = PROPOSAL;
    let proposals = storage.proposals();
    let proposal = proposals.get(scope, uuid!(number)?).await?;
    assert_eq!(
        (proposal.title.as_str(), proposal.description.as_str()),
        (title, description)
    );
    assert_eq!(proposal.status, ProposalStatus::Open);
    assert_eq!(proposal.created_by, EDITOR);
    assert_eq!(proposal.workspace_id, scope.workspace_id);
    assert_eq!(&proposal.base_revision, head);
    let (item, _, body) = PROPOSED;
    let proposed = storage
        .versions()
        .show(scope, &proposal.proposal_revision, uuid!(item)?)
        .await?;
    assert_eq!(proposed.body, body, "the proposal's candidate is kept");
    let comments = proposals.list_comments(scope, proposal.id, page()).await?;
    let comment = some(comments.items.first(), "the proposal's comment")?;
    assert_eq!(comments.items.len(), 1);
    assert_eq!(
        (
            comment.id.as_str(),
            comment.author.as_str(),
            comment.text.as_str()
        ),
        (COMMENT.0, OWNER, COMMENT.1)
    );
    Ok(())
}

/// A commit the archive brought replays with the one warning that says its check's warnings are
/// not available: no archive carries the kept warnings (and this one was written before the
/// store kept any). The earlier revision still reads as it was.
async fn assert_history(storage: &Storage, scope: &StorageScope) -> TestResult {
    let [seed, _, _] = COMMITS;
    let versions = storage.versions();
    let found = versions.find_commit(scope, uuid!(seed)?, None).await?;
    let found = some(found, "the archive's first content commit")?;
    assert!(found.replayed);
    assert_eq!(codes(&found.warnings), [REPLAY_WARNINGS_UNAVAILABLE]);
    let before = versions.show(scope, &found.revision, uuid!(11)?).await?;
    assert_eq!(before.body, BUDGET_FIRST, "the budget note before its edit");
    Ok(())
}

/// What both restores bring back, read at the archive's head, which each of them keeps.
async fn assert_content(storage: &Storage, scope: &StorageScope, head: &Revision) -> TestResult {
    assert_notes(storage, scope, head).await?;
    assert_card(storage, scope, head).await?;
    assert_discussion(storage, scope, head).await?;
    assert_history(storage, scope).await
}

/// What only a replacement restore brings back: the finished job with its retained export, the
/// completed upload, the second editor's draft and grant, and the workspace's event.
async fn assert_replaced_records(storage: &Storage, scope: &StorageScope) -> TestResult {
    let jobs = storage
        .records()
        .list_jobs(&JobScope::Workspace(scope.clone()), page())
        .await?;
    let job = some(jobs.items.first(), "the job")?;
    assert_eq!(jobs.items.len(), 1);
    assert_eq!(
        (job.kind, &job.state),
        (JobKind::ExportWorkspace, &JobState::Succeeded)
    );
    let artifact = some(job.artifact.as_ref(), "the job's artifact")?;
    assert_eq!(artifact.sha256.as_str(), sha256(EXPORT));
    assert_eq!(read(storage, scope, &artifact.sha256).await?, EXPORT);
    let (bytes, filename) = UPLOAD;
    let uploaded = Digest::try_from(sha256(bytes))?;
    assert_eq!(read(storage, scope, &uploaded).await?, bytes);
    let live = storage.data().records_path();
    assert_eq!(
        count(
            &live,
            &format!(
                "SELECT count(*) FROM uploads WHERE object_digest = '{}' \
                 AND filename = '{filename}'",
                uploaded.as_str()
            )
        )?,
        1,
        "the completed upload's slot"
    );
    let draft = storage.drafts().get(scope, uuid!(10)?, EDITOR).await?;
    assert_eq!(some(draft, "the editor's draft")?.body, DRAFTS.1);
    let editors = storage.access().editors(scope).await?;
    assert!(
        editors.iter().any(|subject| subject == EDITOR),
        "the editor's grant: {editors:?}"
    );
    let harbor: ItemId = uuid!(10)?;
    let events = storage
        .events()
        .list(
            &EventScope::Workspace(scope.clone()),
            EventQuery {
                after: None,
                page: page(),
            },
        )
        .await?;
    assert!(
        events
            .events
            .iter()
            .any(|event| event.kind == EventKind::Changed && event.item_id == Some(harbor)),
        "the workspace's event"
    );
    Ok(())
}

/// What the installation archive brings back: the installation's identity, the connector
/// (revoked, since no secret is kept) and the tenant's event.
async fn assert_installation(storage: &Storage, archived: &Archived) -> TestResult {
    let credentials = storage.credentials();
    let identity = credentials.installation_identity().await?;
    assert_eq!(identity.subject, archived.identity);
    let connectors = credentials.list_connectors(true).await?;
    let connector = some(connectors.first(), "the connector")?;
    assert_eq!((connectors.len(), connector.label.as_str()), (1, CONNECTOR));
    assert!(
        connector
            .workspace_ids
            .contains(&archived.scope.workspace_id),
        "the connector's metadata names the workspace"
    );
    assert!(
        connector.revoked_at.is_some(),
        "a connector comes back revoked"
    );
    assert!(credentials.list_connectors(false).await?.is_empty());
    let events = storage
        .events()
        .list(
            &EventScope::Tenant(archived.scope.tenant_id.clone()),
            EventQuery {
                after: None,
                page: page(),
            },
        )
        .await?;
    assert!(
        events
            .events
            .iter()
            .any(|event| event.kind == EventKind::SignedIn),
        "the tenant's event"
    );
    Ok(())
}

/// The fixture of the last build at records schema 2 is what its README says: records at
/// `user_version` 2, with no table of kept warnings.
#[test]
fn the_schema_2_workspace_archive_holds_records_at_schema_2() -> TestResult {
    let archived = archived(SCHEMA_2)?;
    assert_eq!(archived_schema(&archived.workspace)?, (2, 0));
    Ok(())
}

/// A fixture that is not there is reported by name, with what to do.
#[test]
fn a_missing_archive_fixture_is_reported_by_name() -> TestResult {
    let error = err_of(fixture(SCHEMA_2, "absent.zip"))?.to_string();
    assert!(
        error.starts_with("the archive fixture records-schema-2/absent.zip is missing"),
        "{error}"
    );
    assert!(error.contains("restore the file from Git"), "{error}");
    Ok(())
}

/// The fixture of the last build at records schema 3 is what its README says: records at
/// `user_version` 3, with the table of kept warnings.
#[test]
fn the_schema_3_workspace_archive_holds_records_at_schema_3() -> TestResult {
    let archived = archived(SCHEMA_3)?;
    assert_eq!(archived_schema(&archived.workspace)?, (3, 1));
    Ok(())
}

/// The archives the build at records schema 2 wrote restore by replacement into an empty data
/// directory on this build: the installation archive, then the workspace archive of its
/// tenant, with everything they hold.
#[tokio::test]
async fn the_schema_2_archives_restore_by_replacement_into_an_empty_directory() -> TestResult {
    restores_by_replacement(SCHEMA_2, 2).await
}

/// The archives the build at records schema 3 wrote restore by replacement, as schema 2's do.
#[tokio::test]
async fn the_schema_3_archives_restore_by_replacement_into_an_empty_directory() -> TestResult {
    restores_by_replacement(SCHEMA_3, 3).await
}

/// The workspace archive the build at records schema 2 wrote restores by import into a blank
/// workspace of another tenant, in another data directory, on this build. An import carries
/// the content and the records about it; it gives a draft back only to an editor it is told
/// of, and carries no job.
#[tokio::test]
async fn the_schema_2_workspace_archive_restores_by_import_into_a_blank_workspace() -> TestResult {
    restores_by_import(SCHEMA_2).await
}

/// The workspace archive the build at records schema 3 wrote restores by import, as schema 2's
/// does.
#[tokio::test]
async fn the_schema_3_workspace_archive_restores_by_import_into_a_blank_workspace() -> TestResult {
    restores_by_import(SCHEMA_3).await
}

/// SPEC section 12: the fixed pre-V5 archive retains records schema 4 and its job seal.
/// Expected values come from the released `RECORDS_V4` declaration, not the current writer.
#[test]
fn the_schema_4_workspace_archive_holds_records_at_schema_4() -> TestResult {
    let archived = archived(SCHEMA_4)?;
    assert_eq!(archived_schema(&archived.workspace)?, (4, 1));
    let scratch = tempfile::tempdir()?;
    let records = scratch.path().join("records.sqlite");
    std::fs::write(&records, entry(&archived.workspace, "records.sqlite")?)?;
    assert_eq!(
        count(
            &records,
            "SELECT count(*) FROM pragma_table_info('jobs') WHERE name = 'sealed'"
        )?,
        1,
        "schema 4 keeps the released job-seal column"
    );
    Ok(())
}

/// SPEC section 12: restore all independently archived schema-4 content and records.
#[tokio::test]
async fn the_schema_4_archives_restore_by_replacement_into_an_empty_directory() -> TestResult {
    restores_by_replacement(SCHEMA_4, 4).await
}

/// SPEC section 12: import fixed schema-4 bytes into another tenant's blank workspace.
#[tokio::test]
async fn the_schema_4_workspace_archive_restores_by_import_into_a_blank_workspace() -> TestResult {
    restores_by_import(SCHEMA_4).await
}

/// The archives of `schema`, written at records `user_version` `version`, restore by
/// replacement into an empty data directory on this build: the installation archive, then the
/// workspace archive of its tenant, with everything they hold.
async fn restores_by_replacement(schema: &str, version: i64) -> TestResult {
    let archived = archived(schema)?;
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    // This build's schema: a new data directory is created at it.
    let latest = count(&storage.data().records_path(), "PRAGMA user_version")?;
    let reports = storage
        .backups()
        .restore_replacement(
            &archived.installation,
            std::slice::from_ref(&archived.workspace),
        )
        .await?;
    let report = some(reports.first(), "the restore report")?;
    assert_eq!(
        (
            report.items,
            report.drafts_restored,
            report.drafts_unassigned
        ),
        (4, 2, 0)
    );
    let scope = &archived.scope;
    let listed = storage.catalog().list(&scope.tenant_id, true).await?;
    let shown = some(listed.first(), "the restored workspace")?;
    assert_eq!(listed.len(), 1, "the one workspace the archives hold");
    assert_eq!(
        (shown.id, shown.name.as_str(), shown.description.as_str()),
        (scope.workspace_id, WORKSPACE.0, WORKSPACE.1)
    );
    assert_eq!(shown.head, archived.head, "the head is the archive's");
    assert_content(&storage, scope, &archived.head).await?;
    assert_replaced_records(&storage, scope).await?;
    assert_installation(&storage, &archived).await?;
    let live = storage.data().records_path();
    assert!(latest >= version, "this build reads schema {version}");
    assert_eq!(
        count(&live, "PRAGMA user_version")?,
        latest,
        "the restored records are at this build's schema, whatever the archive's ({version})"
    );
    assert_eq!(
        count(&live, "SELECT count(*) FROM commit_warnings")?,
        0,
        "an archive brings no kept warnings"
    );
    Ok(())
}

/// The workspace archive of `schema` restores by import into a blank workspace of another
/// tenant, in another data directory, on this build. An import carries the content and the
/// records about it; it gives a draft back only to an editor it is told of, and carries no
/// job.
async fn restores_by_import(schema: &str) -> TestResult {
    let archived = archived(schema)?;
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, IMPORT_TENANT, 80, "Imported", "").await?;
    let uploaded = storage
        .blobs()
        .put(
            &target,
            Box::pin(std::io::Cursor::new(std::fs::read(&archived.workspace)?)),
            1 << 30,
            None,
        )
        .await?;
    let report = storage
        .backups()
        .restore_import(&target, uploaded.digest, uuid!(84)?, vec![OWNER.to_owned()])
        .await?;
    assert_eq!(
        (
            report.items,
            report.drafts_restored,
            report.drafts_unassigned
        ),
        (4, 1, 1)
    );
    let versions = storage.versions();
    let head = versions.head(&target).await?;
    let written = versions.find_commit(&target, uuid!(84)?, None).await?;
    assert_eq!(some(written, "the import's commit")?.revision, head);
    assert_ne!(head, archived.head, "the import's commit is on top");
    for (item, path, body) in NOTES {
        let shown = versions.show(&target, &head, uuid!(item)?).await?;
        assert_eq!(shown.body, body, "the body of {path} at the new head");
    }
    assert_content(&storage, &target, &archived.head).await?;
    let unassigned = storage.drafts().get(&target, uuid!(10)?, EDITOR).await?;
    assert!(
        unassigned.is_none(),
        "a draft of an editor the import was not told of stays in the archive"
    );
    let jobs = storage
        .records()
        .list_jobs(&JobScope::Workspace(target.clone()), page())
        .await?;
    assert!(jobs.items.is_empty(), "an import carries no job");
    assert!(count(&storage.data().records_path(), "PRAGMA user_version")? >= 3);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
