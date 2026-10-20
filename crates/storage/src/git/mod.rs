//! Versioned workspace content (`VersionStore`) and the workspace catalog over Git.
//!
//! Every write follows one path under the workspace's lock: scan for the mutation's trailer
//! after the expected head (a replay returns that commit), refuse a moved head, materialize the
//! base tree in `<data>/staging/<tenant>/<workspace>/<mutation>` (refusing a tree that holds two
//! spellings of one path, unless the first edit replaces the whole tree), apply the edits
//! there, check path collisions, then maintain the folder indexes and the change log (which
//! cannot make a collision that check passed, `edit`'s header), run the caller's
//! `CandidateCheck` on that directory, write the tree and the commit, and only then move the
//! reference with a compare-and-set. The staging directory is removed when the call returns.
//! The check's warnings are kept in the records database just before the reference moves, and a
//! replay answers them (`warnings`).
//!
//! `write_history` writes the accepted line up to a revision as a Git bundle into the blob
//! store (`export`); `committed_files` reads every file of a revision a page at a time, for a
//! portable export (`read`).

use std::sync::Arc;

use git2::{Oid, Repository};
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::history::{BlameResponse, DiffResponse, LogResponse};
use okf_jawn_contract::identity::{
    Digest, ItemId, MutationId, ProposalId, Revision, WorkspacePath,
};
use okf_jawn_contract::item::{ItemDocument, ItemSummary, TypeDefinition};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{
    BlameQuery, CandidateChanges, CandidateCheck, CommitChanges, Committed, CommittedFiles,
    DiffQuery, FilePage, FolderListing, LogQuery, ObjectInfo, Page, Promotion, Provenance,
    RulesFile, StorageScope, TreeEdit, VersionStore, WorkspaceTree,
};

use crate::blobs::{LocalBlobs, file_reader};
use crate::db::Db;

pub(crate) use item::ItemFile;
pub(crate) use read::appearance as appearance_of;
pub(crate) use read::items as items_of;

use edit::{Staged, apply, maintain};
use item::Holder;
use repo::{
    HEAD_REF, Repositories, commit_of, find_on_line, find_trailer, git, head, internal,
    materialize, message_with_trailer, oid_of, reference_target, revision_of, signature,
    staged_tree, sync_directories,
};

/// `VersionStore` over one Git repository per workspace.
#[derive(Debug, Clone)]
pub struct GitVersions {
    repositories: Repositories,
    /// The records database, where a written history is recorded under its mutation.
    records: Db,
    /// Where a written history is retained.
    blobs: LocalBlobs,
}

/// What a staged write commits on top of.
struct Write {
    mutation_id: MutationId,
    base: Oid,
    author: Provenance,
    committer: Provenance,
    message: String,
    edits: Vec<TreeEdit>,
}

/// The refusal of a candidate whose proposal's reference another write already set: it is about
/// no base, so it carries no `StaleBase`.
const CANDIDATE_TAKEN: &str = "this proposal already has a candidate from another write";

impl GitVersions {
    pub(crate) const fn new(repositories: Repositories, records: Db, blobs: LocalBlobs) -> Self {
        Self {
            repositories,
            records,
            blobs,
        }
    }

    /// See `VersionStore::write_history`.
    async fn history(
        &self,
        scope: &StorageScope,
        revision: &Revision,
        mutation_id: MutationId,
    ) -> Result<ObjectInfo, ApiError> {
        if let Some(object) = export::recorded(&self.records, scope, mutation_id).await? {
            return Ok(object);
        }
        crate::purge::guard::refuse_scope(&self.records, scope).await?;
        let wanted = revision.clone();
        let blobs = self.blobs.clone();
        let records = self.records.clone();
        let runtime = tokio::runtime::Handle::current();
        // The staging directory is made, written, retained and removed under the workspace's
        // write lock, like every other staged write, so no write or purge of the workspace
        // runs while it exists. The purge refusal is checked again under that lock, under which
        // a purge plans the workspace, and the history is recorded before the lock is released:
        // a purge either refuses it or finds its record in the plan.
        self.blocking(scope, move |repositories, scope| {
            repositories.locked(scope, || {
                refuse_locked(&records, scope)?;
                // A call under the same mutation that held the lock first recorded its bundle:
                // answer that one and store no second bundle that no record names.
                if let Some(object) =
                    runtime.block_on(export::recorded(&records, scope, mutation_id))?
                {
                    return Ok(object);
                }
                let staging = repositories.stage(scope, mutation_id)?;
                let bundle = staging.path.join(export::BUNDLE_FILE);
                let size = export::write_bundle(&repositories.open(scope)?, &wanted, &bundle)?;
                // A workspace-scoped put, like every other: pinned, and recorded as a pending put
                // before the bytes are retained, which the history's record lands. A stop between
                // the two leaves the pending put, so a purge of the workspace deletes the bytes.
                let (object, pinned) = runtime.block_on(blobs.put_pinned(
                    scope,
                    file_reader(Some(bundle), 0, size),
                    size,
                    None,
                ))?;
                drop(staging);
                let recorded = runtime.block_on(export::record(
                    &records,
                    scope,
                    mutation_id,
                    &wanted,
                    object,
                ));
                // The write ends only once its record transaction has ended.
                pinned.end()?;
                #[cfg(test)]
                runtime.block_on(crate::pause::point(&history_recorded_point(
                    repositories,
                    scope,
                    mutation_id,
                )))?;
                recorded
            })
        })
        .await
    }

    /// Run a Git write: refused while a purge names the workspace (checked here, before the
    /// write is staged, and again by the write under the workspace's lock, `refuse_locked`).
    /// On success the write returns, beside its value, the objects whose pending puts land
    /// (`puts`): those the stored commit's cards name, since that commit is durable. A replay
    /// answered by an earlier commit lands only the objects that commit names, so a retry's
    /// different bytes keep their pending puts. A crash between the commit and the landing
    /// leaves pending puts of referenced objects, which cost nothing.
    async fn guarded<T>(
        &self,
        scope: &StorageScope,
        write: impl std::future::Future<Output = Result<(T, Vec<Digest>), ApiError>>,
    ) -> Result<T, ApiError> {
        crate::purge::guard::refuse_scope(&self.records, scope).await?;
        let (written, landed) = write.await?;
        if !landed.is_empty() {
            let (tenant, workspace) = crate::db::scope_key(scope);
            self.records
                .transaction(move |transaction| {
                    crate::puts::land(transaction, &tenant, &workspace, &landed)
                })
                .await?;
        }
        Ok(written)
    }

    async fn blocking<T, F>(&self, scope: &StorageScope, work: F) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: FnOnce(&Repositories, &StorageScope) -> Result<T, ApiError> + Send + 'static,
    {
        let repositories = self.repositories.clone();
        let scope = scope.clone();
        tokio::task::spawn_blocking(move || work(&repositories, &scope))
            .await
            .map_err(|error| internal(format!("a version store task stopped: {error}")))?
    }
}

impl VersionStore for GitVersions {
    fn head<'a>(&'a self, scope: &'a StorageScope) -> PortFuture<'a, Revision> {
        Box::pin(self.blocking(scope, |repositories, scope| {
            revision_of(head(&repositories.open(scope)?)?)
        }))
    }

    fn list<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        folder: Option<&'a WorkspacePath>,
        page: Page,
    ) -> PortFuture<'a, FolderListing> {
        let revision = revision.clone();
        let folder = folder.cloned();
        Box::pin(self.blocking(scope, move |repositories, scope| {
            read::list(
                &repositories.open(scope)?,
                &revision,
                folder.as_ref(),
                &page,
            )
        }))
    }

    fn tree<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
    ) -> PortFuture<'a, WorkspaceTree> {
        let revision = revision.clone();
        Box::pin(self.blocking(scope, move |repositories, scope| {
            read::tree(&repositories.open(scope)?, &revision)
        }))
    }

    fn item_at_path<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        path: &'a WorkspacePath,
    ) -> PortFuture<'a, Option<ItemSummary>> {
        let revision = revision.clone();
        let path = path.clone();
        Box::pin(self.blocking(scope, move |repositories, scope| {
            read::item_at_path(&repositories.open(scope)?, &revision, &path)
        }))
    }

    fn show<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        item: ItemId,
    ) -> PortFuture<'a, ItemDocument> {
        let revision = revision.clone();
        Box::pin(self.blocking(scope, move |repositories, scope| {
            read::show(&repositories.open(scope)?, &revision, item)
        }))
    }

    fn read_file<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        path: &'a WorkspacePath,
    ) -> PortFuture<'a, Vec<u8>> {
        let revision = revision.clone();
        let path = path.as_str().to_owned();
        Box::pin(self.blocking(scope, move |repositories, scope| {
            read::file(&repositories.open(scope)?, &revision, &path)?
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "No file at that path here"))
        }))
    }

    fn rules<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
    ) -> PortFuture<'a, Option<RulesFile>> {
        let revision = revision.clone();
        Box::pin(self.blocking(scope, move |repositories, scope| {
            read::rules(&repositories.open(scope)?, &revision)
        }))
    }

    fn types<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
    ) -> PortFuture<'a, Vec<TypeDefinition>> {
        let revision = revision.clone();
        Box::pin(self.blocking(scope, move |repositories, scope| {
            read::types(&repositories.open(scope)?, &revision)
        }))
    }

    fn correction<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        item: ItemId,
        digest: &'a Digest,
    ) -> PortFuture<'a, Option<String>> {
        let revision = revision.clone();
        let digest = digest.clone();
        Box::pin(self.blocking(scope, move |repositories, scope| {
            read::correction(&repositories.open(scope)?, &revision, item, &digest)
        }))
    }

    fn commit<'a>(
        &'a self,
        scope: &'a StorageScope,
        changes: CommitChanges,
        check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Committed> {
        let named = card_objects(&changes.edits);
        let records = self.records.clone();
        let commit = self.blocking(scope, move |repositories, scope| {
            repositories.locked(scope, || {
                refuse_locked(&records, scope)?;
                // A test holds a commit here: under the workspace's lock, before its head
                // comparison (`crate::pause`).
                #[cfg(test)]
                tokio::runtime::Handle::current().block_on(crate::pause::point(
                    &commit_locked_point(repositories, scope, changes.mutation_id),
                ))?;
                let repository = repositories.open_durable(scope)?;
                let expected = held_head(&repository, &changes.expected_head)?;
                let current = head(&repository)?;
                if let Some(found) =
                    find_trailer(&repository, current, expected, changes.mutation_id)?
                {
                    let replay = warnings::replayed(&records, scope, changes.mutation_id, found)?;
                    return Ok((replay, named_by(&repository, found, named)?));
                }
                if current != expected {
                    return Err(moved_head(&repository));
                }
                let mutation_id = changes.mutation_id;
                let write = Write {
                    mutation_id,
                    base: current,
                    author: changes.author.clone(),
                    committer: changes.author,
                    message: changes.message,
                    edits: changes.edits,
                };
                let (commit, given) =
                    stage_and_commit(repositories, scope, &repository, write, &*check)?;
                // Kept before the commit lands, for this commit by its identity, so a replay
                // of it answers them; a row a failed landing leaves is replaced by the next
                // attempt.
                warnings::keep(&records, scope, mutation_id, commit, current, &given)?;
                move_reference(&repository, HEAD_REF, commit, Some(current))?;
                let committed = Committed {
                    revision: revision_of(commit)?,
                    replayed: false,
                    warnings: given,
                };
                Ok((committed, named))
            })
        });
        Box::pin(self.guarded(scope, commit))
    }

    /// `Some(since)` walks the head's history after `since`, as a commit's replay scan does; a
    /// `since` the repository does not hold is `NotFound`.
    ///
    /// `None` walks `refs/heads/main` from the head back through first parents, newest first,
    /// to the root commit, with no time bound: a mutation whose lease expired without
    /// completion or release is kept until it is reconciled (SPEC section 8), however old its
    /// commit. Proposal candidates and every other reference are never examined. Core asks
    /// this only on a resumed attempt, so the walk's cost falls on resumes.
    fn find_commit<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        since: Option<&'a Revision>,
    ) -> PortFuture<'a, Option<Committed>> {
        let since = since.cloned();
        let records = self.records.clone();
        Box::pin(self.blocking(scope, move |repositories, scope| {
            let repository = repositories.open(scope)?;
            let tip = head(&repository)?;
            let found = if let Some(since) = since {
                // A `since` this repository never held is the caller's error: `NotFound`.
                let since = commit_of(&repository, &since)?.id();
                find_trailer(&repository, tip, since, mutation_id)?
            } else {
                find_on_line(&repository, tip, mutation_id)?
            };
            found
                .map(|found| warnings::replayed(&records, scope, mutation_id, found))
                .transpose()
        }))
    }

    fn create_candidate<'a>(
        &'a self,
        scope: &'a StorageScope,
        proposal_id: ProposalId,
        changes: CandidateChanges,
        check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Committed> {
        let named = card_objects(&changes.edits);
        let records = self.records.clone();
        let candidate = self.blocking(scope, move |repositories, scope| {
            repositories.locked(scope, || {
                refuse_locked(&records, scope)?;
                let repository = repositories.open_durable(scope)?;
                let name = proposal_reference(proposal_id);
                let base = oid_of(&changes.base)?;
                commit_of(&repository, &changes.base)?;
                let retained = reference_target(&repository, &name)?;
                if let Some(retained) = retained {
                    if find_trailer(&repository, retained, base, changes.mutation_id)?
                        == Some(retained)
                    {
                        let replay =
                            warnings::replayed(&records, scope, changes.mutation_id, retained)?;
                        return Ok((replay, named_by(&repository, retained, named)?));
                    }
                    return Err(ApiError::new(ErrorCode::Conflict, CANDIDATE_TAKEN));
                }
                let mutation_id = changes.mutation_id;
                let write = Write {
                    mutation_id,
                    base,
                    author: changes.author.clone(),
                    committer: changes.author,
                    message: changes.message,
                    edits: changes.edits,
                };
                let (commit, given) =
                    stage_and_commit(repositories, scope, &repository, write, &*check)?;
                // Kept before the reference names the candidate, as a commit's are, so a
                // repeat answers them.
                warnings::keep(&records, scope, mutation_id, commit, base, &given)?;
                move_reference(&repository, &name, commit, None)?;
                sync_directories(
                    &repository,
                    &["refs", "refs/okf-jawn", "refs/okf-jawn/proposals"],
                )?;
                let committed = Committed {
                    revision: revision_of(commit)?,
                    replayed: false,
                    warnings: given,
                };
                Ok((committed, named))
            })
        });
        Box::pin(self.guarded(scope, candidate))
    }

    fn promote_candidate<'a>(
        &'a self,
        scope: &'a StorageScope,
        promotion: Promotion,
    ) -> PortFuture<'a, Committed> {
        let records = self.records.clone();
        let promoted = self.blocking(scope, move |repositories, scope| {
            repositories.locked(scope, || {
                refuse_locked(&records, scope)?;
                // A promotion stores the candidate's tree, whose objects landed with the
                // candidate.
                Ok((
                    promote(
                        &repositories.open_durable(scope)?,
                        &records,
                        scope,
                        &promotion,
                    )?,
                    Vec::new(),
                ))
            })
        });
        Box::pin(self.guarded(scope, promoted))
    }

    fn log<'a>(&'a self, scope: &'a StorageScope, query: LogQuery) -> PortFuture<'a, LogResponse> {
        Box::pin(self.blocking(scope, move |repositories, scope| {
            history::log(&repositories.open(scope)?, &query)
        }))
    }

    fn diff<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: DiffQuery,
    ) -> PortFuture<'a, DiffResponse> {
        Box::pin(self.blocking(scope, move |repositories, scope| {
            history::diff(&repositories.open(scope)?, &query)
        }))
    }

    fn blame<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: BlameQuery,
    ) -> PortFuture<'a, BlameResponse> {
        Box::pin(self.blocking(scope, move |repositories, scope| {
            history::blame(&repositories.open(scope)?, scope, &query)
        }))
    }

    fn body_start<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        item_id: ItemId,
    ) -> PortFuture<'a, u32> {
        let revision = revision.clone();
        Box::pin(self.blocking(scope, move |repositories, scope| {
            history::body_start(&repositories.open(scope)?, &revision, item_id)
        }))
    }

    fn write_history<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        mutation_id: MutationId,
    ) -> PortFuture<'a, ObjectInfo> {
        Box::pin(self.history(scope, revision, mutation_id))
    }

    fn committed_files<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        page: FilePage,
    ) -> PortFuture<'a, CommittedFiles> {
        let revision = revision.clone();
        Box::pin(self.blocking(scope, move |repositories, scope| {
            read::committed_files(&repositories.open(scope)?, &revision, &page)
        }))
    }
}

/// The objects the source cards of `edits` name, read structurally as a purge reads them
/// (`purge::references`): each appearance's original (`object`) and its conversion record
/// (`extraction.digest`). A digest only in a card's text names nothing, so it lands no pending
/// put, and a purge of the workspace still deletes that put's bytes.
fn card_objects(edits: &[TreeEdit]) -> Vec<Digest> {
    let mut found = std::collections::BTreeSet::new();
    for edit in edits {
        if let TreeEdit::WriteSourceCard(card) = edit {
            found.insert(card.appearance.object.clone());
            if let Some(record) = &card.appearance.extraction.digest {
                found.insert(record.clone());
            }
        }
    }
    found.into_iter().collect()
}

/// The objects of `named` (a write's cards) that the stored commit `stored` names, for a write
/// answered by that earlier commit: a retry that put different bytes keeps their pending puts.
fn named_by(
    repository: &Repository,
    stored: Oid,
    named: Vec<Digest>,
) -> Result<Vec<Digest>, ApiError> {
    let appearances = crate::purge::references::appearances(repository, stored)?;
    Ok(named
        .into_iter()
        .filter(|digest| appearances.contains(digest))
        .collect())
}

/// Stage `write.base`, apply the edits, check, and write the commit (no reference moves).
fn stage_and_commit(
    repositories: &Repositories,
    scope: &StorageScope,
    repository: &Repository,
    write: Write,
    check: &dyn CandidateCheck,
) -> Result<(Oid, Vec<Warning>), ApiError> {
    let staging = repositories.stage(scope, write.mutation_id)?;
    let base = repository
        .find_commit(write.base)
        .map_err(|error| git(&error))?;
    let staged_root = staging.path.clone();
    // A first edit that replaces the whole tree reads nothing of the base, so the base is not
    // staged: a base that cannot be staged (two paths sharing a collision key, `materialize`)
    // can still be replaced by an earlier revision that can.
    let replaced = matches!(write.edits.first(), Some(TreeEdit::RestoreWorkspace { .. }));
    let candidate = || -> Result<(Oid, Vec<Warning>), ApiError> {
        let tree = base.tree().map_err(|error| git(&error))?;
        if !replaced {
            materialize(repository, &tree, &staged_root, Holder::Head)?;
        }
        let mut staged = Staged::scan(&staged_root)?;
        apply(repository, &mut staged, write.edits)?;
        // Before the indexes and the log are written: what they write cannot collide once this
        // passes (`Staged::refuse_collisions`), so the rule holds on what is committed.
        staged.refuse_collisions()?;
        maintain(&staged_root, &write.message)?;
        // The check is told the files the edits made the store rewrite on its own; what the
        // edits report about themselves follows its findings (`Committed::warnings`).
        let mut warnings = check.check(&staged_root, &staged.rewritten()?)?;
        warnings.extend(staged.notes().iter().cloned());
        Ok((staged_tree(repository, &staged_root)?, warnings))
    };
    let (tree, warnings) = candidate().map_err(|error| without_staging(error, &staging.path))?;
    let tree = repository.find_tree(tree).map_err(|error| git(&error))?;
    let commit = repository
        .commit(
            None,
            &signature(&write.author)?,
            &signature(&write.committer)?,
            &message_with_trailer(&write.message, write.mutation_id),
            &tree,
            &[&base],
        )
        .map_err(|error| git(&error))?;
    sync_directories(repository, &["objects"])?;
    Ok((commit, warnings))
}

/// An error with the server's staging directory removed from its message, so a refusal names
/// the workspace path the request gave and never a path on the server.
fn without_staging(mut error: ApiError, staging: &std::path::Path) -> ApiError {
    let shown = staging.display().to_string();
    for prefix in [
        format!("{shown}{}", std::path::MAIN_SEPARATOR),
        format!("{shown}/"),
        shown.clone(),
    ] {
        error.message = error.message.replace(&prefix, "");
    }
    error
}

/// Accept a candidate: its exact tree on top of the unchanged head, the approver committing.
fn promote(
    repository: &Repository,
    records: &Db,
    scope: &StorageScope,
    promotion: &Promotion,
) -> Result<Committed, ApiError> {
    let expected = held_head(repository, &promotion.expected_head)?;
    let current = head(repository)?;
    if let Some(found) = find_trailer(repository, current, expected, promotion.mutation_id)? {
        return warnings::replayed(records, scope, promotion.mutation_id, found);
    }
    if current != expected {
        return Err(moved_head(repository));
    }
    let retained = reference_target(repository, &proposal_reference(promotion.proposal_id))?;
    let candidate = oid_of(&promotion.candidate)?;
    if retained != Some(candidate) {
        return Err(ApiError::new(
            ErrorCode::Conflict,
            "the candidate is not the revision this proposal retains",
        ));
    }
    let candidate = repository
        .find_commit(candidate)
        .map_err(|error| git(&error))?;
    if candidate.parent_ids().collect::<Vec<_>>() != vec![expected] {
        // Contract ruling 5: a base that is not the head is stale, so the refusal names the head,
        // which is `expected` here (a head that moved was refused above).
        return Err(ApiError::new(
            ErrorCode::Conflict,
            "the candidate was not drafted on the current head",
        )
        .with_detail(ErrorDetail::StaleBase {
            head: revision_of(current)?,
        }));
    }
    let parent = repository
        .find_commit(current)
        .map_err(|error| git(&error))?;
    let tree = candidate.tree().map_err(|error| git(&error))?;
    let commit = repository
        .commit(
            None,
            &candidate.author(),
            &signature(&promotion.approver)?,
            &message_with_trailer(&promotion.message, promotion.mutation_id),
            &tree,
            &[&parent],
        )
        .map_err(|error| git(&error))?;
    sync_directories(repository, &["objects"])?;
    // No check ran, so the answer kept for a replay is the empty one this call gives.
    warnings::keep(records, scope, promotion.mutation_id, commit, current, &[])?;
    move_reference(repository, HEAD_REF, commit, Some(current))?;
    Ok(Committed {
        revision: revision_of(commit)?,
        replayed: false,
        warnings: Vec::new(),
    })
}

/// Point `name` at `commit`, only if it still points at `current`; with no `current`, only if
/// it does not exist yet.
///
/// A lost compare-and-set of the head (`current` given) means the head is not the expected one:
/// `moved_head`, with `StaleBase`. A candidate's reference that another write set meanwhile
/// (no `current`) is about no base: a plain `Conflict` (`CANDIDATE_TAKEN`).
fn move_reference(
    repository: &Repository,
    name: &str,
    commit: Oid,
    current: Option<Oid>,
) -> Result<(), ApiError> {
    let moved = match current {
        Some(current) => {
            repository.reference_matching(name, commit, true, current, "okf-jawn commit")
        }
        None => repository.reference(name, commit, false, "okf-jawn candidate"),
    };
    moved.map(|_| ()).map_err(|error| {
        if !matches!(
            error.code(),
            git2::ErrorCode::Modified | git2::ErrorCode::Exists
        ) {
            return git(&error);
        }
        match current {
            Some(_) => moved_head(repository),
            None => ApiError::new(ErrorCode::Conflict, CANDIDATE_TAKEN),
        }
    })
}

/// The refusal of a write whose expected head is not the head: `Conflict` with
/// `ErrorDetail::StaleBase` naming the head it was compared with; the caller, which knows the
/// request, names its field. A head that cannot be read is answered as that read's error.
fn moved_head(repository: &Repository) -> ApiError {
    match head(repository).and_then(revision_of) {
        Ok(head) => ApiError::new(
            ErrorCode::Conflict,
            "the workspace head moved since the change was prepared",
        )
        .with_detail(ErrorDetail::StaleBase { head }),
        Err(error) => error,
    }
}

/// The commit an `expected_head` names. One the repository does not hold (stale after a
/// purge rewrote the line, or never held) cannot be the head, so it answers `Conflict` like a
/// moved head, not `Unavailable`. One that cannot name a commit here at all (64 hex
/// characters, `repo::oid_of`) is `NotFound`.
fn held_head(repository: &Repository, expected: &Revision) -> Result<Oid, ApiError> {
    let id = oid_of(expected)?;
    match repository.find_commit(id) {
        Ok(_) => Ok(id),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Err(moved_head(repository)),
        Err(error) => Err(git(&error)),
    }
}

/// The name of the test-only pause point (`crate::pause`) of a written history under
/// `mutation`: after it is recorded, before it releases the workspace's lock. It names the
/// workspace's repository path, unique to each test's data directory.
#[cfg(test)]
fn history_recorded_point(
    repositories: &Repositories,
    scope: &StorageScope,
    mutation: MutationId,
) -> String {
    format!(
        "history-recorded|{}|{}",
        repositories.path(scope).display(),
        mutation.0
    )
}

/// The name of the test-only pause point (`crate::pause`) of a commit under `mutation`: holding
/// the workspace's write lock, before its head comparison. It names the workspace's repository
/// path, unique to each test's data directory.
#[cfg(test)]
pub(crate) fn commit_locked_point(
    repositories: &Repositories,
    scope: &StorageScope,
    mutation: MutationId,
) -> String {
    format!(
        "commit-locked|{}|{}",
        repositories.path(scope).display(),
        mutation.0
    )
}

/// The reference that retains a proposal's candidate.
/// Refuse a write to `scope` a purge names, from inside the workspace's lock (blocking). The
/// purge's plan reads the workspace under the same lock, so a commit, candidate or promotion
/// either landed before that read or is refused.
fn refuse_locked(records: &Db, scope: &StorageScope) -> Result<(), ApiError> {
    tokio::runtime::Handle::current().block_on(crate::purge::guard::refuse_scope(records, scope))
}

pub(crate) fn proposal_reference(proposal: ProposalId) -> String {
    format!("refs/okf-jawn/proposals/{}", proposal.0)
}

mod edit;
mod export;
mod history;
mod item;
mod read;
pub(crate) mod repo;
mod warnings;

/// Unit tests here, not in `tests/`: they hold a workspace's write lock, which is private.
/// They return `ApiError` like core's unit tests.
#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use okf_jawn_contract::access::AccessRoute;
    use okf_jawn_contract::error::{ApiError, ErrorCode};
    use okf_jawn_contract::identity::{MutationId, TenantId};
    use okf_jawn_core::storage::{
        NewWorkspace, Provenance, StorageScope, VersionStore, WorkspaceCatalog,
    };

    use crate::storage::Storage;

    fn fault(message: &dyn std::fmt::Display) -> ApiError {
        ApiError::new(ErrorCode::Internal, message.to_string())
    }

    /// A workspace "Notes" of tenant `local` created under mutation 1, and its head.
    async fn notes(
        storage: &Storage,
    ) -> Result<(StorageScope, okf_jawn_contract::identity::Revision), ApiError> {
        let tenant = TenantId::try_from("local".to_owned()).map_err(|error| fault(&error))?;
        let created = storage
            .catalog()
            .create(
                &tenant,
                MutationId(uuid::Uuid::from_u128(1)),
                NewWorkspace {
                    name: "Notes".to_owned(),
                    description: String::new(),
                    creator: Provenance {
                        subject: "ana".to_owned(),
                        route: AccessRoute::LocalOwner,
                        client_id: None,
                    },
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

    /// Wait until a caller of `scope`'s write lock besides the test's own holder has taken the
    /// lock's handle: it passed every check it makes before the lock, and it waits for the lock.
    async fn until_waiting_for_the_lock(
        storage: &Storage,
        scope: &StorageScope,
    ) -> Result<(), ApiError> {
        let started = std::time::Instant::now();
        while storage.repositories.lock_users(scope)? < 2 {
            if started.elapsed() > Duration::from_secs(120) {
                return Err(fault(&"no write began to wait for the workspace's lock"));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(())
    }

    /// Backup fix round, ruling 1 (the written-history form of PR #20 finding 4): a written
    /// history is recorded before it releases the workspace's lock. The history is held at its
    /// pause point after its record, inside the lock: the lock is still its own, and the record
    /// is already there.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_written_history_is_recorded_before_it_releases_the_lock() -> Result<(), ApiError> {
        let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
        let storage = Storage::open(directory.path())?;
        let (scope, head) = notes(&storage).await?;
        let mutation = MutationId(uuid::Uuid::from_u128(2));
        let (arrival, release) = crate::pause::hold(super::history_recorded_point(
            &storage.repositories,
            &scope,
            mutation,
        ))?;
        let writer = {
            let (versions, scope) = (storage.versions(), scope.clone());
            tokio::spawn(async move { versions.write_history(&scope, &head, mutation).await })
        };
        crate::pause::arrived(arrival).await?;
        let holding = storage.repositories.lock_users(&scope)?;
        let key = mutation.0.to_string();
        let recorded = storage
            .records
            .call(move |connection| {
                connection
                    .query_row(
                        "SELECT count(*) FROM history_exports WHERE mutation_id = ?1",
                        [key],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(|error| fault(&error))
            })
            .await?;
        release.send(()).map_err(|error| fault(&error))?;
        writer.await.map_err(|error| fault(&error))??;
        assert_eq!(holding, 1, "the history still holds the workspace's lock");
        assert_eq!(recorded, 1, "recorded while the lock was still held");
        Ok(())
    }

    /// Backup fix round, ruling 1: a written history that passed its first check and waits for
    /// the workspace's lock while the purge is recorded is refused under the lock.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_written_history_waiting_for_the_lock_when_the_purge_is_recorded_is_refused()
    -> Result<(), ApiError> {
        use okf_jawn_core::jobs::RecordStore;
        let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
        let storage = Storage::open(directory.path())?;
        let (scope, blank) = notes(&storage).await?;
        let (held, release) = (mpsc::channel(), mpsc::channel::<()>());
        let holder = {
            let repositories = storage.repositories.clone();
            let scope = scope.clone();
            std::thread::spawn(move || {
                repositories.locked(&scope, || {
                    held.0.send(()).map_err(|error| fault(&error))?;
                    release.1.recv().map_err(|error| fault(&error))
                })
            })
        };
        held.1.recv().map_err(|error| fault(&error))?;
        let writer = {
            let (versions, scope) = (storage.versions(), scope.clone());
            tokio::spawn(async move {
                versions
                    .write_history(&scope, &blank, MutationId(uuid::Uuid::from_u128(2)))
                    .await
            })
        };
        // The history passed its first check and waits for the lock.
        until_waiting_for_the_lock(&storage, &scope).await?;
        storage
            .records()
            .create_purge(
                &scope.tenant_id,
                MutationId(uuid::Uuid::from_u128(50)),
                okf_jawn_core::jobs::NewPurge {
                    id: okf_jawn_contract::identity::PurgeId(uuid::Uuid::from_u128(60)),
                    target: okf_jawn_contract::purge::PurgeTarget::Workspace {
                        workspace_id: scope.workspace_id,
                    },
                    initiator: Provenance {
                        subject: "ana".to_owned(),
                        route: AccessRoute::LocalOwner,
                        client_id: None,
                    },
                },
            )
            .await?;
        release.0.send(()).map_err(|error| fault(&error))?;
        holder
            .join()
            .map_err(|_| fault(&"the lock holder stopped"))??;
        let error = match writer.await.map_err(|error| fault(&error))? {
            Ok(object) => return Err(fault(&format!("the history was written: {object:?}"))),
            Err(error) => error,
        };
        assert_eq!(error.code, ErrorCode::Conflict, "{error}");
        assert!(error.message.contains("purged by purge"), "{error}");
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_written_history_is_staged_only_under_the_workspace_lock() -> Result<(), ApiError> {
        let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
        let storage = Storage::open(directory.path())?;
        let tenant = TenantId::try_from("local".to_owned()).map_err(|error| fault(&error))?;
        let mutation = |n: u128| MutationId(uuid::Uuid::from_u128(n));
        let created = storage
            .catalog()
            .create(
                &tenant,
                mutation(1),
                NewWorkspace {
                    name: "Notes".to_owned(),
                    description: String::new(),
                    creator: Provenance {
                        subject: "ana".to_owned(),
                        route: AccessRoute::LocalOwner,
                        client_id: None,
                    },
                },
            )
            .await?;
        let scope = StorageScope {
            tenant_id: tenant,
            workspace_id: created.id,
        };
        let staging = storage
            .data()
            .staging_path()
            .join("local")
            .join(created.id.0.to_string())
            .join(mutation(2).0.to_string());
        let (held, release) = (mpsc::channel(), mpsc::channel::<()>());
        let holder = {
            let repositories = storage.repositories.clone();
            let scope = scope.clone();
            std::thread::spawn(move || {
                repositories.locked(&scope, || {
                    held.0.send(()).map_err(|error| fault(&error))?;
                    release.1.recv().map_err(|error| fault(&error))
                })
            })
        };
        held.1.recv().map_err(|error| fault(&error))?;
        let versions = storage.versions();
        let writer = {
            let scope = scope.clone();
            let revision = created.head.clone();
            tokio::spawn(
                async move { versions.write_history(&scope, &revision, mutation(2)).await },
            )
        };
        // Give the export ample time to start: under the lock it never does.
        for _ in 0..50 {
            if staging.exists() || writer.is_finished() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            !staging.exists() && !writer.is_finished(),
            "nothing is staged while another write holds the workspace"
        );
        release.0.send(()).map_err(|error| fault(&error))?;
        holder
            .join()
            .map_err(|_| fault(&"the lock holder stopped"))??;
        writer.await.map_err(|error| fault(&error))??;
        assert!(
            !staging.exists(),
            "the staging directory is removed afterwards"
        );
        Ok(())
    }
}
