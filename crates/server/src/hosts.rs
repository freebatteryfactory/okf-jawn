//! Which host a request names: the one rule, shared by the application origin's service and the
//! sandbox origin's.
//!
//! A request names a host in its `Host` header and, for an absolute-form target or over HTTP/2,
//! in the authority of its request target. Every one present must be a host the service answers
//! for, compared whole and ASCII case-insensitively, so a guard and any proxy that routes on the
//! other value can never disagree. More than one `Host` header is refused outright, whatever
//! they name, and so is a request that names no host at all. Both guards call this one function:
//! the sandbox serves hostile content, and its guard is exactly as strict as the application's.

use axum::http::Request;
use axum::http::header::HOST;

/// Whether every host `request` names is one of `expected`, and it names at least one.
#[must_use]
pub fn names_only<B>(request: &Request<B>, expected: &[String]) -> bool {
    let known = |host: &str| {
        expected
            .iter()
            .any(|known| known.eq_ignore_ascii_case(host))
    };
    let mut headers = request.headers().get_all(HOST).iter();
    let header = headers.next();
    if headers.next().is_some() {
        return false;
    }
    let authority = request.uri().authority();
    let header_ok = header.is_none_or(|host| host.to_str().is_ok_and(known));
    let authority_ok = authority.is_none_or(|authority| known(authority.as_str()));
    (header.is_some() || authority.is_some()) && header_ok && authority_ok
}
