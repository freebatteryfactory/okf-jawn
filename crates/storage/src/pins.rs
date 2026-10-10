//! In-flight object pins and the purge's sweep of them (review B6, windows 1 and 2).
//!
//! The durable pending-put rows (`puts`) say which objects a workspace stored and has not yet
//! referenced; they have no lifetime and survive a crash. Pins keep what those rows cannot:
//! which writes are **running now**, and the order of a put against a purge's delete.
//!
//! - Every workspace-scoped put pins (workspace, digest) before it records its pending put and
//!   retains the object, and the pin lives exactly as long as the write (`Pinned`; dropping it,
//!   a cancelled or failed write included, ends it). A pin has no other state and no lifetime.
//! - A purge's objects step runs as a `Sweep`, registered before the step reads any surviving
//!   reference. Its touched set starts as the digests of every running pin of another
//!   workspace, and every pin another workspace takes later joins it, under the same lock; the
//!   end of a write never removes a digest from it. A write that ended before the sweep began
//!   recorded its pending put before it retained the object, and that row is deleted only in
//!   the transaction of the record that references the object, so the reference read, which
//!   begins after the sweep, sees the row or the record.
//! - Each delete runs under that lock (`Sweep::delete_unless_touched`). A put pins before it
//!   looks whether the object is held, so a put that pins after a delete finds the object
//!   missing and writes it again.
//! - Before the target's objects go, the sweep waits until the target holds no running pin
//!   (`Sweep::drain`). Every write of the target that pins after the purge was recorded is
//!   refused before it records or stores anything (`LocalBlobs::put_pinned`), and every write
//!   that pinned earlier ends, so the wait is bounded by the writes already running; after
//!   `DRAIN_LIMIT` the purge stops with `Unavailable` and resumes when repeated.
//!
//! So: a candidate object is deleted only when no durable reference visible to the step names
//! it, no pending put of another workspace names it, and no other workspace's write pinned it
//! at any moment from the start of the reference read until its deletion.
//!
//! Pins live in memory: the data directory has one writer process, and a crash ends every
//! write in flight with it.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::Digest;

use crate::db::internal;

/// The pins of this process, shared by every store over the data directory.
#[derive(Debug, Clone, Default)]
pub(crate) struct Pins {
    table: Arc<Mutex<Table>>,
}

/// The pin table behind the lock.
#[derive(Debug, Default)]
struct Table {
    pins: Vec<Pin>,
    /// The objects steps running now.
    sweeps: Vec<Watch>,
    /// The identity the next pin or sweep gets.
    next: u64,
}

/// One running write's put of an object in a workspace.
#[derive(Debug, Clone)]
struct Pin {
    id: u64,
    tenant: String,
    workspace: String,
    digest: Digest,
}

/// What one running objects step has seen in the table.
#[derive(Debug)]
struct Watch {
    id: u64,
    tenant: String,
    target: String,
    /// Every digest a workspace other than the target pinned since the step began.
    touched: BTreeSet<Digest>,
    /// Whether the step is waiting for the target's running writes (`Sweep::drain`).
    draining: bool,
}

/// The pin of one running write; it ends when the write ends (`end`, or when dropped).
#[derive(Debug)]
pub(crate) struct Pinned {
    pins: Pins,
    id: u64,
}

/// One running objects step of a purge of `target`; removed from the table when dropped.
#[derive(Debug)]
pub(crate) struct Sweep {
    pins: Pins,
    id: u64,
    tenant: String,
    target: String,
}

/// How often `drain` looks at the table again.
const DRAIN_POLL: Duration = Duration::from_millis(10);

/// How long `drain` waits before the purge stops with `Unavailable`, to be resumed later.
const DRAIN_LIMIT: Duration = Duration::from_mins(10);

impl Pins {
    /// Pin `digest` for a write in `workspace` of `tenant` until the returned `Pinned` ends.
    /// Every running sweep of another workspace of the tenant marks the digest touched.
    pub(crate) fn pin(
        &self,
        tenant: &str,
        workspace: &str,
        digest: &Digest,
    ) -> Result<Pinned, ApiError> {
        let mut table = self.lock()?;
        let id = table.next;
        table.next = table.next.saturating_add(1);
        table.pins.push(Pin {
            id,
            tenant: tenant.to_owned(),
            workspace: workspace.to_owned(),
            digest: digest.clone(),
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
        })
    }

    /// Begin the objects step of a purge of `target` (module header); returns the sweep and
    /// the digests it is touched by from the start, so the step can follow conversion records
    /// among them.
    pub(crate) fn sweep(
        &self,
        tenant: &str,
        target: &str,
    ) -> Result<(Sweep, BTreeSet<Digest>), ApiError> {
        let mut table = self.lock()?;
        let touched: BTreeSet<Digest> = table
            .pins
            .iter()
            .filter(|pin| pin.tenant == tenant && pin.workspace != target)
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

    /// Whether a sweep of `target` is waiting for its running writes; for tests that act
    /// inside that wait, which comes after the sweep read every surviving reference.
    #[cfg(test)]
    pub(crate) fn draining(&self, tenant: &str, target: &str) -> Result<bool, ApiError> {
        Ok(self
            .lock()?
            .sweeps
            .iter()
            .any(|watch| watch.tenant == tenant && watch.target == target && watch.draining))
    }

    fn lock(&self) -> Result<MutexGuard<'_, Table>, ApiError> {
        self.table
            .lock()
            .map_err(|_| internal("the object pin table was poisoned"))
    }

    /// Remove pin `id`.
    fn remove(&self, id: u64) -> Result<(), ApiError> {
        let mut table = self.lock()?;
        table.pins.retain(|pin| pin.id != id);
        Ok(())
    }
}

impl Pinned {
    /// The write ended (its pending put and, if it landed, its record are durable).
    pub(crate) fn end(self) -> Result<(), ApiError> {
        // The drop that follows finds nothing more to remove.
        self.pins.remove(self.id)
    }
}

impl Sweep {
    /// Wait until the target holds no running pin.
    ///
    /// Only a purge sweeps, after the purge is recorded: every write of the target that pins
    /// from then on is refused before it records or stores anything, and every write that
    /// pinned earlier ends, so the wait is bounded by the writes already running. A write that
    /// has not ended after `DRAIN_LIMIT` stops the purge with `Unavailable`; repeating the
    /// purge resumes it.
    pub(crate) async fn drain(&self) -> Result<(), ApiError> {
        let started = std::time::Instant::now();
        loop {
            {
                let mut table = self.pins.lock()?;
                if let Some(watch) = table.sweeps.iter_mut().find(|watch| watch.id == self.id) {
                    watch.draining = true;
                }
                if !table
                    .pins
                    .iter()
                    .any(|pin| pin.tenant == self.tenant && pin.workspace == self.target)
                {
                    return Ok(());
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
        // A poisoned table is already reported to every later caller.
        if let Ok(mut table) = self.pins.lock() {
            table.pins.retain(|pin| pin.id != self.id);
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
