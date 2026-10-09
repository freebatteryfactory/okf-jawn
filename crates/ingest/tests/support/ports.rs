//! Minimal fakes of the storage ports the handler calls for the jobs that are not imports.
//!
//! Each fake answers only the methods those jobs call, and counts the calls so a test can see
//! that a repeated run repeated no effect. Every other method answers `NotImplemented`, so a
//! handler that starts calling one fails loudly instead of being answered with an invented
//! value.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use okf_jawn_contract::conventions::NamingRules;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::history::{BlameResponse, DiffResponse, LogResponse};
use okf_jawn_contract::identity::{
    Digest, ItemId, MutationId, PurgeId, Revision, TenantId, WorkspaceId, WorkspacePath,
};
use okf_jawn_contract::item::{ItemDocument, TypeDefinition};
use okf_jawn_contract::purge::PurgeReport;
use okf_jawn_contract::search::{GetGraphResponse, GetLinksResponse, SearchResponse};
use okf_jawn_contract::workspace::RestoreReport;
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::search::{GraphQuery, LinkQuery, SearchIndex, SearchQuery};
use okf_jawn_core::storage::{
    Backups, BlameQuery, CandidateChanges, CandidateCheck, CommitChanges, Committed, DiffQuery,
    FolderListing, LogQuery, ObjectInfo, ObjectRead, Page, Promotion, Purger, StorageScope,
    VersionStore,
};

/// A `VersionStore` that knows only its head.
pub struct FakeVersions {
    /// The head `head` returns.
    pub head: Revision,
}

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

impl VersionStore for FakeVersions {
    fn head<'a>(&'a self, _scope: &'a StorageScope) -> PortFuture<'a, Revision> {
        let head = self.head.clone();
        Box::pin(async move { Ok(head) })
    }
    fn list<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
        _folder: Option<&'a WorkspacePath>,
        _page: Page,
    ) -> PortFuture<'a, FolderListing> {
        unused("list")
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
        _changes: CommitChanges,
        _check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Committed> {
        unused("commit")
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
        _editors: Vec<String>,
    ) -> PortFuture<'a, RestoreReport> {
        unused("restore_import")
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

fn unused<'a, T: Send + 'a>(method: &'static str) -> PortFuture<'a, T> {
    Box::pin(async move {
        Err(ApiError::new(
            ErrorCode::NotImplemented,
            format!("the fake does not model {method}"),
        ))
    })
}
