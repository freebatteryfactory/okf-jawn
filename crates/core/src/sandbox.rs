//! Sandbox-origin capability tokens: stored hashed, minted and resolved by storage.
//!
//! The store never receives a plaintext token or a URL. The application generates the token,
//! passes only `token_hash` of it to `mint`, and builds the sandbox-origin URL itself. The
//! sandbox route applies the same `token_hash` to the token it is presented before `resolve`.
//! `create_sandbox_capability` is a read operation, so minting carries no `MutationId`.

use okf_jawn_contract::identity::{Digest, ItemId, Revision};
use sha2::{Digest as _, Sha256};

use crate::ports::PortFuture;
use crate::storage::StorageScope;

/// What one short-lived capability is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxMint {
    /// Item whose representation is served.
    pub item_id: ItemId,
    /// Exact revision.
    pub revision: Revision,
    /// Object digest served.
    pub object: Digest,
    /// Media type the sandbox route sends for the object.
    pub media_type: String,
    /// RFC 3339 expiry.
    pub expires_at: String,
}

/// The binding of an unexpired capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxResolved {
    /// Scope the capability was minted for.
    pub scope: StorageScope,
    /// Bound item.
    pub item_id: ItemId,
    /// Bound revision.
    pub revision: Revision,
    /// Bound object.
    pub object: Digest,
    /// Media type recorded at minting.
    pub media_type: String,
}

/// Hashed sandbox capability tokens; storage owns the implementation.
pub trait SandboxCapabilityStore: Send + Sync {
    /// Record a capability under the hash of its token.
    fn mint<'a>(
        &'a self,
        scope: &'a StorageScope,
        hash: [u8; 32],
        mint: SandboxMint,
    ) -> PortFuture<'a, ()>;
    /// Resolve the hash of a presented token to its binding when unexpired.
    fn resolve(&self, hash: [u8; 32]) -> PortFuture<'_, Option<SandboxResolved>>;
}

/// The only form in which a sandbox token reaches the store: SHA-256 of its bytes.
///
/// Both the caller that mints and the route that resolves use this function, so they cannot
/// disagree about the hash.
#[must_use]
pub fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}
