//! Connect token verification with a test-only RSA key: signature, issuer, audience, expiry, the
//! algorithm and the required claims. The key in `tests/keys/` exists only for these tests.
#![cfg(feature = "runtime")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_core::ports::PortFuture;
use okf_jawn_server::config::HostedConfig;
use okf_jawn_server::connect::{ConnectTokens, KeySource};
use okf_jawn_server::identity::ConnectVerifier;
use serde_json::{Value, json};

use check::{TestResult, err_of};

/// The test key set; counts how often it was asked for a fresh copy.
struct TestKeys {
    set: JwkSet,
    refreshes: AtomicUsize,
}

const ISSUER: &str = "https://auth.example.test";
const AUDIENCE: &str = "https://okf.example.test/mcp";
const KID: &str = "test-key-1";
const PRIVATE_KEY: &str = include_str!("keys/test-only-rsa.pem");
const KEY_SET: &str = include_str!("keys/test-only-rsa.jwks.json");

impl KeySource for TestKeys {
    fn keys(&self, refresh: bool) -> PortFuture<'_, JwkSet> {
        Box::pin(async move {
            if refresh {
                self.refreshes.fetch_add(1, Ordering::SeqCst);
            }
            Ok(self.set.clone())
        })
    }
}

fn config() -> HostedConfig {
    HostedConfig {
        client_id: "client_01".to_owned(),
        api_key: "not-a-real-api-key".to_owned(),
        issuer: ISSUER.to_owned(),
        resource: AUDIENCE.to_owned(),
        redirect_uri: "https://okf.example.test/auth/callback".to_owned(),
    }
}

fn verifier() -> Result<(ConnectTokens, Arc<TestKeys>), Box<dyn std::error::Error>> {
    let keys = Arc::new(TestKeys {
        set: serde_json::from_str(KEY_SET)?,
        refreshes: AtomicUsize::new(0),
    });
    Ok((ConnectTokens::new(keys.clone(), &config())?, keys))
}

fn now() -> Result<u64, std::time::SystemTimeError> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn claims() -> Result<Value, std::time::SystemTimeError> {
    Ok(json!({
        "iss": ISSUER,
        "aud": AUDIENCE,
        "sub": "user_01",
        "org_id": "org_01",
        "client_id": "client_mcp",
        "exp": now()?.saturating_add(300),
        "iat": now()?,
    }))
}

fn sign(claims: &Value, kid: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = kid.map(str::to_owned);
    Ok(encode(
        &header,
        claims,
        &EncodingKey::from_rsa_pem(PRIVATE_KEY.as_bytes())?,
    )?)
}

fn with(field: &str, value: Value) -> Result<Value, Box<dyn std::error::Error>> {
    let mut claims = claims()?;
    let object = claims.as_object_mut().ok_or("claims are an object")?;
    if value.is_null() {
        object.remove(field);
    } else {
        object.insert(field.to_owned(), value);
    }
    Ok(claims)
}

#[tokio::test]
async fn a_valid_token_names_subject_organization_and_client_apart() -> TestResult {
    let (verifier, _) = verifier()?;
    let identity = verifier.verify(&sign(&claims()?, Some(KID))?).await?;
    assert_eq!(identity.subject, "user_01");
    assert_eq!(identity.organization_id, "org_01");
    assert_eq!(identity.client_id, "client_mcp");
    Ok(())
}

#[tokio::test]
async fn a_wrong_issuer_audience_or_an_expired_token_is_refused() -> TestResult {
    let (verifier, _) = verifier()?;
    for claims in [
        with("iss", json!("https://other-issuer.example.test"))?,
        with("aud", json!("https://other-resource.example.test/mcp"))?,
        with("exp", json!(now()?.saturating_sub(3600)))?,
        with("exp", Value::Null)?,
        with("org_id", Value::Null)?,
        with("client_id", Value::Null)?,
        with("sub", Value::Null)?,
    ] {
        let refused = err_of(verifier.verify(&sign(&claims, Some(KID))?).await)?;
        assert_eq!(refused.code, ErrorCode::Unauthenticated);
    }
    Ok(())
}

#[tokio::test]
async fn a_tampered_token_or_another_algorithm_is_refused() -> TestResult {
    let (verifier, _) = verifier()?;
    let token = sign(&claims()?, Some(KID))?;
    let mut parts: Vec<String> = token.split('.').map(str::to_owned).collect();
    let forged = sign(&with("sub", json!("someone_else"))?, Some(KID))?;
    let forged_claims = forged
        .split('.')
        .nth(1)
        .ok_or("a claims segment")?
        .to_owned();
    if let Some(segment) = parts.get_mut(1) {
        *segment = forged_claims;
    }
    let tampered = parts.join(".");
    assert_eq!(
        err_of(verifier.verify(&tampered).await)?.code,
        ErrorCode::Unauthenticated
    );

    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some(KID.to_owned());
    let hmac = encode(
        &header,
        &claims()?,
        &EncodingKey::from_secret(KEY_SET.as_bytes()),
    )?;
    assert_eq!(
        err_of(verifier.verify(&hmac).await)?.code,
        ErrorCode::Unauthenticated
    );
    Ok(())
}

#[tokio::test]
async fn an_unknown_key_id_refreshes_the_key_set_once_and_is_refused() -> TestResult {
    let (verifier, keys) = verifier()?;
    let refused = err_of(
        verifier
            .verify(&sign(&claims()?, Some("rotated-away"))?)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Unauthenticated);
    assert_eq!(keys.refreshes.load(Ordering::SeqCst), 1);
    let refused = err_of(verifier.verify(&sign(&claims()?, None)?).await)?;
    assert_eq!(refused.code, ErrorCode::Unauthenticated);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
