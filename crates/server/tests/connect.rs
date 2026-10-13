//! Connect token verification with test-only RSA keys: signature, issuer, audience, expiry,
//! not-before, the algorithm, the required claims, and the key set's timing against a clock the
//! tests move. The keys in `tests/keys/` exist only for these tests.
#![cfg(feature = "runtime")]

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_server::config::HostedConfig;
use okf_jawn_server::connect::{ConnectTokens, KeySource};
use okf_jawn_server::grants::Clock;
use okf_jawn_server::identity::ConnectVerifier;
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::error::Elapsed;
use tokio::time::timeout;

use check::{TestResult, err_of};

/// The issuer's key endpoint: it serves the set a test last published, or fails, and counts
/// every fetch that starts. A test can hold the next fetch before it answers, for a while or
/// for ever.
struct Issuer {
    published: Mutex<Option<JwkSet>>,
    fetches: AtomicUsize,
    /// The next fetch waits here, after it was counted, until it is let go.
    gate: Mutex<Option<Arc<Notify>>>,
    /// Signalled when a fetch is waiting at the gate.
    waiting: Notify,
    /// How many fetches have given their answer.
    answers: AtomicUsize,
    /// Signalled when a fetch gives its answer.
    answered: Notify,
}

/// A clock that moves only when a test advances it.
struct TestClock {
    start: Instant,
    elapsed_ms: AtomicU64,
}

/// One verifier, its issuer and its clock.
struct Setup {
    verifier: Arc<ConnectTokens>,
    issuer: Arc<Issuer>,
    clock: Arc<TestClock>,
}

const ISSUER: &str = "https://auth.example.test";
const AUDIENCE: &str = "https://okf.example.test/mcp";
const KID: &str = "test-key-1";
const SECOND_KID: &str = "test-key-2";
const PRIVATE_KEY: &str = include_str!("keys/test-only-rsa.pem");
const KEY_SET: &str = include_str!("keys/test-only-rsa.jwks.json");
const SECOND_PRIVATE_KEY: &str = include_str!("keys/test-only-rsa-2.pem");
const SECOND_KEY_SET: &str = include_str!("keys/test-only-rsa-2.jwks.json");

/// What a request that must not wait is given: far longer than an answer from the cache takes,
/// and it decides only a failure.
const AT_ONCE: Duration = Duration::from_secs(2);

impl KeySource for Issuer {
    fn fetch(&self) -> PortFuture<'_, JwkSet> {
        Box::pin(async move {
            self.fetches.fetch_add(1, Ordering::SeqCst);
            let gate = self.gate.lock().map_err(|_| poisoned())?.take();
            if let Some(gate) = gate {
                self.waiting.notify_one();
                gate.notified().await;
            }
            let published = self.published.lock().map_err(|_| poisoned())?.clone();
            self.answers.fetch_add(1, Ordering::SeqCst);
            self.answered.notify_one();
            published.ok_or_else(|| ApiError::new(ErrorCode::Unavailable, "the issuer is down"))
        })
    }
}

impl Issuer {
    fn publish(&self, set: Option<&str>) -> TestResult {
        let set = set.map(serde_json::from_str::<JwkSet>).transpose()?;
        *self.published.lock().map_err(|_| "poisoned")? = set;
        Ok(())
    }

    fn fetches(&self) -> usize {
        self.fetches.load(Ordering::SeqCst)
    }

    /// Make the next fetch wait before it answers; the returned handle lets it go. A test that
    /// never uses the handle has an issuer that never answers.
    fn hold_next(&self) -> Result<Arc<Notify>, ApiError> {
        let gate = Arc::new(Notify::new());
        *self.gate.lock().map_err(|_| poisoned())? = Some(gate.clone());
        Ok(gate)
    }

    /// Wait until a fetch is held at the gate. When none starts within ten seconds this is an
    /// error, so a test whose fetch never begins fails instead of waiting for ever.
    async fn held(&self) -> Result<(), Elapsed> {
        timeout(Duration::from_secs(10), self.waiting.notified()).await
    }

    /// Wait until `count` fetches have given their answer.
    async fn answered(&self, count: usize) {
        while self.answers.load(Ordering::SeqCst) < count {
            self.answered.notified().await;
        }
    }
}

impl Clock for TestClock {
    fn now(&self) -> Instant {
        self.start
            .checked_add(Duration::from_millis(
                self.elapsed_ms.load(Ordering::SeqCst),
            ))
            .unwrap_or(self.start)
    }
}

impl TestClock {
    fn advance(&self, by: Duration) -> Result<(), std::num::TryFromIntError> {
        self.elapsed_ms
            .fetch_add(u64::try_from(by.as_millis())?, Ordering::SeqCst);
        Ok(())
    }
}

impl Setup {
    /// A verifier whose issuer publishes `set` (`None`: the issuer is down).
    fn new(set: Option<&str>) -> Result<Self, Box<dyn std::error::Error>> {
        let issuer = Arc::new(Issuer {
            published: Mutex::new(None),
            fetches: AtomicUsize::new(0),
            gate: Mutex::new(None),
            waiting: Notify::new(),
            answers: AtomicUsize::new(0),
            answered: Notify::new(),
        });
        issuer.publish(set)?;
        let clock = Arc::new(TestClock {
            start: Instant::now(),
            elapsed_ms: AtomicU64::new(0),
        });
        let config = HostedConfig {
            client_id: "client_01".to_owned(),
            api_key: "not-a-real-api-key".to_owned(),
            issuer: ISSUER.to_owned(),
            resource: AUDIENCE.to_owned(),
            redirect_uri: "https://okf.example.test/auth/callback".to_owned(),
        };
        Ok(Self {
            verifier: Arc::new(ConnectTokens::new(issuer.clone(), &config, clock.clone())?),
            issuer,
            clock,
        })
    }

    async fn code_of(&self, token: &str) -> Result<ErrorCode, String> {
        Ok(err_of(self.verifier.verify(token).await)?.error.code)
    }

    /// The refusal of `token`: its code, and the seconds after which it may be tried again.
    async fn refusal_of(&self, token: &str) -> Result<(ErrorCode, Option<u32>), String> {
        let refusal = err_of(self.verifier.verify(token).await)?;
        Ok((refusal.error.code, refusal.retry_after))
    }

    /// Verify `token` in a task of its own; the task answers the refusal's code, if any.
    fn verifying(&self, token: String) -> JoinHandle<Option<ErrorCode>> {
        let verifier = self.verifier.clone();
        tokio::spawn(async move {
            verifier
                .verify(&token)
                .await
                .err()
                .map(|refusal| refusal.error.code)
        })
    }
}

fn poisoned() -> ApiError {
    ApiError::new(ErrorCode::Internal, "poisoned")
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

fn sign_with(
    claims: &Value,
    kid: Option<&str>,
    private_key: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = kid.map(str::to_owned);
    Ok(encode(
        &header,
        claims,
        &EncodingKey::from_rsa_pem(private_key.as_bytes())?,
    )?)
}

fn sign(claims: &Value, kid: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
    sign_with(claims, kid, PRIVATE_KEY)
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
    let setup = Setup::new(Some(KEY_SET))?;
    let identity = setup.verifier.verify(&sign(&claims()?, Some(KID))?).await?;
    assert_eq!(identity.subject, "user_01");
    assert_eq!(identity.organization_id, "org_01");
    assert_eq!(identity.client_id, "client_mcp");
    Ok(())
}

#[tokio::test]
async fn a_verified_token_carries_its_own_expiry() -> TestResult {
    let setup = Setup::new(Some(KEY_SET))?;
    // Two tokens, two expiries: what an event stream later compares with the clock is the
    // token's signed `exp`, not a constant.
    for ahead in [300_u64, 7200] {
        let exp = now()?.saturating_add(ahead);
        let token = sign(&with("exp", json!(exp))?, Some(KID))?;
        let identity = setup.verifier.verify(&token).await?;
        assert_eq!(identity.expires_at, exp);
    }
    Ok(())
}

#[tokio::test]
async fn a_wrong_issuer_or_audience_or_a_missing_claim_is_refused() -> TestResult {
    let setup = Setup::new(Some(KEY_SET))?;
    for claims in [
        with("iss", json!("https://other-issuer.example.test"))?,
        with("aud", json!("https://other-resource.example.test/mcp"))?,
        with("exp", json!(now()?.saturating_sub(3600)))?,
        with("exp", Value::Null)?,
        with("org_id", Value::Null)?,
        with("client_id", Value::Null)?,
        with("sub", Value::Null)?,
    ] {
        let code = setup.code_of(&sign(&claims, Some(KID))?).await?;
        assert_eq!(code, ErrorCode::Unauthenticated);
    }
    Ok(())
}

#[tokio::test]
async fn an_empty_subject_organization_or_client_is_refused() -> TestResult {
    let setup = Setup::new(Some(KEY_SET))?;
    for field in ["sub", "org_id", "client_id"] {
        let code = setup
            .code_of(&sign(&with(field, json!(""))?, Some(KID))?)
            .await?;
        assert_eq!(code, ErrorCode::Unauthenticated, "empty {field}");
    }
    Ok(())
}

#[tokio::test]
async fn expiry_has_thirty_seconds_of_leeway_and_no_more() -> TestResult {
    let setup = Setup::new(Some(KEY_SET))?;
    let just_past = with("exp", json!(now()?.saturating_sub(20)))?;
    setup.verifier.verify(&sign(&just_past, Some(KID))?).await?;
    let past = with("exp", json!(now()?.saturating_sub(45)))?;
    let code = setup.code_of(&sign(&past, Some(KID))?).await?;
    assert_eq!(code, ErrorCode::Unauthenticated);
    Ok(())
}

#[tokio::test]
async fn a_token_that_is_not_yet_valid_is_refused() -> TestResult {
    let setup = Setup::new(Some(KEY_SET))?;
    let soon = with("nbf", json!(now()?.saturating_add(20)))?;
    setup.verifier.verify(&sign(&soon, Some(KID))?).await?;
    for ahead in [45, 3600] {
        let later = with("nbf", json!(now()?.saturating_add(ahead)))?;
        let code = setup.code_of(&sign(&later, Some(KID))?).await?;
        assert_eq!(code, ErrorCode::Unauthenticated, "nbf {ahead} s ahead");
    }
    Ok(())
}

#[tokio::test]
async fn a_tampered_token_or_another_algorithm_is_refused() -> TestResult {
    let setup = Setup::new(Some(KEY_SET))?;
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
    let code = setup.code_of(&parts.join(".")).await?;
    assert_eq!(code, ErrorCode::Unauthenticated);

    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some(KID.to_owned());
    let hmac = encode(
        &header,
        &claims()?,
        &EncodingKey::from_secret(KEY_SET.as_bytes()),
    )?;
    assert_eq!(setup.code_of(&hmac).await?, ErrorCode::Unauthenticated);
    // A token signed by a key the issuer never published, under a published key's id.
    let impostor = sign_with(&claims()?, Some(KID), SECOND_PRIVATE_KEY)?;
    assert_eq!(setup.code_of(&impostor).await?, ErrorCode::Unauthenticated);
    Ok(())
}

#[tokio::test]
async fn a_token_of_another_algorithm_with_an_unknown_key_causes_no_fetch() -> TestResult {
    let setup = Setup::new(Some(KEY_SET))?;
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("never-published".to_owned());
    let hmac = encode(
        &header,
        &claims()?,
        &EncodingKey::from_secret(b"any secret"),
    )?;
    assert_eq!(setup.code_of(&hmac).await?, ErrorCode::Unauthenticated);
    assert_eq!(setup.issuer.fetches(), 0);
    Ok(())
}

#[tokio::test]
async fn unknown_key_ids_cause_at_most_one_fetch_a_minute() -> TestResult {
    let setup = Setup::new(Some(KEY_SET))?;
    setup.verifier.verify(&sign(&claims()?, Some(KID))?).await?;
    assert_eq!(setup.issuer.fetches(), 1);
    // Twenty-five forged tokens, each naming a key nobody published: answered from the cache.
    for forged in 0..25 {
        let token = sign(&claims()?, Some(&format!("forged-{forged}")))?;
        assert_eq!(setup.code_of(&token).await?, ErrorCode::Unavailable);
    }
    assert_eq!(setup.issuer.fetches(), 1);
    // A token with no key id at all asks the issuer nothing.
    assert_eq!(
        setup.code_of(&sign(&claims()?, None)?).await?,
        ErrorCode::Unauthenticated
    );
    setup.clock.advance(Duration::from_secs(59))?;
    setup
        .code_of(&sign(&claims()?, Some("forged-late"))?)
        .await?;
    assert_eq!(setup.issuer.fetches(), 1);
    // At sixty seconds the issuer may be asked again, once.
    setup.clock.advance(Duration::from_secs(1))?;
    let asked = setup.code_of(&sign(&claims()?, Some("forged-a"))?).await?;
    let after = setup.code_of(&sign(&claims()?, Some("forged-b"))?).await?;
    assert_eq!(setup.issuer.fetches(), 2);
    // The one whose own fetch did not bring its key is false; the next cannot be judged yet.
    assert_eq!(asked, ErrorCode::Unauthenticated);
    assert_eq!(after, ErrorCode::Unavailable);
    Ok(())
}

#[tokio::test]
async fn a_rotated_key_is_accepted_after_one_refresh() -> TestResult {
    let setup = Setup::new(Some(KEY_SET))?;
    setup.verifier.verify(&sign(&claims()?, Some(KID))?).await?;
    setup.issuer.publish(Some(SECOND_KEY_SET))?;
    let rotated = sign_with(&claims()?, Some(SECOND_KID), SECOND_PRIVATE_KEY)?;
    // Inside the interval the new key is unknown and the issuer is not asked.
    assert_eq!(setup.code_of(&rotated).await?, ErrorCode::Unavailable);
    assert_eq!(setup.issuer.fetches(), 1);
    setup.clock.advance(Duration::from_secs(60))?;
    let identity = setup.verifier.verify(&rotated).await?;
    assert_eq!(identity.subject, "user_01");
    assert_eq!(setup.issuer.fetches(), 2);
    // The withdrawn key is no longer trusted.
    let old = sign(&claims()?, Some(KID))?;
    // It cannot be called false inside the minute; the next fetch settles it.
    assert_eq!(setup.code_of(&old).await?, ErrorCode::Unavailable);
    setup.clock.advance(Duration::from_secs(60))?;
    assert_eq!(setup.code_of(&old).await?, ErrorCode::Unauthenticated);
    assert_eq!(setup.issuer.fetches(), 3);
    Ok(())
}

#[tokio::test]
async fn a_cached_key_set_is_refreshed_once_it_is_ten_minutes_old() -> TestResult {
    let setup = Setup::new(Some(KEY_SET))?;
    let token = sign(&with("exp", json!(now()?.saturating_add(7200)))?, Some(KID))?;
    setup.verifier.verify(&token).await?;
    setup.issuer.publish(Some(SECOND_KEY_SET))?;
    setup.clock.advance(Duration::from_millis(599_999))?;
    // Not yet ten minutes: the known key answers from the cache.
    setup.verifier.verify(&token).await?;
    assert_eq!(setup.issuer.fetches(), 1);
    setup.clock.advance(Duration::from_millis(1))?;
    // Ten minutes: the set is fetched again, and the key the issuer withdrew is refused.
    assert_eq!(setup.code_of(&token).await?, ErrorCode::Unauthenticated);
    assert_eq!(setup.issuer.fetches(), 2);
    Ok(())
}

#[tokio::test]
async fn a_failed_refresh_keeps_the_cached_set_for_at_most_an_hour() -> TestResult {
    let setup = Setup::new(Some(KEY_SET))?;
    let token = sign(&with("exp", json!(now()?.saturating_add(7200)))?, Some(KID))?;
    setup.verifier.verify(&token).await?;
    setup.issuer.publish(None)?;
    setup.clock.advance(Duration::from_secs(600))?;
    // The refresh fails; the cached set stays in use.
    setup.verifier.verify(&token).await?;
    assert_eq!(setup.issuer.fetches(), 2);
    // Still inside the minute: no further attempt.
    setup.verifier.verify(&token).await?;
    assert_eq!(setup.issuer.fetches(), 2);
    setup.clock.advance(Duration::from_millis(2_999_999))?;
    // One millisecond short of an hour after the last successful fetch.
    setup.verifier.verify(&token).await?;
    setup.clock.advance(Duration::from_millis(1))?;
    assert_eq!(setup.code_of(&token).await?, ErrorCode::Unavailable);
    // The issuer comes back: the next permitted refresh restores verification.
    setup.issuer.publish(Some(KEY_SET))?;
    setup.clock.advance(Duration::from_secs(60))?;
    setup.verifier.verify(&token).await?;
    Ok(())
}

#[tokio::test]
async fn with_no_key_set_and_the_issuer_down_the_answer_is_unavailable() -> TestResult {
    let setup = Setup::new(None)?;
    let token = sign(&claims()?, Some(KID))?;
    assert_eq!(setup.code_of(&token).await?, ErrorCode::Unavailable);
    assert_eq!(setup.code_of(&token).await?, ErrorCode::Unavailable);
    // One attempt for both: a down issuer is not asked on every request either.
    assert_eq!(setup.issuer.fetches(), 1);
    Ok(())
}

#[tokio::test]
async fn a_token_that_cannot_be_judged_yet_is_unavailable_with_the_seconds_left() -> TestResult {
    let setup = Setup::new(Some(KEY_SET))?;
    setup.verifier.verify(&sign(&claims()?, Some(KID))?).await?;
    let unknown = sign(&claims()?, Some("not-in-the-set"))?;
    let not_yet = |seconds: u32| (ErrorCode::Unavailable, Some(seconds));
    // The issuer was asked this instant: a whole minute is left, then less, rounded up.
    assert_eq!(setup.refusal_of(&unknown).await?, not_yet(60));
    setup.clock.advance(Duration::from_millis(30_500))?;
    assert_eq!(setup.refusal_of(&unknown).await?, not_yet(30));
    setup.clock.advance(Duration::from_millis(29_499))?;
    assert_eq!(setup.refusal_of(&unknown).await?, not_yet(1));
    assert_eq!(setup.issuer.fetches(), 1);
    // A token that fails for another reason is false, inside the minute as outside it.
    let impostor = sign_with(&claims()?, Some(KID), SECOND_PRIVATE_KEY)?;
    let expired = sign(&with("exp", json!(now()?.saturating_sub(3600)))?, Some(KID))?;
    let keyless = sign(&claims()?, None)?;
    for token in [&impostor, &expired, &keyless] {
        assert_eq!(
            setup.refusal_of(token).await?,
            (ErrorCode::Unauthenticated, None)
        );
    }
    // At sixty seconds the issuer is asked, and does not bring the key: now the token is false.
    setup.clock.advance(Duration::from_millis(1))?;
    assert_eq!(
        setup.refusal_of(&unknown).await?,
        (ErrorCode::Unauthenticated, None)
    );
    assert_eq!(setup.issuer.fetches(), 2);
    // The next one, a moment later, cannot be judged again for a minute.
    assert_eq!(setup.refusal_of(&unknown).await?, not_yet(60));

    // A fetch that fails judges nothing: the request that made it is told to come back too.
    setup.issuer.publish(None)?;
    setup.clock.advance(Duration::from_secs(60))?;
    assert_eq!(setup.refusal_of(&unknown).await?, not_yet(60));
    assert_eq!(setup.issuer.fetches(), 3);
    // With no key set at all, a token whose key would be known is told the same.
    let down = Setup::new(None)?;
    let valid = sign(&claims()?, Some(KID))?;
    assert_eq!(down.refusal_of(&valid).await?, not_yet(60));
    down.clock.advance(Duration::from_secs(45))?;
    assert_eq!(down.refusal_of(&valid).await?, not_yet(15));
    Ok(())
}

#[tokio::test]
async fn a_hanging_fetch_holds_up_no_token_whose_key_is_cached() -> TestResult {
    let setup = Setup::new(Some(KEY_SET))?;
    let valid = sign(&claims()?, Some(KID))?;
    setup.verifier.verify(&valid).await?;
    setup.clock.advance(Duration::from_secs(61))?;
    // A token naming an unknown key takes the minute's attempt, and the issuer never answers.
    let _never = setup.issuer.hold_next()?;
    let stuck = setup.verifying(sign(&claims()?, Some("unknown-key"))?);
    setup.issuer.held().await?;
    assert_eq!(setup.issuer.fetches(), 2);
    // The valid token's key is in the cached set, 61 seconds old: it is verified at once.
    let identity = timeout(AT_ONCE, setup.verifier.verify(&valid)).await??;
    assert_eq!(identity.subject, "user_01");
    assert_eq!(setup.issuer.fetches(), 2);
    assert!(
        !stuck.is_finished(),
        "the request that asked the issuer waits for it"
    );
    stuck.abort();
    Ok(())
}

#[tokio::test]
async fn two_unknown_key_ids_at_the_same_moment_cause_one_fetch() -> TestResult {
    let setup = Setup::new(Some(KEY_SET))?;
    setup.verifier.verify(&sign(&claims()?, Some(KID))?).await?;
    setup.clock.advance(Duration::from_secs(60))?;
    let gate = setup.issuer.hold_next()?;
    let first = setup.verifying(sign(&claims()?, Some("unknown-a"))?);
    setup.issuer.held().await?;
    // The second arrives while the first's fetch is under way: answered from the cached set,
    // without waiting and without a fetch of its own.
    let second = sign(&claims()?, Some("unknown-b"))?;
    let code = timeout(AT_ONCE, setup.code_of(&second)).await??;
    assert_eq!(code, ErrorCode::Unavailable);
    assert_eq!(setup.issuer.fetches(), 2);
    gate.notify_one();
    assert_eq!(first.await?, Some(ErrorCode::Unauthenticated));
    // Nor does a third, inside the minute after that fetch.
    setup.clock.advance(Duration::from_secs(59))?;
    setup.code_of(&sign(&claims()?, Some("unknown-c"))?).await?;
    assert_eq!(setup.issuer.fetches(), 2);
    Ok(())
}

#[tokio::test]
async fn a_fetch_over_ten_seconds_is_a_failed_fetch_and_the_cached_set_stays_in_use() -> TestResult
{
    let setup = Setup::new(Some(KEY_SET))?;
    let token = sign(&with("exp", json!(now()?.saturating_add(7200)))?, Some(KID))?;
    setup.verifier.verify(&token).await?;
    // Ten minutes on, the set is stale: this request takes the attempt, and the issuer hangs.
    setup.clock.advance(Duration::from_secs(600))?;
    let _never = setup.issuer.hold_next()?;
    let began = Instant::now();
    let waited = timeout(Duration::from_secs(40), setup.verifier.verify(&token)).await;
    let took = began.elapsed();
    // The server's own limit ended the fetch, and the set fetched before answers (stale on
    // error).
    waited??;
    assert!(took >= Duration::from_secs(10), "answered after {took:?}");
    assert_eq!(setup.issuer.fetches(), 2);
    // That was the minute's attempt: the next request asks nobody and waits for nothing.
    timeout(AT_ONCE, setup.verifier.verify(&token)).await??;
    assert_eq!(setup.issuer.fetches(), 2);
    Ok(())
}

#[tokio::test]
async fn a_fetch_outlives_the_request_that_began_it() -> TestResult {
    let setup = Setup::new(Some(KEY_SET))?;
    setup.verifier.verify(&sign(&claims()?, Some(KID))?).await?;
    setup.issuer.publish(Some(SECOND_KEY_SET))?;
    setup.clock.advance(Duration::from_secs(60))?;
    let rotated = sign_with(&claims()?, Some(SECOND_KID), SECOND_PRIVATE_KEY)?;
    let gate = setup.issuer.hold_next()?;
    let gone = setup.verifying(rotated.clone());
    setup.issuer.held().await?;
    // The caller that took the minute's attempt goes away while the issuer is still answering.
    gone.abort();
    assert!(gone.await.is_err(), "the request was dropped");
    gate.notify_one();
    timeout(AT_ONCE, setup.issuer.answered(2)).await?;
    // Its fetch was kept: the new key verifies inside the same minute, with no second fetch.
    let identity = timeout(AT_ONCE, setup.verifier.verify(&rotated)).await??;
    assert_eq!(identity.subject, "user_01");
    assert_eq!(setup.issuer.fetches(), 2);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
