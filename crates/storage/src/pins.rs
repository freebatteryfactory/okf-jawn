//! In-flight object pins (review B6, windows 1 and 2).
//!
//! Objects are shared inside a tenant, and a record that references a freshly stored object
//! lands only later: a job's outputs when it completes, a card when its commit lands, an
//! upload's object when it completes. So every workspace-scoped put pins (workspace, digest)
//! before it retains the bytes, and the pin lives until the referencing record is durable.
//!
//! A pin moves through three states:
//!
//! - **active** while the write that pinned is still running (`Pinned`, held by that write);
//! - **pending** once that write returned and the referencing record lands in a later call
//!   (`BlobStore::put` followed by a commit or a job completion), until `Pins::release`;
//! - **abandoned** when the write failed after it may have stored the bytes: no record will
//!   name them. A `Pinned` dropped unresolved (a cancelled future) is abandoned too.
//!
//! A purge's object step uses the pins of its target: it waits until the target holds no
//! active pin (`Pins::drain`), and then deletes the target's pending and abandoned objects
//! that no survivor references, the bytes of a write the purge refused among them. A target
//! put that pins after the drain looked finds the purge record when it checks again after
//! pinning (`LocalBlobs::put_pinned`) and stores nothing.
//!
//! Pins live in memory: the data directory has one writer process, and a crash ends every
//! write in flight with it, so after a restart no pin is owed (a retried job stores its object
//! again). A pin is dropped after `PIN_LIFETIME_MS` whatever its state; until then a pending
//! pin whose record never lands (a failed job) only keeps an object its workspace stored.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::Digest;

use crate::db::internal;
use crate::seam;

/// The pins of this process, shared by every store over the data directory.
#[derive(Debug, Clone, Default)]
pub(crate) struct Pins {
    table: Arc<Mutex<Table>>,
}

/// The pin table behind the lock.
#[derive(Debug, Default)]
struct Table {
    pins: Vec<Pin>,
    /// The identity the next pin gets.
    next: u64,
}

/// One object put in a workspace whose referencing record has not landed yet.
#[derive(Debug, Clone)]
struct Pin {
    id: u64,
    tenant: String,
    workspace: String,
    digest: Digest,
    pinned_ms: i64,
    state: State,
}

/// Where a pin's write stands (module header).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Active,
    Pending,
    Abandoned,
}

/// The pin of one write that is still running; abandoned when dropped unresolved.
#[derive(Debug)]
pub(crate) struct Pinned {
    pins: Pins,
    id: u64,
    resolved: bool,
}

/// How long a pin keeps its object at most: longer than any one write in flight.
const PIN_LIFETIME_MS: i64 = 24 * 60 * 60 * 1000;

/// How often `drain` looks at the table again.
const DRAIN_POLL: Duration = Duration::from_millis(10);

/// How long `drain` waits before the purge stops with `Unavailable`, to be resumed later.
const DRAIN_LIMIT: Duration = Duration::from_mins(10);

impl Pins {
    /// Pin `digest` for a write in `workspace` of `tenant`; the pin is active until the write
    /// resolves the returned `Pinned`.
    pub(crate) fn pin(
        &self,
        tenant: &str,
        workspace: &str,
        digest: &Digest,
    ) -> Result<Pinned, ApiError> {
        let pinned_ms = seam::now_ms()?;
        let mut table = self.lock()?;
        let id = table.next;
        table.next = table.next.saturating_add(1);
        table.pins.push(Pin {
            id,
            tenant: tenant.to_owned(),
            workspace: workspace.to_owned(),
            digest: digest.clone(),
            pinned_ms,
            state: State::Active,
        });
        Ok(Pinned {
            pins: self.clone(),
            id,
            resolved: false,
        })
    }

    /// Release one pending pin of `workspace` for each of `digests`: its referencing record is
    /// durable now. A digest with no pending pin releases nothing.
    pub(crate) fn release<'a>(
        &self,
        workspace: &str,
        digests: impl IntoIterator<Item = &'a Digest>,
    ) -> Result<(), ApiError> {
        let digests: Vec<&Digest> = digests.into_iter().collect();
        let mut table = self.lock()?;
        for digest in digests {
            if let Some(index) = table.pins.iter().position(|pin| {
                pin.state == State::Pending && pin.workspace == workspace && pin.digest == *digest
            }) {
                table.pins.swap_remove(index);
            }
        }
        Ok(())
    }

    /// The live pins of `tenant` held by workspaces other than `target`, active or pending:
    /// objects a surviving workspace is about to reference. Expired pins are dropped.
    pub(crate) fn others(&self, tenant: &str, target: &str) -> Result<Vec<Digest>, ApiError> {
        let table = self.live()?;
        Ok(table
            .pins
            .iter()
            .filter(|pin| {
                pin.tenant == tenant && pin.workspace != target && pin.state != State::Abandoned
            })
            .map(|pin| pin.digest.clone())
            .collect())
    }

    /// Wait until `workspace` of `tenant` holds no active pin, then return the objects its
    /// pending and abandoned pins name: bytes it stored that no record of it may name.
    ///
    /// Only a purge calls this, after the purge is recorded: every write of the workspace that
    /// pins from then on is refused before it stores anything, and every write that pinned
    /// earlier ends (a record write refuses inside its transaction), so the wait is bounded by
    /// the writes already running. A write that has not ended after `DRAIN_LIMIT` stops the
    /// purge with `Unavailable`; repeating the purge resumes it.
    pub(crate) async fn drain(
        &self,
        tenant: &str,
        workspace: &str,
    ) -> Result<Vec<Digest>, ApiError> {
        let started = std::time::Instant::now();
        loop {
            {
                let table = self.live()?;
                let mut active = false;
                let mut held = Vec::new();
                for pin in table
                    .pins
                    .iter()
                    .filter(|pin| pin.tenant == tenant && pin.workspace == workspace)
                {
                    active = active || pin.state == State::Active;
                    held.push(pin.digest.clone());
                }
                if !active {
                    return Ok(held);
                }
            }
            if started.elapsed() > DRAIN_LIMIT {
                return Err(ApiError::new(
                    ErrorCode::Unavailable,
                    format!(
                        "a write to workspace {workspace} that began before the purge was \
                         recorded is still running; repeat the purge to resume it"
                    ),
                ));
            }
            tokio::time::sleep(DRAIN_POLL).await;
        }
    }

    /// Run `delete` for `digest` of `tenant` unless a workspace other than `target` holds a
    /// live pin on it; returns whether it ran. The table stays locked meanwhile, so a put pins
    /// either before this check (and the object is kept) or after the delete (and the put,
    /// which checks whether the object is held only after it pinned, stores it again).
    pub(crate) fn delete_unless_pinned(
        &self,
        tenant: &str,
        target: &str,
        digest: &Digest,
        delete: impl FnOnce() -> Result<bool, ApiError>,
    ) -> Result<bool, ApiError> {
        let table = self.live()?;
        if table.pins.iter().any(|pin| {
            pin.tenant == tenant
                && pin.workspace != target
                && pin.state != State::Abandoned
                && pin.digest == *digest
        }) {
            return Ok(false);
        }
        delete().map(|_| true)
    }

    /// The table with expired pins dropped.
    fn live(&self) -> Result<MutexGuard<'_, Table>, ApiError> {
        let now = seam::now_ms()?;
        let mut table = self.lock()?;
        table
            .pins
            .retain(|pin| now.saturating_sub(pin.pinned_ms) < PIN_LIFETIME_MS);
        Ok(table)
    }

    fn lock(&self) -> Result<MutexGuard<'_, Table>, ApiError> {
        self.table
            .lock()
            .map_err(|_| internal("the object pin table was poisoned"))
    }

    /// Move pin `id` to `state`, or remove it when `state` is none.
    fn resolve(&self, id: u64, state: Option<State>) -> Result<(), ApiError> {
        let mut table = self.lock()?;
        match state {
            Some(state) => {
                if let Some(pin) = table.pins.iter_mut().find(|pin| pin.id == id) {
                    pin.state = state;
                }
            }
            None => {
                if let Some(index) = table.pins.iter().position(|pin| pin.id == id) {
                    table.pins.swap_remove(index);
                }
            }
        }
        Ok(())
    }
}

impl Pinned {
    /// The write returned; its referencing record lands in a later call, which releases the
    /// pin (`Pins::release`).
    pub(crate) fn settle(mut self) -> Result<(), ApiError> {
        self.resolved = true;
        self.pins.resolve(self.id, Some(State::Pending))
    }

    /// The write's referencing record is durable, or the write stored nothing.
    pub(crate) fn release(mut self) -> Result<(), ApiError> {
        self.resolved = true;
        self.pins.resolve(self.id, None)
    }

    /// The write failed after it may have stored the bytes: no record of it names them.
    pub(crate) fn abandon(mut self) -> Result<(), ApiError> {
        self.resolved = true;
        self.pins.resolve(self.id, Some(State::Abandoned))
    }
}

impl Drop for Pinned {
    fn drop(&mut self) {
        if !self.resolved {
            // A poisoned table is already reported to every later caller.
            let _ = self.pins.resolve(self.id, Some(State::Abandoned));
        }
    }
}
