//! Building the hosted Connect verifier selects jsonwebtoken's `rust_crypto` provider for the
//! process. One test in its own binary, so nothing else in the process installs a provider or
//! resolves the default first.
#![cfg(feature = "runtime")]

use std::sync::Arc;

use jsonwebtoken::crypto::rust_crypto::DEFAULT_PROVIDER;
use jsonwebtoken::jwk::JwkSet;
use okf_jawn_core::ports::PortFuture;
use okf_jawn_server::config::HostedConfig;
use okf_jawn_server::connect::{ConnectTokens, KeySource, install_crypto};
use okf_jawn_server::grants::SystemClock;

use check::TestResult;

struct NoKeys;

impl KeySource for NoKeys {
    fn fetch(&self) -> PortFuture<'_, JwkSet> {
        Box::pin(async { Ok(JwkSet::default()) })
    }
}

#[test]
fn building_the_verifier_installs_the_rust_crypto_provider() -> TestResult {
    let config = HostedConfig {
        client_id: "client_01".to_owned(),
        api_key: "not-a-real-api-key".to_owned(),
        issuer: "https://auth.example.test".to_owned(),
        resource: "https://okf.example.test/mcp".to_owned(),
        redirect_uri: "https://okf.example.test/auth/callback".to_owned(),
    };
    ConnectTokens::new(Arc::new(NoKeys), &config, Arc::new(SystemClock))?;
    // A provider is now installed for the process, so installing one again is refused.
    assert!(DEFAULT_PROVIDER.install_default().is_err());
    // And the service's own call stays harmless.
    install_crypto()?;
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
