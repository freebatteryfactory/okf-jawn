//! The browser session cookies, read and written through the `cookie` crate; nothing here parses
//! or formats a cookie by hand.
//!
//! The session cookie is named by the service's public origin (`session_cookie_name`):
//! `__Host-okf-session` over `https`, always `Secure`, `Path=/` and with no `Domain`, so a sibling
//! host or another port cannot set one; `okf-session` on plain `http` (loopback), where a `__Host-`
//! cookie cannot be set. It is `HttpOnly` and `SameSite=Strict`, and the server reads only the
//! name of its own scheme. Its value is an opaque random secret whose SHA-256 is the stored
//! session id. The anti-CSRF cookie (`okf-csrf`) carries the session's
//! derived token with the same attributes except `HttpOnly`, so the page can read it and send it
//! back in the `x-okf-csrf` header. A request that carries two cookies of one name is read as
//! carrying none: a sibling site can set a cookie on the same host, and the server never guesses
//! which one is its own.

use std::time::Duration;

use axum::http::header::COOKIE;
use axum::http::{HeaderMap, HeaderValue};
use cookie::{Cookie, SameSite};
use okf_jawn_contract::access::{CSRF_COOKIE, SESSION_COOKIE, SESSION_COOKIE_HTTPS};
use okf_jawn_contract::error::{ApiError, ErrorCode};

use crate::hosted::{LOGIN_COOKIE, LOGIN_MAX_AGE};
use crate::identity::{CookieReader, csrf_token_of};

/// Reads cookies with `cookie::Cookie::split_parse`.
#[derive(Debug, Clone, Copy, Default)]
pub struct CrateCookies;

/// How the session cookies are written.
#[derive(Debug, Clone, Copy)]
pub struct CookiePolicy {
    /// Send `Secure`: the public origin is `https`.
    pub secure: bool,
    /// How long a session lasts; the cookies' `Max-Age`.
    pub lifetime: Duration,
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

/// The session cookie's name for a public origin that is `https` (`secure`) or plain `http`:
/// `SESSION_COOKIE_HTTPS` or `SESSION_COOKIE`.
#[must_use]
pub const fn session_cookie_name(secure: bool) -> &'static str {
    if secure {
        SESSION_COOKIE_HTTPS
    } else {
        SESSION_COOKIE
    }
}

/// The two `Set-Cookie` values that open a session whose cookie secret is `secret`.
///
/// # Errors
/// Returns `Internal` when the lifetime does not fit a cookie or a value no header can carry.
pub fn session_cookies(secret: &str, policy: CookiePolicy) -> Result<[HeaderValue; 2], ApiError> {
    let max_age = time::Duration::try_from(policy.lifetime)
        .map_err(|_| ApiError::new(ErrorCode::Internal, "The session lifetime is too long"))?;
    let session = Cookie::build((session_cookie_name(policy.secure), secret.to_owned()))
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(policy.secure)
        .path("/")
        .max_age(max_age)
        .build();
    let csrf = Cookie::build((CSRF_COOKIE, csrf_token_of(secret)))
        .http_only(false)
        .same_site(SameSite::Strict)
        .secure(policy.secure)
        .path("/")
        .max_age(max_age)
        .build();
    Ok([header(&session)?, header(&csrf)?])
}

/// The two `Set-Cookie` values that remove the session cookies.
///
/// # Errors
/// Returns `Internal` when a value cannot be carried in a header.
pub fn cleared_cookies(secure: bool) -> Result<[HeaderValue; 2], ApiError> {
    let session = Cookie::build((session_cookie_name(secure), ""))
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(secure)
        .path("/")
        .removal()
        .build();
    let csrf = Cookie::build((CSRF_COOKIE, ""))
        .same_site(SameSite::Strict)
        .secure(secure)
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
