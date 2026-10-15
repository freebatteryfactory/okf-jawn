//! The browser session cookies, read and written through the `cookie` crate; nothing here parses
//! or formats a cookie by hand.
//!
//! Both cookies are named by the scheme of the service's public origin, one value decided once
//! (`CookieScheme`, held by the identity middleware and handed to the session routes through
//! `Identity::cookie_policy`), so the name written is the name read. Over `https` they are
//! `__Host-okf-session` and `__Host-okf-csrf`, always `Secure`, `Path=/` and with no `Domain`,
//! so a sibling host or another port cannot set one; on plain `http` (loopback) they are
//! `okf-session` and `okf-csrf`, where a `__Host-` cookie cannot be set. The session cookie is
//! `HttpOnly` and `SameSite=Strict`, and the server reads only the name of its own scheme. Its
//! value is an opaque random secret whose SHA-256 is the stored session id. The anti-CSRF cookie
//! carries the session's derived token with the same attributes except `HttpOnly`, so the page
//! can read it and send it back in the `x-okf-csrf` header; the server reads only that header,
//! never the cookie, and the page reads only the cookie name of its own scheme. A request that
//! carries two cookies of one name is read as carrying none: a sibling site can set a cookie on
//! the same host, and the server never guesses which one is its own.

use std::time::Duration;

use axum::http::header::COOKIE;
use axum::http::{HeaderMap, HeaderValue};
use cookie::{Cookie, SameSite};
use okf_jawn_contract::access::{
    CSRF_COOKIE, CSRF_COOKIE_HTTPS, SESSION_COOKIE, SESSION_COOKIE_HTTPS,
};
use okf_jawn_contract::error::{ApiError, ErrorCode};

use crate::hosted::{LOGIN_COOKIE, LOGIN_MAX_AGE};
use crate::identity::{CookieReader, csrf_token_of};

/// Reads cookies with `cookie::Cookie::split_parse`.
#[derive(Debug, Clone, Copy, Default)]
pub struct CrateCookies;

/// The browser cookies' names and `Secure` attribute of one deployment, decided once by the
/// scheme of its public origin (ruling 17).
///
/// The identity middleware decides it when it is built (`Identity::new`) and is its only source:
/// the session routes write and clear their cookies with the one the middleware reads by
/// (`Identity::cookie_policy`), so the name written is the name read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CookieScheme {
    /// The public origin is `https`.
    secure: bool,
}

/// How the session cookies are written: the deployment's `CookieScheme` and the session's
/// lifetime. Only `Identity::cookie_policy` makes one, so it always carries the scheme the
/// identity middleware reads by.
#[derive(Debug, Clone, Copy)]
pub struct CookiePolicy {
    scheme: CookieScheme,
    lifetime: Duration,
}

impl CookieReader for CrateCookies {
    fn read(&self, headers: &HeaderMap, name: &str) -> Option<String> {
        let mut found: Option<String> = None;
        for value in headers.get_all(COOKIE) {
            let Ok(text) = value.to_str() else {
                continue;
            };
            for cookie in Cookie::split_parse(text).flatten() {
                if cookie.name() == name {
                    if found.is_some() {
                        return None;
                    }
                    found = Some(cookie.value().to_owned());
                }
            }
        }
        found
    }
}

impl CookieScheme {
    /// The scheme of a service whose cookie requests come from `origins`, its own public origin:
    /// `https` when there is one and every one is `https`, plain `http` otherwise, since a browser
    /// sends a `__Host-` cookie only over `https`.
    pub(crate) fn of_origins(origins: &[String]) -> Self {
        let https = |origin: &String| {
            origin
                .get(..8)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
        };
        Self {
            secure: !origins.is_empty() && origins.iter().all(https),
        }
    }

    /// Whether the cookies are `Secure` and carry the `__Host-` prefix: the public origin is
    /// `https`.
    #[must_use]
    pub const fn secure(self) -> bool {
        self.secure
    }

    /// The session cookie's name: `SESSION_COOKIE_HTTPS` over `https`, else `SESSION_COOKIE`.
    #[must_use]
    pub const fn session_cookie(self) -> &'static str {
        if self.secure {
            SESSION_COOKIE_HTTPS
        } else {
            SESSION_COOKIE
        }
    }

    /// The anti-CSRF cookie's name: `CSRF_COOKIE_HTTPS` over `https`, else `CSRF_COOKIE`.
    #[must_use]
    pub const fn csrf_cookie(self) -> &'static str {
        if self.secure {
            CSRF_COOKIE_HTTPS
        } else {
            CSRF_COOKIE
        }
    }
}

impl CookiePolicy {
    /// The policy of a deployment of `scheme` whose sessions last `lifetime`.
    pub(crate) const fn new(scheme: CookieScheme, lifetime: Duration) -> Self {
        Self { scheme, lifetime }
    }

    /// The deployment's cookie scheme.
    #[must_use]
    pub const fn scheme(self) -> CookieScheme {
        self.scheme
    }

    /// How long a session lasts; the cookies' `Max-Age`.
    #[must_use]
    pub const fn lifetime(self) -> Duration {
        self.lifetime
    }
}

/// The two `Set-Cookie` values that open a session whose cookie secret is `secret`.
///
/// # Errors
/// Returns `Internal` when the lifetime does not fit a cookie or a value no header can carry.
pub fn session_cookies(secret: &str, policy: CookiePolicy) -> Result<[HeaderValue; 2], ApiError> {
    let max_age = time::Duration::try_from(policy.lifetime)
        .map_err(|_| ApiError::new(ErrorCode::Internal, "The session lifetime is too long"))?;
    let scheme = policy.scheme;
    let session = Cookie::build((scheme.session_cookie(), secret.to_owned()))
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(scheme.secure())
        .path("/")
        .max_age(max_age)
        .build();
    let csrf = Cookie::build((scheme.csrf_cookie(), csrf_token_of(secret)))
        .http_only(false)
        .same_site(SameSite::Strict)
        .secure(scheme.secure())
        .path("/")
        .max_age(max_age)
        .build();
    Ok([header(&session)?, header(&csrf)?])
}

/// The two `Set-Cookie` values that remove the session cookies of `scheme`, with the names and
/// attributes they were set with.
///
/// # Errors
/// Returns `Internal` when a value cannot be carried in a header.
pub fn cleared_cookies(scheme: CookieScheme) -> Result<[HeaderValue; 2], ApiError> {
    let session = Cookie::build((scheme.session_cookie(), ""))
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(scheme.secure())
        .path("/")
        .removal()
        .build();
    let csrf = Cookie::build((scheme.csrf_cookie(), ""))
        .same_site(SameSite::Strict)
        .secure(scheme.secure())
        .path("/")
        .removal()
        .build();
    Ok([header(&session)?, header(&csrf)?])
}

/// The `Set-Cookie` value that hands a browser its sign-in in progress: `value` in
/// `__Host-okf-login`, `HttpOnly`, `Secure`, `SameSite=Lax` (the callback is a navigation from the
/// identity provider, which `Strict` would not carry), `Path=/`, `Max-Age=600`, no `Domain`.
///
/// # Errors
/// Returns `Internal` when the value cannot be carried in a header.
pub fn login_cookie(value: &str) -> Result<HeaderValue, ApiError> {
    let max_age = time::Duration::try_from(LOGIN_MAX_AGE)
        .map_err(|_| ApiError::new(ErrorCode::Internal, "The sign-in lifetime is too long"))?;
    let cookie = Cookie::build((LOGIN_COOKIE, value.to_owned()))
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(max_age)
        .build();
    header(&cookie)
}

/// The `Set-Cookie` value that removes the sign-in cookie, with the attributes it was set with.
///
/// # Errors
/// Returns `Internal` when the value cannot be carried in a header.
pub fn cleared_login_cookie() -> Result<HeaderValue, ApiError> {
    let cookie = Cookie::build((LOGIN_COOKIE, ""))
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .path("/")
        .removal()
        .build();
    header(&cookie)
}

fn header(cookie: &Cookie<'_>) -> Result<HeaderValue, ApiError> {
    HeaderValue::from_str(&cookie.to_string())
        .map_err(|_| ApiError::new(ErrorCode::Internal, "A cookie could not be written"))
}
