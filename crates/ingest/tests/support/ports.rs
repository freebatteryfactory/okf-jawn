//! Minimal fakes of the ports the handler calls: storage's, the converter and the check.
//!
//! Each fake answers only the methods the handler calls, encodes only what the port docs state
//! (the commit replay rule, upload consumption by one job), and records the calls so a test can
//! see that a repeated run repeated no effect. Every other method answers `NotImplemented`, so
//! a handler that starts calling one fails loudly instead of being answered with an invented
//! value.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use okf_jawn_contract::common::{PageRange, Warning};
use okf_jawn_contract::conventions::NamingRules;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::{ConverterIdentity, FailureReason};
use okf_jawn_contract::history::{BlameResponse, DiffResponse, LogResponse};
use okf_jawn_contract::identity::{
    Digest, ItemId, JobId, MutationId, PurgeId, Revision, TenantId, UploadId, WorkspaceId,
    WorkspacePath,
};
use okf_jawn_contract::item::{ItemDocument, TypeDefinition};
use okf_jawn_contract::purge::PurgeReport;
use okf_jawn_contract::search::{GetGraphResponse, GetLinksResponse, SearchResponse};
use okf_jawn_contract::workspace::RestoreReport;
use okf_jawn_core::conversion::{
    Conversion, ConversionInput, ConversionStatus, ConvertedDocument, Converter, ConverterLimits,
    WindowCoverage,
};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::search::{GraphQuery, LinkQuery, SearchIndex, SearchQuery};
use okf_jawn_core::storage::{
    Backups, BlameQuery, BlobStore, ByteReader, CandidateChanges, CandidateCheck, CommitChanges,
    Committed, DiffQuery, FolderListing, LocalSource, LogQuery, ObjectInfo, ObjectRead, Page,
    Promotion, Purger, StorageScope, VersionStore,
};
use okf_jawn_core::uploads::{NewUpload, UploadRecord, UploadStore};

/// A `VersionStore` that keeps a line of commits and nothing of their trees.
///
/// `commit` encodes the port doc: a commit carrying the same mutation after the expected head
/// is returned as `replayed`; otherwise a head that moved is `Conflict`. Folders list empty.
pub struct FakeVersions {
    /// The head before any commit.
    pub head: Revision,
    /// Commits written, in order, with the revision each made.
    pub commits: Mutex<Vec<(CommitChanges, Revision)>>,
}

/// An `UploadStore` holding complete uploads and recording which job consumed each.
#[derive(Default)]
pub struct FakeUploads {
    /// The slots `get` returns.
    pub records: Mutex<Vec<UploadRecord>>,
    /// Consumptions asked for: upload and job.
    pub consumed: Mutex<Vec<(UploadId, JobId)>>,
}

/// A `BlobStore` in memory; `materialize` writes the object into a temporary directory.
pub struct FakeBlobs {
    /// Objects by digest.
    pub objects: Mutex<BTreeMap<String, Vec<u8>>>,
    /// Where materialized objects are written.
    pub directory: tempfile::TempDir,
}

/// A `Converter` that converts every window of a paginated original, except the windows that
/// start on a page in `capped`, which end at the memory cap.
pub struct FakeConverter {
    /// The page count it reports.
    pub pages: Option<u32>,
    /// First pages of the windows that hit the cap.
    pub capped: Vec<u32>,
    /// The bounds it reports.
    pub limits: ConverterLimits,
    /// Windows asked for.
    pub windows: Mutex<Vec<Option<PageRange>>>,
}

/// A `CandidateCheck` that accepts every tree.
pub struct AcceptAll;

/// A `SearchIndex` that records the heads it was rebuilt at.
#[derive(Default)]
pub struct FakeSearch {
    /// Heads rebuilt, in order.
    pub rebuilt: Mutex<Vec<Revision>>,
}

/// `Backups` that write one archive per mutation id and count the writes.
pub struct FakeBackups {
    /// The archive every write returns.
    pub archive: ObjectInfo,
    /// Writes asked for.
    pub writes: AtomicUsize,
    /// The editors each restore was given.
    pub restored: Mutex<Vec<Vec<String>>>,
}

/// A `Purger` that returns one report and counts the calls.
pub struct FakePurger {
    /// The report every call returns.
    pub report: PurgeReport,
    /// Calls made.
    pub calls: AtomicUsize,
}

impl FakeSearch {
    /// The heads rebuilt so far.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn rebuilt(&self) -> Result<Vec<Revision>, ApiError> {
        self.rebuilt
            .lock()
            .map(|heads| heads.clone())
            .map_err(|_| ApiError::new(ErrorCode::Internal, "poisoned"))
    }
}

impl FakeVersions {
    /// A store whose head is `head`.
    #[must_use]
    pub fn at(head: Revision) -> Self {
        Self {
            head,
            commits: Mutex::new(Vec::new()),
        }
    }

    /// The commits written so far.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn written(&self) -> Result<Vec<(CommitChanges, Revision)>, ApiError> {
        self.commits
            .lock()
            .map(|commits| commits.clone())
            .map_err(|_| poisoned())
    }

    /// Apply the port's replay and head rules to one commit.
    fn apply(&self, changes: CommitChanges) -> Result<Committed, ApiError> {
        let mut commits = self.commits.lock().map_err(|_| poisoned())?;
        let head = commits
            .last()
            .map_or_else(|| self.head.clone(), |(_, revision)| revision.clone());
        // The commits after the expected head, on the line.
        let after: Vec<&(CommitChanges, Revision)> = if changes.expected_head == self.head {
            commits.iter().collect()
        } else {
            commits
                .iter()
                .skip_while(|(_, revision)| *revision != changes.expected_head)
                .skip(1)
                .collect()
        };
        if let Some((_, revision)) = after
            .iter()
            .find(|(earlier, _)| earlier.mutation_id == changes.mutation_id)
        {
            return Ok(Committed {
                revision: revision.clone(),
                replayed: true,
                warnings: Vec::new(),
            });
        }
        if changes.expected_head != head {
            return Err(ApiError::new(ErrorCode::Conflict, "the head moved"));
        }
        let revision = Revision::try_from(format!("{:040x}", commits.len().saturating_add(1)))
            .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
        commits.push((changes, revision.clone()));
        Ok(Committed {
            revision,
            replayed: false,
            warnings: Vec::new(),
        })
    }
}

impl FakeUploads {
    /// The consumptions so far.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn consumed(&self) -> Result<Vec<(UploadId, JobId)>, ApiError> {
        self.consumed
            .lock()
            .map(|consumed| consumed.clone())
            .map_err(|_| poisoned())
    }
}

impl FakeBlobs {
    /// An empty store.
    ///
    /// # Errors
    /// Returns when the temporary directory cannot be made.
    pub fn new() -> Result<Self, std::io::Error> {
        Ok(Self {
            objects: Mutex::new(BTreeMap::new()),
            directory: tempfile::tempdir()?,
        })
    }

    /// Store bytes directly, as an upload would have.
    ///
    /// # Errors
    /// Returns when the lock is poisoned or the digest cannot be made.
    pub fn insert(&self, bytes: &[u8]) -> Result<ObjectInfo, ApiError> {
        let digest = okf_jawn_ingest::record::sha256_digest(bytes)?;
        self.objects
            .lock()
            .map_err(|_| poisoned())?
            .insert(digest.as_str().to_owned(), bytes.to_vec());
        Ok(ObjectInfo {
            digest,
            size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        })
    }

    /// The bytes stored under a digest.
    ///
    /// # Errors
    /// Returns `NotFound` when absent.
    pub fn bytes(&self, digest: &Digest) -> Result<Vec<u8>, ApiError> {
        self.objects
            .lock()
            .map_err(|_| poisoned())?
            .get(digest.as_str())
            .cloned()
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such object"))
    }
}

impl VersionStore for FakeVersions {
    fn head<'a>(&'a self, _scope: &'a StorageScope) -> PortFuture<'a, Revision> {
        let head = self.commits.lock().map_err(|_| poisoned()).map(|commits| {
            commits
                .last()
                .map_or_else(|| self.head.clone(), |(_, revision)| revision.clone())
        });
        Box::pin(async move { head })
    }
    fn list<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
        _folder: Option<&'a WorkspacePath>,
        _page: Page,
    ) -> PortFuture<'a, FolderListing> {
        Box::pin(async move {
            Ok(FolderListing {
                items: Vec::new(),
                folders: Vec::new(),
                next_cursor: None,
            })
        })
    }
    fn show<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
        _item: ItemId,
    ) -> PortFuture<'a, ItemDocument> {
        unused("show")
    }
    fn read_file<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
        _path: &'a WorkspacePath,
    ) -> PortFuture<'a, Vec<u8>> {
        unused("read_file")
    }
    fn rules<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
    ) -> PortFuture<'a, Option<NamingRules>> {
        unused("rules")
    }
    fn types<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
    ) -> PortFuture<'a, Vec<TypeDefinition>> {
        unused("types")
    }
    fn correction<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
        _item: ItemId,
        _digest: &'a Digest,
    ) -> PortFuture<'a, Option<String>> {
        unused("correction")
    }
    fn commit<'a>(
        &'a self,
        _scope: &'a StorageScope,
        changes: CommitChanges,
        _check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Committed> {
        let committed = self.apply(changes);
        Box::pin(async move { committed })
    }
    fn find_commit<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _since: &'a Revision,
    ) -> PortFuture<'a, Option<Revision>> {
        unused("find_commit")
    }
    fn create_candidate<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _proposal_id: okf_jawn_contract::identity::ProposalId,
        _changes: CandidateChanges,
        _check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Revision> {
        unused("create_candidate")
    }
    fn promote_candidate<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _promotion: Promotion,
    ) -> PortFuture<'a, Committed> {
        unused("promote_candidate")
    }
    fn log<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: LogQuery,
    ) -> PortFuture<'a, LogResponse> {
        unused("log")
    }
    fn diff<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: DiffQuery,
    ) -> PortFuture<'a, DiffResponse> {
        unused("diff")
    }
    fn blame<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: BlameQuery,
    ) -> PortFuture<'a, BlameResponse> {
        unused("blame")
    }
    fn write_history<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
        _mutation_id: MutationId,
    ) -> PortFuture<'a, ObjectInfo> {
        unused("write_history")
    }
}

impl SearchIndex for FakeSearch {
    fn index_revision<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: Revision,
    ) -> PortFuture<'a, ()> {
        unused("index_revision")
    }
    fn search<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: SearchQuery,
    ) -> PortFuture<'a, SearchResponse> {
        unused("search")
    }
    fn links<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: LinkQuery,
    ) -> PortFuture<'a, GetLinksResponse> {
        unused("links")
    }
    fn graph<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: GraphQuery,
    ) -> PortFuture<'a, GetGraphResponse> {
        unused("graph")
    }
    fn rebuild<'a>(&'a self, _scope: &'a StorageScope, head: Revision) -> PortFuture<'a, ()> {
        let recorded = self
            .rebuilt
            .lock()
            .map(|mut heads| heads.push(head))
            .map_err(|_| ApiError::new(ErrorCode::Internal, "poisoned"));
        Box::pin(async move { recorded })
    }
}

impl Backups for FakeBackups {
    fn write_workspace_archive<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
    ) -> PortFuture<'a, ObjectInfo> {
        let _previous = self.writes.fetch_add(1, Ordering::SeqCst);
        let archive = self.archive.clone();
        Box::pin(async move { Ok(archive) })
    }
    fn write_installation_archive<'a>(
        &'a self,
        _tenant: &'a TenantId,
        _mutation_id: MutationId,
    ) -> PortFuture<'a, ObjectInfo> {
        let _previous = self.writes.fetch_add(1, Ordering::SeqCst);
        let archive = self.archive.clone();
        Box::pin(async move { Ok(archive) })
    }
    fn open_installation_archive<'a>(
        &'a self,
        _tenant: &'a TenantId,
        _digest: &'a Digest,
        _offset: u64,
        _length: u64,
    ) -> PortFuture<'a, ObjectRead> {
        unused("open_installation_archive")
    }
    fn restore_import<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _archive: Digest,
        _mutation_id: MutationId,
        editors: Vec<String>,
    ) -> PortFuture<'a, RestoreReport> {
        let recorded = self
            .restored
            .lock()
            .map(|mut restored| restored.push(editors))
            .map_err(|_| poisoned())
            .map(|()| RestoreReport {
                items: 3,
                drafts_restored: 1,
                drafts_unassigned: 1,
            });
        Box::pin(async move { recorded })
    }
}

impl Purger for FakePurger {
    fn purge_workspace<'a>(
        &'a self,
        _tenant: &'a TenantId,
        _mutation_id: MutationId,
        _purge: PurgeId,
        _workspace: WorkspaceId,
    ) -> PortFuture<'a, PurgeReport> {
        let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
        let report = self.report.clone();
        Box::pin(async move { Ok(report) })
    }
    fn purge_item<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _purge: PurgeId,
        _item: ItemId,
    ) -> PortFuture<'a, PurgeReport> {
        let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
        let report = self.report.clone();
        Box::pin(async move { Ok(report) })
    }
}

impl UploadStore for FakeUploads {
    fn create<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _upload: NewUpload,
    ) -> PortFuture<'a, UploadRecord> {
        unused("create")
    }
    fn get<'a>(
        &'a self,
        _scope: &'a StorageScope,
        upload: UploadId,
    ) -> PortFuture<'a, UploadRecord> {
        let found = self
            .records
            .lock()
            .map_err(|_| poisoned())
            .and_then(|records| {
                records
                    .iter()
                    .find(|record| record.id == upload)
                    .cloned()
                    .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such upload"))
            });
        Box::pin(async move { found })
    }
    fn put_content<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _upload: UploadId,
        _body: ByteReader,
        _limit: u64,
    ) -> PortFuture<'a, UploadRecord> {
        unused("put_content")
    }
    fn complete<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _upload: UploadId,
        _sha256: Digest,
    ) -> PortFuture<'a, UploadRecord> {
        unused("complete")
    }
    fn consume<'a>(
        &'a self,
        scope: &'a StorageScope,
        upload: UploadId,
        job: JobId,
    ) -> PortFuture<'a, UploadRecord> {
        let recorded = self
            .consumed
            .lock()
            .map(|mut consumed| {
                if !consumed.contains(&(upload, job)) {
                    consumed.push((upload, job));
                }
            })
            .map_err(|_| poisoned());
        Box::pin(async move {
            recorded?;
            self.get(scope, upload).await
        })
    }
}

impl BlobStore for FakeBlobs {
    fn put<'a>(
        &'a self,
        _scope: &'a StorageScope,
        mut body: ByteReader,
        _limit: u64,
        expected: Option<Digest>,
    ) -> PortFuture<'a, ObjectInfo> {
        Box::pin(async move {
            let mut bytes = Vec::new();
            let _read = tokio::io::AsyncReadExt::read_to_end(&mut body, &mut bytes)
                .await
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
            let object = self.insert(&bytes)?;
            if expected.is_some_and(|expected| expected != object.digest) {
                return Err(ApiError::new(ErrorCode::Conflict, "digest mismatch"));
            }
            Ok(object)
        })
    }
    fn open<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _digest: &'a Digest,
        _offset: u64,
        _length: u64,
    ) -> PortFuture<'a, ObjectRead> {
        unused("open")
    }
    fn materialize<'a>(
        &'a self,
        _scope: &'a StorageScope,
        digest: &'a Digest,
    ) -> PortFuture<'a, LocalSource> {
        Box::pin(async move {
            let bytes = self.bytes(digest)?;
            let path = self.directory.path().join(digest.as_str());
            std::fs::write(&path, &bytes)
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
            Ok(LocalSource {
                path,
                object: ObjectInfo {
                    digest: digest.clone(),
                    size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                },
            })
        })
    }
}

impl Converter for FakeConverter {
    fn page_count<'a>(
        &'a self,
        _source: &'a LocalSource,
        _file_name: &'a str,
    ) -> PortFuture<'a, Option<u32>> {
        let pages = self.pages;
        Box::pin(async move { Ok(pages) })
    }
    fn convert(&self, input: ConversionInput) -> PortFuture<'_, Conversion> {
        Box::pin(async move {
            self.windows
                .lock()
                .map_err(|_| poisoned())?
                .push(input.window.clone());
            let converter = ConverterIdentity {
                name: "docling".to_owned(),
                version: "2.3.0".to_owned(),
                packages: Vec::new(),
                settings: input.settings.clone(),
                models: Vec::new(),
                page_window: input.window.as_ref().map(|_| self.limits.window_pages),
            };
            let source_digest = input.source.object.digest.clone();
            let start = input.window.as_ref().map_or(1, |window| window.start);
            if self.capped.contains(&start) {
                return Ok(Conversion {
                    source_digest,
                    converter,
                    status: ConversionStatus::Failure(FailureReason::MemoryLimit {
                        limit_bytes: self.limits.memory_limit_bytes.to_string(),
                    }),
                    document: None,
                    coverage: None,
                    issues: Vec::new(),
                });
            }
            let structured = input.output_directory.join("document.json");
            std::fs::write(&structured, format!("{{\"window\":{start}}}"))
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
            Ok(Conversion {
                source_digest,
                converter,
                status: ConversionStatus::Success,
                coverage: input.window.as_ref().map(|window| WindowCoverage {
                    window: window.clone(),
                    converted: vec![window.clone()],
                    partly_extracted: Vec::new(),
                    not_converted: Vec::new(),
                }),
                document: Some(ConvertedDocument {
                    markdown: format!("# Pages from {start}\n\nText.\n"),
                    structured,
                    outline: Vec::new(),
                    assets: Vec::new(),
                    locations: Vec::new(),
                    tables: Vec::new(),
                    warnings: Vec::new(),
                }),
                issues: Vec::new(),
            })
        })
    }
    fn limits(&self) -> ConverterLimits {
        self.limits
    }
}

impl CandidateCheck for AcceptAll {
    fn check(&self, _root: &Path) -> Result<Vec<Warning>, ApiError> {
        Ok(Vec::new())
    }
}

fn poisoned() -> ApiError {
    ApiError::new(ErrorCode::Internal, "poisoned")
}

fn unused<'a, T: Send + 'a>(method: &'static str) -> PortFuture<'a, T> {
    Box::pin(async move {
        Err(ApiError::new(
            ErrorCode::NotImplemented,
            format!("the fake does not model {method}"),
        ))
    })
}
