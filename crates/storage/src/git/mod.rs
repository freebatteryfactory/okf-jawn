//! Versioned workspace content (`VersionStore`) and the workspace catalog over Git.
//!
//! Every write follows one path under the workspace's lock: scan for the mutation's trailer
//! after the expected head (a replay returns that commit), refuse a moved head, materialize the
//! base tree in `<data>/staging/<tenant>/<workspace>/<mutation>`, apply the edits there, check
//! path collisions, maintain the folder indexes and the change log, run the caller's
//! `CandidateCheck` on that directory, write the tree and the commit, and only then move the
//! reference with a compare-and-set. The staging directory is removed when the call returns.
//!
//! `write_history` writes the accepted line up to a revision as a Git bundle into the blob
//! store (`export`).

use std::sync::Arc;

use git2::{Oid, Repository};
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::conventions::NamingRules;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::history::{BlameResponse, DiffResponse, LogResponse};
use okf_jawn_contract::identity::{
    Digest, ItemId, MutationId, ProposalId, Revision, WorkspacePath,
};
use okf_jawn_contract::item::{ItemDocument, ItemSummary, TypeDefinition};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{
    BlameQuery, CandidateChanges, CandidateCheck, CommitChanges, Committed, DiffQuery,
    FolderListing, LogQuery, ObjectInfo, Page, Promotion, Provenance, StorageScope, TreeEdit,
    VersionStore,
};

use crate::blobs::{LocalBlobs, file_reader};
use crate::db::Db;

pub(crate) use item::ItemFile;
pub(crate) use read::appearance as appearance_of;
pub(crate) use read::items as items_of;

use edit::{Staged, apply, maintain};
use repo::{
    HEAD_REF, Repositories, commit_of, find_on_line, find_trailer, git, head, internal,
    materialize, message_with_trailer, reference_target, revision_of, signature, staged_tree,
    sync_directories,
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
                let staging = repositories.stage(scope, mutation_id)?;
                let bundle = staging.path.join(export::BUNDLE_FILE);
                let size = export::write_bundle(&repositories.open(scope)?, &wanted, &bundle)?;
                let object = runtime.block_on(blobs.put_for(
                    &scope.tenant_id,
                    file_reader(Some(bundle), 0, size),
                    size,
                    None,
                ))?;
                drop(staging);
                runtime.block_on(export::record(
                    &records,
                    scope,
                    mutation_id,
                    &wanted,
                    object,
                ))
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
    ) -> PortFuture<'a, Option<NamingRules>> {
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
                let repository = repositories.open_durable(scope)?;
                let expected = held_head(&repository, &changes.expected_head)?;
                let current = head(&repository)?;
                if let Some(found) =
                    find_trailer(&repository, current, expected, changes.mutation_id)?
                {
                    return Ok((replayed(found)?, named_by(&repository, found, named)?));
                }
                if current != expected {
                    return Err(moved_head());
                }
                let write = Write {
                    mutation_id: changes.mutation_id,
                    base: current,
                    author: changes.author.clone(),
                    committer: changes.author,
                    message: changes.message,
                    edits: changes.edits,
                };
                let (commit, warnings) =
                    stage_and_commit(repositories, scope, &repository, write, &*check)?;
                move_reference(&repository, HEAD_REF, commit, Some(current))?;
                let committed = Committed {
                    revision: revision_of(commit)?,
                    replayed: false,
                    warnings,
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
    ) -> PortFuture<'a, Option<Revision>> {
        let since = since.cloned();
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
            found.map(revision_of).transpose()
        }))
    }

    fn create_candidate<'a>(
        &'a self,
        scope: &'a StorageScope,
        proposal_id: ProposalId,
        changes: CandidateChanges,
        check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Revision> {
        let named = card_objects(&changes.edits);
        let records = self.records.clone();
        let candidate = self.blocking(scope, move |repositories, scope| {
            repositories.locked(scope, || {
                refuse_locked(&records, scope)?;
                let repository = repositories.open_durable(scope)?;
                let name = proposal_reference(proposal_id);
                let base = oid(&changes.base)?;
                commit_of(&repository, &changes.base)?;
                let retained = reference_target(&repository, &name)?;
                if let Some(retained) = retained {
                    if find_trailer(&repository, retained, base, changes.mutation_id)?
                        == Some(retained)
                    {
                        return Ok((
                            revision_of(retained)?,
                            named_by(&repository, retained, named)?,
                        ));
                    }
                    return Err(ApiError::new(
                        ErrorCode::Conflict,
                        "this proposal already has a candidate from another write",
                    ));
                }
                let write = Write {
                    mutation_id: changes.mutation_id,
                    base,
                    author: changes.author.clone(),
                    committer: changes.author,
                    message: changes.message,
                    edits: changes.edits,
                };
                let (commit, _) =
                    stage_and_commit(repositories, scope, &repository, write, &*check)?;
                move_reference(&repository, &name, commit, None)?;
                sync_directories(
                    &repository,
                    &["refs", "refs/okf-jawn", "refs/okf-jawn/proposals"],
                )?;
                Ok((revision_of(commit)?, named))
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
                    promote(&repositories.open_durable(scope)?, &promotion)?,
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
}

/// The objects the source cards of `edits` name: every digest token of their appearances.
fn card_objects(edits: &[TreeEdit]) -> Vec<Digest> {
    let mut found = std::collections::BTreeSet::new();
    for edit in edits {
        if let TreeEdit::WriteSourceCard(card) = edit
            && let Ok(text) = serde_json::to_vec(&card.appearance)
        {
            crate::purge::references::tokens(&text, &mut found);
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
    let candidate = || -> Result<(Oid, Vec<Warning>), ApiError> {
        let tree = base.tree().map_err(|error| git(&error))?;
        materialize(repository, &tree, &staged_root)?;
        let mut staged = Staged::scan(&staged_root)?;
        apply(repository, &mut staged, write.edits)?;
        staged.refuse_collisions()?;
        maintain(&staged_root, &write.message)?;
        let warnings = check.check(&staged_root)?;
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
fn promote(repository: &Repository, promotion: &Promotion) -> Result<Committed, ApiError> {
    let expected = held_head(repository, &promotion.expected_head)?;
    let current = head(repository)?;
    if let Some(found) = find_trailer(repository, current, expected, promotion.mutation_id)? {
        return replayed(found);
    }
    if current != expected {
        return Err(moved_head());
    }
    let retained = reference_target(repository, &proposal_reference(promotion.proposal_id))?;
    let candidate = oid(&promotion.candidate)?;
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
        return Err(ApiError::new(
            ErrorCode::Conflict,
            "the candidate was not drafted on the current head",
        ));
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
    move_reference(repository, HEAD_REF, commit, Some(current))?;
    Ok(Committed {
        revision: revision_of(commit)?,
        replayed: false,
        warnings: Vec::new(),
    })
}

/// Point `name` at `commit`, only if it still points at `current` (or does not exist).
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
        if matches!(
            error.code(),
            git2::ErrorCode::Modified | git2::ErrorCode::Exists
        ) {
            moved_head()
        } else {
            git(&error)
        }
    })
}

fn replayed(found: Oid) -> Result<Committed, ApiError> {
    Ok(Committed {
        revision: revision_of(found)?,
        replayed: true,
        warnings: Vec::new(),
    })
}

fn moved_head() -> ApiError {
    ApiError::new(
        ErrorCode::Conflict,
        "the workspace head moved since the change was prepared",
    )
}

/// The commit an `expected_head` names. One the repository does not hold (stale after a
/// purge rewrote the line, or never held) cannot be the head, so it answers `Conflict` like a
/// moved head, not `Unavailable`.
fn held_head(repository: &Repository, expected: &Revision) -> Result<Oid, ApiError> {
    let id = oid(expected)?;
    match repository.find_commit(id) {
        Ok(_) => Ok(id),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Err(moved_head()),
        Err(error) => Err(git(&error)),
    }
}

fn oid(revision: &Revision) -> Result<Oid, ApiError> {
    Oid::from_str(revision.as_str()).map_err(|error| git(&error))
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

    /// Backup fix round, ruling 1 (the written-history form of PR #20 finding 4): a written
    /// history is recorded before it releases the workspace's lock. The test takes the lock the
    /// moment the history lets go (it is seen holding it by its staging directory) and reads the
    /// record there.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_written_history_is_recorded_before_it_releases_the_lock() -> Result<(), ApiError> {
        let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
        let storage = Storage::open(directory.path())?;
        let (scope, head) = notes(&storage).await?;
        let mutation = MutationId(uuid::Uuid::from_u128(2));
        let staging = storage
            .data()
            .staging_path()
            .join("local")
            .join(scope.workspace_id.0.to_string())
            .join(mutation.0.to_string());
        let writer = {
            let (versions, scope) = (storage.versions(), scope.clone());
            tokio::spawn(async move { versions.write_history(&scope, &head, mutation).await })
        };
        let started = std::time::Instant::now();
        while !staging.exists() {
            if writer.is_finished() || started.elapsed() > Duration::from_secs(60) {
                return Err(fault(&"the history was never seen holding the lock"));
            }
            tokio::task::yield_now().await;
        }
        let recorded = {
            let (storage, scope) = (storage.clone(), scope.clone());
            tokio::task::spawn_blocking(move || {
                storage.repositories.locked(&scope, || {
                    let key = mutation.0.to_string();
                    tokio::runtime::Handle::current().block_on(storage.records.call(
                        move |connection| {
                            connection
                                .query_row(
                                    "SELECT count(*) FROM history_exports WHERE mutation_id = ?1",
                                    [key],
                                    |row| row.get::<_, i64>(0),
                                )
                                .map_err(|error| fault(&error))
                        },
                    ))
                })
            })
            .await
            .map_err(|error| fault(&error))??
        };
        writer.await.map_err(|error| fault(&error))??;
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
        // The history passes its first check at once and then waits for the lock (if it did
        // not get there first, that first check refuses it instead).
        tokio::time::sleep(Duration::from_millis(500)).await;
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
