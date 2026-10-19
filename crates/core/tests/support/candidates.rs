//! A `VersionStore` for the proposal handlers: the fixture's `FakeVersions` for every method,
//! with proposal candidates and their promotion kept as `VersionStore::create_candidate` and
//! `promote_candidate` document them, each rule quoted above the code that keeps it. Not a
//! production adapter.
//!
//! A candidate is staged in a directory of its own: the fixture's conformant tree
//! (`Staged::conformant`) with each `CreateItem` written at its path (its type, its properties
//! and its body) and each `EditItem` written at the edited item's path at the base, and the
//! check it is handed runs there. Its refusal stores nothing: no reference, no kept warnings,
//! no revision used. A promotion lands as a new head of the fixture's line (`set_head`).
//!
//! Where it is kinder than storage's Git store: a `MoveItem`, a `SetStatus` and a
//! `SupplyExtraction` are not written into the staged tree; nothing is readable at a candidate
//! or a promoted revision (`show` there is `NotFound`); no path collision is refused and no
//! store-rewritten file is reported to the check (`rewritten` is empty); a promotion is found
//! again (by `promote_candidate` and `find_commit`) only from the head it was promoted onto, not
//! from an earlier one; and nothing is refused while a purge names the workspace.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use okf_jawn_contract::common::Warning;
use okf_jawn_contract::conventions::NamingRules;
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::history::{BlameResponse, DiffResponse, LogResponse};
use okf_jawn_contract::identity::{
    Digest, ItemId, MutationId, ProposalId, Revision, WorkspacePath,
};
use okf_jawn_contract::item::{ItemDocument, ItemSummary, TypeDefinition};
use okf_jawn_core::items::refuse_header_change;
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{
    BlameQuery, CandidateChanges, CandidateCheck, CommitChanges, Committed, CommittedFiles,
    DiffQuery, FilePage, FolderListing, LogQuery, ObjectInfo, Page, Promotion, StorageScope,
    TreeEdit, VersionStore, replay_warnings_unavailable,
};
use serde_json::Value;

use crate::fixture::{Built, FakeVersions, Staged};

/// `VersionStore` over the fixture's versions, with candidates and promotions of its own.
pub struct CandidateVersions {
    inner: Arc<FakeVersions>,
    state: Mutex<CandidateState>,
    fail_next_promote: AtomicBool,
}

#[derive(Default)]
struct CandidateState {
    /// The revisions the next candidates and promotions produce, in order.
    queued: Vec<Revision>,
    /// Each proposal's reference: (proposal, candidate, its base, the mutation that wrote it).
    references: Vec<(ProposalId, Revision, Revision, MutationId)>,
    /// The warnings kept for each candidate and promotion, by mutation.
    warnings: Vec<(MutationId, Vec<Warning>)>,
    /// Every candidate written, in order.
    candidates: Vec<CandidateChanges>,
    /// Every promotion that landed, with the revision it produced.
    promotions: Vec<(Promotion, Revision)>,
    /// Every `promote_candidate` call, refused or not, in order.
    attempts: Vec<Promotion>,
    /// How many candidate checks ran.
    checks: usize,
}

impl CandidateVersions {
    /// Candidates over `inner`.
    #[must_use]
    pub fn new(inner: Arc<FakeVersions>) -> Self {
        Self {
            inner,
            state: Mutex::new(CandidateState::default()),
            fail_next_promote: AtomicBool::new(false),
        }
    }

    /// The revision the next new candidate or promotion produces.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn queue(&self, revision: Revision) -> Built<()> {
        lock(&self.state)?.queued.push(revision);
        Ok(())
    }

    /// Every candidate written, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn candidates(&self) -> Built<Vec<CandidateChanges>> {
        Ok(lock(&self.state)?.candidates.clone())
    }

    /// Every promotion that landed, with its revision, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn promotions(&self) -> Built<Vec<(Promotion, Revision)>> {
        Ok(lock(&self.state)?.promotions.clone())
    }

    /// Every promotion asked for, refused or not, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn attempts(&self) -> Built<Vec<Promotion>> {
        Ok(lock(&self.state)?.attempts.clone())
    }

    /// The candidate revision `proposal`'s reference retains, if any.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn reference(&self, proposal: ProposalId) -> Built<Option<Revision>> {
        Ok(lock(&self.state)?
            .references
            .iter()
            .find(|(held, ..)| *held == proposal)
            .map(|(_, candidate, ..)| candidate.clone()))
    }

    /// Retain `candidate`, drafted on `base` by `mutation`, as `proposal`'s reference, as a
    /// candidate written earlier would be.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn retain(
        &self,
        proposal: ProposalId,
        candidate: Revision,
        base: Revision,
        mutation: MutationId,
    ) -> Built<()> {
        lock(&self.state)?
            .references
            .push((proposal, candidate, base, mutation));
        Ok(())
    }

    /// How many candidate checks ran.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn checks(&self) -> Built<usize> {
        Ok(lock(&self.state)?.checks)
    }

    /// Make the next `promote_candidate` fail with `Unavailable` before it writes, as a store
    /// fault would.
    pub fn fail_next_promote(&self) {
        self.fail_next_promote.store(true, Ordering::SeqCst);
    }

    /// Stage `changes`' candidate tree: the fixture's conformant tree with each created and
    /// edited item written; an edit that changes the application header is refused as storage
    /// refuses it.
    async fn staged(
        &self,
        scope: &StorageScope,
        changes: &CandidateChanges,
    ) -> Result<Staged, ApiError> {
        let staged = Staged::conformant().map_err(internal)?;
        for edit in &changes.edits {
            match edit {
                TreeEdit::CreateItem {
                    path,
                    type_name,
                    body,
                    properties,
                    ..
                } => {
                    let mut written = properties.clone();
                    written.insert("type".to_owned(), Value::String(type_name.clone()));
                    write(&staged, path, &item_file(&written, body))?;
                }
                TreeEdit::EditItem {
                    item_id,
                    body,
                    properties,
                } => {
                    let stored: ItemDocument =
                        self.inner.show(scope, &changes.base, *item_id).await?;
                    refuse_header_change(&stored.properties, properties, "/properties")?;
                    write(&staged, &stored.summary.path, &item_file(properties, body))?;
                }
                _ => {}
            }
        }
        Ok(staged)
    }
}

impl VersionStore for CandidateVersions {
    fn head<'a>(&'a self, scope: &'a StorageScope) -> PortFuture<'a, Revision> {
        self.inner.head(scope)
    }
    fn list<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        folder: Option<&'a WorkspacePath>,
        page: Page,
    ) -> PortFuture<'a, FolderListing> {
        self.inner.list(scope, revision, folder, page)
    }
    fn item_at_path<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        path: &'a WorkspacePath,
    ) -> PortFuture<'a, Option<ItemSummary>> {
        self.inner.item_at_path(scope, revision, path)
    }
    fn show<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        item: ItemId,
    ) -> PortFuture<'a, ItemDocument> {
        self.inner.show(scope, revision, item)
    }
    fn read_file<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        path: &'a WorkspacePath,
    ) -> PortFuture<'a, Vec<u8>> {
        self.inner.read_file(scope, revision, path)
    }
    fn rules<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
    ) -> PortFuture<'a, Option<NamingRules>> {
        self.inner.rules(scope, revision)
    }
    fn types<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
    ) -> PortFuture<'a, Vec<TypeDefinition>> {
        self.inner.types(scope, revision)
    }
    fn correction<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        item: ItemId,
        digest: &'a Digest,
    ) -> PortFuture<'a, Option<String>> {
        self.inner.correction(scope, revision, item, digest)
    }
    fn commit<'a>(
        &'a self,
        scope: &'a StorageScope,
        changes: CommitChanges,
        check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Committed> {
        self.inner.commit(scope, changes, check)
    }
    // "Find the commit on the head's history that carries `mutation_id`." A promotion is found
    // from the head it was promoted onto (its parent); every other commit as the fixture finds it.
    fn find_commit<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        since: Option<&'a Revision>,
    ) -> PortFuture<'a, Option<Committed>> {
        Box::pin(async move {
            let promoted = {
                let state = lock_api(&self.state)?;
                state
                    .promotions
                    .iter()
                    .find(|(promotion, _)| {
                        promotion.mutation_id == mutation_id
                            && since.is_none_or(|since| *since == promotion.expected_head)
                    })
                    .map(|(_, revision)| state.replay_of(mutation_id, revision.clone()))
            };
            match promoted {
                Some(found) => Ok(Some(found)),
                None => self.inner.find_commit(scope, mutation_id, since).await,
            }
        })
    }
    // "Write a proposal candidate commit on top of `changes.base` and retain it at
    // `refs/okf-jawn/proposals/<proposal id>`. The head does not move."
    fn create_candidate<'a>(
        &'a self,
        scope: &'a StorageScope,
        proposal_id: ProposalId,
        changes: CandidateChanges,
        check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Committed> {
        Box::pin(async move {
            {
                let state = lock_api(&self.state)?;
                if let Some((_, candidate, _, written_by)) = state
                    .references
                    .iter()
                    .find(|(held, ..)| *held == proposal_id)
                {
                    // "Idempotent on `changes.mutation_id`: when the reference already points at
                    // a commit carrying this mutation's trailer, that revision is returned and
                    // nothing is written. ... a repeat answers the same findings, `replayed`
                    // set". "When another write already set the reference, the call fails with a
                    // plain `Conflict`".
                    if *written_by == changes.mutation_id {
                        return Ok(state.replay_of(changes.mutation_id, candidate.clone()));
                    }
                    return Err(ApiError::new(
                        ErrorCode::Conflict,
                        "this proposal already has a candidate from another write",
                    ));
                }
            }
            // "`check` runs on the staged candidate tree exactly as it does for `commit`".
            let staged = self.staged(scope, &changes).await?;
            {
                let mut state = lock_api(&self.state)?;
                state.checks = state.checks.saturating_add(1);
            }
            let warnings = check.check(&staged.0, &[])?;
            let mut state = lock_api(&self.state)?;
            if state.queued.is_empty() {
                return Err(ApiError::new(ErrorCode::Internal, "no revision queued"));
            }
            let revision = state.queued.remove(0);
            // "The candidate is answered with the check's findings, which the store keeps with
            // it as it keeps a commit's".
            state
                .warnings
                .retain(|(kept, _)| *kept != changes.mutation_id);
            state.warnings.push((changes.mutation_id, warnings.clone()));
            state.references.push((
                proposal_id,
                revision.clone(),
                changes.base.clone(),
                changes.mutation_id,
            ));
            state.candidates.push(changes);
            Ok(Committed {
                revision,
                replayed: false,
                warnings,
            })
        })
    }
    // "Accept a candidate: commit the candidate's exact tree on top of `expected_head`."
    fn promote_candidate<'a>(
        &'a self,
        scope: &'a StorageScope,
        promotion: Promotion,
    ) -> PortFuture<'a, Committed> {
        Box::pin(async move {
            lock_api(&self.state)?.attempts.push(promotion.clone());
            if self.fail_next_promote.swap(false, Ordering::SeqCst) {
                return Err(ApiError::new(ErrorCode::Unavailable, "the store failed"));
            }
            let head = self.inner.head(scope).await?;
            let mut state = lock_api(&self.state)?;
            // "Idempotent on `promotion.mutation_id` exactly as `commit` is".
            if let Some((_, revision)) = state.promotions.iter().find(|(landed, _)| {
                landed.mutation_id == promotion.mutation_id
                    && landed.expected_head == promotion.expected_head
            }) {
                return Ok(state.replay_of(promotion.mutation_id, revision.clone()));
            }
            // "Fails with `Conflict`, writing nothing, when the head is not `expected_head`
            // (with `ErrorDetail::StaleBase` naming the head and no `field`, as `commit`)".
            if head != promotion.expected_head {
                return Err(ApiError::new(ErrorCode::Conflict, "the head moved")
                    .with_detail(ErrorDetail::StaleBase { head }));
            }
            let Some((_, candidate, base, _)) = state
                .references
                .iter()
                .find(|(held, ..)| *held == promotion.proposal_id)
                .cloned()
            else {
                return Err(ApiError::new(
                    ErrorCode::Conflict,
                    "the candidate is not the revision this proposal retains",
                ));
            };
            // "... or when `candidate` is not the revision the proposal reference retains."
            if candidate != promotion.candidate {
                return Err(ApiError::new(
                    ErrorCode::Conflict,
                    "the candidate is not the revision this proposal retains",
                ));
            }
            // "... and when the candidate's parent is not `expected_head`, which is then the head,
            // also with `StaleBase` naming the head and no `field`: by contract ruling 5 a
            // candidate drafted on a base that is not the head is stale."
            if base != promotion.expected_head {
                return Err(ApiError::new(
                    ErrorCode::Conflict,
                    "the candidate was not drafted on the current head",
                )
                .with_detail(ErrorDetail::StaleBase { head }));
            }
            if state.queued.is_empty() {
                return Err(ApiError::new(ErrorCode::Internal, "no revision queued"));
            }
            let revision = state.queued.remove(0);
            // "no check runs here and the result has no warnings ... the store keeps that empty
            // answer as it keeps a commit's warnings, so a replay answers no warnings either."
            state.warnings.push((promotion.mutation_id, Vec::new()));
            state.promotions.push((promotion, revision.clone()));
            drop(state);
            self.inner.set_head(revision.clone()).map_err(internal)?;
            Ok(Committed {
                revision,
                replayed: false,
                warnings: Vec::new(),
            })
        })
    }
    fn log<'a>(&'a self, scope: &'a StorageScope, query: LogQuery) -> PortFuture<'a, LogResponse> {
        self.inner.log(scope, query)
    }
    fn diff<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: DiffQuery,
    ) -> PortFuture<'a, DiffResponse> {
        self.inner.diff(scope, query)
    }
    fn blame<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: BlameQuery,
    ) -> PortFuture<'a, BlameResponse> {
        self.inner.blame(scope, query)
    }
    fn body_start<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        item_id: ItemId,
    ) -> PortFuture<'a, u32> {
        self.inner.body_start(scope, revision, item_id)
    }
    fn write_history<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        mutation_id: MutationId,
    ) -> PortFuture<'a, ObjectInfo> {
        self.inner.write_history(scope, revision, mutation_id)
    }
    fn committed_files<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        page: FilePage,
    ) -> PortFuture<'a, CommittedFiles> {
        self.inner.committed_files(scope, revision, page)
    }
}

impl CandidateState {
    /// A candidate or promotion that already exists, answered as a replay: "a repeat answers the
    /// same findings, `replayed` set (or the one warning `replay_warnings_unavailable` when it
    /// kept none for that candidate)".
    fn replay_of(&self, mutation: MutationId, revision: Revision) -> Committed {
        let warnings = self
            .warnings
            .iter()
            .find(|(kept, _)| *kept == mutation)
            .map_or_else(
                || vec![replay_warnings_unavailable("the fake store kept none")],
                |(_, warnings)| warnings.clone(),
            );
        Committed {
            revision,
            replayed: true,
            warnings,
        }
    }
}

/// An item file as storage writes one: frontmatter of the properties (one `key: <JSON>` line
/// each, which YAML reads as the same value), then the body.
fn item_file(properties: &std::collections::BTreeMap<String, Value>, body: &str) -> String {
    let mut text = String::from("---\n");
    for (key, value) in properties {
        text.push_str(key);
        text.push_str(": ");
        text.push_str(&value.to_string());
        text.push('\n');
    }
    text.push_str("---\n\n");
    text.push_str(body);
    text.push('\n');
    text
}

fn write(staged: &Staged, path: &WorkspacePath, text: &str) -> Result<(), ApiError> {
    let file = staged.0.join(path.as_str());
    if let Some(folder) = file.parent() {
        std::fs::create_dir_all(folder).map_err(internal)?;
    }
    std::fs::write(file, text).map_err(internal)
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
