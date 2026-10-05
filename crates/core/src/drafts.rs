//! Per-editor drafts: one draft per (scope, item, subject); saving never creates a revision.
//!
//! `save` takes `MutationId` and enforces uniqueness.

use okf_jawn_contract::identity::{ItemId, MutationId};
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
    pub base_revision: okf_jawn_contract::identity::Revision,
    /// Drafted Markdown body.
    pub body: String,
    /// Drafted complete property map.
    pub properties: std::collections::BTreeMap<String, serde_json::Value>,
}

/// Draft persistence keyed per (item, editor); storage owns the implementation.
pub trait DraftStore: Send + Sync {
    /// Save or replace the editor's draft for one item.
    ///
    /// Unique on `mutation_id`; a reused id returns the prior draft, never a duplicate effect.
    fn save<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        draft: DraftWrite,
    ) -> PortFuture<'a, Draft>;
    /// Look up a draft saved under `mutation_id`, for abandoned-lease reconciliation.
    fn find_by_mutation<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
    ) -> PortFuture<'a, Option<Draft>>;
    /// Read the caller's own draft content for one item, if any.
    fn get<'a>(
        &'a self,
        scope: &'a StorageScope,
        item: ItemId,
        editor: &'a str,
    ) -> PortFuture<'a, Option<DraftContent>>;
    /// List the caller's own drafts in a workspace.
    fn list<'a>(
        &'a self,
        scope: &'a StorageScope,
        editor: &'a str,
    ) -> PortFuture<'a, Vec<Draft>>;
    /// Remove the caller's own draft of one item.
    fn discard<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        item: ItemId,
        editor: &'a str,
    ) -> PortFuture<'a, Draft>;
}
