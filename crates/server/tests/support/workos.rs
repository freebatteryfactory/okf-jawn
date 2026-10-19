//! A scripted WorkOS transport: the real `workos::Client` sends its requests here instead of to
//! WorkOS, so the production calls, their paths, queries and bodies, and the decoding of the
//! 3.3.0 model shapes all run in a test with no network.
//!
//! Each request takes the next scripted reply in order and is recorded. A request with no reply
//! left is answered `599` and recorded as unscripted, so a test that checks `unscripted()` finds
//! it: the script is never more forgiving than WorkOS (it answers no request it was not given).

use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use serde_json::Value;
use workos::transport::{HttpRequest, HttpResponse, HttpTransport, TransportError};

/// One scripted answer.
#[derive(Debug, Clone)]
pub enum Reply {
    /// An HTTP answer with a JSON body.
    Json(u16, Value),
    /// A transport failure (a refused connection).
    Down,
    /// No answer, ever.
    Hang,
}

/// One request the client sent.
#[derive(Debug, Clone)]
pub struct Seen {
    /// The method.
    pub method: Method,
    /// The path, without the query.
    pub path: String,
    /// The query's pairs, decoded.
    pub query: BTreeMap<String, String>,
    /// The JSON body, when there was one.
    pub body: Option<Value>,
    /// The `Authorization` header, as sent.
    pub authorization: Option<String>,
}

/// The scripted transport.
#[derive(Debug, Default)]
pub struct Scripted {
    replies: Mutex<VecDeque<Reply>>,
    seen: Mutex<Vec<Seen>>,
    unscripted: Mutex<Vec<String>>,
}

/// A transport that never answers is held this long before it is dropped by the caller's bound.
const HANG: Duration = Duration::from_secs(3600);

impl Scripted {
    /// A transport that answers `replies` in order.
    pub fn new(replies: Vec<Reply>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            ..Self::default()
        })
    }

    /// Every request sent, in order.
    pub fn seen(&self) -> Vec<Seen> {
        self.seen
            .lock()
            .map(|seen| seen.clone())
            .unwrap_or_default()
    }

    /// The URLs of requests that found no reply left.
    pub fn unscripted(&self) -> Vec<String> {
        self.unscripted.lock().map_or_else(
            |_| vec!["the unscripted list is poisoned".to_owned()],
            |unscripted| unscripted.clone(),
        )
    }

    /// How many scripted replies were not used.
    pub fn left(&self) -> usize {
        self.replies
            .lock()
            .map_or(usize::MAX, |replies| replies.len())
    }

    fn record(&self, request: &HttpRequest) -> Option<Reply> {
        let (path, query) = split(&request.url);
        let body = request
            .body
            .as_ref()
            .and_then(|body| serde_json::from_slice(body).ok());
        let authorization = request
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(Seen {
                method: request.method.clone(),
                path,
                query,
                body,
                authorization,
            });
        }
        let reply = self
            .replies
            .lock()
            .ok()
            .and_then(|mut replies| replies.pop_front());
        if reply.is_none()
            && let Ok(mut unscripted) = self.unscripted.lock()
        {
            unscripted.push(request.url.clone());
        }
        reply
    }
}

impl HttpTransport for Scripted {
    fn execute<'a, 'b>(
        &'a self,
        request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'b>>
    where
        'a: 'b,
        Self: 'b,
    {
        let reply = self.record(&request);
        Box::pin(async move {
            match reply {
                Some(Reply::Json(status, body)) => Ok(json(status, &body)),
                Some(Reply::Down) => Err(TransportError::connect("connection refused")),
                Some(Reply::Hang) => {
                    tokio::time::sleep(HANG).await;
                    Err(TransportError::timeout("held"))
                }
                None => Ok(json(599, &serde_json::json!({ "message": "unscripted" }))),
            }
        })
    }
}

/// An HTTP answer of `status` carrying `body` as JSON.
fn json(status: u16, body: &Value) -> HttpResponse {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    HttpResponse {
        status: StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        headers,
        body: Bytes::from(serde_json::to_vec(body).unwrap_or_default()),
    }
}

/// The path of `url` after the API base, and its decoded query.
fn split(url: &str) -> (String, BTreeMap<String, String>) {
    match url::Url::parse(url) {
        Ok(parsed) => (
            parsed.path().to_owned(),
            parsed
                .query_pairs()
                .map(|(name, value)| (name.into_owned(), value.into_owned()))
                .collect(),
        ),
        Err(_) => (url.to_owned(), BTreeMap::new()),
    }
}

/// A WorkOS list response: `data` and an `after` cursor when more pages follow.
pub fn list(data: &[Value], after: Option<&str>) -> Value {
    serde_json::json!({
        "object": "list",
        "data": data,
        "list_metadata": { "before": null, "after": after },
        "an_unknown_field": true,
    })
}

/// An organization membership of `user` in `organization` with `roles`, as the 3.3.0 model
/// `UserOrganizationMembership` reads it.
pub fn membership(id: &str, user: &str, organization: &str, roles: &[&str]) -> Value {
    let first = roles.first().copied().unwrap_or("member");
    serde_json::json!({
        "object": "organization_membership",
        "id": id,
        "user_id": user,
        "organization_id": organization,
        "status": "active",
        "directory_managed": false,
        "created_at": "2026-10-18T00:00:00.000Z",
        "updated_at": "2026-10-18T00:00:00.000Z",
        "role": { "slug": first },
        "roles": roles.iter().map(|slug| serde_json::json!({ "slug": slug })).collect::<Vec<_>>(),
        "user": user_value(user),
        "an_unknown_field": 1,
    })
}

/// A user, as the 3.3.0 model `User` reads it.
pub fn user_value(id: &str) -> Value {
    serde_json::json!({
        "object": "user",
        "id": id,
        "email": format!("{id}@example.test"),
        "email_verified": true,
        "first_name": null,
        "last_name": null,
        "profile_picture_url": null,
        "last_sign_in_at": null,
        "external_id": null,
        "metadata": {},
        "locale": null,
        "created_at": "2026-10-18T00:00:00.000Z",
        "updated_at": "2026-10-18T00:00:00.000Z",
    })
}

/// A permission, as the 3.3.0 model `AuthorizationPermission` reads it.
pub fn permission(slug: &str) -> Value {
    serde_json::json!({
        "object": "permission",
        "id": format!("perm_{slug}"),
        "slug": slug,
        "name": slug,
        "description": null,
        "system": false,
        "resource_type_slug": "workspace",
        "created_at": "2026-10-18T00:00:00.000Z",
        "updated_at": "2026-10-18T00:00:00.000Z",
    })
}

/// A resource, as the 3.3.0 model `AuthorizationResource` reads it.
pub fn resource(external_id: &str, organization: &str) -> Value {
    serde_json::json!({
        "object": "authorization_resource",
        "id": format!("res_{external_id}"),
        "name": external_id,
        "description": null,
        "organization_id": organization,
        "parent_resource_id": null,
        "external_id": external_id,
        "resource_type_slug": "workspace",
        "created_at": "2026-10-18T00:00:00.000Z",
        "updated_at": "2026-10-18T00:00:00.000Z",
    })
}

/// A role assignment of `role` on the workspace `external_id`, as `UserRoleAssignment` reads it.
pub fn assignment(membership_id: &str, role: &str, external_id: &str) -> Value {
    serde_json::json!({
        "object": "role_assignment",
        "id": format!("ra_{role}_{external_id}"),
        "organization_membership_id": membership_id,
        "role": { "slug": role },
        "resource": {
            "id": format!("res_{external_id}"),
            "external_id": external_id,
            "resource_type_slug": "workspace",
        },
        "source": { "type": "direct" },
        "created_at": "2026-10-18T00:00:00.000Z",
        "updated_at": "2026-10-18T00:00:00.000Z",
    })
}

/// A membership as `list_memberships_for_resource_by_external_id` lists it.
pub fn resource_member(id: &str, user: &str, organization: &str, status: &str) -> Value {
    serde_json::json!({
        "object": "organization_membership",
        "id": id,
        "user_id": user,
        "organization_id": organization,
        "status": status,
        "directory_managed": false,
        "created_at": "2026-10-18T00:00:00.000Z",
        "updated_at": "2026-10-18T00:00:00.000Z",
        "user": user_value(user),
    })
}

/// A WorkOS error answer of `status`.
pub fn error(status: u16, message: &str) -> Reply {
    Reply::Json(
        status,
        serde_json::json!({ "code": "an_error", "message": message }),
    )
}
