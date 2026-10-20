//! Hosted browser sign-in over the real service, with an identity-provider double: only the
//! browser that began a sign-in completes it, nothing reaches the provider without that
//! browser's cookie, the cookie is cleared on every outcome of the callback it is waiting for,
//! and a callback that is not this browser's sign-in leaves the cookie alone.
#![cfg(feature = "runtime")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use okf_jawn_contract::access::{
    AccessRoute, CSRF_COOKIE, CSRF_COOKIE_HTTPS, CSRF_HEADER, SESSION_COOKIE, SESSION_COOKIE_HTTPS,
};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::identity::TenantId;
use okf_jawn_core::credentials::CredentialStore;
use okf_jawn_core::ports::PortFuture;
use okf_jawn_server::cookies::CrateCookies;
use okf_jawn_server::hosted::{HostedLogin, HostedUser, LoginStart};
use okf_jawn_server::identity::{
    BearerAuth, ConnectIdentity, Identity, IdentityPorts, csrf_token_of, session_id_of,
};
use okf_jawn_server::local::SecretSource;
use okf_jawn_server::sessions::{SessionEntry, SessionParts, Sessions, routes};
use tower::ServiceExt;

use check::{TestResult, some};
use support::{HOST, LocalApp, OneToken};

/// An identity provider that begins one fixed sign-in and exchanges its code once, only with
/// the verifier of the sign-in it began. It counts every exchange it is asked for.
#[derive(Default)]
struct Provider {
    exchanges: AtomicUsize,
    used: AtomicBool,
    /// While set, every exchange fails as `Unavailable`: the provider is down.
    down: AtomicBool,
}

/// A secret source with one fixed secret, so the test knows the session it opens.
struct Fixed;

/// The service, its stores and its provider.
struct Hosted {
    app: Router,
    base: LocalApp,
    provider: Arc<Provider>,
}

const STATE: &str = "state-of-the-sign-in-this-browser-began-001";
const VERIFIER: &str = "verifier.of-the_sign-in~this-browser-began-001";
const OTHER_STATE: &str = "state-of-a-sign-in-another-browser-began-02";
const OTHER_VERIFIER: &str = "verifier.of-a_sign-in~another-browser-began-02";
const CODE: &str = "code-1";
const SESSION_SECRET: &str = "hosted-session-secret";
const LOGIN_COOKIE: &str = "__Host-okf-login";
/// The service's own origin, served over https as a hosted deployment is.
const HTTPS_ORIGIN: &str = "https://127.0.0.1:7711";

impl HostedLogin for Provider {
    fn begin(&self) -> Result<LoginStart, ApiError> {
        Ok(LoginStart {
            url: format!("https://auth.example.test/authorize?state={STATE}"),
            verifier: VERIFIER.to_owned(),
            state: STATE.to_owned(),
        })
    }

    fn complete<'a>(&'a self, code: &'a str, verifier: &'a str) -> PortFuture<'a, HostedUser> {
        Box::pin(async move {
            self.exchanges.fetch_add(1, Ordering::SeqCst);
            if self.down.load(Ordering::SeqCst) {
                return Err(ApiError::new(
                    ErrorCode::Unavailable,
                    "the provider is down",
                ));
            }
            // The code is bound to the challenge of one verifier, and is single use.
            if code == CODE && verifier == VERIFIER && !self.used.swap(true, Ordering::SeqCst) {
                Ok(HostedUser {
                    subject: "user_01".to_owned(),
                    organization_id: "org_01".to_owned(),
                })
            } else {
                Err(ApiError::new(ErrorCode::Unauthenticated, "not accepted"))
            }
        })
    }
}

impl SecretSource for Fixed {
    fn secret(&self) -> Result<String, ApiError> {
        Ok(SESSION_SECRET.to_owned())
    }
}

impl Hosted {
    /// A hosted service over https: one identity middleware for that origin, whose cookie
    /// policy the session routes write by (review N3).
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let base = LocalApp::new()?;
        let provider = Arc::new(Provider::default());
        // Hosted: browser sessions of route `BrowserSession`, bearer tokens from WorkOS Connect
        // (none is presented here).
        let identity = Identity::new(
            BearerAuth::Hosted {
                connect: Arc::new(OneToken {
                    token: "no request presents this token".to_owned(),
                    identity: ConnectIdentity {
                        subject: "user_01".to_owned(),
                        organization_id: "org_01".to_owned(),
                        client_id: "client_01".to_owned(),
                        expires_at: u64::MAX,
                    },
                }),
                resource_metadata: format!("{HTTPS_ORIGIN}/.well-known/oauth-protected-resource"),
            },
            IdentityPorts {
                credentials: base.credentials.clone(),
                events: base.ports.events.clone(),
                cookies: Arc::new(CrateCookies),
                refusals: base.refusals.clone(),
            },
            vec![HTTPS_ORIGIN.to_owned()],
        );
        let sessions = Sessions::new(SessionParts {
            credentials: base.credentials.clone(),
            events: base.ports.events.clone(),
            secrets: Arc::new(Fixed),
            refusals: base.refusals.clone(),
            policy: identity.cookie_policy(Duration::from_secs(3600)),
            entry: SessionEntry::Hosted {
                login: provider.clone(),
            },
        });
        let app = base.service_behind(identity, routes(sessions), Vec::new(), None)?;
        Ok(Self {
            app,
            base,
            provider,
        })
    }

    fn exchanges(&self) -> usize {
        self.provider.exchanges.load(Ordering::SeqCst)
    }

    /// The callback as the provider's redirect delivers it, with `cookie` as the browser's
    /// `Cookie` header.
    async fn callback(
        &self,
        state: &str,
        code: &str,
        cookie: Option<&str>,
    ) -> Result<Response, Box<dyn std::error::Error>> {
        let mut builder = Request::get(format!("/auth/callback?state={state}&code={code}"))
            .header("host", HOST)
            .header("sec-fetch-site", "cross-site");
        if let Some(cookie) = cookie {
            builder = builder.header("cookie", cookie);
        }
        Ok(self
            .app
            .clone()
            .oneshot(builder.body(Body::empty())?)
            .await?)
    }

    /// Whether a session was stored for the fixed secret.
    async fn signed_in(&self) -> Result<bool, ApiError> {
        Ok(self
            .base
            .credentials
            .get_session(&session_id_of(SESSION_SECRET))
            .await?
            .is_some())
    }
}

fn get(path: &str) -> Result<Request<Body>, axum::http::Error> {
    Request::get(path).header("host", HOST).body(Body::empty())
}

/// Every `Set-Cookie` of `response` for the cookie `name`.
fn set_cookies(response: &Response, name: &str) -> Vec<String> {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter(|value| value.starts_with(&format!("{name}=")))
        .map(str::to_owned)
        .collect()
}

/// The attributes of a `Set-Cookie` value, sorted, without its `name=value`.
fn attributes(cookie: &str) -> Vec<&str> {
    let mut attributes: Vec<&str> = cookie.split("; ").skip(1).collect();
    attributes.sort_unstable();
    attributes
}

/// The response clears the sign-in cookie, with the attributes it was set with.
fn assert_login_cookie_cleared(response: &Response) -> TestResult {
    let cleared = set_cookies(response, LOGIN_COOKIE);
    assert_eq!(cleared.len(), 1, "one clearing cookie: {cleared:?}");
    let cleared = some(cleared.first(), "the clearing cookie")?;
    assert!(cleared.starts_with("__Host-okf-login=;"), "{cleared}");
    let attributes = attributes(cleared);
    for expected in ["HttpOnly", "Max-Age=0", "Path=/", "SameSite=Lax", "Secure"] {
        assert!(attributes.contains(&expected), "{expected} in {cleared}");
    }
    assert!(
        !attributes
            .iter()
            .any(|attribute| attribute.starts_with("Domain"))
    );
    Ok(())
}

/// A refusal of the callback the browser's sign-in was waiting for (its state matched): no
/// sign-in, and the sign-in cookie cleared.
async fn assert_refused(hosted: &Hosted, response: &Response) -> TestResult {
    assert_no_sign_in(hosted, response).await?;
    assert_login_cookie_cleared(response)
}

/// A refusal of a callback that is not this browser's sign-in: no sign-in, and the sign-in
/// cookie neither cleared nor set, so a sign-in begun in another tab can still finish.
async fn assert_left_alone(hosted: &Hosted, response: &Response) -> TestResult {
    assert_no_sign_in(hosted, response).await?;
    assert_eq!(set_cookies(response, LOGIN_COOKIE), Vec::<String>::new());
    Ok(())
}

/// A refusal of the callback: 401, private, and no session.
async fn assert_no_sign_in(hosted: &Hosted, response: &Response) -> TestResult {
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    assert_eq!(
        response
            .headers()
            .get("referrer-policy")
            .and_then(|v| v.to_str().ok()),
        Some("no-referrer")
    );
    assert_eq!(
        set_cookies(response, SESSION_COOKIE_HTTPS),
        Vec::<String>::new()
    );
    assert!(!hosted.signed_in().await?);
    Ok(())
}

fn pending(state: &str, verifier: &str) -> String {
    format!("{LOGIN_COOKIE}={state}:{verifier}")
}

#[tokio::test]
async fn beginning_a_sign_in_hands_the_browser_its_cookie_with_each_attribute() -> TestResult {
    let hosted = Hosted::new()?;
    let begun = hosted.app.clone().oneshot(get("/auth/login")?).await?;
    assert_eq!(begun.status(), StatusCode::FOUND);
    assert_eq!(
        begun
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok()),
        Some(format!("https://auth.example.test/authorize?state={STATE}").as_str())
    );
    let cookies = set_cookies(&begun, LOGIN_COOKIE);
    assert_eq!(cookies.len(), 1);
    let cookie = some(cookies.first(), "the sign-in cookie")?;
    assert!(cookie.starts_with(&format!("{LOGIN_COOKIE}={STATE}:{VERIFIER}; ")));
    // Exactly these attributes: each one named, and no `Domain`.
    assert_eq!(
        attributes(cookie),
        vec![
            "HttpOnly",
            "Max-Age=600",
            "Path=/",
            "SameSite=Lax",
            "Secure"
        ]
    );
    // Beginning a sign-in opens no session and exchanges nothing.
    assert_eq!(
        set_cookies(&begun, SESSION_COOKIE_HTTPS),
        Vec::<String>::new()
    );
    assert_eq!(hosted.exchanges(), 0);
    Ok(())
}

#[tokio::test]
async fn the_browser_that_began_a_sign_in_completes_it_and_its_cookie_is_cleared() -> TestResult {
    let hosted = Hosted::new()?;
    let response = hosted
        .callback(STATE, CODE, Some(&pending(STATE, VERIFIER)))
        .await?;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_login_cookie_cleared(&response)?;
    // Over https the session cookie is `__Host-okf-session` (ruling 17), and no `okf-session`
    // is set beside it.
    assert_eq!(set_cookies(&response, SESSION_COOKIE), Vec::<String>::new());
    let session = set_cookies(&response, SESSION_COOKIE_HTTPS);
    let session = some(session.first(), "the session cookie")?;
    assert_eq!(
        attributes(session),
        vec![
            "HttpOnly",
            "Max-Age=3600",
            "Path=/",
            "SameSite=Strict",
            "Secure"
        ]
    );
    // Over https the anti-CSRF cookie is `__Host-okf-csrf` (review N13), and no `okf-csrf` is
    // set beside it.
    assert_eq!(set_cookies(&response, CSRF_COOKIE), Vec::<String>::new());
    let csrf = set_cookies(&response, CSRF_COOKIE_HTTPS);
    let csrf = some(csrf.first(), "the anti-CSRF cookie")?;
    assert_eq!(
        attributes(csrf),
        vec!["Max-Age=3600", "Path=/", "SameSite=Strict", "Secure"]
    );
    assert_eq!(hosted.exchanges(), 1);

    let stored = some(
        hosted
            .base
            .credentials
            .get_session(&session_id_of(SESSION_SECRET))
            .await?,
        "the stored session",
    )?;
    assert_eq!(stored.principal.route, AccessRoute::BrowserSession);
    assert_eq!(stored.principal.subject, "user_01");
    assert_eq!(
        stored.principal.tenant_id,
        TenantId::try_from("org_01".to_owned())?
    );
    let signed_in = hosted
        .base
        .ports
        .events
        .appended()?
        .into_iter()
        .filter(|(_, event)| event.kind == EventKind::SignedIn)
        .count();
    assert_eq!(signed_in, 1);
    Ok(())
}

#[tokio::test]
async fn a_hosted_refusal_before_any_tenant_is_known_writes_no_log() -> TestResult {
    let hosted = Hosted::new()?;
    // A callback this browser never began, and a bearer that does not authenticate.
    let response = hosted.callback("not-a-state-of-ours", "code", None).await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let bearer = Request::post("/api/items/list-drafts")
        .header("host", HOST)
        .header("authorization", "Bearer not-the-token")
        .header("content-type", "application/json")
        .body(Body::from("{}"))?;
    assert_eq!(
        hosted.app.clone().oneshot(bearer).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    let refused = hosted
        .base
        .ports
        .events
        .appended()?
        .into_iter()
        .filter(|(_, event)| event.kind == EventKind::SignInRefused)
        .count();
    assert_eq!(refused, 0);
    Ok(())
}

/// The value of the cookie `name` that `response` sets.
fn set_value(response: &Response, name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let cookies = set_cookies(response, name);
    let cookie = some(cookies.first(), "the cookie")?;
    let pair = some(cookie.split("; ").next(), "its name and value")?;
    Ok(some(pair.strip_prefix(&format!("{name}=")), "its value")?.to_owned())
}

/// A same-origin request over https carrying `cookie` and the anti-CSRF header `csrf`.
fn https_post(path: &str, cookie: &str, csrf: &str) -> Result<Request<Body>, axum::http::Error> {
    Request::post(path)
        .header("host", HOST)
        .header("origin", HTTPS_ORIGIN)
        .header("sec-fetch-site", "same-origin")
        .header("content-type", "application/json")
        .header("cookie", cookie)
        .header(CSRF_HEADER, csrf)
        .body(Body::from("{}"))
}

/// Review N3: the name the server writes at a hosted sign-in over https is the name it reads.
/// The browser sends back the `__Host-okf-session` it was given with the `__Host-okf-csrf`
/// token, and the request is the signed-in person's; the same secret under the plain name is
/// not read.
#[tokio::test]
async fn a_session_signed_in_over_https_is_read_back_from_the_cookie_it_set() -> TestResult {
    let hosted = Hosted::new()?;
    let signed_in = hosted
        .callback(STATE, CODE, Some(&pending(STATE, VERIFIER)))
        .await?;
    assert_eq!(signed_in.status(), StatusCode::FOUND);
    let secret = set_value(&signed_in, SESSION_COOKIE_HTTPS)?;
    let token = set_value(&signed_in, CSRF_COOKIE_HTTPS)?;
    assert_eq!(token, csrf_token_of(&secret));
    let cookie = format!("{SESSION_COOKIE_HTTPS}={secret}");
    hosted
        .app
        .clone()
        .oneshot(https_post("/api/session/get-session", &cookie, &token)?)
        .await?;
    let seen = hosted.base.counting.contexts("get_session")?;
    let context = some(seen.first(), "the signed-in request")?;
    assert_eq!(context.principal.subject, "user_01");
    assert_eq!(context.principal.route, AccessRoute::BrowserSession);
    let plain = format!("{SESSION_COOKIE}={secret}");
    let refused = hosted
        .app
        .clone()
        .oneshot(https_post("/api/session/get-session", &plain, &token)?)
        .await?;
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(hosted.base.counting.call_count("get_session")?, 1);
    Ok(())
}

/// Review N10: a logout over https clears `__Host-okf-session` and `__Host-okf-csrf`, with the
/// attributes they were set with and expired, and clears no plain-named cookie.
#[tokio::test]
async fn a_logout_over_https_clears_the_host_cookies_it_set() -> TestResult {
    let hosted = Hosted::new()?;
    let signed_in = hosted
        .callback(STATE, CODE, Some(&pending(STATE, VERIFIER)))
        .await?;
    let secret = set_value(&signed_in, SESSION_COOKIE_HTTPS)?;
    let token = set_value(&signed_in, CSRF_COOKIE_HTTPS)?;
    let cookie = format!("{SESSION_COOKIE_HTTPS}={secret}");
    let response = hosted
        .app
        .clone()
        .oneshot(https_post("/auth/logout", &cookie, &token)?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!hosted.signed_in().await?, "the session is revoked");
    for (name, expected) in [
        (
            SESSION_COOKIE_HTTPS,
            vec![
                "HttpOnly",
                "Max-Age=0",
                "Path=/",
                "SameSite=Strict",
                "Secure",
            ],
        ),
        (
            CSRF_COOKIE_HTTPS,
            vec!["Max-Age=0", "Path=/", "SameSite=Strict", "Secure"],
        ),
    ] {
        let cleared = set_cookies(&response, name);
        assert_eq!(cleared.len(), 1, "{name}: {cleared:?}");
        let cleared = some(cleared.first(), "the clearing cookie")?;
        assert!(cleared.starts_with(&format!("{name}=;")), "{cleared}");
        let mut attributes = attributes(cleared);
        attributes.retain(|attribute| !attribute.starts_with("Expires="));
        assert_eq!(attributes, expected, "{cleared}");
    }
    for plain in [SESSION_COOKIE, CSRF_COOKIE] {
        assert_eq!(
            set_cookies(&response, plain),
            Vec::<String>::new(),
            "{plain}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_callback_from_a_browser_that_began_no_sign_in_reaches_no_provider() -> TestResult {
    let hosted = Hosted::new()?;
    // The attacker began the sign-in; the victim's browser has no sign-in cookie.
    hosted.app.clone().oneshot(get("/auth/login")?).await?;
    let response = hosted.callback(STATE, CODE, None).await?;
    assert_left_alone(&hosted, &response).await?;
    assert_eq!(hosted.exchanges(), 0);
    // A session cookie of the same browser is no sign-in cookie either.
    let response = hosted
        .callback(STATE, CODE, Some("okf-session=anything"))
        .await?;
    assert_left_alone(&hosted, &response).await?;
    assert_eq!(hosted.exchanges(), 0);
    Ok(())
}

#[tokio::test]
async fn a_callback_with_another_sign_ins_cookie_or_another_state_reaches_no_provider() -> TestResult
{
    let hosted = Hosted::new()?;
    // The cookie of another sign-in, with this sign-in's state in the link.
    let response = hosted
        .callback(STATE, CODE, Some(&pending(OTHER_STATE, OTHER_VERIFIER)))
        .await?;
    assert_left_alone(&hosted, &response).await?;
    // This browser's cookie, with a state it did not begin, an empty one and an oversized one.
    let oversized = "s".repeat(1025);
    for state in [OTHER_STATE, "forged", "", oversized.as_str()] {
        let response = hosted
            .callback(state, CODE, Some(&pending(STATE, VERIFIER)))
            .await?;
        assert_left_alone(&hosted, &response).await?;
    }
    assert_eq!(hosted.exchanges(), 0);
    Ok(())
}

#[tokio::test]
async fn a_state_that_differs_in_case_or_is_a_prefix_or_an_extension_reaches_no_provider()
-> TestResult {
    let hosted = Hosted::new()?;
    let upper = STATE.to_ascii_uppercase();
    let one_letter_raised = format!("S{}", STATE.chars().skip(1).collect::<String>());
    let prefix: String = STATE.chars().take(16).collect();
    let first: String = STATE.chars().take(1).collect();
    let extension = format!("{STATE}0");
    // The browser's own cookie, and a `state` that is nearly its state.
    for state in [&upper, &one_letter_raised, &prefix, &first, &extension] {
        assert_ne!(state, STATE);
        let response = hosted
            .callback(state, CODE, Some(&pending(STATE, VERIFIER)))
            .await?;
        assert_left_alone(&hosted, &response).await?;
        assert_eq!(hosted.exchanges(), 0, "state {state}");
    }
    // The same the other way round: the cookie's state is the near one.
    for state in [&upper, &prefix, &extension] {
        let response = hosted
            .callback(STATE, CODE, Some(&pending(state, VERIFIER)))
            .await?;
        assert_left_alone(&hosted, &response).await?;
        assert_eq!(hosted.exchanges(), 0, "cookie state {state}");
    }
    // The state itself, whole, completes: the refusals above are the comparison's alone.
    let response = hosted
        .callback(STATE, CODE, Some(&pending(STATE, VERIFIER)))
        .await?;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(hosted.exchanges(), 1);
    Ok(())
}

#[tokio::test]
async fn a_malformed_or_oversized_sign_in_cookie_reaches_no_provider() -> TestResult {
    let hosted = Hosted::new()?;
    let long_verifier = "v".repeat(129);
    let huge = "x".repeat(5000);
    for cookie in [
        format!("{LOGIN_COOKIE}="),
        format!("{LOGIN_COOKIE}={STATE}"),
        format!("{LOGIN_COOKIE}={STATE}:"),
        format!("{LOGIN_COOKIE}={STATE}:too-short"),
        format!("{LOGIN_COOKIE}={STATE}:{long_verifier}"),
        format!("{LOGIN_COOKIE}={STATE}:{VERIFIER}:extra"),
        format!("{LOGIN_COOKIE}={STATE}:{VERIFIER}/"),
        format!("{LOGIN_COOKIE}={STATE}.:{VERIFIER}"),
        format!("{LOGIN_COOKIE}={STATE}:{huge}"),
        // Two cookies of the name: the server never guesses which is its own.
        format!("{}; {}", pending(STATE, VERIFIER), pending(STATE, VERIFIER)),
    ] {
        let response = hosted.callback(STATE, CODE, Some(&cookie)).await?;
        assert_left_alone(&hosted, &response).await?;
        assert_eq!(hosted.exchanges(), 0, "cookie {cookie}");
    }
    // A code over the bound, or none, is not sent to the provider either. The state matched, so
    // this was the callback the sign-in was waiting for, and its cookie is cleared.
    for code in ["c".repeat(1025), String::new()] {
        let response = hosted
            .callback(STATE, &code, Some(&pending(STATE, VERIFIER)))
            .await?;
        assert_refused(&hosted, &response).await?;
        assert_eq!(hosted.exchanges(), 0);
    }
    Ok(())
}

#[tokio::test]
async fn the_code_is_exchanged_with_the_verifier_of_the_browsers_cookie() -> TestResult {
    let hosted = Hosted::new()?;
    // A person who edits their own cookie's verifier completes nothing: the provider refuses.
    let response = hosted
        .callback(STATE, CODE, Some(&pending(STATE, OTHER_VERIFIER)))
        .await?;
    assert_refused(&hosted, &response).await?;
    assert_eq!(hosted.exchanges(), 1);
    Ok(())
}

#[tokio::test]
async fn the_same_callback_sent_twice_signs_in_once() -> TestResult {
    let hosted = Hosted::new()?;
    let cookie = pending(STATE, VERIFIER);
    let first = hosted.callback(STATE, CODE, Some(&cookie)).await?;
    assert_eq!(first.status(), StatusCode::FOUND);
    // The browser dropped its cookie when the first answer cleared it.
    let again = hosted.callback(STATE, CODE, None).await?;
    assert_eq!(again.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(set_cookies(&again, LOGIN_COOKIE), Vec::<String>::new());
    assert_eq!(hosted.exchanges(), 1);
    // A replay that kept the cookie reaches the provider, where the code is spent.
    let replay = hosted.callback(STATE, CODE, Some(&cookie)).await?;
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
    assert_login_cookie_cleared(&replay)?;
    assert_eq!(
        set_cookies(&replay, SESSION_COOKIE_HTTPS),
        Vec::<String>::new()
    );
    assert_eq!(hosted.exchanges(), 2);
    Ok(())
}

#[tokio::test]
async fn a_sign_in_begun_in_a_second_tab_still_completes_after_the_first_tabs_callback()
-> TestResult {
    let hosted = Hosted::new()?;
    // Two tabs each began a sign-in; the browser holds one cookie, the second tab's.
    let cookie = pending(STATE, VERIFIER);
    // The first tab returns from the provider with its own state, which the cookie no longer
    // holds: refused, with nothing asked of the provider and the cookie left as it is.
    let first = hosted
        .callback(OTHER_STATE, "code-of-the-first-tab", Some(&cookie))
        .await?;
    assert_left_alone(&hosted, &first).await?;
    assert_eq!(hosted.exchanges(), 0);
    // The second tab's callback finds its sign-in still pending and completes it.
    let second = hosted.callback(STATE, CODE, Some(&cookie)).await?;
    assert_eq!(second.status(), StatusCode::FOUND);
    assert_login_cookie_cleared(&second)?;
    assert!(hosted.signed_in().await?);
    assert_eq!(hosted.exchanges(), 1);
    Ok(())
}

#[tokio::test]
async fn a_matching_callback_clears_the_cookie_when_the_provider_or_the_store_faults() -> TestResult
{
    let cookie = pending(STATE, VERIFIER);
    // The provider is down: a fault, not a refusal.
    let hosted = Hosted::new()?;
    hosted.provider.down.store(true, Ordering::SeqCst);
    let response = hosted.callback(STATE, CODE, Some(&cookie)).await?;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_login_cookie_cleared(&response)?;
    assert_eq!(
        set_cookies(&response, SESSION_COOKIE_HTTPS),
        Vec::<String>::new()
    );
    assert_eq!(hosted.exchanges(), 1);
    // The provider answers, and the store cannot take the session.
    let hosted = Hosted::new()?;
    hosted.base.credentials.set_down(true);
    let response = hosted.callback(STATE, CODE, Some(&cookie)).await?;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_login_cookie_cleared(&response)?;
    assert_eq!(
        set_cookies(&response, SESSION_COOKIE_HTTPS),
        Vec::<String>::new()
    );
    assert_eq!(hosted.exchanges(), 1);
    hosted.base.credentials.set_down(false);
    assert!(!hosted.signed_in().await?);
    Ok(())
}

#[tokio::test]
async fn hosted_mode_has_no_launch_token_exchange() -> TestResult {
    let hosted = Hosted::new()?;
    let response = hosted
        .app
        .clone()
        .oneshot(get("/auth/local?token=anything")?)
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "../../core/tests/support/mod.rs"]
mod fixtures;
mod support;
