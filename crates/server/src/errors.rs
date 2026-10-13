//! Every refusal the HTTP service writes is an `ApiError` JSON body whose status follows its code.
//!
//! An unknown path and a method no route accepts are answered here, never with axum's empty
//! default. A body that cannot be read as JSON is described in fixed words: the message never
//! quotes the client's bytes, so a megabyte of malformed input cannot come back in its own
//! refusal or travel into logs. A wrong method is `not_found` (404): the published contract
//! declares no 405, and every status this service sends follows its `ErrorCode`. No refusal
//! quotes the request's method either, and a path or query the framework cannot read is refused
//! through `Readable`, as JSON, like everything else.

use axum::extract::FromRequestParts;
use axum::extract::rejection::JsonRejection;
use axum::http::header::RETRY_AFTER;
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};

/// An extractor whose rejection is an `ApiError` JSON body, never the framework's plain text.
///
/// Every route that reads a path or a query takes it through this wrapper, so a parameter that
/// cannot be read (bytes that are not UTF-8 after percent-decoding, say) is `invalid_input` in
/// the same shape as every other refusal, in fixed words that quote none of the request.
pub struct Readable<E>(pub E);

impl<S, E> FromRequestParts<S> for Readable<E>
where
    S: Send + Sync,
    E: FromRequestParts<S>,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        E::from_request_parts(parts, state)
            .await
            .map(Self)
            .map_err(|_| {
                error_response(ApiError::new(
                    ErrorCode::InvalidInput,
                    "A path or query parameter of the request could not be read",
                ))
            })
    }
}

/// The status an `ErrorCode` is sent with.
#[must_use]
pub const fn status_of(code: &ErrorCode) -> StatusCode {
    match code {
        ErrorCode::InvalidInput => StatusCode::BAD_REQUEST,
        ErrorCode::Unauthenticated => StatusCode::UNAUTHORIZED,
        ErrorCode::Forbidden => StatusCode::FORBIDDEN,
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Conflict
        | ErrorCode::AlreadyIssued
        | ErrorCode::InProgress
        | ErrorCode::Cancelled => StatusCode::CONFLICT,
        ErrorCode::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::Unsupported => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::NotImplemented => StatusCode::NOT_IMPLEMENTED,
        ErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Render `error` as its JSON body, with `Retry-After` for an in-progress refusal.
#[must_use]
pub fn error_response(error: ApiError) -> Response {
    let status = status_of(&error.code);
    let wait = retry_after(&error);
    let mut response = (status, Json(error)).into_response();
    if let Some(value) = wait {
        response.headers_mut().insert(RETRY_AFTER, value);
    }
    response
}

/// Render `error` as its JSON body with `Retry-After: seconds`: the refusal of a request that
/// cannot be judged yet and may be sent again after that long.
#[must_use]
pub fn retry_response(error: ApiError, seconds: u32) -> Response {
    let mut response = error_response(error);
    response
        .headers_mut()
        .insert(RETRY_AFTER, HeaderValue::from(seconds));
    response
}

/// Describe a body that could not be read as JSON, in fixed words that quote none of it.
///
/// An over-limit body is `too_large` (413). Everything else, including a missing or wrong
/// `Content-Type` (which axum would answer 415), is `invalid_input` (400): the API accepts only
/// JSON, and 415 is not among the statuses the published contract declares.
#[must_use]
pub fn body_rejection(rejection: &JsonRejection, limit: usize) -> ApiError {
    match rejection {
        JsonRejection::MissingJsonContentType(_) => ApiError::new(
            ErrorCode::InvalidInput,
            "The request body must be JSON sent with `Content-Type: application/json`",
        ),
        JsonRejection::JsonSyntaxError(_) => ApiError::new(
            ErrorCode::InvalidInput,
            "The request body is not valid JSON",
        ),
        JsonRejection::JsonDataError(_) => ApiError::new(
            ErrorCode::InvalidInput,
            "The request body is JSON of the wrong shape",
        ),
        other if other.status() == StatusCode::PAYLOAD_TOO_LARGE => ApiError::new(
            ErrorCode::TooLarge,
            format!("The request body exceeds the {limit}-byte limit"),
        ),
        _ => ApiError::new(
            ErrorCode::InvalidInput,
            "The request body could not be read",
        ),
    }
}

/// `router` with the two fallbacks: an unknown path and a method no route of a known path
/// accepts are `not_found` JSON.
///
/// Call it once, after every router of the service is merged: the method fallback attaches to
/// the routes registered so far.
pub fn with_fallbacks(router: Router) -> Router {
    router
        .fallback(unknown_path)
        .method_not_allowed_fallback(wrong_method)
}

/// The answer to a path no route serves.
pub async fn unknown_path() -> Response {
    error_response(ApiError::new(
        ErrorCode::NotFound,
        "No route serves this path",
    ))
}

/// The answer to a method the path's routes do not accept. It does not quote the method: a
/// method is the client's text, of any length the connection allows.
pub async fn wrong_method() -> Response {
    error_response(ApiError::new(
        ErrorCode::NotFound,
        "No route accepts this method on this path",
    ))
}

/// The `Retry-After` value for an in-progress refusal, taken from its typed detail.
fn retry_after(error: &ApiError) -> Option<HeaderValue> {
    let ErrorDetail::InProgress {
        retry_after: seconds,
        ..
    } = error.detail.as_deref()?
    else {
        return None;
    };
    Some(HeaderValue::from(*seconds))
}
