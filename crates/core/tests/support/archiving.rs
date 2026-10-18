//! An `Application` for dispatch tests of the archive rule: every operation is the scripted
//! `CountingApplication`'s, and dispatch's archive question (`Application::archived_at`) is
//! answered from a script, with every question kept. Not a production adapter.
//!
//! It is kinder than `ApplicationService` in one way: an unscripted workspace answers "not
//! archived" where the service's catalog would answer `NotFound` for a workspace the tenant does
//! not hold. A test that needs the refusal scripts it (`fail_next`).

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Timestamp, WorkspaceId};
use okf_jawn_core::context::OperationContext;
use okf_jawn_core::ports::{Application, PortFuture};
use okf_jawn_core::storage::StorageScope;

use crate::support::counting::CountingApplication;

macro_rules! archiving_operations {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        impl Application for Archiving {
            $(fn $id<'a>(&'a self, context: &'a OperationContext, request: $request)
                -> PortFuture<'a, $response> {
                self.handlers.$id(context, request)
            })*

            fn archived_at<'a>(&'a self, scope: &'a StorageScope)
                -> PortFuture<'a, Option<Timestamp>> {
                Box::pin(async move { self.answer(scope) })
            }
        }
    };
}

/// The scripted handlers, with a scripted archive state per workspace.
#[derive(Default)]
pub struct Archiving {
    /// Every operation's handler.
    pub handlers: CountingApplication,
    archived: Mutex<BTreeMap<WorkspaceId, Timestamp>>,
    asked: Mutex<Vec<StorageScope>>,
    failure: Mutex<Option<ApiError>>,
}

impl Archiving {
    /// Answer that `workspace` was archived at `at`.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn archive(&self, workspace: WorkspaceId, at: Timestamp) -> Result<(), ApiError> {
        lock(&self.archived)?.insert(workspace, at);
        Ok(())
    }

    /// Answer that `workspace` is not archived.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn unarchive(&self, workspace: WorkspaceId) -> Result<(), ApiError> {
        lock(&self.archived)?.remove(&workspace);
        Ok(())
    }

    /// Make the next question fail with `error`, as a catalog that cannot answer.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn fail_next(&self, error: ApiError) -> Result<(), ApiError> {
        *lock(&self.failure)? = Some(error);
        Ok(())
    }

    /// Every scope dispatch asked about, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn asked(&self) -> Result<Vec<StorageScope>, ApiError> {
        Ok(lock(&self.asked)?.clone())
    }

    fn answer(&self, scope: &StorageScope) -> Result<Option<Timestamp>, ApiError> {
        lock(&self.asked)?.push(scope.clone());
        if let Some(error) = lock(&self.failure)?.take() {
            return Err(error);
        }
        Ok(lock(&self.archived)?.get(&scope.workspace_id).cloned())
    }
}

okf_jawn_contract::for_each_operation!(archiving_operations);

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, ApiError> {
    mutex
        .lock()
        .map_err(|_| ApiError::new(ErrorCode::Internal, "an archiving fixture lock is poisoned"))
}
