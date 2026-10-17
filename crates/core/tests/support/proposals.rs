//! A `ProposalStore` fake, and the world the proposal and review handlers are tested in: the
//! fixture's fakes (`fixture::World`) with this file's store, the confirmation store
//! (`confirmations.rs`), the candidate store (`candidates.rs`), the review records
//! (`reviews.rs`), an event log that can fail one kind of event, and an access port that answers
//! the reader's grants on other workspaces. Not production adapters.
//!
//! `FakeProposals` encodes what `ProposalStore`'s doc states, each rule quoted above the code
//! that keeps it. A test can land another writer's status change just before the next `update`
//! is judged (`land_before_next_update`), to place a race between a handler's read of a proposal
//! and its write. Where it is kinder than storage's `SqliteProposals`: its cursor is a decimal
//! offset into the listing rather than a row number, so a row inserted between two pages shifts
//! the next page (storage's does not); and it refuses nothing while a purge names the workspace
//! (storage refuses every write then).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use okf_jawn_contract::access::{Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::{Event, EventKind, ListEventsResponse};
use okf_jawn_contract::identity::{MutationId, ProposalId, WorkspaceId};
use okf_jawn_contract::proposal::{Comment, ListProposalsResponse, Proposal, ProposalStatus};
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::application::ApplicationService;
use okf_jawn_core::context::{TenantGrant, WorkspaceGrant};
use okf_jawn_core::events::{EventLog, EventQuery, EventScope, NewEvent};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::proposals::{CommentPage, ProposalFilter, ProposalStore};
use okf_jawn_core::storage::{Page, StorageScope};
use time::OffsetDateTime;

use crate::candidates::CandidateVersions;
use crate::confirmations::FakeConfirmations;
use crate::fixture::{Built, FakeEvents, World, config};
use crate::review_store::ReviewRecords;

/// A `ProposalStore` call a test can make fail once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalCall {
    /// `insert`.
    Insert,
    /// `update`.
    Update,
    /// `add_comment`.
    AddComment,
}

/// `ProposalStore` over proposal and comment rows held in memory.
#[derive(Default)]
pub struct FakeProposals {
    state: Mutex<ProposalRows>,
    failing: Mutex<Vec<ProposalCall>>,
    /// Another writer's status change that lands just before the next `update` call is judged.
    interloper: Mutex<Option<StatusChange>>,
}

/// A status change as `update` is called with it: scope, mutation, record, expected status.
type StatusChange = (StorageScope, MutationId, Proposal, ProposalStatus);

#[derive(Default)]
struct ProposalRows {
    /// Every proposal, oldest first, with its scope and the mutation that inserted it.
    proposals: Vec<(StorageScope, MutationId, Proposal)>,
    /// Every comment, oldest first, with its scope, mutation and proposal.
    comments: Vec<(StorageScope, MutationId, ProposalId, Comment)>,
    /// Every `update` call that wrote, in order: the status written.
    writes: Vec<ProposalStatus>,
    /// The mutation that wrote each proposal's stored status, with its scope.
    writers: Vec<(StorageScope, ProposalId, MutationId)>,
}

/// `EventLog` over the fixture's `FakeEvents`, that can fail the next append of one kind.
pub struct EventsFailing {
    inner: Arc<FakeEvents>,
    fail_kind: Mutex<Option<EventKind>>,
}

/// `AccessControl` answering the reader's raw grant on each workspace a test names, and
/// `Forbidden`, or a scripted error, for any other.
#[derive(Default)]
pub struct ReadersAccess {
    grants: Mutex<BTreeMap<WorkspaceId, Vec<Permission>>>,
    errors: Mutex<BTreeMap<WorkspaceId, ErrorCode>>,
    lookups: Mutex<Vec<WorkspaceId>>,
}

/// The service over the fixture's fakes, with this package's fakes in their ports.
pub struct ProposalWorld {
    /// The service under test.
    pub service: ApplicationService,
    /// The fixture's fakes; their service is not the one under test.
    pub world: World,
    /// Proposals and comments.
    pub proposals: Arc<FakeProposals>,
    /// Confirmations.
    pub confirmations: Arc<FakeConfirmations>,
    /// Versions with candidates and promotions.
    pub versions: Arc<CandidateVersions>,
    /// Records with reviews.
    pub records: Arc<ReviewRecords>,
    /// Events, able to fail one kind.
    pub events: Arc<EventsFailing>,
    /// The reader's grants on other workspaces.
    pub access: Arc<ReadersAccess>,
}

impl ProposalWorld {
    /// The fixture world with this package's fakes, its service reading the instant `noon`.
    ///
    /// # Errors
    /// Returns when a fixture cannot be built.
    pub fn new() -> Built<Self> {
        let world = World::new()?;
        let proposals = Arc::new(FakeProposals::default());
        let confirmations = Arc::new(FakeConfirmations::new()?);
        let versions = Arc::new(CandidateVersions::new(Arc::clone(&world.versions)));
        let records = Arc::new(ReviewRecords::new(Arc::clone(&world.records)));
        let events = Arc::new(EventsFailing {
            inner: Arc::clone(&world.events),
            fail_kind: Mutex::new(None),
        });
        let access = Arc::new(ReadersAccess::default());
        let mut ports = world.service.ports().clone();
        ports.proposals = proposals.clone();
        ports.confirmations = confirmations.clone();
        ports.versions = versions.clone();
        ports.records = records.clone();
        ports.events = events.clone();
        ports.access = access.clone();
        let service = ApplicationService::new(ports, config())?.with_clock(noon);
        Ok(Self {
            service,
            world,
            proposals,
            confirmations,
            versions,
            records,
            events,
            access,
        })
    }
}

impl FakeProposals {
    /// Make the next call of `call` fail with `Unavailable`, as a store fault would.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn fail_next(&self, call: ProposalCall) -> Built<()> {
        lock(&self.failing)?.push(call);
        Ok(())
    }

    /// Land another writer's status change (`update` under `mutation`, from `expected`) just
    /// before the next `update` call is judged, as if that writer had won a race between the
    /// handler's read of the proposal and its write. The change itself follows the doc's rules.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn land_before_next_update(
        &self,
        scope: &StorageScope,
        mutation: MutationId,
        proposal: Proposal,
        expected: ProposalStatus,
    ) -> Built<()> {
        *lock(&self.interloper)? = Some((scope.clone(), mutation, proposal, expected));
        Ok(())
    }

    /// Hold `proposal` in `scope` as if `mutation` had inserted it.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put(&self, scope: &StorageScope, mutation: MutationId, proposal: Proposal) -> Built<()> {
        lock(&self.state)?
            .proposals
            .push((scope.clone(), mutation, proposal));
        Ok(())
    }

    /// Every proposal held, oldest first.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn rows(&self) -> Built<Vec<Proposal>> {
        Ok(lock(&self.state)?
            .proposals
            .iter()
            .map(|(_, _, proposal)| proposal.clone())
            .collect())
    }

    /// Every comment held, oldest first, with the mutation that wrote it.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn comments(&self) -> Built<Vec<(MutationId, Comment)>> {
        Ok(lock(&self.state)?
            .comments
            .iter()
            .map(|(_, mutation, _, comment)| (*mutation, comment.clone()))
            .collect())
    }

    /// The status each `update` that wrote set, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn writes(&self) -> Built<Vec<ProposalStatus>> {
        Ok(lock(&self.state)?.writes.clone())
    }

    fn injected(&self, call: ProposalCall) -> Result<(), ApiError> {
        let mut failing = lock_api(&self.failing)?;
        if let Some(position) = failing.iter().position(|failing| *failing == call) {
            failing.remove(position);
            return Err(ApiError::new(ErrorCode::Unavailable, "the store failed"));
        }
        Ok(())
    }
}

impl ProposalRows {
    /// `ProposalStore::update` as its doc states it (quoted above `FakeProposals::update`).
    fn update(
        &mut self,
        scope: &StorageScope,
        mutation_id: MutationId,
        proposal: Proposal,
        expected: &ProposalStatus,
    ) -> Result<Proposal, ApiError> {
        let written = proposal.status.clone();
        let record = serde_json::to_value(&proposal).map_err(internal)?;
        let row = self
            .proposals
            .iter_mut()
            .find(|(held, _, row)| held == scope && row.id == proposal.id)
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such proposal"))?;
        let wrote = self.writers.iter().any(|(held, id, writer)| {
            held == scope && *id == proposal.id && *writer == mutation_id
        });
        if wrote && serde_json::to_value(&row.2).map_err(internal)? == record {
            return Ok(proposal);
        }
        if row.2.status != *expected {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                "the proposal's status is not the expected one",
            ));
        }
        row.2 = proposal.clone();
        self.writes.push(written);
        self.writers.push((scope.clone(), proposal.id, mutation_id));
        Ok(proposal)
    }
}

impl ProposalStore for FakeProposals {
    // "Persist a new open proposal. Unique on `mutation_id`: a repeated id inserts nothing and
    // returns the prior proposal."
    fn insert<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        proposal: Proposal,
    ) -> PortFuture<'a, Proposal> {
        Box::pin(async move {
            self.injected(ProposalCall::Insert)?;
            let mut state = lock_api(&self.state)?;
            if let Some((_, _, prior)) = state
                .proposals
                .iter()
                .find(|(_, held, _)| *held == mutation_id)
            {
                return Ok(prior.clone());
            }
            state
                .proposals
                .push((scope.clone(), mutation_id, proposal.clone()));
            Ok(proposal)
        })
    }

    // "Read one proposal by identity; `NotFound` when the workspace has no such proposal."
    fn get<'a>(
        &'a self,
        scope: &'a StorageScope,
        proposal: ProposalId,
    ) -> PortFuture<'a, Proposal> {
        Box::pin(async move {
            lock_api(&self.state)?
                .proposals
                .iter()
                .find(|(held, _, row)| held == scope && row.id == proposal)
                .map(|(_, _, row)| row.clone())
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such proposal"))
        })
    }

    // "List proposals, newest first."
    fn list<'a>(
        &'a self,
        scope: &'a StorageScope,
        filter: ProposalFilter,
    ) -> PortFuture<'a, ListProposalsResponse> {
        Box::pin(async move {
            let state = lock_api(&self.state)?;
            let matching: Vec<Proposal> = state
                .proposals
                .iter()
                .rev()
                .filter(|(held, _, row)| {
                    held == scope
                        && filter
                            .status
                            .as_ref()
                            .is_none_or(|status| *status == row.status)
                })
                .map(|(_, _, row)| row.clone())
                .collect();
            let (items, next_cursor) = paged(matching, &filter.page)?;
            Ok(ListProposalsResponse { items, next_cursor })
        })
    }

    // "Replace a proposal's status and retained fields ... only while its stored status is
    // `expected` ... The store keeps the mutation that wrote each change. Fails with `Conflict`,
    // writing nothing, when the stored status is not `expected`, and with `NotFound` when the
    // workspace has no such proposal. Writing again exactly the record that is stored (its
    // status and every field) under the mutation that wrote it changes nothing and returns it,
    // whatever `expected` says ... The same record under another mutation is a change like any
    // other, refused when the stored status is not `expected`".
    fn update<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        proposal: Proposal,
        expected: ProposalStatus,
    ) -> PortFuture<'a, Proposal> {
        Box::pin(async move {
            self.injected(ProposalCall::Update)?;
            let interloper = lock_api(&self.interloper)?.take();
            let mut state = lock_api(&self.state)?;
            if let Some((held, writer, landed, from)) = interloper {
                state.update(&held, writer, landed, &from)?;
            }
            state.update(scope, mutation_id, proposal, &expected)
        })
    }

    // "Append a discussion comment without certifying content. Unique on `mutation_id`: a
    // repeated id inserts nothing and returns the prior comment. `NotFound`, writing nothing,
    // when the workspace has no such proposal."
    fn add_comment<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        proposal: ProposalId,
        comment: Comment,
    ) -> PortFuture<'a, Comment> {
        Box::pin(async move {
            self.injected(ProposalCall::AddComment)?;
            let mut state = lock_api(&self.state)?;
            if !state
                .proposals
                .iter()
                .any(|(held, _, row)| held == scope && row.id == proposal)
            {
                return Err(ApiError::new(ErrorCode::NotFound, "no such proposal"));
            }
            if let Some((_, _, _, prior)) = state
                .comments
                .iter()
                .find(|(_, held, _, _)| *held == mutation_id)
            {
                return Ok(prior.clone());
            }
            state
                .comments
                .push((scope.clone(), mutation_id, proposal, comment.clone()));
            Ok(comment)
        })
    }

    // "List a proposal's discussion, oldest first; `NotFound` when the workspace has no such
    // proposal."
    fn list_comments<'a>(
        &'a self,
        scope: &'a StorageScope,
        proposal: ProposalId,
        page: Page,
    ) -> PortFuture<'a, CommentPage> {
        Box::pin(async move {
            let state = lock_api(&self.state)?;
            if !state
                .proposals
                .iter()
                .any(|(held, _, row)| held == scope && row.id == proposal)
            {
                return Err(ApiError::new(ErrorCode::NotFound, "no such proposal"));
            }
            let matching: Vec<Comment> = state
                .comments
                .iter()
                .filter(|(held, _, of, _)| held == scope && *of == proposal)
                .map(|(_, _, _, comment)| comment.clone())
                .collect();
            let (items, next_cursor) = paged(matching, &page)?;
            Ok(CommentPage { items, next_cursor })
        })
    }
}

impl EventsFailing {
    /// Make the next append of an event of `kind` fail with `Unavailable`.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn fail_next(&self, kind: EventKind) -> Built<()> {
        *lock(&self.fail_kind)? = Some(kind);
        Ok(())
    }
}

impl EventLog for EventsFailing {
    fn append<'a>(
        &'a self,
        scope: &'a EventScope,
        mutation_id: Option<MutationId>,
        event: NewEvent,
    ) -> PortFuture<'a, Event> {
        Box::pin(async move {
            {
                let mut failing = lock_api(&self.fail_kind)?;
                if failing.as_ref() == Some(&event.kind) {
                    *failing = None;
                    return Err(ApiError::new(ErrorCode::Unavailable, "the log failed"));
                }
            }
            self.inner.append(scope, mutation_id, event).await
        })
    }
    fn list<'a>(
        &'a self,
        scope: &'a EventScope,
        query: EventQuery,
    ) -> PortFuture<'a, ListEventsResponse> {
        self.inner.list(scope, query)
    }
}

impl ReadersAccess {
    /// Give the reader the raw grant `permissions` on `workspace`.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn grant(&self, workspace: WorkspaceId, permissions: Vec<Permission>) -> Built<()> {
        lock(&self.grants)?.insert(workspace, permissions);
        Ok(())
    }

    /// Make every lookup of `workspace` fail with `code`.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn fail(&self, workspace: WorkspaceId, code: ErrorCode) -> Built<()> {
        lock(&self.errors)?.insert(workspace, code);
        Ok(())
    }

    /// Every workspace looked up, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn lookups(&self) -> Built<Vec<WorkspaceId>> {
        Ok(lock(&self.lookups)?.clone())
    }
}

impl AccessControl for ReadersAccess {
    // "Look up the caller's raw grant on one workspace (before route/delegation intersection)."
    // The local adapter answers a grant, possibly empty, for any workspace; this one answers
    // `Forbidden` for a workspace no test granted, as a hosted adapter may.
    fn authorize<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
        _permission: Permission,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(async move {
            lock_api(&self.lookups)?.push(workspace);
            if let Some(code) = lock_api(&self.errors)?.get(&workspace) {
                return Err(ApiError::new(code.clone(), "the grant lookup failed"));
            }
            let permissions = lock_api(&self.grants)?
                .get(&workspace)
                .cloned()
                .ok_or_else(|| ApiError::new(ErrorCode::Forbidden, "no grant"))?;
            Ok(WorkspaceGrant {
                scope: StorageScope {
                    tenant_id: principal.tenant_id.clone(),
                    workspace_id: workspace,
                },
                permissions,
            })
        })
    }
    fn authorize_tenant<'a>(
        &'a self,
        _principal: &'a Principal,
        _permission: Permission,
    ) -> PortFuture<'a, TenantGrant> {
        not_used()
    }
    fn grants<'a>(&'a self, _principal: &'a Principal) -> PortFuture<'a, Vec<WorkspaceGrant>> {
        not_used()
    }
    fn grant_creator<'a>(
        &'a self,
        _principal: &'a Principal,
        _workspace: WorkspaceId,
    ) -> PortFuture<'a, WorkspaceGrant> {
        not_used()
    }
    fn editors<'a>(&'a self, _scope: &'a StorageScope) -> PortFuture<'a, Vec<String>> {
        not_used()
    }
}

/// The instant the world's service reads: 2026-10-09T12:00:00Z, the fixture's `instant`.
#[must_use]
pub fn noon() -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH.saturating_add(time::Duration::seconds(1_791_547_200))
}

/// One page of `items` from the decimal offset `page.cursor` names, at most `page.limit` long,
/// with the next offset as the cursor while items remain.
fn paged<T>(items: Vec<T>, page: &Page) -> Result<(Vec<T>, Option<String>), ApiError> {
    let start: usize = match page.cursor.as_deref() {
        None => 0,
        Some(cursor) => cursor
            .parse()
            .map_err(|_| ApiError::new(ErrorCode::InvalidInput, "not a cursor of this store"))?,
    };
    let limit = usize::from(page.limit.max(1));
    let total = items.len();
    let taken: Vec<T> = items.into_iter().skip(start).take(limit).collect();
    let next = start.saturating_add(taken.len());
    Ok((taken, (next < total).then(|| next.to_string())))
}

fn not_used<'a, T: Send + 'a>() -> PortFuture<'a, T> {
    Box::pin(async { Err(ApiError::new(ErrorCode::Internal, "not used by this test")) })
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::new(ErrorCode::Internal, error.to_string())
}

fn lock<T>(mutex: &Mutex<T>) -> Built<MutexGuard<'_, T>> {
    mutex.lock().map_err(|_| "a fake's lock is poisoned".into())
}

fn lock_api<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, ApiError> {
    mutex
        .lock()
        .map_err(|_| ApiError::new(ErrorCode::Internal, "a fake's lock is poisoned"))
}
