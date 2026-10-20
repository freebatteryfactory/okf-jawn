//! Hosted browser sign-in through WorkOS AuthKit: PKCE, a state bound to the browser that began
//! it, and the code exchange.
//!
//! The server keeps nothing about a sign-in in progress. `GET /auth/login` asks
//! [`HostedLogin::begin`] for an AuthKit authorization URL with a fresh PKCE verifier and state,
//! and hands both to the browser in the cookie [`LOGIN_COOKIE`] (`__Host-okf-login`: `HttpOnly`,
//! `Secure`, `SameSite=Lax`, `Path=/`, `Max-Age=600`, no `Domain`). `GET /auth/callback` refuses,
//! before any call to the provider, unless that cookie is present, well formed
//! ([`PendingLogin::parse`]) and its state is the `state` parameter; it then exchanges the code
//! with the cookie's verifier ([`HostedLogin::complete`]). So only the browser that began a
//! sign-in can complete it, and an unauthenticated caller of `/auth/login` costs the server no
//! memory.
//!
//! The cookie is cleared on every outcome of the callback whose state matches it, success or
//! refusal. A callback that does not match (no cookie, a cookie that is not a sign-in, another
//! state) is refused and leaves the cookie alone: a browser holds one sign-in cookie, so when
//! two tabs each begin a sign-in the later one's cookie stands, the earlier tab's callback is
//! refused without touching it, and the later tab's still completes. Leaving it costs nothing:
//! the cookie is useless without the provider's code for its own verifier, and the browser
//! drops it after its `Max-Age`.
//!
//! The cookie is not sealed, and need not be. It is `HttpOnly` and `__Host-`, so no script and no
//! other host can read or set it; a person who edits their own cookie can only complete their own
//! sign-in, as themselves; and the authorization code is single use at the provider, bound there
//! to the PKCE challenge of the verifier this cookie holds. The 600 seconds are the browser's
//! `Max-Age` for the cookie and no promise of the server's: the server keeps no clock for a
//! sign-in, and a code's real lifetime is the provider's.
//!
//! The user and organization of a completed exchange become the subject and tenant of a
//! server-side browser session with route `BrowserSession`. Codes, verifiers and tokens are never
//! logged or echoed. The production login is WorkOS (`WorkosLogin`, runtime); the routes are
//! bound against the trait.

use std::time::Duration;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::ports::PortFuture;

/// The identity provider's half of a browser sign-in.
pub trait HostedLogin: Send + Sync {
    /// An authorization URL carrying a fresh PKCE challenge and state.
    ///
    /// # Errors
    /// Returns `Unavailable` when the URL cannot be built.
    fn begin(&self) -> Result<LoginStart, ApiError>;

    /// Exchange an authorization code with its PKCE verifier for the signed-in user.
    fn complete<'a>(&'a self, code: &'a str, verifier: &'a str) -> PortFuture<'a, HostedUser>;
}

/// A started sign-in: where to send the browser, and what the callback must present.
#[derive(Clone)]
pub struct LoginStart {
    /// The AuthKit authorization URL.
    pub url: String,
    /// The PKCE verifier; it travels only in the browser's `HttpOnly` sign-in cookie.
    pub verifier: String,
    /// The state the callback must return.
    pub state: String,
}

/// The user a completed sign-in names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedUser {
    /// The WorkOS user id, the session's subject.
    pub subject: String,
    /// The WorkOS organization, the session's tenant.
    pub organization_id: String,
}

/// What the sign-in cookie holds: the state and the PKCE verifier of the sign-in its browser
/// began.
#[derive(Clone, PartialEq, Eq)]
pub struct PendingLogin {
    /// The state the callback's `state` parameter must equal.
    pub state: String,
    /// The PKCE verifier the code is exchanged with.
    pub verifier: String,
}

/// The cookie that holds a browser's sign-in in progress.
pub const LOGIN_COOKIE: &str = "__Host-okf-login";

/// The sign-in cookie's `Max-Age`: how long the browser keeps it.
pub const LOGIN_MAX_AGE: Duration = Duration::from_secs(600);

/// Fewest and most characters of a state: unreserved URL characters without `.` and `~`.
const STATE_CHARS: (usize, usize) = (16, 128);

/// Fewest and most characters of a PKCE verifier (RFC 7636 section 4.1): unreserved characters.
const VERIFIER_CHARS: (usize, usize) = (43, 128);

/// The longest authorization code or `state` parameter the callback reads.
pub const MAX_CALLBACK_PARAMETER_CHARS: usize = 1024;

/// What separates the state from the verifier in the cookie; neither may contain it.
const SEPARATOR: char = ':';

impl PendingLogin {
    /// The cookie value of a started sign-in: its state, `:`, its verifier.
    ///
    /// # Errors
    /// Returns `Internal` when the provider's state or verifier is not of the bounded shape the
    /// callback will accept: nothing malformed is ever handed to a browser.
    pub fn encode(start: &LoginStart) -> Result<String, ApiError> {
        if is_state(&start.state) && is_verifier(&start.verifier) {
            Ok(format!("{}{SEPARATOR}{}", start.state, start.verifier))
        } else {
            Err(ApiError::new(
                ErrorCode::Internal,
                "The identity provider started a sign-in of an unexpected shape",
            ))
        }
    }

    /// Read a cookie value back; `None` unless it is exactly one bounded state, `:`, and one
    /// bounded verifier, each of its own characters.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let (state, verifier) = value.split_once(SEPARATOR)?;
        (is_state(state) && is_verifier(verifier)).then(|| Self {
            state: state.to_owned(),
            verifier: verifier.to_owned(),
        })
    }
}

/// Whether `text` is a state: 16 to 128 of `A-Z a-z 0-9 - _`.
fn is_state(text: &str) -> bool {
    (STATE_CHARS.0..=STATE_CHARS.1).contains(&text.len())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Whether `text` is a PKCE verifier: 43 to 128 of `A-Z a-z 0-9 - . _ ~`.
fn is_verifier(text: &str) -> bool {
    (VERIFIER_CHARS.0..=VERIFIER_CHARS.1).contains(&text.len())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
}
