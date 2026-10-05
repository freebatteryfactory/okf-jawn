//! Per-editor drafts: one draft per (scope, item, editor); saving never creates a revision.
//!
//! `save` and `discard` take `MutationId` and are unique on that id together with the item:
//! repeating a call writes nothing and returns what the first call returned. One Snapshot
//! removes several drafts under a single `MutationId`, which is why the item is part of the key.

use std::collections::BTreeMap;

use okf_jawn_contract::identity::{Digest, ItemId, MutationId, Revision};
use okf_jawn_contract::item::{Draft, DraftContent};

use crate::ports::PortFuture;
use crate::storage::StorageScope;

/// Draft body and properties retained for one editor.
#[derive(Debug, Clone)]
pub struct DraftWrite {
    /// Item being drafted.
    pub item_id: ItemId,
    /// Server-established editor subject.
    pub editor: String,
    /// Head revision the draft is based on.
    pub base_revision: Revision,
    /// Drafted Markdown body.
    pub body: String,
    /// Drafted complete property map.
    pub properties: BTreeMap<String, serde_json::Value>,
    /// Digest of the drafted body and properties, computed by the application.
    pub content_digest: Digest,
}

/// Draft persistence keyed per (item, editor); storage owns the implementation.
pub trait DraftStore: Send + Sync {
    /// Save or replace the editor's draft of one item and stamp the save time.
    ///
    /// A repeated `mutation_id` for the same item writes nothing and returns the draft as that
    /// first save left it.
    fn save<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        draft: DraftWrite,
    ) -> PortFuture<'a, Draft>;
    /// Read the editor's own draft content for one item, if any.
    fn get<'a>(
        &'a self,
        scope: &'a StorageScope,
        item: ItemId,
        editor: &'a str,
    ) -> PortFuture<'a, Option<DraftContent>>;
    /// List the editor's own drafts in a workspace, most recently saved first.
    fn list<'a>(&'a self, scope: &'a StorageScope, editor: &'a str) -> PortFuture<'a, Vec<Draft>>;
    /// Remove the editor's own draft of one item and return its metadata.
    ///
    /// The store remembers what it removed under the `mutation_id` and item: a repeated call
    /// removes nothing and returns that same metadata. `NotFound` when the editor has no draft
    /// of the item and this mutation removed none.
    fn discard<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        item: ItemId,
        editor: &'a str,
    ) -> PortFuture<'a, Draft>;
}
