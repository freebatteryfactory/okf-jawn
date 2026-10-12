//! `stream_events`: a workspace's notifications as Server-Sent Events, resumable by
//! `Last-Event-ID`.
//!
//! The stream holds no behaviour of its own: each poll is the `list_events` operation run through
//! `core::dispatch` as the request's own principal, so authorization (and the rule that only a
//! workspace administrator sees security events) is exactly the JSON operation's. Each event is
//! sent with its cursor as the SSE `id`, so a reconnecting client resumes after the last one it
//! received. A refusal on the first poll is the operation's `ApiError` JSON; a refusal later
//! (a revoked grant, say) ends the stream. Notifications are projections, not canonical content.

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

use crate::errors::error_response;
use crate::{BoundApplication, SessionId};

/// Where an open stream stands.
struct Polling {
    bound: BoundApplication,
    principal: Principal,
    session_id: Option<String>,
    workspace_id: String,
    after: Option<String>,
    pending: VecDeque<okf_jawn_contract::events::Event>,
    /// Whether the next poll may run without waiting first.
    waited: bool,
    ended: bool,
}

/// The route (`api/transports.json` `stream_events`).
pub const EVENTS_PATH: &str = "/api/workspaces/{workspace_id}/events";

/// How often an open stream asks for new notifications.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Most notifications one poll reads.
const PAGE_LIMIT: u16 = 100;

/// Longest `Last-Event-ID` accepted; a cursor is short and opaque.
const MAX_CURSOR_CHARS: usize = 256;

/// The stream route, for `ServiceParts::extra`.
pub fn routes(bound: BoundApplication) -> Router {
    Router::new()
        .route(EVENTS_PATH, get(stream_events))
        .with_state(bound)
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
    State(bound): State<BoundApplication>,
    principal: Option<Extension<Principal>>,
    session: Option<Extension<SessionId>>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(Extension(principal)) = principal else {
        return error_response(ApiError::new(
            ErrorCode::Unauthenticated,
            "Authentication is required",
        ));
    };
    let session_id = session.map(|Extension(SessionId(id))| id);
    let after = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .filter(|cursor| !cursor.is_empty() && cursor.chars().count() <= MAX_CURSOR_CHARS)
        .map(str::to_owned);
    let first = match poll(
        &bound,
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
        bound,
        principal,
        session_id,
        workspace_id,
        after,
        pending: first.events.into(),
        waited: false,
        ended: false,
    };
    let events = stream::unfold(state, next_event);
    Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response()
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
        if !state.waited {
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        state.waited = false;
        match poll(
            &state.bound,
            &state.principal,
            state.session_id.as_deref(),
            &state.workspace_id,
            state.after.as_deref(),
        )
        .await
        {
            Ok(page) => state.pending = page.events.into(),
            Err(_) => state.ended = true,
        }
    }
}
