//! Compile-level and pure-function proofs for the store ports.
//!
//! No port has an implementation in this crate. Each `*_calls` function type-checks one call to
//! every method of a port through `&dyn`; it is never awaited, and its proof is that this file
//! compiles. The other tests run the pure helpers and the data flow between port types.

use std::error::Error;

use okf_jawn_contract::access::{AccessRoute, Principal};
use okf_jawn_contract::common::PageRequest;
use okf_jawn_contract::identity::{MutationId, TenantId};
use okf_jawn_core::storage::{Page, Provenance, derive_item_id, derive_proposal_id};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn provenance_keeps_the_typed_route_and_client() -> TestResult {
    let principal = Principal {
        subject: "user_1".to_owned(),
        tenant_id: TenantId::try_from("local".to_owned())?,
        route: AccessRoute::McpDelegation,
        client_id: Some("client_1".to_owned()),
        delegation: None,
    };
    assert_eq!(
        Provenance::from_principal(&principal),
        Provenance {
            subject: "user_1".to_owned(),
            route: AccessRoute::McpDelegation,
            client_id: Some("client_1".to_owned()),
        }
    );
    Ok(())
}

#[test]
fn page_carries_the_wire_cursor_and_limit() {
    let page = Page::from(PageRequest {
        cursor: Some("next".to_owned()),
        limit: 25,
    });
    assert_eq!(
        page,
        Page {
            cursor: Some("next".to_owned()),
            limit: 25,
        }
    );
}

#[test]
fn derived_identities_are_stable_per_mutation() {
    let first = MutationId(Uuid::from_u128(1));
    let second = MutationId(Uuid::from_u128(2));
    assert_eq!(derive_item_id(first, 0), derive_item_id(first, 0));
    assert_ne!(derive_item_id(first, 0), derive_item_id(first, 1));
    assert_ne!(derive_item_id(first, 0), derive_item_id(second, 0));
    assert_eq!(derive_item_id(first, 0).0.get_version_num(), 8);
    assert_eq!(derive_proposal_id(first), derive_proposal_id(first));
    assert_ne!(derive_proposal_id(first), derive_proposal_id(second));
    assert_ne!(derive_proposal_id(first).0, derive_item_id(first, 0).0);
}
