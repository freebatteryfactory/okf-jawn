//! A `ConfirmationStore` fake for the proposal and review handlers; not a production adapter.
//!
//! It encodes what the port's doc states, each rule quoted above the code that keeps it, and
//! makes its checks in the order storage's `SqliteConfirmations::consume` makes them, with the
//! same codes: an identity this workspace does not hold is `NotFound`; another subject or
//! session is `Forbidden`; a confirmation another mutation consumed is `Conflict`; another
//! action or target, another revision or digest, and an expired one are `Conflict`. The same
//! mutation may consume it again, but only while every check still passes: an expired
//! confirmation is refused to it too, as storage refuses it. Its clock is the test's
//! (`set_now`). Where it is kinder than storage: it refuses nothing while a purge names the
//! workspace.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{ConfirmationId, MutationId, Timestamp};
use okf_jawn_contract::review::Confirmation;
use okf_jawn_core::confirmations::{ConfirmationConsume, ConfirmationCreate, ConfirmationStore};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::StorageScope;
use uuid::Uuid;

use crate::fixture::{Built, instant};

/// `ConfirmationStore` over challenges held in memory.
pub struct FakeConfirmations {
    held: Mutex<Vec<Held>>,
    now: Mutex<Timestamp>,
    consumes: AtomicUsize,
}

/// One issued challenge.
#[derive(Clone)]
struct Held {
    scope: StorageScope,
    mutation: MutationId,
    id: ConfirmationId,
    create: ConfirmationCreate,
    consumed_by: Option<MutationId>,
}

impl FakeConfirmations {
    /// A store whose clock reads the fixture's instant.
    ///
    /// # Errors
    /// Returns when the instant does not parse.
    pub fn new() -> Built<Self> {
        Ok(Self {
            held: Mutex::new(Vec::new()),
            now: Mutex::new(instant()?),
            consumes: AtomicUsize::new(0),
        })
    }

    /// Set the instant the store compares expiries with.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn set_now(&self, now: Timestamp) -> Built<()> {
        *lock(&self.now)? = now;
        Ok(())
    }

    /// Every challenge issued, with its identity and the mutation that consumed it.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn issued(&self) -> Built<Vec<(ConfirmationId, ConfirmationCreate, Option<MutationId>)>> {
        Ok(lock(&self.held)?
            .iter()
            .map(|held| (held.id, held.create.clone(), held.consumed_by))
            .collect())
    }

    /// How many times `consume` was called.
    #[must_use]
    pub fn consumes(&self) -> usize {
        self.consumes.load(Ordering::SeqCst)
    }
}

impl ConfirmationStore for FakeConfirmations {
    // "Create a session-bound challenge and allocate its identity. Unique on `mutation_id`: a
    // repeated id creates nothing and returns the prior challenge."
    fn create<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        create: ConfirmationCreate,
    ) -> PortFuture<'a, Confirmation> {
        Box::pin(async move {
            let mut held = lock_api(&self.held)?;
            if let Some(prior) = held.iter().find(|held| held.mutation == mutation_id) {
                return Ok(confirmation(prior));
            }
            let number = u128::try_from(held.len()).map_err(internal)?;
            let issued = Held {
                scope: scope.clone(),
                mutation: mutation_id,
                id: ConfirmationId(Uuid::from_u128(number.saturating_add(0xc0_0000))),
                create,
                consumed_by: None,
            };
            let answer = confirmation(&issued);
            held.push(issued);
            Ok(answer)
        })
    }

    // "Atomically consume a confirmation for `mutation_id`. Checks subject, session, action,
    // target, revision, digest and expiry, and records which `MutationId` consumed it.
    // Consuming again with the same `MutationId` succeeds while those checks still pass: the
    // expiry is checked on every consume, so once the confirmation has expired it is refused
    // (`Conflict`) to the mutation that consumed it too ... A different `MutationId` fails with
    // `Conflict` as already used." The order and codes are storage's.
    fn consume<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        consume: ConfirmationConsume,
    ) -> PortFuture<'a, Confirmation> {
        self.consumes.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let now = lock_api(&self.now)?.clone();
            let mut held = lock_api(&self.held)?;
            let challenge = held
                .iter_mut()
                .find(|held| held.id == consume.confirmation_id && held.scope == *scope)
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such confirmation"))?;
            let create = &challenge.create;
            if create.subject != consume.subject || create.session_id != consume.session_id {
                return Err(ApiError::new(
                    ErrorCode::Forbidden,
                    "this confirmation belongs to another session",
                ));
            }
            if challenge
                .consumed_by
                .is_some_and(|consumer| consumer != mutation_id)
            {
                return Err(conflict("this confirmation was already used"));
            }
            if create.action != consume.action || create.target != consume.target {
                return Err(conflict(
                    "this confirmation was issued for another action or target",
                ));
            }
            if create.revision != consume.revision
                || create.content_digest != consume.content_digest
            {
                return Err(conflict(
                    "this confirmation was issued for another revision or content",
                ));
            }
            if create.expires_at.as_str() <= now.as_str() {
                return Err(conflict("this confirmation has expired"));
            }
            challenge.consumed_by = Some(mutation_id);
            Ok(confirmation(challenge))
        })
    }
}

fn confirmation(held: &Held) -> Confirmation {
    Confirmation {
        id: held.id,
        expires_at: held.create.expires_at.clone(),
        revision: held.create.revision.clone(),
    }
}

fn conflict(message: &str) -> ApiError {
    ApiError::new(ErrorCode::Conflict, message)
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::new(ErrorCode::Internal, error.to_string())
}

fn lock<T>(mutex: &Mutex<T>) -> Built<MutexGuard<'_, T>> {
    mutex.lock().map_err(|_| "a fake's lock is poisoned".into())
}

fn lock_api<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, ApiError> {
    mutex
        .lock()
        .map_err(|_| ApiError::new(ErrorCode::Internal, "a fake's lock is poisoned"))
}
