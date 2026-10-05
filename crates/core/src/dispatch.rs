//! Route only declared operations through validation, target authorization, and the mutation ledger.
//!
//! Order: validate, decode, `targets()`, authorize every target, build the context,
//! `MutationStore::begin`, the handler, `complete`. `request_workspace` is gone.

use okf_jawn_contract::access::Principal;
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{ConnectorId, IdempotencyKey, MutationId};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::scope::{ReplayPolicy, RequestScope, Target};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::OnceLock;
use uuid::Uuid;

use crate::access::{self, AccessControl};
use crate::context::{OperationContext, TenantGrant, WorkspaceGrant};
use crate::mutations::{
    AbandonedEffects, BeginOutcome, MutationKey, MutationStore, request_digest,
};
use crate::ports::Application;

/// Ports dispatch needs beyond the typed application handlers.
pub struct DispatchPorts<'a> {
    /// Grant lookup.
    pub access: &'a dyn AccessControl,
    /// Idempotency ledger.
    pub mutations: &'a dyn MutationStore,
    /// Creating-store lookups for abandoned-lease reconciliation.
    pub effects: &'a dyn AbandonedEffects,
}

macro_rules! dispatch_operations {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        /// Dispatch one declared JSON operation into its typed implementation.
        ///
        /// # Errors
        /// Returns input, authorization, idempotency, implementation, or serialization errors.
        pub async fn dispatch(
            service: &dyn Application,
            ports: &DispatchPorts<'_>,
            principal: &Principal,
            operation_id: &str,
            input: Value,
        ) -> Result<Value, ApiError> {
            let operation = parse_operation(operation_id)?;
            match operation_id {
                $(stringify!($id) => {
                    static VALIDATOR: OnceLock<Result<jsonschema::Validator, String>> = OnceLock::new();
                    let request_value = input.clone();
                    let request: $request = decode_validated(input, &VALIDATOR)?;
                    let mut context = authorize_targets(
                        ports.access,
                        principal,
                        operation,
                        &request,
                    ).await?;
                    let replay = <$request as RequestScope>::REPLAY;
                    let key = request.idempotency_key().copied();
                    match prepare_mutation(ports, &mut context, replay, key, &request_value).await? {
                        MutationGate::Run => {
                            let response = service.$id(&context, request).await?;
                            finish_if_mutation(ports, &context, replay, response).await
                        }
                        MutationGate::ShortCircuit(value) => Ok(value),
                    }
                }),*
                _ => Err(ApiError::new(ErrorCode::NotFound, "Unknown operation")),
            }
        }
    };
}

enum MutationGate {
    Run,
    ShortCircuit(Value),
}

async fn prepare_mutation(
    ports: &DispatchPorts<'_>,
    context: &mut OperationContext,
    replay: ReplayPolicy,
    idempotency_key: Option<IdempotencyKey>,
    request_value: &Value,
) -> Result<MutationGate, ApiError> {
    let Some(key) = idempotency_key else {
        return Ok(MutationGate::Run);
    };
    let digest = request_digest(request_value)?;
    let mutation_key = MutationKey {
        tenant_id: context.principal.tenant_id.clone(),
        subject: context.principal.subject.clone(),
        operation: context.operation,
        key,
    };
    match ports.mutations.begin(&mutation_key, &digest).await? {
        BeginOutcome::New(mutation_id) => {
            context.mutation = Some(mutation_id);
            Ok(MutationGate::Run)
        }
        BeginOutcome::Replay(stored) => {
            replay_response(replay, stored.body).map(MutationGate::ShortCircuit)
        }
        BeginOutcome::Conflict { operation } => Err(ApiError::new(
            ErrorCode::Conflict,
            "Idempotency key was reused with a different request body",
        )
        .with_detail(ErrorDetail::IdempotencyConflict { operation })),
        BeginOutcome::InProgress {
            mutation_id,
            retry_after,
        } => Err(ApiError::new(
            ErrorCode::InProgress,
            "Another attempt with this idempotency key is still running",
        )
        .with_detail(ErrorDetail::InProgress {
            mutation_id,
            retry_after,
        })),
        BeginOutcome::Abandoned { mutation_id } => {
            if let Some(body) = ports
                .effects
                .lookup(context.operation, mutation_id)
                .await?
            {
                ports.mutations.complete(mutation_id, body.clone()).await?;
                return replay_response(replay, body).map(MutationGate::ShortCircuit);
            }
            if let Some(stored) = ports.mutations.find(mutation_id).await? {
                ports
                    .mutations
                    .complete(mutation_id, stored.body.clone())
                    .await?;
                return replay_response(replay, stored.body).map(MutationGate::ShortCircuit);
            }
            context.mutation = Some(mutation_id);
            Ok(MutationGate::Run)
        }
    }
}

async fn finish_if_mutation<Res: Serialize>(
    ports: &DispatchPorts<'_>,
    context: &OperationContext,
    _replay: ReplayPolicy,
    response: Res,
) -> Result<Value, ApiError> {
    let body = serialize_response(response)?;
    let Some(mutation_id) = context.mutation else {
        return Ok(body);
    };
    let stored = match context.operation {
        OperationName::CreateConnector => strip_connector_secret(body.clone())?,
        _ => body.clone(),
    };
    ports.mutations.complete(mutation_id, stored).await?;
    Ok(body)
}

fn serialize_response<Res: Serialize>(response: Res) -> Result<Value, ApiError> {
    serde_json::to_value(response)
        .map_err(|_| ApiError::new(ErrorCode::Internal, "Response serialization failed"))
}

fn strip_connector_secret(body: Value) -> Result<Value, ApiError> {
    let connector_id = body
        .get("connector")
        .and_then(|connector| connector.get("connector_id"))
        .cloned()
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::Internal,
                "Issued connector response missing connector_id",
            )
        })?;
    Ok(serde_json::json!({ "connector_id": connector_id }))
}

fn replay_response(replay: ReplayPolicy, body: Value) -> Result<Value, ApiError> {
    match replay {
        ReplayPolicy::StoredResponse => Ok(body),
        ReplayPolicy::AlreadyIssued { .. } => {
            let connector_id: ConnectorId = body
                .get("connector_id")
                .cloned()
                .ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::Internal,
                        "AlreadyIssued ledger row missing connector_id",
                    )
                })
                .and_then(|value| {
                    serde_json::from_value(value)
                        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))
                })?;
            Err(ApiError::new(
                ErrorCode::AlreadyIssued,
                "Connector was already issued under this idempotency key",
            )
            .with_detail(ErrorDetail::AlreadyIssued { connector_id }))
        }
    }
}

async fn authorize_targets(
    access: &dyn AccessControl,
    principal: &Principal,
    operation: OperationName,
    request: &impl RequestScope,
) -> Result<OperationContext, ApiError> {
    let targets = request.targets();
    let mut grants = Vec::new();
    let mut tenant: Option<TenantGrant> = None;
    for target in targets {
        match target {
            Target::Authenticated => {}
            Target::Deployment(permission) => {
                let raw = access.authorize_tenant(principal, permission).await?;
                let grant = access::authorize_tenant(principal, raw, permission)?;
                tenant = Some(grant);
            }
            Target::Workspace(workspace, permission) => {
                let raw = access.authorize(principal, workspace, permission).await?;
                let grant = access::authorize_workspace(principal, raw, permission)?;
                if !grants
                    .iter()
                    .any(|existing: &WorkspaceGrant| existing.workspace_id() == workspace)
                {
                    grants.push(grant);
                }
            }
        }
    }
    Ok(OperationContext {
        principal: principal.clone(),
        operation,
        tenant,
        grants,
        mutation: None,
    })
}

fn parse_operation(operation_id: &str) -> Result<OperationName, ApiError> {
    OperationName::ALL
        .iter()
        .copied()
        .find(|name| name.as_str() == operation_id)
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "Unknown operation"))
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T, ApiError> {
    serde_json::from_value(value)
        .map_err(|error| ApiError::new(ErrorCode::InvalidInput, error.to_string()))
}

fn decode_validated<T: DeserializeOwned + JsonSchema>(
    value: Value,
    cell: &OnceLock<Result<jsonschema::Validator, String>>,
) -> Result<T, ApiError> {
    let validator = cell
        .get_or_init(|| {
            let schema = SchemaSettings::draft2020_12()
                .into_generator()
                .into_root_schema_for::<T>();
            let document = serde_json::to_value(schema).map_err(|error| error.to_string())?;
            jsonschema::validator_for(&document).map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|message| ApiError::new(ErrorCode::Internal, message.clone()))?;
    validator
        .validate(&value)
        .map_err(|error| ApiError::new(ErrorCode::InvalidInput, error.to_string()))?;
    decode(value)
}

/// Allocate a fresh mutation identity for adapters and fixtures.
#[must_use]
pub fn new_mutation_id() -> MutationId {
    MutationId(Uuid::new_v4())
}

okf_jawn_contract::for_each_operation!(dispatch_operations);
