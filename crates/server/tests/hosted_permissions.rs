//! A workspace's effective permissions under the hosted grant cache (`grants::CachedAccess`),
//! composed as the hosted service composes it: one access adapter, the cache over a raw adapter
//! (the real local one stands in for the hosted raw adapter), for dispatch and the application
//! service alike, over real storage.
//!
//! SPEC section 11: "Workspace list/open responses include the caller's effective permissions so
//! the UI can show the right actions." A grant the cache answers carries only `read`, `write`
//! and `propose`, and `grant_creator` seeds the cache, so an `open_workspace` that took its
//! permissions from the grant dispatch looked up for its `read` check would answer three where
//! `list_workspaces` answers six. Both must answer the caller's grant as the adapter's
//! `grants` reads it, which the cache never answers.
#![cfg(feature = "runtime")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{JobId, TenantId};
use okf_jawn_contract::workspace::{ListWorkspacesResponse, Workspace};
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::application::{ApplicationConfig, ApplicationService, Ports};
use okf_jawn_core::conversion::{
    Conversion, ConversionInput, Converter, ConverterLimits, PageCount,
};
use okf_jawn_core::credentials::CredentialStore;
use okf_jawn_core::dispatch::{Caller, DispatchPorts, dispatch};
use okf_jawn_core::jobs::{JobQueue, JobScope};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::LocalSource;
use okf_jawn_server::grants::{CachedAccess, SystemClock};
use okf_jawn_storage::Storage;
use serde_json::{Value, json};

use check::{TestResult, some};

/// A data directory of its own, removed when dropped.
struct Scratch(PathBuf);

/// The job queue: nothing in this test starts a job.
struct NoQueue;

/// The converter: nothing in this test converts.
struct NoConverter;

/// Every permission, in the contract's order: what the local adapter grants a creator.
const EVERY: [Permission; 6] = [
    Permission::Read,
    Permission::Write,
    Permission::Propose,
    Permission::Approve,
    Permission::Review,
    Permission::Admin,
];

impl Scratch {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = std::env::temp_dir().join(format!(
            "okf-jawn-hosted-permissions-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: a leftover directory under the temporary directory harms nothing.
        let _left = std::fs::remove_dir_all(&self.0);
    }
}

impl JobQueue for NoQueue {
    fn enqueue(&self, _scope: JobScope, _job: JobId) -> PortFuture<'_, ()> {
        Box::pin(async { Err(unused()) })
    }
}

impl Converter for NoConverter {
    fn page_count<'a>(
        &'a self,
        _source: &'a LocalSource,
        _file_name: &'a str,
    ) -> PortFuture<'a, PageCount> {
        Box::pin(async { Err(unused()) })
    }

    fn convert(&self, _input: ConversionInput) -> PortFuture<'_, Conversion> {
        Box::pin(async { Err(unused()) })
    }

    fn limits(&self) -> ConverterLimits {
        ConverterLimits {
            window_pages: 1,
            memory_limit_bytes: 1,
        }
    }
}

fn unused() -> ApiError {
    ApiError::new(ErrorCode::Internal, "not used by this test")
}

/// The application service over every store of `storage`, authorizing through `access`.
fn application(
    storage: &Storage,
    access: Arc<dyn AccessControl>,
) -> Result<ApplicationService, ApiError> {
    let ports = Ports {
        access,
        mutations: Arc::new(storage.mutations()),
        catalog: Arc::new(storage.catalog()),
        blobs: Arc::new(storage.blobs()),
        versions: Arc::new(storage.versions()),
        records: Arc::new(storage.records()),
        queue: Arc::new(NoQueue),
        search: Arc::new(storage.search()),
        converter: Arc::new(NoConverter),
        proposals: Arc::new(storage.proposals()),
        uploads: Arc::new(storage.uploads()),
        events: Arc::new(storage.events()),
        credentials: Arc::new(storage.credentials()),
        drafts: Arc::new(storage.drafts()),
        confirmations: Arc::new(storage.confirmations()),
        sandbox: Arc::new(storage.sandbox()),
        readiness: Arc::new(storage.readiness()),
        backups: Arc::new(storage.backups()),
    };
    ApplicationService::new(
        ports,
        ApplicationConfig::new("http://127.0.0.1:8788".to_owned(), Duration::from_secs(60)),
    )
}

async fn call(
    service: &ApplicationService,
    ports: &DispatchPorts<'_>,
    principal: &Principal,
    operation: &str,
    input: Value,
) -> Result<Value, ApiError> {
    let caller = Caller {
        principal,
        session_id: None,
    };
    dispatch(service, ports, &caller, operation, input).await
}

#[tokio::test]
async fn open_and_list_answer_the_same_permissions_under_the_hosted_grant_cache() -> TestResult {
    let scratch = Scratch::new()?;
    let storage = Storage::open(&scratch.0)?;
    let identity = storage.credentials().installation_identity().await?;
    let access: Arc<dyn AccessControl> = Arc::new(CachedAccess::new(
        Arc::new(storage.access()),
        Arc::new(SystemClock),
    ));
    let service = application(&storage, Arc::clone(&access))?;
    let mutations = storage.mutations();
    let events = storage.events();
    let ports = DispatchPorts {
        access: access.as_ref(),
        mutations: &mutations,
        events: &events,
    };
    let owner = Principal {
        subject: identity.subject,
        tenant_id: TenantId::local(),
        route: AccessRoute::BrowserSession,
        client_id: None,
        delegation: None,
    };
    // `grant_creator` seeds the cache with the creator's grant, so even the first open's `read`
    // check is answered from it.
    let created: Workspace = serde_json::from_value(
        call(
            &service,
            &ports,
            &owner,
            "create_workspace",
            json!({
                "name": "hosted", "description": "",
                "idempotency_key": "00000000-0000-4000-8000-000000000001"
            }),
        )
        .await?,
    )?;
    assert_eq!(created.permissions, EVERY.to_vec(), "the creator's answer");
    let listed: ListWorkspacesResponse = serde_json::from_value(
        call(
            &service,
            &ports,
            &owner,
            "list_workspaces",
            json!({ "page": { "limit": 10 } }),
        )
        .await?,
    )?;
    let in_list = some(listed.items.first(), "the listed workspace")?;
    assert_eq!(in_list.id, created.id);
    assert_eq!(in_list.permissions, EVERY.to_vec(), "the listing");
    // Twice: the first open after the creation and one whose `read` check is again the cache's.
    for open in 0..2 {
        let opened: Workspace = serde_json::from_value(
            call(
                &service,
                &ports,
                &owner,
                "open_workspace",
                json!({ "workspace_id": created.id }),
            )
            .await?,
        )?;
        assert_eq!(
            opened.permissions, in_list.permissions,
            "open number {open}"
        );
    }
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
