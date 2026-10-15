//! `stream_events`: a workspace's notifications as Server-Sent Events, resumable by
//! `Last-Event-ID`.
//!
//! The stream holds no behaviour of its own: each poll is the `list_events` operation run through
//! `core::dispatch` as the request's own principal, so authorization (and the rule that only a
//! workspace administrator sees security events) is exactly the JSON operation's. Each event is
//! sent with its cursor as the SSE `id`, and `Last-Event-ID` is forwarded as the operation's
//! `after`, so a reconnecting client resumes after the last event it received; a `Last-Event-ID`
//! that is not a short text cursor is refused as `InvalidInput`, never dropped into a replay from
//! the start. A refusal on the first poll is the operation's `ApiError` JSON; a refusal later
//! (a revoked grant, say) ends the stream. Notifications are projections, not canonical content.
//!
//! A stream lives only while its credential does (SPEC section 11: "Logging out ends the session
//! immediately"). Before every poll after the first, the stream asks the identity middleware's own
//! `Identity::still_authenticates` whether the credential that opened it would authenticate a new
//! request now (the session not revoked or expired, the connector not revoked, the Connect token
//! not past its `exp`), and ends if not. So a logout or a revocation closes an open stream within
//! one poll interval, and no event is delivered after that check fails. The check guards the
//! next poll, not what an earlier poll already fetched: notifications fetched while the
//! credential lived (at most one page, [`PAGE_LIMIT`]) are still delivered after it dies, as
//! they would have been a moment sooner. A check the store cannot answer counts as failed.
//!
//! By ruling, a hosted stream ends at its token's `exp`, not when the issuer withdraws the key
//! that signed the token: the check compares the stored expiry with the clock and asks the
//! verifier nothing, so a withdrawn key ends no stream already open (a new stream with that
//! token is not opened). The check allows no leeway, so a stream ends up to 30 seconds before
//! a new request with the same token would be refused.
//!
//! A poll that returns a full page is followed by the next poll at once; only a page that is not
//! full is followed by a wait of the poll interval. Each open stream costs one `list_events` per
//! interval; nothing here limits the streams of one principal (carried to M2).

use std::collections::VecDeque;
use std::time::Duration;

use axum::Extension;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::stream;
use okf_jawn_contract::access::Principal;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::ListEventsResponse;
use okf_jawn_core::dispatch::{Caller, DispatchPorts, dispatch};
use serde_json::json;

use crate::errors::{Readable, error_response};
use crate::identity::{Credential, Identity};
use crate::{BoundApplication, SessionId};

/// What the stream route is built from; built only by [`EventStreams::new`], which refuses an
/// interval of zero.
#[derive(Clone)]
pub struct EventStreams {
    /// The application and dispatch's ports.
    bound: BoundApplication,
    /// The identity middleware's state, asked before every poll whether the stream's credential
    /// still authenticates.
    identity: Identity,
    /// How long an open stream waits after a page that was not full; never zero.
    interval: Duration,
}

/// Where an open stream stands.
struct Polling {
    streams: EventStreams,
    principal: Principal,
    credential: Credential,
    session_id: Option<String>,
    workspace_id: String,
    after: Option<String>,
    pending: VecDeque<okf_jawn_contract::events::Event>,
    /// Whether the last poll returned a full page, so more may be waiting.
    full_page: bool,
    ended: bool,
}

/// The route (`api/transports.json` `stream_events`).
pub const EVENTS_PATH: &str = "/api/workspaces/{workspace_id}/events";

/// How long a production stream waits after a page that was not full.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Most notifications one poll reads; a poll that returns this many is a full page.
pub const PAGE_LIMIT: u16 = 100;

/// Longest `Last-Event-ID` accepted; a cursor is short and opaque.
pub const MAX_CURSOR_CHARS: usize = 256;

impl EventStreams {
    /// The stream route's state: polls run through `bound`, `identity` is the identity
    /// middleware's own state (asked before every poll whether the stream's credential still
    /// authenticates), and an open stream waits `interval` after a page that was not full
    /// ([`POLL_INTERVAL`] in production).
    ///
    /// # Errors
    /// Returns `InvalidInput` naming `interval` when it is zero: a stream with no wait would
    /// poll the application without pause for as long as it stays open.
    pub fn new(
        bound: BoundApplication,
        identity: Identity,
        interval: Duration,
    ) -> Result<Self, ApiError> {
        if interval.is_zero() {
            return Err(ApiError::new(
                ErrorCode::InvalidInput,
                "The event stream's poll interval must be longer than zero",
            )
            .with_field("interval"));
        }
        Ok(Self {
            bound,
            identity,
            interval,
        })
    }
}

/// The stream route, for `ServiceParts::extra`.
pub fn routes(streams: EventStreams) -> Router {
    Router::new()
        .route(EVENTS_PATH, get(stream_events))
        .with_state(streams)
}

/// The cursor a reconnecting client resumes after: `Ok(None)` without the header or with an
/// empty one (which resets the cursor).
///
/// # Errors
/// Returns `InvalidInput` when the header is not text or is longer than [`MAX_CURSOR_CHARS`].
/// The message names the header; the refusal has no `field`, which is a JSON Pointer into the
/// request body (ruling 5), and a header is not in the body.
pub fn last_event_id(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    let Some(value) = headers.get("last-event-id") else {
        return Ok(None);
    };
    let invalid = || {
        ApiError::new(
            ErrorCode::InvalidInput,
            "The Last-Event-ID header must be a cursor of at most 256 characters",
        )
    };
    let cursor = value.to_str().map_err(|_| invalid())?;
    if cursor.chars().count() > MAX_CURSOR_CHARS {
        return Err(invalid());
    }
    Ok((!cursor.is_empty()).then(|| cursor.to_owned()))
}

/// One poll: the notifications after `after`, as `list_events` answers the caller.
async fn poll(
    bound: &BoundApplication,
    principal: &Principal,
    session_id: Option<&str>,
    workspace_id: &str,
    after: Option<&str>,
) -> Result<ListEventsResponse, ApiError> {
    let ports = DispatchPorts {
        access: bound.access.as_ref(),
        mutations: bound.mutations.as_ref(),
        events: bound.events.as_ref(),
    };
    let caller = Caller {
        principal,
        session_id,
    };
    let mut input = json!({ "workspace_id": workspace_id, "page": { "limit": PAGE_LIMIT } });
    if let (Some(cursor), Some(object)) = (after, input.as_object_mut()) {
        object.insert("after".to_owned(), json!(cursor));
    }
    let output = dispatch(
        bound.application.as_ref(),
        &ports,
        &caller,
        "list_events",
        input,
    )
    .await?;
    serde_json::from_value(output).map_err(|_| {
        ApiError::new(
            ErrorCode::Internal,
            "list_events answered an unexpected shape",
        )
    })
}

async fn stream_events(
    State(streams): State<EventStreams>,
    principal: Option<Extension<Principal>>,
    credential: Option<Extension<Credential>>,
    session: Option<Extension<SessionId>>,
    Readable(Path(workspace_id)): Readable<Path<String>>,
    headers: HeaderMap,
) -> Response {
    let (Some(Extension(principal)), Some(Extension(credential))) = (principal, credential) else {
        return error_response(ApiError::new(
            ErrorCode::Unauthenticated,
            "Authentication is required",
        ));
    };
    let session_id = session.map(|Extension(SessionId(id))| id);
    let after = match last_event_id(&headers) {
        Ok(after) => after,
        Err(error) => return error_response(error),
    };
    let first = match poll(
        &streams.bound,
        &principal,
        session_id.as_deref(),
        &workspace_id,
        after.as_deref(),
    )
    .await
    {
        Ok(first) => first,
        Err(error) => return error_response(error),
    };
    let state = Polling {
        streams,
        principal,
        credential,
        session_id,
        workspace_id,
        after,
        full_page: is_full(&first),
        pending: first.events.into(),
        ended: false,
    };
    let events = stream::unfold(state, next_event);
    Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// Whether `page` is full, so the next poll should not wait.
fn is_full(page: &ListEventsResponse) -> bool {
    page.events.len() >= usize::from(PAGE_LIMIT)
}

/// The stream's next SSE event: a pending notification, or the result of the next poll.
async fn next_event(mut state: Polling) -> Option<(Result<SseEvent, axum::Error>, Polling)> {
    loop {
        if state.ended {
            return None;
        }
        if let Some(event) = state.pending.pop_front() {
            state.after = Some(event.id.clone());
            let sent = SseEvent::default().id(event.id.clone()).json_data(&event);
            return Some((sent, state));
        }
        if !state.full_page {
            tokio::time::sleep(state.streams.interval).await;
        }
        // Asked before every poll: a stream never outlives the credential that opened it.
        if !state
            .streams
            .identity
            .still_authenticates(&state.credential)
            .await
        {
            return None;
        }
        match poll(
            &state.streams.bound,
            &state.principal,
            state.session_id.as_deref(),
            &state.workspace_id,
            state.after.as_deref(),
        )
        .await
        {
            Ok(page) => {
                state.full_page = is_full(&page);
                state.pending = page.events.into();
            }
            Err(_) => state.ended = true,
        }
    }
}
