//! In-flight object pins and the purge's sweep of them (review B6, windows 1 and 2).
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
//! A purge's object step runs as a `Sweep`, registered here before the step reads any
//! surviving reference. Its invariant: a candidate object is deleted only when it holds no
//! durable reference visible to the step, and no pin of another workspace existed for it at
//! any moment from the start of the reference read until its deletion.
//!
//! - The sweep's touched set starts as the digests of every live pin of another workspace
//!   (active or pending), and every pin another workspace takes later is added to it, under
//!   the same lock. A release never removes a digest from it. A pin released before the sweep
//!   began belongs to a record that was durable before the reference read began, so the read
//!   sees that record.
//! - Each delete runs under that lock (`Sweep::delete_unless_touched`). A put pins before it
//!   looks whether the object is held, so a put that pins after a delete finds the object
//!   missing and writes it again.
//! - Before the target's objects go, the sweep waits until the target holds no active pin
//!   (`Sweep::drain`), and then deletes the target's pending and abandoned objects that no
//!   survivor references, the bytes of a write the purge refused among them. A target put that
//!   pins after the drain looked finds the purge record when it checks again after pinning
//!   (`LocalBlobs::put_pinned`) and stores nothing.
//!
//! Pins live in memory: the data directory has one writer process, and a crash ends every
//! write in flight with it, so after a restart no pin is owed (a retried job stores its object
//! again). A pin is dropped after `PIN_LIFETIME_MS` whatever its state; until then a pending
//! pin whose record never lands (a failed job) only keeps an object its workspace stored.

use std::collections::BTreeSet;
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
    /// The object steps running now.
    sweeps: Vec<Watch>,
    /// The identity the next pin or sweep gets.
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

/// What one running object step has seen in the table.
#[derive(Debug)]
struct Watch {
    id: u64,
    tenant: String,
    target: String,
    /// Every digest a workspace other than the target pinned since the step began.
    touched: BTreeSet<Digest>,
    /// Whether the step is waiting for the target's active pins (`Sweep::drain`).
    draining: bool,
}

/// The pin of one write that is still running; abandoned when dropped unresolved.
#[derive(Debug)]
pub(crate) struct Pinned {
    pins: Pins,
    id: u64,
    resolved: bool,
}

/// One running object step of a purge of `target`; removed from the table when dropped.
#[derive(Debug)]
pub(crate) struct Sweep {
    pins: Pins,
    id: u64,
    tenant: String,
    target: String,
}

/// How long a pin keeps its object at most: longer than any one write in flight.
const PIN_LIFETIME_MS: i64 = 24 * 60 * 60 * 1000;

/// How often `drain` looks at the table again.
const DRAIN_POLL: Duration = Duration::from_millis(10);

/// How long `drain` waits before the purge stops with `Unavailable`, to be resumed later.
const DRAIN_LIMIT: Duration = Duration::from_mins(10);

impl Pins {
    /// Pin `digest` for a write in `workspace` of `tenant`; the pin is active until the write
    /// resolves the returned `Pinned`. Every running sweep of another workspace of the tenant
    /// marks the digest touched.
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
        for watch in table
            .sweeps
            .iter_mut()
            .filter(|watch| watch.tenant == tenant && watch.target != workspace)
        {
            watch.touched.insert(digest.clone());
        }
        Ok(Pinned {
            pins: self.clone(),
            id,
            resolved: false,
        })
    }

    /// Release one pending pin of `workspace` for each of `digests`: its referencing record is
    /// durable now. A digest with no pending pin releases nothing. A running sweep keeps the
    /// digest as touched.
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

    /// Begin the object step of a purge of `target` (module header); returns the sweep and the
    /// digests it is touched by from the start, so the step can follow conversion records
    /// among them. Expired pins are dropped first.
    pub(crate) fn sweep(
        &self,
        tenant: &str,
        target: &str,
    ) -> Result<(Sweep, BTreeSet<Digest>), ApiError> {
        let mut table = self.live()?;
        let touched: BTreeSet<Digest> = table
            .pins
            .iter()
            .filter(|pin| {
                pin.tenant == tenant && pin.workspace != target && pin.state != State::Abandoned
            })
            .map(|pin| pin.digest.clone())
            .collect();
        let id = table.next;
        table.next = table.next.saturating_add(1);
        table.sweeps.push(Watch {
            id,
            tenant: tenant.to_owned(),
            target: target.to_owned(),
            touched: touched.clone(),
            draining: false,
        });
        let sweep = Sweep {
            pins: self.clone(),
            id,
            tenant: tenant.to_owned(),
            target: target.to_owned(),
        };
        Ok((sweep, touched))
    }

    /// Whether a sweep of `target` is waiting for its active pins; for tests that act inside
    /// that wait, which comes after the sweep read every surviving reference.
    #[cfg(test)]
    pub(crate) fn draining(&self, tenant: &str, target: &str) -> Result<bool, ApiError> {
        Ok(self
            .lock()?
            .sweeps
            .iter()
            .any(|watch| watch.tenant == tenant && watch.target == target && watch.draining))
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

impl Sweep {
    /// Wait until the target holds no active pin, then return the objects its pending and
    /// abandoned pins name: bytes it stored that no record of it may name.
    ///
    /// Only a purge sweeps, after the purge is recorded: every write of the target that pins
    /// from then on is refused before it stores anything, and every write that pinned earlier
    /// ends (a record write refuses inside its transaction), so the wait is bounded by the
    /// writes already running. A write that has not ended after `DRAIN_LIMIT` stops the purge
    /// with `Unavailable`; repeating the purge resumes it.
    pub(crate) async fn drain(&self) -> Result<Vec<Digest>, ApiError> {
        let started = std::time::Instant::now();
        loop {
            {
                let mut table = self.pins.live()?;
                if let Some(watch) = table.sweeps.iter_mut().find(|watch| watch.id == self.id) {
                    watch.draining = true;
                }
                let mut active = false;
                let mut held = Vec::new();
                for pin in table
                    .pins
                    .iter()
                    .filter(|pin| pin.tenant == self.tenant && pin.workspace == self.target)
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
                        "a write to workspace {} that began before the purge was recorded is \
                         still running; repeat the purge to resume it",
                        self.target
                    ),
                ));
            }
            tokio::time::sleep(DRAIN_POLL).await;
        }
    }

    /// Run `delete` for `digest` unless another workspace pinned it since the sweep began;
    /// returns whether it ran. The table stays locked meanwhile, so a put pins either before
    /// this check (and the object is kept) or after the delete (and the put, which looks
    /// whether the object is held only after it pinned, stores it again).
    pub(crate) fn delete_unless_touched(
        &self,
        digest: &Digest,
        delete: impl FnOnce() -> Result<bool, ApiError>,
    ) -> Result<bool, ApiError> {
        let table = self.pins.lock()?;
        let watch = table
            .sweeps
            .iter()
            .find(|watch| watch.id == self.id)
            .ok_or_else(|| internal("a purge's sweep left the pin table while it ran"))?;
        if watch.touched.contains(digest) {
            return Ok(false);
        }
        delete().map(|_| true)
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

impl Drop for Sweep {
    fn drop(&mut self) {
        // A poisoned table is already reported to every later caller.
        if let Ok(mut table) = self.pins.lock() {
            table.sweeps.retain(|watch| watch.id != self.id);
        }
    }
}
