//! The byte transports of `api/transports.json` that carry an application decision: an upload
//! slot's bytes (`upload_content`), an object authorized through an exact item revision
//! (`download_object`), a workspace artifact (`download_artifact`) and an installation archive
//! (`download_tenant_artifact`).
//!
//! They are not operations: they stream bytes, so they bypass dispatch and the JSON table. The
//! server binds their routes to [`Transfers`] and only moves bytes; `ApplicationService`
//! implements it (`application::transfers`, whose header states each rule), authorizing every
//! call as its caller and recording each `Forbidden` it answers as a `permission_denied` event
//! that names the transport.

use okf_jawn_contract::access::Principal;
use okf_jawn_contract::identity::{ArtifactId, Digest, ItemId, Revision, UploadId, WorkspaceId};
use okf_jawn_contract::import::Upload;

use crate::ports::PortFuture;
use crate::storage::ByteReader;

/// The application side of the byte transports; each call authorizes as `caller`.
pub trait Transfers: Send + Sync {
    /// Append the request body to an upload slot (`upload_content`).
    fn upload_content<'a>(
        &'a self,
        caller: &'a TransferCaller,
        workspace: WorkspaceId,
        upload: UploadId,
        body: ByteReader,
    ) -> PortFuture<'a, Upload>;
    /// An object authorized through an exact item revision (`download_object`).
    fn download_object<'a>(
        &'a self,
        caller: &'a TransferCaller,
        workspace: WorkspaceId,
        item: ItemId,
        revision: Revision,
        digest: Digest,
        range: Option<ByteRange>,
    ) -> PortFuture<'a, Download>;
    /// A workspace artifact (`download_artifact`).
    fn download_artifact<'a>(
        &'a self,
        caller: &'a TransferCaller,
        workspace: WorkspaceId,
        artifact: ArtifactId,
    ) -> PortFuture<'a, Download>;
    /// An installation archive (`download_tenant_artifact`).
    fn download_tenant_artifact<'a>(
        &'a self,
        caller: &'a TransferCaller,
        artifact: ArtifactId,
    ) -> PortFuture<'a, Download>;
}

/// Who a transfer acts for, as the identity middleware established it.
#[derive(Debug, Clone)]
pub struct TransferCaller {
    /// The authenticated principal.
    pub principal: Principal,
    /// The browser session, when the request came with the session cookie.
    pub session_id: Option<String>,
}

/// One requested byte interval: `bytes=start-` or `bytes=start-end` (inclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    /// First byte.
    pub start: u64,
    /// Last byte, inclusive; `None` to the end.
    pub end: Option<u64>,
}

/// Bytes to send and what they are.
pub struct Download {
    /// Media type.
    pub media_type: String,
    /// Whole object size.
    pub total_size: u64,
    /// Offset of the first byte sent.
    pub offset: u64,
    /// Bytes sent.
    pub length: u64,
    /// The bytes.
    pub body: ByteReader,
}
