//! Local connector credentials: the secret returned once, a resumed issue rotating the secret
//! nobody received, the issue and revocation events in the tenant's log, and listing without
//! secrets.

use okf_jawn_contract::access::{
    AccessRoute, Connector, CreateConnectorRequest, ListConnectorsRequest, Permission, Principal,
    RevokeConnectorRequest,
};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::identity::{ConnectorId, IdempotencyKey, MutationId};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_core::context::{Attempt, OperationContext, TenantGrant};
use okf_jawn_core::events::{EventScope, NewEvent};
use okf_jawn_core::ports::Application;
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{ALICE, Built, instant, mutation, scope};
use crate::tenancy::Tenancy;

fn owner() -> Built<Principal> {
    Ok(Principal {
        subject: ALICE.to_owned(),
        tenant_id: scope()?.tenant_id,
        route: AccessRoute::LocalOwner,
        client_id: None,
        delegation: None,
    })
}

/// The context dispatch builds for the local owner: the tenant's `admin`, and `mutation`.
fn context(
    operation: OperationName,
    mutation: Option<MutationId>,
    attempt: Attempt,
) -> Built<OperationContext> {
    Ok(OperationContext {
        principal: owner()?,
        session_id: None,
        operation,
        tenant: Some(TenantGrant {
            tenant_id: scope()?.tenant_id,
            permissions: vec![Permission::Admin],
        }),
        grants: Vec::new(),
        mutation,
        attempt,
    })
}

fn key() -> Built<IdempotencyKey> {
    Ok(serde_json::from_value(serde_json::json!(
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
    ))?)
}

fn issue_request(allow_propose: bool) -> Built<CreateConnectorRequest> {
    Ok(CreateConnectorRequest {
        label: "Desktop agent".to_owned(),
        workspace_ids: vec![scope()?.workspace_id],
        allow_propose,
        idempotency_key: key()?,
    })
}

fn revoke_request(connector: ConnectorId) -> Built<RevokeConnectorRequest> {
    Ok(RevokeConnectorRequest {
        connector_id: connector,
        idempotency_key: key()?,
    })
}

/// Every connector event appended, with its log and mutation.
fn connector_events(tenancy: &Tenancy) -> Built<Vec<(EventScope, Option<MutationId>, NewEvent)>> {
    Ok(tenancy
        .log
        .appended()?
        .into_iter()
        .filter(|(_, _, event)| {
            matches!(
                event.kind,
                EventKind::ConnectorIssued | EventKind::ConnectorRevoked
            )
        })
        .collect())
}

#[tokio::test]
async fn create_connector_returns_the_secret_once_and_records_connector_issued() -> TestResult {
    let tenancy = Tenancy::new()?;
    let issued = tenancy
        .service
        .create_connector(
            &context(
                OperationName::CreateConnector,
                Some(mutation(50)),
                Attempt::First,
            )?,
            issue_request(true)?,
        )
        .await?;
    assert_eq!(issued.connector.label, "Desktop agent");
    assert_eq!(issued.connector.workspace_ids, vec![scope()?.workspace_id]);
    assert_eq!(
        issued.connector.permissions,
        vec![Permission::Read, Permission::Propose]
    );
    let held = tenancy.credentials.secret(issued.connector.connector_id)?;
    assert_eq!(held.as_deref(), Some(issued.secret.as_str()));
    let events = connector_events(&tenancy)?;
    assert_eq!(
        events,
        vec![(
            EventScope::Tenant(scope()?.tenant_id),
            Some(mutation(50)),
            NewEvent::connector_issued(&owner()?, issued.connector.connector_id),
        )]
    );
    let (_, _, event) = some(events.first(), "the event")?;
    assert_eq!(event.connector_id, Some(issued.connector.connector_id));
    assert_eq!(
        event.actor.as_ref().map(|actor| actor.subject.as_str()),
        Some(ALICE)
    );
    Ok(())
}

#[tokio::test]
async fn a_resumed_create_connector_rotates_the_secret_nobody_received() -> TestResult {
    let tenancy = Tenancy::new()?;
    let first = tenancy
        .service
        .create_connector(
            &context(
                OperationName::CreateConnector,
                Some(mutation(50)),
                Attempt::First,
            )?,
            issue_request(false)?,
        )
        .await?;
    // The first attempt's answer never reached the caller; the ledger resumes the mutation.
    let resumed = tenancy
        .service
        .create_connector(
            &context(
                OperationName::CreateConnector,
                Some(mutation(50)),
                Attempt::Resumed,
            )?,
            issue_request(false)?,
        )
        .await?;
    assert_eq!(resumed.connector.connector_id, first.connector.connector_id);
    assert_ne!(resumed.secret, first.secret, "a new secret");
    let held = tenancy.credentials.secret(first.connector.connector_id)?;
    assert_eq!(
        held.as_deref(),
        Some(resumed.secret.as_str()),
        "the secret nobody received stops working"
    );
    assert_eq!(
        tenancy.credentials.counts()?,
        (1, 1),
        "one connector, one rotation"
    );
    assert_eq!(
        connector_events(&tenancy)?.len(),
        1,
        "one event under the mutation"
    );
    Ok(())
}

#[tokio::test]
async fn a_first_attempt_that_finds_a_connector_answers_already_issued_without_a_secret()
-> TestResult {
    let tenancy = Tenancy::new()?;
    let existing = Connector {
        connector_id: ConnectorId(Uuid::from_u128(77)),
        label: "Desktop agent".to_owned(),
        workspace_ids: vec![scope()?.workspace_id],
        permissions: vec![Permission::Read],
        created_at: instant()?,
        revoked_at: None,
    };
    tenancy
        .credentials
        .put(mutation(50), existing.clone(), "okfj_kept")?;
    let refused = err_of(
        tenancy
            .service
            .create_connector(
                &context(
                    OperationName::CreateConnector,
                    Some(mutation(50)),
                    Attempt::First,
                )?,
                issue_request(false)?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::AlreadyIssued);
    assert!(matches!(
        refused.detail.as_deref(),
        Some(ErrorDetail::AlreadyIssued { connector_id }) if *connector_id == existing.connector_id
    ));
    assert_eq!(
        tenancy
            .credentials
            .secret(existing.connector_id)?
            .as_deref(),
        Some("okfj_kept"),
        "nothing rotated"
    );
    assert_eq!(connector_events(&tenancy)?, Vec::new());
    Ok(())
}

#[tokio::test]
async fn an_issue_whose_event_cannot_be_recorded_fails_and_its_resume_records_it_once() -> TestResult
{
    let tenancy = Tenancy::new()?;
    tenancy.log.fail_next();
    let failed = err_of(
        tenancy
            .service
            .create_connector(
                &context(
                    OperationName::CreateConnector,
                    Some(mutation(50)),
                    Attempt::First,
                )?,
                issue_request(false)?,
            )
            .await,
    )?;
    assert_eq!(failed.code, ErrorCode::Unavailable);
    let resumed = tenancy
        .service
        .create_connector(
            &context(
                OperationName::CreateConnector,
                Some(mutation(50)),
                Attempt::Resumed,
            )?,
            issue_request(false)?,
        )
        .await?;
    let events = connector_events(&tenancy)?;
    assert_eq!(events.len(), 1);
    let (_, _, event) = some(events.first(), "the event")?;
    assert_eq!(event.connector_id, Some(resumed.connector.connector_id));
    Ok(())
}

#[tokio::test]
async fn list_connectors_lists_without_secrets_and_revoked_ones_only_when_asked() -> TestResult {
    let tenancy = Tenancy::new()?;
    let caller = |number: u128| {
        context(
            OperationName::CreateConnector,
            Some(mutation(number)),
            Attempt::First,
        )
    };
    let kept = tenancy
        .service
        .create_connector(&caller(51)?, issue_request(false)?)
        .await?;
    let gone = tenancy
        .service
        .create_connector(&caller(52)?, issue_request(false)?)
        .await?;
    tenancy
        .service
        .revoke_connector(
            &context(
                OperationName::RevokeConnector,
                Some(mutation(53)),
                Attempt::First,
            )?,
            revoke_request(gone.connector.connector_id)?,
        )
        .await?;
    let lister = context(OperationName::ListConnectors, None, Attempt::First)?;
    let current = tenancy
        .service
        .list_connectors(
            &lister,
            ListConnectorsRequest {
                include_revoked: false,
            },
        )
        .await?;
    let ids: Vec<ConnectorId> = current
        .connectors
        .iter()
        .map(|connector| connector.connector_id)
        .collect();
    assert_eq!(ids, vec![kept.connector.connector_id]);
    let audit = tenancy
        .service
        .list_connectors(
            &lister,
            ListConnectorsRequest {
                include_revoked: true,
            },
        )
        .await?;
    let ids: Vec<ConnectorId> = audit
        .connectors
        .iter()
        .map(|connector| connector.connector_id)
        .collect();
    assert_eq!(
        ids,
        vec![gone.connector.connector_id, kept.connector.connector_id],
        "newest first"
    );
    let text = serde_json::to_string(&audit)?;
    assert!(!text.contains(&kept.secret), "no secret is listed");
    assert!(!text.contains("secret"));
    Ok(())
}

#[tokio::test]
async fn revoke_connector_revokes_at_once_and_records_connector_revoked_once_per_mutation()
-> TestResult {
    let tenancy = Tenancy::new()?;
    let issued = tenancy
        .service
        .create_connector(
            &context(
                OperationName::CreateConnector,
                Some(mutation(50)),
                Attempt::First,
            )?,
            issue_request(false)?,
        )
        .await?;
    let connector = issued.connector.connector_id;
    let caller = context(
        OperationName::RevokeConnector,
        Some(mutation(53)),
        Attempt::First,
    )?;
    let revoked = tenancy
        .service
        .revoke_connector(&caller, revoke_request(connector)?)
        .await?;
    assert!(revoked.revoked_at.is_some());
    let mut resumed = caller.clone();
    resumed.attempt = Attempt::Resumed;
    let again = tenancy
        .service
        .revoke_connector(&resumed, revoke_request(connector)?)
        .await?;
    assert_eq!(
        again.revoked_at, revoked.revoked_at,
        "repeating changes nothing"
    );
    let revocations: Vec<(EventScope, Option<MutationId>, NewEvent)> = connector_events(&tenancy)?
        .into_iter()
        .filter(|(_, _, event)| event.kind == EventKind::ConnectorRevoked)
        .collect();
    assert_eq!(
        revocations,
        vec![(
            EventScope::Tenant(scope()?.tenant_id),
            Some(mutation(53)),
            NewEvent::connector_revoked(&owner()?, connector),
        )]
    );
    Ok(())
}

#[tokio::test]
async fn revoking_an_unknown_connector_is_not_found_and_records_nothing() -> TestResult {
    let tenancy = Tenancy::new()?;
    let refused: ApiError = err_of(
        tenancy
            .service
            .revoke_connector(
                &context(
                    OperationName::RevokeConnector,
                    Some(mutation(53)),
                    Attempt::First,
                )?,
                revoke_request(ConnectorId(Uuid::from_u128(78)))?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert_eq!(connector_events(&tenancy)?, Vec::new());
    Ok(())
}
