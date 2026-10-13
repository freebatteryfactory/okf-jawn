//! The one request path of an operation command: JSON in, one `POST`, the response body out.
//!
//! The input is read inline, from a file (`@path`) or from standard input (`-`), within the
//! contract's request-body limit, and is sent as it was given once it is known to be JSON.
//! A success body is copied to the output as it arrives, followed by one line feed when it does
//! not end with one; nothing else is written there. A refusal is returned as the service's own
//! `ApiError`; an answer that carries none is given the code its HTTP status stands for.

use std::io::Read;
use std::time::Duration;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::metadata::OperationInfo;
use okf_jawn_contract::transport::MAX_REQUEST_BODY_BYTES;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::credential::Credential;
use crate::endpoint::Endpoint;

/// How long one operation may take, connection and answer together.
const OPERATION_TIMEOUT: Duration = Duration::from_secs(120);

/// Run one operation and copy its response body to `output`.
///
/// # Errors
/// Returns the service's `ApiError` for a refusal, `Unavailable` when the service cannot be
/// reached or answers with a redirect, and `Internal` when the output cannot be written.
pub(crate) async fn call<O: AsyncWrite + Unpin>(
    operation: &OperationInfo,
    endpoint: &Endpoint,
    credential: Option<&Credential>,
    input: Vec<u8>,
    output: &mut O,
) -> Result<(), ApiError> {
    let client = endpoint.client(Some(OPERATION_TIMEOUT))?;
    let mut request = client
        .post(endpoint.url(operation.path))
        .header(CONTENT_TYPE, "application/json")
        .body(input);
    if let Some(credential) = credential {
        request = request.header(AUTHORIZATION, credential.bearer()?);
    }
    let mut response = request.send().await.map_err(|error| unanswered(&error))?;
    let status = response.status();
    if status.is_redirection() {
        return Err(ApiError::new(
            ErrorCode::Unavailable,
            format!(
                "The service answered with a redirect (HTTP {}), which is never followed",
                status.as_u16()
            ),
        ));
    }
    if !status.is_success() {
        let body = error_body(&mut response).await;
        return Err(
            serde_json::from_slice::<ApiError>(&body).unwrap_or_else(|_| {
                ApiError::new(
                    code_of(status.as_u16()),
                    format!(
                        "The service answered HTTP {} without an error it could explain",
                        status.as_u16()
                    ),
                )
            }),
        );
    }
    let mut last = None;
    while let Some(chunk) = response.chunk().await.map_err(|error| unanswered(&error))? {
        output
            .write_all(&chunk)
            .await
            .map_err(|error| unwritable(&error))?;
        last = chunk.last().copied().or(last);
    }
    if last.is_some_and(|byte| byte != b'\n') {
        output
            .write_all(b"\n")
            .await
            .map_err(|error| unwritable(&error))?;
    }
    output.flush().await.map_err(|error| unwritable(&error))
}

/// Read an operation's JSON input: inline text, `@path` for a file, or `-` for `stdin`.
///
/// # Errors
/// Returns `InvalidInput` when the source cannot be read or is not JSON, and `TooLarge` when
/// it is longer than the contract's request-body limit.
pub(crate) async fn input<I: AsyncRead + Unpin>(
    argument: &str,
    stdin: &mut I,
) -> Result<Vec<u8>, ApiError> {
    let limit = u64::try_from(MAX_REQUEST_BODY_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut bytes = Vec::new();
    if argument == "-" {
        stdin
            .take(limit)
            .read_to_end(&mut bytes)
            .await
            .map_err(|error| unreadable("standard input", &error))?;
    } else if let Some(path) = argument.strip_prefix('@') {
        std::fs::File::open(path)
            .and_then(|file| file.take(limit).read_to_end(&mut bytes))
            .map_err(|error| unreadable(path, &error))?;
    } else {
        bytes.extend_from_slice(argument.as_bytes());
    }
    if bytes.len() > MAX_REQUEST_BODY_BYTES {
        return Err(ApiError::new(
            ErrorCode::TooLarge,
            format!(
                "The JSON input exceeds the {} MiB request limit",
                MAX_REQUEST_BODY_BYTES / 1_048_576
            ),
        )
        .with_field("json"));
    }
    serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|error| {
        ApiError::new(
            ErrorCode::InvalidInput,
            format!("The JSON input is not JSON: {error}"),
        )
        .with_field("json")
    })?;
    Ok(bytes)
}

/// The code an HTTP status stands for when the answer carries no `ApiError`: the reverse of the
/// service's own status mapping.
fn code_of(status: u16) -> ErrorCode {
    match status {
        400 => ErrorCode::InvalidInput,
        401 => ErrorCode::Unauthenticated,
        403 => ErrorCode::Forbidden,
        404 => ErrorCode::NotFound,
        409 => ErrorCode::Conflict,
        413 => ErrorCode::TooLarge,
        422 => ErrorCode::Unsupported,
        501 => ErrorCode::NotImplemented,
        502..=504 => ErrorCode::Unavailable,
        _ => ErrorCode::Internal,
    }
}

/// The body of a refusal, up to the request-body limit; a longer or broken body reads as none,
/// and the refusal is then reported by its status.
async fn error_body(response: &mut reqwest::Response) -> Vec<u8> {
    let mut body = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        if body.len().saturating_add(chunk.len()) > MAX_REQUEST_BODY_BYTES {
            return Vec::new();
        }
        body.extend_from_slice(&chunk);
    }
    body
}

fn unanswered(error: &reqwest::Error) -> ApiError {
    ApiError::new(
        ErrorCode::Unavailable,
        format!(
            "The service could not be reached, or its answer was cut off: {}",
            chain(error)
        ),
    )
}

fn unreadable(source: &str, error: &std::io::Error) -> ApiError {
    ApiError::new(
        ErrorCode::InvalidInput,
        format!("The JSON input {source} cannot be read: {error}"),
    )
    .with_field("json")
}

fn unwritable(error: &std::io::Error) -> ApiError {
    ApiError::new(
        ErrorCode::Internal,
        format!("The response could not be written to standard output: {error}"),
    )
}

/// An error and its causes in one line. A `reqwest` error names the URL it was sent to and
/// never a header, and an `Endpoint` URL holds no credentials.
pub(crate) fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}
