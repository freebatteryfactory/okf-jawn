//! Runtime-safe operation metadata projected from the canonical declaration table.
//!
//! `OperationName` is declared beside the table because `macro_rules!` cannot derive a
//! CamelCase variant from a snake_case identifier; a contract test proves the two agree
//! one-to-one and in order.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access::Permission;
use crate::import::JobKind;

macro_rules! collect_operations {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        /// Return canonical operations without opening storage or running an HTTP server.
        #[must_use]
        pub fn operations() -> Vec<OperationInfo> {
            vec![$(OperationInfo {
                id: stringify!($id), path: $path, label: $label, alias: $alias,
                operator_alias: $operator, visibility: $visibility,
                permission: Permission::$permission, ui: $ui, success_status: $status,
                destructive: $destructive, description: $description,
                rules: rules_of(stringify!($id)),
            }),*]
        }

        const _: () = {
            const fn scoped<T: crate::scope::RequestScope>() {}
            $(scoped::<$request>();)*
        };
    };
}

macro_rules! operation_names {
    ($($variant:ident => $wire:literal),* $(,)?) => {
        /// Closed set of canonical operation identifiers, serialized as the `snake_case` id.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema)]
        #[serde(rename_all = "snake_case")]
        pub enum OperationName {
            $(#[doc = concat!("The `", $wire, "` operation.")] $variant),*
        }

        impl OperationName {
            /// Every operation in canonical table order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),*];

            /// The canonical `snake_case` identifier.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire),*
                }
            }
        }
    };
}

/// An operation's shared names and declared execution policy.
#[derive(Debug, Clone, Serialize)]
pub struct OperationInfo {
    /// Canonical Rust, OpenAPI, and telemetry operation identifier.
    pub id: &'static str,
    /// Declared HTTP endpoint.
    pub path: &'static str,
    /// Human-facing operator vocabulary.
    pub label: &'static str,
    /// Agent tool name; empty means the operation is not an MCP tool.
    pub alias: &'static str,
    /// Operator and CLI name for the same operation; empty means only the canonical id.
    pub operator_alias: &'static str,
    /// Model-facing, app-only, or not exposed through MCP.
    pub visibility: &'static str,
    /// Minimum application capability, independent of HTTP method.
    pub permission: Permission,
    /// Optional approved MCP App presentation key.
    pub ui: &'static str,
    /// HTTP success status.
    pub success_status: u16,
    /// Whether the operation destroys or overwrites user state; a hint, never access control.
    pub destructive: bool,
    /// Shared operation documentation.
    pub description: &'static str,
    /// What the contract's lists say of the operation; written beside the other fields.
    #[serde(flatten)]
    pub rules: OperationRules,
}

/// What the contract's lists say of one operation (`operations`), so generated metadata carries
/// them: `api/operations.json` fields and OpenAPI `x-` fields of the same names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OperationRules {
    /// The kind of job the operation starts (`OperationName::starts_job`); `retry_job` and
    /// `cancel_job` on such a job need this operation's permission and route.
    pub starts_job: Option<JobKind>,
    /// Refused on every route that is not a human browser session
    /// (`operations::HUMAN_SESSION_ONLY`).
    pub human_session_only: bool,
    /// Refused on every route but the local owner's (`operations::LOCAL_OWNER_ONLY`).
    pub local_owner_only: bool,
    /// Allowed while its workspace is archived (`operations::ALLOWED_WHILE_ARCHIVED`).
    pub allowed_while_archived: bool,
}

operation_names! {
    ListWorkspaces => "list_workspaces",
    CreateWorkspace => "create_workspace",
    OpenWorkspace => "open_workspace",
    UpdateWorkspace => "update_workspace",
    ArchiveWorkspace => "archive_workspace",
    UnarchiveWorkspace => "unarchive_workspace",
    PurgeWorkspace => "purge_workspace",
    GetPurge => "get_purge",
    ExportWorkspace => "export_workspace",
    BackupWorkspace => "backup_workspace",
    RestoreWorkspace => "restore_workspace",
    BackupInstallation => "backup_installation",
    ListItems => "list_items",
    GetItem => "get_item",
    CreateItem => "create_item",
    SaveDraft => "save_draft",
    ListDrafts => "list_drafts",
    DiscardDraft => "discard_draft",
    MoveItem => "move_item",
    SetLifecycle => "set_lifecycle",
    DeleteItem => "delete_item",
    PurgeItem => "purge_item",
    CreateFolder => "create_folder",
    MoveFolder => "move_folder",
    DeleteFolder => "delete_folder",
    ListTypes => "list_types",
    SetType => "set_type",
    ReadItem => "read_item",
    GetSources => "get_sources",
    GetObject => "get_object",
    CreateSandboxCapability => "create_sandbox_capability",
    SearchItems => "search_items",
    GetLinks => "get_links",
    GetGraph => "get_graph",
    LogItems => "log_items",
    DiffItems => "diff_items",
    CommitItems => "commit_items",
    RestoreItems => "restore_items",
    BlameItem => "blame_item",
    OpenProposal => "open_proposal",
    ListProposals => "list_proposals",
    GetProposal => "get_proposal",
    AcceptProposal => "accept_proposal",
    DeclineProposal => "decline_proposal",
    AddComment => "add_comment",
    ListComments => "list_comments",
    CreateConfirmation => "create_confirmation",
    CreateReview => "create_review",
    ListReviews => "list_reviews",
    CreateUpload => "create_upload",
    CompleteUpload => "complete_upload",
    GetUpload => "get_upload",
    ListUploads => "list_uploads",
    DiscardUpload => "discard_upload",
    StartImport => "start_import",
    GetJob => "get_job",
    ListJobs => "list_jobs",
    GetTenantJob => "get_tenant_job",
    ListTenantJobs => "list_tenant_jobs",
    RetryJob => "retry_job",
    CancelJob => "cancel_job",
    RedigestItem => "redigest_item",
    CorrectDigest => "correct_digest",
    GetRules => "get_rules",
    SetRules => "set_rules",
    PreviewNames => "preview_names",
    ApplyNames => "apply_names",
    GetAttention => "get_attention",
    RebuildIndex => "rebuild_index",
    GetView => "get_view",
    PresentView => "present_view",
    ResolveView => "resolve_view",
    ExportView => "export_view",
    GetCatalog => "get_catalog",
    GetReceipt => "get_receipt",
    ListEvents => "list_events",
    ListTenantEvents => "list_tenant_events",
    GetSession => "get_session",
    CreateConnector => "create_connector",
    ListConnectors => "list_connectors",
    RevokeConnector => "revoke_connector",
    GetHealth => "get_health",
    GetReadiness => "get_readiness",
}

crate::for_each_operation!(collect_operations);

/// The operation whose canonical id is `id`; every row of the table has one (a contract test
/// holds the two in one order).
fn named(id: &str) -> Option<OperationName> {
    OperationName::ALL
        .iter()
        .copied()
        .find(|name| name.as_str() == id)
}

/// What the lists say of the operation whose canonical id is `id`.
fn rules_of(id: &str) -> OperationRules {
    let name = named(id);
    OperationRules {
        starts_job: name.and_then(OperationName::starts_job),
        human_session_only: name.is_some_and(OperationName::human_session_only),
        local_owner_only: name.is_some_and(OperationName::local_owner_only),
        allowed_while_archived: name.is_some_and(OperationName::allowed_while_archived),
    }
}
