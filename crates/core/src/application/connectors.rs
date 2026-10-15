//! Local connector credentials: issuing, listing and revoking them (SPEC section 11).
//!
//! These operations are the local owner's: dispatch refuses them on every other route, and
//! records each refusal, before any grant is looked up (`access::check_local_owner_route`,
//! contract ruling 22). Dispatch has also required the tenant's `admin` and, for each workspace
//! a new connector lists, that the owner may read it (and propose there, when the connector may
//! propose), naming the refused entry `/workspace_ids/<index>`.
//!
//! The secret of a new connector is returned once. The store keeps only its hash and is unique
//! on the mutation (`CredentialStore::create_connector`). A replay of a completed issue is the
//! ledger's: `already_issued`, never the secret again (`ReplayPolicy::AlreadyIssued`). A resumed
//! attempt (an earlier attempt under the mutation ended without answering) finds the connector
//! that attempt created and rotates its secret, so the secret nobody received stops working and
//! the caller gets one that works (SPEC section 11). A first attempt that finds a connector
//! under its mutation, which only a ledger that lost its row could cause, is answered
//! `already_issued` and is shown no secret.
//!
//! Each issue and each revocation is recorded in the tenant's log as a `connector_issued` or
//! `connector_revoked` event naming the connector and the owner, appended under the request's
//! mutation, so a retry appends no second event. A revocation is immediate and repeating it
//! changes nothing in the store; a second revocation request under another key records a
//! second event naming the same connector. Listing answers no secret, and revoked connectors
//! only when asked.

use okf_jawn_contract::access::{
    Connector, CreateConnectorRequest, IssuedConnector, ListConnectorsRequest,
    ListConnectorsResponse, RevokeConnectorRequest,
};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};

use super::ApplicationService;
use super::shared::{mutation_id, tenant_scope};
use crate::context::{Attempt, OperationContext};
use crate::credentials::{ConnectorIssue, NewConnector};
use crate::events::{EventScope, NewEvent};

/// Issue a connector to the listed workspaces and answer its secret, once.
///
/// # Errors
/// Returns `AlreadyIssued` naming the connector when a first attempt finds one under its
/// mutation, or any port error.
pub(super) async fn create_connector(
    service: &ApplicationService,
    context: &OperationContext,
    request: CreateConnectorRequest,
) -> Result<IssuedConnector, ApiError> {
    let mutation = mutation_id(context)?;
    let tenant = tenant_scope(context)?;
    let ports = service.ports();
    let issue = ports
        .credentials
        .create_connector(
            mutation,
            NewConnector {
                label: request.label,
                workspace_ids: request.workspace_ids,
                allow_propose: request.allow_propose,
            },
        )
        .await?;
    let issued = match (issue, context.attempt) {
        (ConnectorIssue::Issued(issued), _) => issued,
        (ConnectorIssue::Existing(existing), Attempt::Resumed) => {
            ports
                .credentials
                .rotate_connector_secret(existing.connector_id)
                .await?
        }
        (ConnectorIssue::Existing(existing), Attempt::First) => {
            return Err(ApiError::new(
                ErrorCode::AlreadyIssued,
                "Connector was already issued under this idempotency key",
            )
            .with_detail(ErrorDetail::AlreadyIssued {
                connector_id: existing.connector_id,
            }));
        }
    };
    ports
        .events
        .append(
            &EventScope::Tenant(tenant),
            Some(mutation),
            NewEvent::connector_issued(&context.principal, issued.connector.connector_id),
        )
        .await?;
    Ok(issued)
}

/// The installation's connectors without their secrets; revoked ones only when asked.
///
/// # Errors
/// Returns any port error.
pub(super) async fn list_connectors(
    service: &ApplicationService,
    _context: &OperationContext,
    request: ListConnectorsRequest,
) -> Result<ListConnectorsResponse, ApiError> {
    let connectors = service
        .ports()
        .credentials
        .list_connectors(request.include_revoked)
        .await?;
    Ok(ListConnectorsResponse { connectors })
}

/// Revoke a connector now; it authenticates nothing from then on.
///
/// # Errors
/// Returns any port error (`NotFound` for a connector the installation does not hold).
pub(super) async fn revoke_connector(
    service: &ApplicationService,
    context: &OperationContext,
    request: RevokeConnectorRequest,
) -> Result<Connector, ApiError> {
    let mutation = mutation_id(context)?;
    let tenant = tenant_scope(context)?;
    let ports = service.ports();
    let revoked = ports
        .credentials
        .revoke_connector(request.connector_id)
        .await?;
    ports
        .events
        .append(
            &EventScope::Tenant(tenant),
            Some(mutation),
            NewEvent::connector_revoked(&context.principal, revoked.connector_id),
        )
        .await?;
    Ok(revoked)
}
