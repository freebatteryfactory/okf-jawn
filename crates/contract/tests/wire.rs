//! Wire names and bounds the bindings share with every client, pinned to their published values.
//!
//! A change to one of these is a wire change: the server, workspace-ui and agents read them, so
//! a new value needs a deliberate contract change, not an edit that a test silently follows.

use okf_jawn_contract::access::{CSRF_COOKIE, CSRF_HEADER};

#[test]
fn the_csrf_double_submit_names_are_pinned() {
    assert_eq!(CSRF_COOKIE, "okf-csrf");
    assert_eq!(CSRF_HEADER, "x-okf-csrf");
    assert_ne!(
        CSRF_COOKIE, "okf-session",
        "the CSRF cookie is not the session cookie"
    );
    assert!(
        CSRF_HEADER
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-'),
        "an HTTP header name, written in lowercase as HTTP/2 sends it"
    );
}

#[path = "../../../tests/support/check.rs"]
mod check;
