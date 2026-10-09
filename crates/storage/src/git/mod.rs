//! Versioned workspace content (`VersionStore`) and the workspace catalog over Git.
//!
//! Every write follows one path under the workspace's lock: scan for the mutation's trailer
//! after the expected head (a replay returns that commit), refuse a moved head, materialize the
//! base tree in `<data>/staging/<tenant>/<workspace>/<mutation>`, apply the edits there, check
//! path collisions, maintain the folder indexes and the change log, run the caller's
//! `CandidateCheck` on that directory, write the tree and the commit, and only then move the
//! reference with a compare-and-set. The staging directory is removed when the call returns.

use std::sync::Arc;

use git2::{Oid, Repository};
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::conventions::NamingRules;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::history::{BlameResponse, DiffResponse, LogResponse};
use okf_jawn_contract::identity::{
    Digest, ItemId, MutationId, ProposalId, Revision, WorkspacePath,
};
use okf_jawn_contract::item::{ItemDocument, TypeDefinition};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{
    BlameQuery, CandidateChanges, CandidateCheck, CommitChanges, Committed, DiffQuery,
    FolderListing, LogQuery, Page, Promotion, Provenance, StorageScope, TreeEdit, VersionStore,
};

use edit::{Staged, apply, maintain};
use repo::{
    HEAD_REF, Repositories, commit_of, find_trailer, git, head, internal, materialize,
    message_with_trailer, reference_target, revision_of, signature, staged_tree,
};

/// `VersionStore` over one Git repository per workspace.
#[derive(Debug, Clone)]
pub struct GitVersions {
    repositories: Repositories,
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
    pub(crate) const fn new(repositories: Repositories) -> Self {
        Self { repositories }
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
        Box::pin(self.blocking(scope, move |repositories, scope| {
            repositories.locked(scope, || {
                let repository = repositories.open(scope)?;
                let expected = oid(&changes.expected_head)?;
                let current = head(&repository)?;
                if let Some(found) =
                    find_trailer(&repository, current, expected, changes.mutation_id)?
                {
                    return replayed(found);
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
                Ok(Committed {
                    revision: revision_of(commit)?,
                    replayed: false,
                    warnings,
                })
            })
        }))
    }

    fn find_commit<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        since: &'a Revision,
    ) -> PortFuture<'a, Option<Revision>> {
        let since = since.clone();
        Box::pin(self.blocking(scope, move |repositories, scope| {
            let repository = repositories.open(scope)?;
            let found = find_trailer(&repository, head(&repository)?, oid(&since)?, mutation_id)?;
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
        Box::pin(self.blocking(scope, move |repositories, scope| {
            repositories.locked(scope, || {
                let repository = repositories.open(scope)?;
                let name = proposal_reference(proposal_id);
                let base = oid(&changes.base)?;
                commit_of(&repository, &changes.base)?;
                let retained = reference_target(&repository, &name)?;
                if let Some(retained) = retained {
                    if find_trailer(&repository, retained, base, changes.mutation_id)?
                        == Some(retained)
                    {
                        return revision_of(retained);
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
                revision_of(commit)
            })
        }))
    }

    fn promote_candidate<'a>(
        &'a self,
        scope: &'a StorageScope,
        promotion: Promotion,
    ) -> PortFuture<'a, Committed> {
        Box::pin(self.blocking(scope, move |repositories, scope| {
            repositories.locked(scope, || promote(&repositories.open(scope)?, &promotion))
        }))
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
    let tree = base.tree().map_err(|error| git(&error))?;
    materialize(repository, &tree, &staging.path)?;
    let mut staged = Staged::scan(&staging.path)?;
    apply(repository, &mut staged, write.edits)?;
    staged.refuse_collisions()?;
    maintain(&staging.path, &write.message)?;
    let warnings = check.check(&staging.path)?;
    let tree = staged_tree(repository, &staging.path)?;
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
    Ok((commit, warnings))
}

/// Accept a candidate: its exact tree on top of the unchanged head, the approver committing.
fn promote(repository: &Repository, promotion: &Promotion) -> Result<Committed, ApiError> {
    let expected = oid(&promotion.expected_head)?;
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

fn oid(revision: &Revision) -> Result<Oid, ApiError> {
    Oid::from_str(revision.as_str()).map_err(|error| git(&error))
}

/// The reference that retains a proposal's candidate.
pub(crate) fn proposal_reference(proposal: ProposalId) -> String {
    format!("refs/okf-jawn/proposals/{}", proposal.0)
}

mod edit;
mod history;
mod item;
mod read;
pub(crate) mod repo;
