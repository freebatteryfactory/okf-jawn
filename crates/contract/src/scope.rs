//! Every request declares its authorization targets and retry identity in its type.
//!
//! Dispatch authorizes every target before the handler runs; a request that touches several
//! workspaces names each one, so no workspace is reached without its own grant. The first
//! target carries the operation's table permission, except `create_confirmation`, whose
//! required permission depends on the confirmed action.

use crate::access::{
    CreateConnectorRequest, ListConnectorsRequest, Permission, RevokeConnectorRequest,
};
use crate::attention::{GetAttentionRequest, RebuildIndexRequest};
use crate::common::Empty;
use crate::conventions::{
    ApplyNamesRequest, GetRulesRequest, PreviewNamesRequest, SetRulesRequest,
};
use crate::events::{GetReceiptRequest, ListEventsRequest};
use crate::health::{HealthRequest, ReadinessRequest};
use crate::history::{BlameRequest, CommitRequest, DiffRequest, LogRequest, RestoreRequest};
use crate::identity::{IdempotencyKey, WorkspaceId};
use crate::import::{
    CancelJobRequest, CompleteUploadRequest, CorrectDigestRequest, CreateUploadRequest,
    GetJobRequest, ListJobsRequest, RedigestRequest, RetryJobRequest, StartImportRequest,
};
use crate::item::{
    CreateFolderRequest, CreateItemRequest, DeleteItemRequest, DiscardDraftRequest, GetItemRequest,
    ListDraftsRequest, ListItemsRequest, ListTypesRequest, MoveItemRequest, SaveDraftRequest,
    SetLifecycleRequest, SetTypeRequest,
};
use crate::proposal::{
    AcceptProposalRequest, AddCommentRequest, DeclineProposalRequest, GetProposalRequest,
    ListProposalsRequest, OpenProposalRequest,
};
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
    ArchiveWorkspaceRequest, BackupWorkspaceRequest, CreateWorkspaceRequest,
    ExportWorkspaceRequest, ListWorkspacesRequest, OpenWorkspaceRequest, RestoreWorkspaceRequest,
    UpdateWorkspaceRequest,
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

macro_rules! deployment_read {
    ($($request:ty),* $(,)?) => {
        $(impl RequestScope for $request {
            fn targets(&self) -> Vec<Target> {
                vec![Target::Deployment(Permission::Read)]
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
    /// Return `already_issued` with the created identity; the response held a secret that is never stored.
    AlreadyIssued,
}

/// Authorization targets and retry identity, implemented by every operation request.
pub trait RequestScope {
    /// Replay behaviour for this operation's completed mutations.
    const REPLAY: ReplayPolicy = ReplayPolicy::StoredResponse;

    /// Every resource that must be authorized, first the one named by the table permission.
    fn targets(&self) -> Vec<Target>;

    /// Caller-chosen retry identity; present exactly when the request is a mutation.
    fn idempotency_key(&self) -> Option<&IdempotencyKey>;
}

impl Target {
    /// The capability this target requires.
    #[must_use]
    pub const fn permission(self) -> Permission {
        match self {
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
    SearchRequest,
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
    SetLifecycleRequest,
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
workspace_keyed!(Propose: OpenProposalRequest);
workspace_keyed!(Approve: AcceptProposalRequest, DeclineProposalRequest);
workspace_keyed!(Admin:
    ArchiveWorkspaceRequest,
    BackupWorkspaceRequest,
    RestoreWorkspaceRequest,
    RebuildIndexRequest,
);

deployment_read!(
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
    fn targets(&self) -> Vec<Target> {
        let mut targets = vec![Target::Workspace(self.workspace_id, Permission::Read)];
        targets.extend(
            self.view
                .bindings
                .iter()
                .map(|binding| Target::Workspace(binding.source.workspace_id, Permission::Read)),
        );
        targets
    }
    fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        None
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
    const REPLAY: ReplayPolicy = ReplayPolicy::AlreadyIssued;

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
