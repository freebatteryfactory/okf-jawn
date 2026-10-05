//! Sandbox-origin capability tokens: stored hashed, minted and resolved by storage.
//!
//! Mint takes `MutationId` when issued through a write path. `create_sandbox_capability` is a
//! Read operation; mint may still record an id when the lane chooses durable issuance.

use okf_jawn_contract::identity::{Digest, ItemId, MutationId, Revision};
use okf_jawn_contract::read::SandboxCapability;

use crate::ports::PortFuture;
use crate::storage::StorageScope;

/// Binding for a short-lived sandbox capability URL.
#[derive(Debug, Clone)]
pub struct SandboxMint {
    /// Item whose representation is served.
    pub item_id: ItemId,
    /// Exact revision.
    pub revision: Revision,
    /// Object digest served.
    pub object: Digest,
    /// Absolute sandbox-origin URL returned to the caller.
    pub url: String,
    /// RFC 3339 expiry.
    pub expires_at: String,
}

/// Resolved capability after token verification.
#[derive(Debug, Clone)]
pub struct SandboxResolved {
    /// Scope the capability was minted for.
    pub scope: StorageScope,
    /// Bound item.
    pub item_id: ItemId,
    /// Bound revision.
    pub revision: Revision,
    /// Bound object.
    pub object: Digest,
}

/// Hashed sandbox capability tokens; storage owns the implementation.
pub trait SandboxCapabilityStore: Send + Sync {
    /// Mint a capability; tokens are stored hashed.
    ///
    /// When `mutation_id` is present, uniqueness is enforced on that id.
    fn mint<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: Option<MutationId>,
        token_hash: &'a [u8],
        mint: SandboxMint,
    ) -> PortFuture<'a, SandboxCapability>;
    /// Resolve a presented token hash to its binding when unexpired.
    fn resolve<'a>(&'a self, token_hash: &'a [u8]) -> PortFuture<'a, Option<SandboxResolved>>;
}
