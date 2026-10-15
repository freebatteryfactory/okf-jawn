//! The Tokio + `RecordStore` runtime: a delivery is only a hint to claim, a repeated delivery
//! runs nothing twice, a handler's fault is recorded through its lease, and unfinished work is
//! reconciled at start and after a lease expires.
//!
//! These run against an in-memory ledger (`support/records.rs`). The crash and restart against
//! the real `RecordStore` is the separate construction receipt `job-crash-against-record-store`
//! (decision I1, Wave 3).
#![cfg(feature = "runtime")]

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "support/records.rs"]
mod records;

mod job_runtime {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use okf_jawn_contract::access::AccessRoute;
    use okf_jawn_contract::error::{ApiError, ErrorCode};
    use okf_jawn_contract::identity::{JobId, MutationId, TenantId, WorkspaceId};
    use okf_jawn_contract::import::JobState;
    use okf_jawn_core::events::{EventLog, EventScope};
    use okf_jawn_core::jobs::{
        ClaimedJob, JobCompletion, JobHandler, JobQueue, JobScope, JobSpec, NewJob, RecordStore,
    };
    use okf_jawn_core::ports::PortFuture;
    use okf_jawn_core::storage::{Provenance, StorageScope};
    use okf_jawn_ingest::runtime::{JobRuntime, RuntimeConfig, run_one};

    use crate::check::TestResult;
    use crate::records::{FakeEvents, FakeRecords, job_id};

    /// What the handler does on one attempt.
    #[derive(Debug, Clone, Copy)]
    enum Action {
        /// Write the completion through the lease.
        Complete,
        /// Return a worker fault.
        Fail,
        /// Return without completing, as an attempt whose process died would leave it.
        Abandon,
    }

    /// The ledger, the handler and the job a test runs.
    type Setup = (Arc<FakeRecords>, Arc<ScriptedHandler>, JobId);

    struct ScriptedHandler {
        records: Arc<FakeRecords>,
        attempts: Mutex<Vec<u32>>,
        script: fn(u32) -> Action,
    }

    /// A handler that never finishes: every worker that takes a job stays busy with it.
    struct BusyHandler;

    impl JobHandler for BusyHandler {
        fn handle<'a>(&'a self, _claimed: &'a ClaimedJob) -> PortFuture<'a, ()> {
            Box::pin(async move {
                std::future::pending::<()>().await;
                Ok(())
            })
        }
    }

    impl JobHandler for ScriptedHandler {
        fn handle<'a>(&'a self, claimed: &'a ClaimedJob) -> PortFuture<'a, ()> {
            Box::pin(async move {
                self.attempts
                    .lock()
                    .map_err(|_| ApiError::new(ErrorCode::Internal, "poisoned"))?
                    .push(claimed.lease.attempt);
                match (self.script)(claimed.lease.attempt) {
                    Action::Complete => {
                        let _job = self
                            .records
                            .complete_job(JobCompletion {
                                lease: claimed.lease.clone(),
                                revision: None,
                                item_ids: Vec::new(),
                                artifact: None,
                                restore: None,
                                backup: None,
                                outputs: Vec::new(),
                                warnings: Vec::new(),
                            })
                            .await?;
                        Ok(())
                    }
                    Action::Fail => {
                        Err(ApiError::new(ErrorCode::Internal, "the worker broke")
                            .with_field("/spec"))
                    }
                    Action::Abandon => Ok(()),
                }
            })
        }
    }

    impl ScriptedHandler {
        fn attempts(&self) -> Result<Vec<u32>, ApiError> {
            self.attempts
                .lock()
                .map(|attempts| attempts.clone())
                .map_err(|_| ApiError::new(ErrorCode::Internal, "poisoned"))
        }
    }

    fn setup(script: fn(u32) -> Action) -> Result<Setup, Box<dyn std::error::Error>> {
        let records = Arc::new(FakeRecords::default());
        let handler = Arc::new(ScriptedHandler {
            records: Arc::clone(&records),
            attempts: Mutex::new(Vec::new()),
            script,
        });
        let id = job_id(1)?;
        let scope = JobScope::Workspace(StorageScope {
            tenant_id: TenantId::try_from("tenant-a".to_owned())?,
            workspace_id: serde_json::from_value::<WorkspaceId>(serde_json::json!(
                "00000000-0000-0000-0000-0000000000aa"
            ))?,
        });
        records.insert(
            id,
            scope,
            NewJob {
                mutation_id: serde_json::from_value::<MutationId>(serde_json::json!(
                    "00000000-0000-0000-0000-0000000000bb"
                ))?,
                initiator: Provenance {
                    subject: "owner".to_owned(),
                    route: AccessRoute::LocalOwner,
                    client_id: None,
                },
                spec: JobSpec::RebuildIndex,
            },
        )?;
        Ok((records, handler, id))
    }

    fn config() -> RuntimeConfig {
        RuntimeConfig {
            workers: 2,
            sweep_interval: Duration::from_millis(20),
        }
    }

    /// Wait, up to five seconds, until the job reaches `state`.
    async fn until_state(records: &FakeRecords, id: JobId, state: JobState) -> TestResult {
        let reached = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if records
                    .entry(id)
                    .is_ok_and(|entry| entry.job.state == state)
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        reached.map_err(|_| format!("the job never reached {state:?}").into())
    }

    #[tokio::test]
    async fn a_repeated_delivery_claims_nothing_and_runs_nothing_twice() -> TestResult {
        let (records, handler, id) = setup(|_| Action::Complete)?;
        assert!(
            run_one(
                records.as_ref(),
                &FakeEvents::default(),
                handler.as_ref(),
                id
            )
            .await?
        );
        assert!(
            !run_one(
                records.as_ref(),
                &FakeEvents::default(),
                handler.as_ref(),
                id
            )
            .await?
        );
        assert_eq!(handler.attempts()?, [1]);
        let entry = records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded);
        assert_eq!(entry.completions.len(), 1);

        // The same through the queue: three deliveries, one run.
        let (records, handler, id) = setup(|_| Action::Complete)?;
        let (runtime, queue) = JobRuntime::start(
            &(Arc::clone(&records) as Arc<dyn RecordStore>),
            &(Arc::new(FakeEvents::default()) as Arc<dyn EventLog>),
            &(Arc::clone(&handler) as Arc<dyn JobHandler>),
            config(),
        );
        let scope = records.entry(id)?.scope;
        for _ in 0..3 {
            queue.enqueue(scope.clone(), id).await?;
        }
        until_state(&records, id, JobState::Succeeded).await?;
        tokio::time::sleep(Duration::from_millis(60)).await;
        runtime.shutdown().await;
        assert_eq!(handler.attempts()?, [1]);
        Ok(())
    }

    #[tokio::test]
    async fn a_claim_refused_for_a_recorded_purge_is_skipped_not_a_fault() -> TestResult {
        let (records, handler, id) = setup(|_| Action::Complete)?;
        records.purge_workspace_of(id)?;
        let events = FakeEvents::default();
        assert!(!run_one(records.as_ref(), &events, handler.as_ref(), id).await?);
        // Nothing ran, nothing was failed or announced, and the job is left for the purge's
        // caller to cancel.
        assert_eq!(handler.attempts()?, Vec::<u32>::new());
        let entry = records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Queued);
        assert_eq!(entry.failures, Vec::<(String, bool)>::new());
        assert_eq!(events.appended()?, Vec::new());
        Ok(())
    }

    #[tokio::test]
    async fn a_handler_fault_is_recorded_as_a_retryable_failure_through_its_lease() -> TestResult {
        let (records, handler, id) = setup(|_| Action::Fail)?;
        let events = FakeEvents::default();
        assert!(run_one(records.as_ref(), &events, handler.as_ref(), id).await?);
        let entry = records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        assert_eq!(entry.failures, [("the worker broke".to_owned(), true)]);
        // The handler's error is kept as it gave it, its field included.
        assert_eq!(
            entry
                .job
                .error
                .as_ref()
                .and_then(|error| error.field.as_deref()),
            Some("/spec")
        );
        // The failure it wrote is announced, in the job's workspace log, with no mutation.
        let JobScope::Workspace(scope) = entry.scope else {
            return Err("a workspace job was expected".into());
        };
        let appended = events.appended()?;
        assert_eq!(appended.len(), 1);
        let (log, mutation, event) = crate::check::some(appended.first(), "the event")?;
        assert_eq!(*log, EventScope::Workspace(scope));
        assert_eq!(*mutation, None);
        assert_eq!(
            (&event.kind, event.job_id),
            (&okf_jawn_contract::events::EventKind::JobUpdated, Some(id))
        );
        Ok(())
    }

    #[tokio::test]
    async fn work_a_dead_process_left_running_is_reclaimed_and_run_at_start() -> TestResult {
        let (records, handler, id) = setup(|_| Action::Complete)?;
        // A previous process claimed the job and died; its lease has since expired.
        let _dead = records.claim_job(id).await?;
        records.expire_running()?;
        let (runtime, _queue) = JobRuntime::start(
            &(Arc::clone(&records) as Arc<dyn RecordStore>),
            &(Arc::new(FakeEvents::default()) as Arc<dyn EventLog>),
            &(Arc::clone(&handler) as Arc<dyn JobHandler>),
            config(),
        );
        until_state(&records, id, JobState::Succeeded).await?;
        runtime.shutdown().await;
        assert_eq!(handler.attempts()?, [2]);
        assert_eq!(records.entry(id)?.completions.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn an_attempt_that_dies_without_a_restart_is_retried_after_its_lease_expires()
    -> TestResult {
        let (records, handler, id) = setup(|attempt| {
            if attempt == 1 {
                Action::Abandon
            } else {
                Action::Complete
            }
        })?;
        let (runtime, queue) = JobRuntime::start(
            &(Arc::clone(&records) as Arc<dyn RecordStore>),
            &(Arc::new(FakeEvents::default()) as Arc<dyn EventLog>),
            &(Arc::clone(&handler) as Arc<dyn JobHandler>),
            config(),
        );
        queue.enqueue(records.entry(id)?.scope, id).await?;
        // The first attempt ends with the job still running under its lease.
        let first = tokio::time::timeout(Duration::from_secs(5), async {
            while handler.attempts().map_or(0, |attempts| attempts.len()) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        first.map_err(|_| "the first attempt never ran")?;
        // While the lease is live, sweeps re-deliver the job but nobody can claim it.
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(handler.attempts()?, [1]);
        assert_eq!(records.entry(id)?.job.state, JobState::Running);
        // The lease lapses: the next sweep releases and re-delivers it.
        records.expire_running()?;
        until_state(&records, id, JobState::Succeeded).await?;
        runtime.shutdown().await;
        assert_eq!(handler.attempts()?, [1, 2]);
        Ok(())
    }

    #[tokio::test]
    async fn sweeps_while_every_worker_is_busy_queue_each_pending_job_at_most_once() -> TestResult {
        const PENDING: u128 = 4;
        let (records, _handler, first) = setup(|_| Action::Complete)?;
        let entry = records.entry(first)?;
        for number in 2..=PENDING {
            records.insert(job_id(number)?, entry.scope.clone(), entry.created.clone())?;
        }
        // One worker, which takes the first job and stays busy with it; the sweep runs every
        // 5 ms, re-delivering every unfinished job, the running one included.
        let (runtime, queue) = JobRuntime::start(
            &(Arc::clone(&records) as Arc<dyn RecordStore>),
            &(Arc::new(FakeEvents::default()) as Arc<dyn EventLog>),
            &(Arc::new(BusyHandler) as Arc<dyn JobHandler>),
            RuntimeConfig {
                workers: 1,
                sweep_interval: Duration::from_millis(5),
            },
        );
        until_state(&records, first, JobState::Running).await?;
        // Dozens of sweeps.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let waiting = queue.waiting()?;
        runtime.shutdown().await;
        assert!(waiting >= 1, "no sweep delivered anything");
        assert!(
            waiting <= usize::try_from(PENDING)?,
            "{waiting} deliveries wait for {PENDING} pending jobs"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_stopped_runtime_refuses_deliveries_and_the_job_stays_queued() -> TestResult {
        let (records, handler, id) = setup(|_| Action::Complete)?;
        let (runtime, queue) = JobRuntime::start(
            &(Arc::clone(&records) as Arc<dyn RecordStore>),
            &(Arc::new(FakeEvents::default()) as Arc<dyn EventLog>),
            &(Arc::clone(&handler) as Arc<dyn JobHandler>),
            config(),
        );
        runtime.shutdown().await;
        let refused = queue.enqueue(records.entry(id)?.scope, id).await;
        assert_eq!(
            refused.map_err(|error| error.code),
            Err(ErrorCode::Unavailable)
        );
        assert_eq!(records.entry(id)?.job.state, JobState::Queued);
        Ok(())
    }
}
