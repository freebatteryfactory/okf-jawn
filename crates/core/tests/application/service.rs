//! The service's construction checks and the answer of an operation whose part has not landed.

use std::time::Duration;

use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::health::HealthRequest;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_core::application::ApplicationConfig;
use okf_jawn_core::ports::Application;

use crate::check::{TestResult, err_of};
use crate::fixture::{World, alice, config};

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

#[tokio::test]
async fn an_operation_of_a_later_part_answers_not_implemented() -> TestResult {
    let world = World::new()?;
    let context = alice(OperationName::GetHealth, None)?;
    let refused = err_of(world.service.get_health(&context, HealthRequest {}).await)?;
    assert_eq!(refused.code, ErrorCode::NotImplemented);
    assert!(
        refused.message.contains("get_health"),
        "{}",
        refused.message
    );
    Ok(())
}
