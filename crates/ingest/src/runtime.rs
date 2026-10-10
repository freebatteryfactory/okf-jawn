//! The in-process job runtime: Tokio tasks over the durable `RecordStore`.
//!
//! The queue only delivers. Job truth is the `RecordStore`, and a delivery is a hint to try a
//! claim. A delivery can be repeated, so a worker claims before it runs anything. A claim that
//! returns `None` (already claimed, finished or cancelled) is dropped without effect. The
//! handler runs only under the lease `claim_job` returned, and writes completion through it. A
//! handler that returns `Err` is at fault as a worker, and the runtime records the failure
//! through the same lease, as retryable.
//!
//! Reconciliation runs at start and then on every sweep: `expire_leases()` releases the claims
//! of attempts that died, and `pending_jobs()` re-delivers every unfinished job. A job already
//! waiting in the queue is not delivered again: the queue keeps the ids of the deliveries no
//! worker has taken yet, and a worker removes a job's id as it takes it. So however many sweeps
//! pass while every worker is busy, at most one delivery per pending job waits. So a job whose
//! process was killed is retried at the next start, and an attempt that died without a restart
//! is retried after its lease expires. No external job engine or queue service is involved.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::JobId;
use okf_jawn_core::jobs::{JobHandler, JobQueue, JobScope, RecordStore};
use okf_jawn_core::ports::PortFuture;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// One delivery: a durable job identity to try to claim.
type Delivery = (JobScope, JobId);

/// How the runtime runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeConfig {
    /// Jobs run at once.
    pub workers: usize,
    /// How often leases are expired and pending jobs re-delivered. A third of the lease is a
    /// sound choice: a dead attempt is then retried soon after its lease lapses.
    pub sweep_interval: Duration,
}

/// The running workers; drop or `shutdown` to stop them.
#[derive(Debug)]
pub struct JobRuntime {
    cancel: CancellationToken,
    tasks: JoinSet<()>,
}

/// The `JobQueue` the application enqueues through: an in-process channel to the workers.
#[derive(Debug, Clone)]
pub struct TokioJobQueue {
    sender: mpsc::UnboundedSender<Delivery>,
    waiting: Arc<std::sync::Mutex<Waiting>>,
}

/// The deliveries sent and not yet taken by a worker.
#[derive(Debug, Default)]
struct Waiting {
    /// Their jobs; a job is delivered again only once a worker has taken it.
    jobs: BTreeSet<JobId>,
    /// How many deliveries are in the channel.
    deliveries: usize,
}

impl JobRuntime {
    /// Start the workers and the sweeper, and return the queue that feeds them.
    ///
    /// The first sweep runs at once, so jobs a previous process left unfinished are
    /// reconciled before new work arrives. Call from within a Tokio runtime.
    #[must_use]
    pub fn start(
        records: &Arc<dyn RecordStore>,
        handler: &Arc<dyn JobHandler>,
        config: RuntimeConfig,
    ) -> (Self, TokioJobQueue) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let receiver = Arc::new(Mutex::new(receiver));
        let waiting = Arc::new(std::sync::Mutex::new(Waiting::default()));
        let cancel = CancellationToken::new();
        let mut tasks = JoinSet::new();
        for _ in 0..config.workers.max(1) {
            tasks.spawn(work(
                Arc::clone(records),
                Arc::clone(handler),
                Arc::clone(&receiver),
                Arc::clone(&waiting),
                cancel.clone(),
            ));
        }
        let queue = TokioJobQueue { sender, waiting };
        tasks.spawn(sweep(
            Arc::clone(records),
            queue.clone(),
            config.sweep_interval,
            cancel.clone(),
        ));
        (Self { cancel, tasks }, queue)
    }

    /// Stop taking deliveries and wait for the tasks to end. A job running at that moment is
    /// abandoned under its lease, which expires and is reclaimed by the next process.
    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        while self.tasks.join_next().await.is_some() {}
    }
}

impl Drop for JobRuntime {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl TokioJobQueue {
    /// How many deliveries wait for a worker.
    ///
    /// # Errors
    /// Returns `Internal` when the queue's bookkeeping lock is poisoned.
    pub fn waiting(&self) -> Result<usize, ApiError> {
        Ok(lock(&self.waiting)?.deliveries)
    }

    /// Send one delivery, unless one for the same job already waits.
    fn deliver(&self, scope: JobScope, job: JobId) -> Result<(), ApiError> {
        if self.sender.is_closed() {
            return Err(stopped());
        }
        let mut waiting = lock(&self.waiting)?;
        if waiting.jobs.contains(&job) {
            return Ok(());
        }
        self.sender.send((scope, job)).map_err(|_| stopped())?;
        let _new = waiting.jobs.insert(job);
        waiting.deliveries = waiting.deliveries.saturating_add(1);
        Ok(())
    }
}

impl JobQueue for TokioJobQueue {
    fn enqueue(&self, scope: JobScope, job: JobId) -> PortFuture<'_, ()> {
        let sent = self.deliver(scope, job);
        Box::pin(async move { sent })
    }
}

/// Expire dead leases and re-deliver every unfinished job.
///
/// # Errors
/// Returns the record store's error, or `Unavailable` when the runtime has stopped.
pub async fn reconcile(records: &dyn RecordStore, queue: &dyn JobQueue) -> Result<u32, ApiError> {
    let _released = records.expire_leases().await?;
    let pending = records.pending_jobs().await?;
    let mut delivered = 0_u32;
    for (scope, job) in pending {
        queue.enqueue(scope, job).await?;
        delivered = delivered.saturating_add(1);
    }
    Ok(delivered)
}

/// Claim and run one delivered job.
///
/// # Errors
/// Returns the record store's error from the claim or from recording a failure. A handler's
/// own error is recorded as the job's failure, not returned.
pub async fn run_one(
    records: &dyn RecordStore,
    handler: &dyn JobHandler,
    job: JobId,
) -> Result<bool, ApiError> {
    let Some(claimed) = records.claim_job(job).await? else {
        return Ok(false);
    };
    if let Err(error) = handler.handle(&claimed).await {
        let retryable = !matches!(
            error.code,
            ErrorCode::InvalidInput
                | ErrorCode::Forbidden
                | ErrorCode::Unsupported
                | ErrorCode::NotImplemented
        );
        let _failed = records
            .fail_job(claimed.lease.clone(), error.message, retryable)
            .await?;
    }
    Ok(true)
}

/// One worker: take deliveries until cancelled.
async fn work(
    records: Arc<dyn RecordStore>,
    handler: Arc<dyn JobHandler>,
    receiver: Arc<Mutex<mpsc::UnboundedReceiver<Delivery>>>,
    waiting: Arc<std::sync::Mutex<Waiting>>,
    cancel: CancellationToken,
) {
    loop {
        let delivery = {
            let mut receiver = receiver.lock().await;
            tokio::select! {
                () = cancel.cancelled() => None,
                delivery = receiver.recv() => delivery,
            }
        };
        let Some((_scope, job)) = delivery else {
            return;
        };
        // Taken: a sweep may deliver the job again from now on. A poisoned lock only means the
        // bookkeeping is lost; the delivery still runs, as a repeated delivery is harmless.
        if let Ok(mut waiting) = waiting.lock() {
            let _was_waiting = waiting.jobs.remove(&job);
            waiting.deliveries = waiting.deliveries.saturating_sub(1);
        }
        // A store error here leaves the job as the store has it; the next sweep re-delivers
        // it if it is still unfinished.
        let _ran = tokio::select! {
            () = cancel.cancelled() => return,
            ran = run_one(records.as_ref(), handler.as_ref(), job) => ran,
        };
    }
}

/// Reconcile at start and then every `interval` until cancelled.
async fn sweep(
    records: Arc<dyn RecordStore>,
    queue: TokioJobQueue,
    interval: Duration,
    cancel: CancellationToken,
) {
    let mut ticks = tokio::time::interval(interval.max(Duration::from_millis(1)));
    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            _instant = ticks.tick() => {}
        }
        // A failed sweep is retried at the next tick; nothing is lost, the store keeps the jobs.
        let _delivered = reconcile(records.as_ref(), &queue).await;
    }
}

/// The queue's bookkeeping, locked.
fn lock(
    waiting: &std::sync::Mutex<Waiting>,
) -> Result<std::sync::MutexGuard<'_, Waiting>, ApiError> {
    waiting.lock().map_err(|_| {
        ApiError::new(
            ErrorCode::Internal,
            "the job queue's bookkeeping lock is poisoned",
        )
    })
}

/// The error of a delivery to a runtime that has stopped.
fn stopped() -> ApiError {
    ApiError::new(
        ErrorCode::Unavailable,
        "the job runtime has stopped; the job stays queued in the record store",
    )
}
