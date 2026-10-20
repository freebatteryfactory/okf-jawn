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
//! A page's cursor is where the stream resumes once that page's events are delivered: each
//! delivered event moves the resume point to its own id, and then the page's `next_cursor`, when
//! it has one, moves it past what the page held and did not show (a non-administrator's
//! security events, contract ruling 21), so a run of hidden events is crossed, never re-read.
//! The SSE `id` stays each delivered event's own id, so a reconnecting client re-reads such a
//! run and crosses it the same way. A poll whose page is full, or carries a cursor, is followed
//! by the next poll at once; only a page that is neither is followed by a wait of the poll
//! interval. This relies on `list_events` answering a cursor only while it read something: when
//! the log has no more, its cursor is the last hidden event's and the next page is empty with
//! no cursor, which waits. Each open stream costs one `list_events` per interval while it is
//! caught up. A caller (tenant, subject, route and client) holds at most [`MAX_STREAMS_PER_CALLER`]
//! streams open at once; one more is refused `Unavailable` with `Retry-After`, and a stream that
//! ends for any reason gives its place back.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::Extension;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::stream;
use okf_jawn_contract::access::{AccessRoute, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::ListEventsResponse;
use okf_jawn_core::dispatch::{Caller, DispatchPorts, dispatch};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::errors::{Readable, error_response, retry_response};
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
    /// How long an open stream waits after a page that was neither full nor carried a cursor;
    /// never zero.
    interval: Duration,
    /// How many streams each caller holds open now.
    open: Arc<Mutex<BTreeMap<CallerKey, usize>>>,
    /// Shutdown of the composed service ends streams even while a poll or its wait is pending.
    stop: CancellationToken,
}

/// A caller, as the grant cache keys one: tenant, subject, route and client.
type CallerKey = (String, String, u8, Option<String>);

/// One open stream's place among its caller's: given back when the stream ends, however it ends.
struct Slot {
    open: Arc<Mutex<BTreeMap<CallerKey, usize>>>,
    caller: CallerKey,
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
    /// Held for as long as the stream lives.
    _slot: Slot,
    /// The last page's cursor: where the next poll resumes once `pending` is delivered.
    resume: Option<String>,
    /// Whether more may be waiting after the last page: it was full or it carried a cursor.
    more: bool,
    ended: bool,
}

/// The route (`api/transports.json` `stream_events`).
pub const EVENTS_PATH: &str = "/api/workspaces/{workspace_id}/events";

/// How long a production stream waits after a page that was neither full nor carried a cursor.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Most notifications one poll reads; a poll that returns this many is a full page.
pub const PAGE_LIMIT: u16 = 100;

/// Most event streams one caller (tenant, subject, route, client) holds open at once.
pub const MAX_STREAMS_PER_CALLER: usize = 16;

/// The `Retry-After`, in seconds, of a stream refused because its caller holds too many.
const STREAM_RETRY_SECONDS: u32 = 5;

/// Longest `Last-Event-ID` accepted; a cursor is short and opaque.
pub const MAX_CURSOR_CHARS: usize = 256;

impl EventStreams {
    /// The stream route's state: polls run through `bound`, `identity` is the identity
    /// middleware's own state (asked before every poll whether the stream's credential still
    /// authenticates), and an open stream waits `interval` after a page that was neither full nor
    /// carried a cursor ([`POLL_INTERVAL`] in production).
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
            open: Arc::new(Mutex::new(BTreeMap::new())),
            stop: CancellationToken::new(),
        })
    }

    /// End every stream when `stop` is cancelled, interrupting its next poll or interval wait.
    #[must_use]
    pub fn with_stop(mut self, stop: CancellationToken) -> Self {
        self.stop = stop;
        self
    }

    /// A place for one more stream of `principal`, or `None` when it holds
    /// [`MAX_STREAMS_PER_CALLER`] open already.
    fn take_slot(&self, principal: &Principal) -> Result<Option<Slot>, ApiError> {
        let caller = caller_key(principal);
        let mut open = lock(&self.open)?;
        let held = open.entry(caller.clone()).or_insert(0);
        if *held >= MAX_STREAMS_PER_CALLER {
            return Ok(None);
        }
        *held = held.saturating_add(1);
        Ok(Some(Slot {
            open: Arc::clone(&self.open),
            caller,
        }))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        // A poisoned count keeps the place taken: the caller may open fewer streams, never more.
        if let Ok(mut open) = self.open.lock()
            && let Some(held) = open.get_mut(&self.caller)
        {
            *held = held.saturating_sub(1);
            if *held == 0 {
                open.remove(&self.caller);
            }
        }
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
    let slot = match streams.take_slot(&principal) {
        Ok(Some(slot)) => slot,
        Ok(None) => {
            return retry_response(
                ApiError::new(
                    ErrorCode::Unavailable,
                    "Too many event streams are open for this caller; close one and try again",
                ),
                STREAM_RETRY_SECONDS,
            );
        }
        Err(error) => return error_response(error),
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
        more: more_waiting(&first),
        resume: first.next_cursor,
        pending: first.events.into(),
        ended: false,
        _slot: slot,
    };
    let events = stream::unfold(state, next_event);
    Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// Whether more may be waiting after `page`, so the next poll should not wait: it is full, or
/// it carries a cursor (`list_events` answers one past events it did not show).
fn more_waiting(page: &ListEventsResponse) -> bool {
    page.events.len() >= usize::from(PAGE_LIMIT) || page.next_cursor.is_some()
}

/// The stream's next SSE event: a pending notification, or the result of the next poll.
async fn next_event(state: Polling) -> Option<(Result<SseEvent, axum::Error>, Polling)> {
    let stop = state.streams.stop.clone();
    tokio::select! {
        biased;
        () = stop.cancelled() => None,
        event = advance(state) => event,
    }
}

/// Advance while shutdown is not signalled; the outer select also interrupts store calls.
async fn advance(mut state: Polling) -> Option<(Result<SseEvent, axum::Error>, Polling)> {
    loop {
        if state.ended {
            return None;
        }
        if let Some(event) = state.pending.pop_front() {
            state.after = Some(event.id.clone());
            let sent = SseEvent::default().id(event.id.clone()).json_data(&event);
            return Some((sent, state));
        }
        // The page is delivered: its cursor, past the events it did not show, is where the
        // next poll resumes.
        if let Some(resume) = state.resume.take() {
            state.after = Some(resume);
        }
        if !state.more {
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
                state.more = more_waiting(&page);
                state.resume = page.next_cursor;
                state.pending = page.events.into();
            }
            Err(_) => state.ended = true,
        }
    }
}

/// The caller a stream counts against: tenant, subject, route and client.
fn caller_key(principal: &Principal) -> CallerKey {
    let route = match principal.route {
        AccessRoute::BrowserSession => 0,
        AccessRoute::McpDelegation => 1,
        AccessRoute::LocalOwner => 2,
        AccessRoute::Service => 3,
    };
    (
        principal.tenant_id.as_str().to_owned(),
        principal.subject.clone(),
        route,
        principal.client_id.clone(),
    )
}

/// The open-stream counts, locked.
fn lock(
    open: &Mutex<BTreeMap<CallerKey, usize>>,
) -> Result<MutexGuard<'_, BTreeMap<CallerKey, usize>>, ApiError> {
    open.lock().map_err(|_| {
        ApiError::new(
            ErrorCode::Internal,
            "the open event streams' count is poisoned",
        )
    })
}
