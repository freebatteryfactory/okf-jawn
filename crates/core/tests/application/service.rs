//! The service's construction checks and the answer of an operation whose part has not landed.

use std::time::Duration;

use okf_jawn_contract::conventions::GetRulesRequest;
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::identity::At;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_core::application::ApplicationConfig;
use okf_jawn_core::ports::Application;
use serde_json::json;

use crate::check::{TestResult, err_of};
use crate::fixture::{World, alice, config, scope};

#[test]
fn the_configuration_needs_a_bare_http_origin_and_a_lifetime() -> TestResult {
    for origin in [
        "https://sandbox.example.test",
        "http://127.0.0.1:8788",
        "https://[::1]:9000",
    ] {
        ApplicationConfig {
            sandbox_origin: origin.to_owned(),
            ..config()
        }
        .check()?;
    }
    for origin in [
        "sandbox.example.test",
        "ftp://sandbox.example.test",
        "https://",
        "https://sandbox.example.test/",
        "https://sandbox.example.test/path",
        "https://sandbox.example.test?query",
        "https://user@sandbox.example.test",
        "https://sandbox.example.test:http",
    ] {
        let refused = err_of(
            ApplicationConfig {
                sandbox_origin: origin.to_owned(),
                ..config()
            }
            .check(),
        )?;
        assert_eq!(refused.code, ErrorCode::InvalidInput, "{origin}");
        assert_eq!(refused.field.as_deref(), Some("sandbox_origin"), "{origin}");
    }
    let refused = err_of(
        ApplicationConfig {
            sandbox_ttl: Duration::ZERO,
            ..config()
        }
        .check(),
    )?;
    assert_eq!(refused.field.as_deref(), Some("sandbox_ttl"));
    Ok(())
}

#[test]
fn the_sandbox_origin_port_lies_in_one_to_65535() -> TestResult {
    let check = |origin: &str| {
        ApplicationConfig {
            sandbox_origin: origin.to_owned(),
            ..config()
        }
        .check()
    };
    for origin in [
        "https://sandbox.example.test:1",
        "https://sandbox.example.test:65535",
        "http://127.0.0.1:1",
        "https://[::1]:65535",
    ] {
        check(origin)?;
    }
    for origin in [
        "https://sandbox.example.test:0",
        "https://sandbox.example.test:65536",
        "https://sandbox.example.test:99999",
        "https://sandbox.example.test:",
        "https://[::1]:0",
        "https://[::1]:65536",
        "https://[::1]x",
        "https://[::1",
        "https://sandbox.example.test#fragment",
        "https://sandbox example.test",
        "https://a:1:2",
    ] {
        let refused = err_of(check(origin))?;
        assert_eq!(refused.code, ErrorCode::InvalidInput, "{origin}");
        assert_eq!(refused.field.as_deref(), Some("sandbox_origin"), "{origin}");
    }
    Ok(())
}

#[test]
fn an_ipv6_sandbox_host_must_be_an_ipv6_address() -> TestResult {
    let check = |origin: &str| {
        ApplicationConfig {
            sandbox_origin: origin.to_owned(),
            ..config()
        }
        .check()
    };
    check("https://[::1]:8443")?;
    check("http://[2001:db8::7]")?;
    for origin in [
        "https://[:::]",
        "http://[:]",
        "https://[1:2:3]",
        "https://[::g]",
    ] {
        let refused = err_of(check(origin))?;
        assert_eq!(refused.code, ErrorCode::InvalidInput, "{origin}");
        assert_eq!(refused.field.as_deref(), Some("sandbox_origin"), "{origin}");
    }
    Ok(())
}

#[tokio::test]
async fn an_operation_of_a_later_part_answers_not_implemented() -> TestResult {
    let world = World::new()?;
    let context = alice(OperationName::GetRules, None)?;
    let refused = err_of(
        world
            .service
            .get_rules(
                &context,
                GetRulesRequest {
                    workspace_id: scope()?.workspace_id,
                    at: At::Latest,
                },
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotImplemented);
    assert!(refused.message.contains("get_rules"), "{}", refused.message);
    Ok(())
}

/// The rows contract batch 2 added and no part has built yet answer `NotImplemented`, naming
/// themselves, through the production service: core's `route!` sends an operation it does not
/// name to `pending`. (`list_comments` is built with the proposals, and `get_upload` and
/// `list_uploads` with the upload slots.)
#[tokio::test]
async fn the_rows_of_contract_batch_2_answer_not_implemented() -> TestResult {
    let world = World::new()?;
    let workspace = scope()?.workspace_id;
    let upload = json!("77777777-7777-4777-8777-777777777777");
    let key = json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    let base = json!("a".repeat(40));
    let refused = [
        err_of(
            world
                .service
                .discard_upload(
                    &alice(OperationName::DiscardUpload, None)?,
                    serde_json::from_value(json!({
                        "workspace_id": workspace, "upload_id": upload, "idempotency_key": key
                    }))?,
                )
                .await,
        )?,
        err_of(
            world
                .service
                .move_folder(
                    &alice(OperationName::MoveFolder, None)?,
                    serde_json::from_value(json!({
                        "workspace_id": workspace, "base_revision": base, "folder": "a",
                        "destination": "b", "idempotency_key": key
                    }))?,
                )
                .await,
        )?,
        err_of(
            world
                .service
                .delete_folder(
                    &alice(OperationName::DeleteFolder, None)?,
                    serde_json::from_value(json!({
                        "workspace_id": workspace, "base_revision": base, "folder": "a",
                        "idempotency_key": key
                    }))?,
                )
                .await,
        )?,
    ];
    let names = ["discard_upload", "move_folder", "delete_folder"];
    for (error, name) in refused.iter().zip(names) {
        assert_eq!(error.code, ErrorCode::NotImplemented, "{name}");
        assert!(error.message.contains(name), "{name}: {}", error.message);
    }
    Ok(())
}
