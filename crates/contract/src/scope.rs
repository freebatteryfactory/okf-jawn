//! Every request declares in its type its authorization targets, its retry identity and the
//! rules its wire schema cannot state.
//!
//! Dispatch runs `check_rules` after decoding and before authorization, so a request that
//! breaks one is refused whatever the caller's grants. A request carrying a View document
//! checks there that the View binds only to its own workspace (SPEC section 10).
//!
//! Dispatch authorizes every target before the handler runs; a request that touches several
//! workspaces names each one, so no workspace is reached without its own grant. The first
//! target carries the operation's table permission, except `create_confirmation`, whose
//! required permission depends on the confirmed action. `Target::Authenticated` asks only
//! for a signed-in principal: session, workspace listing, catalog and health results are
//! filtered by the caller's grants instead of being gated by one.
//!
//! Purges, the installation backup, tenant jobs and tenant events authorize at the tenant
//! (`Target::Deployment(Admin)`) only: a purge removes the workspace's grants, so a workspace
//! target could not authorize the retry of a half-finished purge.

use crate::access::{
    CreateConnectorRequest, ListConnectorsRequest, Permission, RevokeConnectorRequest,
};
use crate::attention::{GetAttentionRequest, RebuildIndexRequest};
use crate::common::Empty;
use crate::conventions::{
    ApplyNamesRequest, GetRulesRequest, PreviewNamesRequest, SetRulesRequest,
};
use crate::error::{ApiError, ErrorCode};
use crate::events::{GetReceiptRequest, ListEventsRequest, ListTenantEventsRequest};
use crate::health::{HealthRequest, ReadinessRequest};
use crate::history::{BlameRequest, CommitRequest, DiffRequest, LogRequest, RestoreRequest};
use crate::identity::{IdempotencyKey, WorkspaceId};
use crate::import::{
    CancelJobRequest, CompleteUploadRequest, CorrectDigestRequest, CreateUploadRequest,
    GetJobRequest, GetTenantJobRequest, ListJobsRequest, ListTenantJobsRequest, RedigestRequest,
    RetryJobRequest, StartImportRequest,
};
use crate::item::{
    CreateFolderRequest, CreateItemRequest, DeleteItemRequest, DiscardDraftRequest, GetItemRequest,
    ItemStatus, ListDraftsRequest, ListItemsRequest, ListTypesRequest, MoveItemRequest,
    SaveDraftRequest, SetLifecycleRequest, SetTypeRequest,
};
use crate::proposal::{
    AcceptProposalRequest, AddCommentRequest, DeclineProposalRequest, GetProposalRequest,
    ListProposalsRequest, OpenProposalRequest, ProposalKind,
};
use crate::purge::{GetPurgeRequest, PurgeItemRequest, PurgeWorkspaceRequest};
use crate::read::{CreateSandboxCapabilityRequest, ReadItemRequest};
use crate::review::{
    ConfirmationAction, CreateConfirmationRequest, CreateReviewRequest, ListReviewsRequest,
};
use crate::search::{GetGraphRequest, GetLinksRequest, SearchRequest};
use crate::source::{GetObjectRequest, GetSourcesRequest};
use crate::views::{
    ExportViewRequest, GetCatalogRequest, GetViewRequest, PresentRequest, ResolveViewRequest,
};
use crate::workspace::{
    ArchiveWorkspaceRequest, BackupInstallationRequest, BackupWorkspaceRequest,
    CreateWorkspaceRequest, ExportWorkspaceRequest, ListWorkspacesRequest, OpenWorkspaceRequest,
    RestoreWorkspaceRequest, UnarchiveWorkspaceRequest, UpdateWorkspaceRequest,
};

macro_rules! workspace_read {
    ($($request:ty),* $(,)?) => {
        $(impl RequestScope for $request {
            fn targets(&self) -> Vec<Target> {
                vec![Target::Workspace(self.workspace_id, Permission::Read)]
            }
            fn idempotency_key(&self) -> Option<&IdempotencyKey> {
                None
            }
        })*
    };
}

macro_rules! workspace_keyed {
    ($permission:ident: $($request:ty),* $(,)?) => {
        $(impl RequestScope for $request {
            fn targets(&self) -> Vec<Target> {
                vec![Target::Workspace(self.workspace_id, Permission::$permission)]
            }
            fn idempotency_key(&self) -> Option<&IdempotencyKey> {
                Some(&self.idempotency_key)
            }
        })*
    };
}

macro_rules! tenant_admin_keyed {
    ($($request:ty),* $(,)?) => {
        $(impl RequestScope for $request {
            fn targets(&self) -> Vec<Target> {
                vec![Target::Deployment(Permission::Admin)]
            }
            fn idempotency_key(&self) -> Option<&IdempotencyKey> {
                Some(&self.idempotency_key)
            }
        })*
    };
}

macro_rules! tenant_admin_read {
    ($($request:ty),* $(,)?) => {
        $(impl RequestScope for $request {
            fn targets(&self) -> Vec<Target> {
                vec![Target::Deployment(Permission::Admin)]
            }
            fn idempotency_key(&self) -> Option<&IdempotencyKey> {
                None
            }
        })*
    };
}

macro_rules! authenticated {
    ($($request:ty),* $(,)?) => {
        $(impl RequestScope for $request {
            fn targets(&self) -> Vec<Target> {
                vec![Target::Authenticated]
            }
            fn idempotency_key(&self) -> Option<&IdempotencyKey> {
                None
            }
        })*
    };
}

/// A resource the caller must hold a permission on before the handler runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    /// Any signed-in principal; no grant lookup. The handler filters results by grants.
    Authenticated,
    /// The caller's tenant within this deployment, checked through the tenant grant.
    Deployment(Permission),
    /// One workspace, checked through that workspace's grant.
    Workspace(WorkspaceId, Permission),
}

/// What a replay of a completed mutation under the same key returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReplayPolicy {
    /// Return the stored response unchanged.
    StoredResponse,
    /// Return `already_issued` with the created identity; the response held a secret that is
    /// never stored or replayed.
    AlreadyIssued {
        /// JSON pointer into the response to the created identity.
        id_pointer: &'static str,
    },
}

/// Authorization targets and retry identity, implemented by every operation request.
pub trait RequestScope {
    /// Replay behaviour for this operation's completed mutations.
    const REPLAY: ReplayPolicy = ReplayPolicy::StoredResponse;

    /// Every resource that must be authorized, first the one named by the table permission.
    fn targets(&self) -> Vec<Target>;

    /// Caller-chosen retry identity; present exactly when the request is a mutation.
    fn idempotency_key(&self) -> Option<&IdempotencyKey>;

    /// Refuse a decoded request that breaks a rule its wire schema cannot state.
    ///
    /// Dispatch calls this before any grant lookup or handler, so the refusal does not depend
    /// on who asks. Most requests have no such rule.
    ///
    /// # Errors
    /// Returns `InvalidInput` naming the offending field.
    fn check_rules(&self) -> Result<(), ApiError> {
        Ok(())
    }
}

impl Target {
    /// The capability this target requires; sign-in alone is reported as `Read`.
    #[must_use]
    pub const fn permission(self) -> Permission {
        match self {
            Self::Authenticated => Permission::Read,
            Self::Deployment(permission) | Self::Workspace(_, permission) => permission,
        }
    }
}

workspace_read!(
    OpenWorkspaceRequest,
    ListItemsRequest,
    GetItemRequest,
    ListDraftsRequest,
    ListTypesRequest,
    ReadItemRequest,
    GetSourcesRequest,
    CreateSandboxCapabilityRequest,
    GetLinksRequest,
    GetGraphRequest,
    LogRequest,
    DiffRequest,
    BlameRequest,
    ListProposalsRequest,
    GetProposalRequest,
    ListReviewsRequest,
    GetJobRequest,
    ListJobsRequest,
    GetRulesRequest,
    PreviewNamesRequest,
    GetAttentionRequest,
    GetViewRequest,
    ResolveViewRequest,
    GetReceiptRequest,
    ListEventsRequest,
);

workspace_keyed!(Write:
    UpdateWorkspaceRequest,
    ExportWorkspaceRequest,
    CreateItemRequest,
    SaveDraftRequest,
    DiscardDraftRequest,
    MoveItemRequest,
    DeleteItemRequest,
    CreateFolderRequest,
    SetTypeRequest,
    CommitRequest,
    RestoreRequest,
    CreateUploadRequest,
    CompleteUploadRequest,
    StartImportRequest,
    RetryJobRequest,
    CancelJobRequest,
    RedigestRequest,
    CorrectDigestRequest,
    SetRulesRequest,
    ApplyNamesRequest,
    ExportViewRequest,
);
workspace_keyed!(Approve: AcceptProposalRequest, DeclineProposalRequest);
workspace_keyed!(Admin:
    ArchiveWorkspaceRequest,
    UnarchiveWorkspaceRequest,
    BackupWorkspaceRequest,
    RestoreWorkspaceRequest,
    RebuildIndexRequest,
);

tenant_admin_keyed!(
    PurgeWorkspaceRequest,
    PurgeItemRequest,
    BackupInstallationRequest,
);

tenant_admin_read!(
    GetPurgeRequest,
    GetTenantJobRequest,
    ListTenantJobsRequest,
    ListTenantEventsRequest,
);

authenticated!(
    ListWorkspacesRequest,
    GetCatalogRequest,
    Empty,
    HealthRequest,
    ReadinessRequest,
);

impl RequestScope for CreateWorkspaceRequest {
    fn targets(&self) -> Vec<Target> {
        vec![Target::Deployment(Permission::Admin)]
    }
    fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        Some(&self.idempotency_key)
    }
}

impl RequestScope for SearchRequest {
    fn targets(&self) -> Vec<Target> {
        vec![Target::Workspace(self.workspace_id, Permission::Read)]
    }
    fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        None
    }
    /// An empty query matches nothing to rank; it lists sources only through a filter.
    fn check_rules(&self) -> Result<(), ApiError> {
        if self.query.trim().is_empty() && self.extraction.is_none() {
            Err(ApiError::new(
                ErrorCode::InvalidInput,
                "The query may be empty only with an extraction filter",
            )
            .with_field("/query"))
        } else {
            Ok(())
        }
    }
}

impl RequestScope for SetLifecycleRequest {
    fn targets(&self) -> Vec<Target> {
        vec![Target::Workspace(self.workspace_id, Permission::Write)]
    }
    fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        Some(&self.idempotency_key)
    }
    /// A change names a status word, the archive flag, or both; `other` is a word only a
    /// producer outside the application writes.
    fn check_rules(&self) -> Result<(), ApiError> {
        if self.status.is_none() && self.archived.is_none() {
            return Err(ApiError::new(
                ErrorCode::InvalidInput,
                "Set the status, the archived flag, or both",
            ));
        }
        if self.status == Some(ItemStatus::Other) {
            return Err(ApiError::new(
                ErrorCode::InvalidInput,
                "The status must be draft, stable or deprecated",
            )
            .with_field("/status"));
        }
        Ok(())
    }
}

impl RequestScope for OpenProposalRequest {
    fn targets(&self) -> Vec<Target> {
        vec![Target::Workspace(self.workspace_id, Permission::Propose)]
    }
    fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        Some(&self.idempotency_key)
    }
    /// A supply proposal holds only supply changes, each for a different item.
    fn check_rules(&self) -> Result<(), ApiError> {
        ProposalKind::of(&self.changes).map(|_| ())
    }
}

impl RequestScope for GetObjectRequest {
    fn targets(&self) -> Vec<Target> {
        vec![Target::Workspace(
            self.source.workspace_id,
            Permission::Read,
        )]
    }
    fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        None
    }
}

impl RequestScope for PresentRequest {
    /// Only the View's own workspace: `check_rules` has already refused a binding elsewhere.
    fn targets(&self) -> Vec<Target> {
        vec![Target::Workspace(self.workspace_id, Permission::Read)]
    }
    fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        None
    }
    fn check_rules(&self) -> Result<(), ApiError> {
        self.view.require_own_workspace(self.workspace_id, "/view")
    }
}

impl RequestScope for AddCommentRequest {
    fn targets(&self) -> Vec<Target> {
        let mut targets = vec![Target::Workspace(self.workspace_id, Permission::Write)];
        if let Some(source) = &self.source {
            targets.push(Target::Workspace(source.workspace_id, Permission::Read));
        }
        targets
    }
    fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        Some(&self.idempotency_key)
    }
}

impl RequestScope for CreateConfirmationRequest {
    fn targets(&self) -> Vec<Target> {
        let permission = match self.action {
            ConfirmationAction::Review => Permission::Review,
            ConfirmationAction::AcceptProposal => Permission::Approve,
        };
        vec![Target::Workspace(self.workspace_id, permission)]
    }
    fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        Some(&self.idempotency_key)
    }
}

impl RequestScope for CreateReviewRequest {
    fn targets(&self) -> Vec<Target> {
        vec![Target::Workspace(
            self.source.workspace_id,
            Permission::Review,
        )]
    }
    fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        Some(&self.idempotency_key)
    }
}

impl RequestScope for CreateConnectorRequest {
    const REPLAY: ReplayPolicy = ReplayPolicy::AlreadyIssued {
        id_pointer: "/connector/connector_id",
    };

    fn targets(&self) -> Vec<Target> {
        let mut targets = vec![Target::Deployment(Permission::Admin)];
        for workspace in &self.workspace_ids {
            targets.push(Target::Workspace(*workspace, Permission::Read));
            if self.allow_propose {
                targets.push(Target::Workspace(*workspace, Permission::Propose));
            }
        }
        targets
    }
    fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        Some(&self.idempotency_key)
    }
}

impl RequestScope for ListConnectorsRequest {
    fn targets(&self) -> Vec<Target> {
        vec![Target::Deployment(Permission::Admin)]
    }
    fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        None
    }
}

impl RequestScope for RevokeConnectorRequest {
    fn targets(&self) -> Vec<Target> {
        vec![Target::Deployment(Permission::Admin)]
    }
    fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        Some(&self.idempotency_key)
    }
}
