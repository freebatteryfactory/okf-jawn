//! In-flight object pins (review B6, window 2).
//!
//! Objects are shared inside a tenant, and a record that references a freshly stored object
//! lands only later: a job's outputs when it completes, a card when its commit lands, an
//! upload's object when it completes. A purge of another workspace that stores an identical
//! object would otherwise find no surviving reference in that gap and delete bytes a surviving
//! workspace is about to reference. So every workspace-scoped put pins (workspace, digest)
//! until the referencing record is durable, and the purge's object step treats another
//! workspace's pins as surviving references, and deletes the target's own pinned objects.
//!
//! Pins live in memory: the data directory has one writer process, and a crash ends every
//! write in flight with it, so after a restart no pin is owed (a retried job stores its object
//! again). A pin whose record never lands (a failed job) is released when the pin expires,
//! after `PIN_LIFETIME_MS`; until then it only keeps an object the surviving workspace stored.

use std::sync::{Arc, Mutex};

use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::identity::Digest;

use crate::db::internal;
use crate::seam;

/// The pins of this process, shared by every store over the data directory.
#[derive(Debug, Clone, Default)]
pub(crate) struct Pins {
    held: Arc<Mutex<Vec<Pin>>>,
}

/// One object put in a workspace whose referencing record has not landed yet.
#[derive(Debug, Clone)]
struct Pin {
    tenant: String,
    workspace: String,
    digest: Digest,
    pinned_ms: i64,
}

/// How long a pin keeps its object at most: longer than any one write in flight.
const PIN_LIFETIME_MS: i64 = 24 * 60 * 60 * 1000;

impl Pins {
    /// Pin `digest` for a write in `workspace` of `tenant`.
    pub(crate) fn pin(
        &self,
        tenant: &str,
        workspace: &str,
        digest: &Digest,
    ) -> Result<(), ApiError> {
        let pinned_ms = seam::now_ms()?;
        self.with(|held| {
            held.push(Pin {
                tenant: tenant.to_owned(),
                workspace: workspace.to_owned(),
                digest: digest.clone(),
                pinned_ms,
            });
        })
    }

    /// Release one pin of `workspace` for each of `digests`: its referencing record is durable
    /// now. A digest that was never pinned releases nothing.
    pub(crate) fn release<'a>(
        &self,
        workspace: &str,
        digests: impl IntoIterator<Item = &'a Digest>,
    ) -> Result<(), ApiError> {
        let digests: Vec<&Digest> = digests.into_iter().collect();
        self.with(|held| {
            for digest in digests {
                if let Some(index) = held
                    .iter()
                    .position(|pin| pin.workspace == workspace && pin.digest == *digest)
                {
                    held.swap_remove(index);
                }
            }
        })
    }

    /// The live pins of `tenant` as (workspace, digest); expired pins are dropped.
    pub(crate) fn held(&self, tenant: &str) -> Result<Vec<(String, Digest)>, ApiError> {
        let now = seam::now_ms()?;
        self.with(|held| {
            held.retain(|pin| now.saturating_sub(pin.pinned_ms) < PIN_LIFETIME_MS);
            held.iter()
                .filter(|pin| pin.tenant == tenant)
                .map(|pin| (pin.workspace.clone(), pin.digest.clone()))
                .collect()
        })
    }

    /// Run `delete` for `digest` of `tenant` unless a workspace other than `target` holds a
    /// live pin on it; returns whether it ran (and what it returned). The table stays locked
    /// meanwhile, so a put pins either before this check (and the object is kept) or after the
    /// delete (and the put stores the object again).
    pub(crate) fn delete_unless_pinned(
        &self,
        tenant: &str,
        target: &str,
        digest: &Digest,
        delete: impl FnOnce() -> Result<bool, ApiError>,
    ) -> Result<bool, ApiError> {
        let now = seam::now_ms()?;
        let mut held = self
            .held
            .lock()
            .map_err(|_| internal("the object pin table was poisoned"))?;
        held.retain(|pin| now.saturating_sub(pin.pinned_ms) < PIN_LIFETIME_MS);
        if held
            .iter()
            .any(|pin| pin.tenant == tenant && pin.workspace != target && pin.digest == *digest)
        {
            return Ok(false);
        }
        delete().map(|_| true)
    }

    fn with<T>(&self, work: impl FnOnce(&mut Vec<Pin>) -> T) -> Result<T, ApiError> {
        let mut held = self
            .held
            .lock()
            .map_err(|_| internal("the object pin table was poisoned"))?;
        Ok(work(&mut held))
    }
}
