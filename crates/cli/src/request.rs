//! The one request path of an operation command: JSON in, one `POST`, the response body out.
//!
//! The input is read inline, from a file (`@path`) or from standard input (`-`), within the
//! contract's request-body limit, and is sent as it was given once it is known to be JSON.
//! A success body is copied to the output as it arrives, followed by one line feed when it does
//! not end with one; nothing else is written there. A refusal is returned with the service's
//! own code and message; an answer that carries none is given the code its HTTP status stands
//! for.
//!
//! The service's refusal is read member by member, not through the contract's `ApiError`,
//! which refuses a member it does not know: a newer service may add a member, a detail kind or
//! a code, and its code and message must still reach the user. A detail of a kind this CLI
//! has no type for is kept as the JSON the service sent; a code it does not know is named in
//! the message, and the HTTP status then decides the code.

use std::io::Read;
use std::time::Duration;

use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::metadata::OperationInfo;
use okf_jawn_contract::transport::MAX_REQUEST_BODY_BYTES;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::credential::Credential;
use crate::endpoint::Endpoint;

/// A failure of an operation: the error, and what of the service's refusal the contract's
/// error type cannot hold.
#[derive(Debug)]
pub(crate) struct Failure {
    /// The error, with the service's code and message when it gave them.
    pub(crate) error: ApiError,
    /// The service's `detail` as JSON, when it is of a kind this CLI has no type for.
    pub(crate) detail: Option<String>,
}

/// How long one operation may take, connection and answer together.
const OPERATION_TIMEOUT: Duration = Duration::from_secs(120);

impl From<ApiError> for Failure {
    fn from(error: ApiError) -> Self {
        Self {
            error,
            detail: None,
        }
    }
}

/// Run one operation and copy its response body to `output`.
///
/// # Errors
/// Returns the service's own error for a refusal, `Unavailable` when the service cannot be
/// reached or answers with a redirect, and `Internal` when the output cannot be written.
pub(crate) async fn call<O: AsyncWrite + Unpin>(
    operation: &OperationInfo,
    endpoint: &Endpoint,
    credential: Option<&Credential>,
    input: Vec<u8>,
    output: &mut O,
) -> Result<(), Failure> {
    call_within(
        operation,
        endpoint,
        credential,
        input,
        output,
        OPERATION_TIMEOUT,
    )
    .await
}

/// [`call`], with the time the whole exchange may take, connection and answer together.
async fn call_within<O: AsyncWrite + Unpin>(
    operation: &OperationInfo,
    endpoint: &Endpoint,
    credential: Option<&Credential>,
    input: Vec<u8>,
    output: &mut O,
    limit: Duration,
) -> Result<(), Failure> {
    let client = endpoint.client(Some(limit))?;
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
        )
        .into());
    }
    if !status.is_success() {
        let body = error_body(&mut response).await;
        return Err(explained(status.as_u16(), &body).unwrap_or_else(|| {
            ApiError::new(
                code_of(status.as_u16()),
                format!(
                    "The service answered HTTP {} without an error it could explain",
                    status.as_u16()
                ),
            )
            .into()
        }));
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
    Ok(output.flush().await.map_err(|error| unwritable(&error))?)
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

/// The service's refusal, read member by member. `None` when the body is not a JSON object
/// with a `message`: the refusal is then reported by its status.
fn explained(status: u16, body: &[u8]) -> Option<Failure> {
    let refusal = serde_json::from_slice::<Value>(body).ok()?;
    let text = |name: &str| refusal.get(name).and_then(Value::as_str).map(str::to_owned);
    let mut message = text("message")?;
    let known = refusal
        .get("code")
        .and_then(|code| serde_json::from_value::<ErrorCode>(code.clone()).ok());
    let code = known.unwrap_or_else(|| {
        // A code this CLI does not know is named in the message; the status decides the code.
        if let Some(named) = text("code") {
            message = format!("{message} (code: {named})");
        }
        code_of(status)
    });
    let (detail, unread) = match refusal.get("detail") {
        None | Some(Value::Null) => (None, None),
        Some(sent) => match serde_json::from_value::<ErrorDetail>(sent.clone()) {
            Ok(typed) => (Some(Box::new(typed)), None),
            Err(_) => (None, Some(sent.to_string())),
        },
    };
    Some(Failure {
        error: ApiError {
            code,
            message,
            field: text("field"),
            request_id: text("request_id"),
            detail,
        },
        detail: unread,
    })
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use okf_jawn_contract::error::ErrorCode;
    use okf_jawn_contract::metadata::operations;

    use super::{OPERATION_TIMEOUT, call_within};
    use crate::endpoint::Endpoint;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn err_of<T: std::fmt::Debug, E>(result: Result<T, E>) -> Result<E, String> {
        match result {
            Ok(value) => Err(format!("expected an error, got Ok({value:?})")),
            Err(error) => Ok(error),
        }
    }

    fn some<T>(value: Option<T>, what: &str) -> Result<T, String> {
        value.ok_or_else(|| format!("expected {what} to be present"))
    }

    /// One operation has a limit in time, connection and answer together (the review's
    /// R11): a service that accepts the connection and never answers ends the operation at
    /// the limit, with `unavailable`. The limit of the shipped CLI is 120 seconds; the test
    /// passes a short one through the same path.
    #[tokio::test]
    async fn an_operation_that_is_never_answered_ends_at_its_time_limit() -> TestResult {
        assert_eq!(OPERATION_TIMEOUT, Duration::from_secs(120));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = Endpoint::parse(&format!("http://{}", listener.local_addr()?))?;
        let holding = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });
        let operation = some(operations().into_iter().next(), "an operation")?;
        let limit = Duration::from_millis(300);
        let mut output = Vec::new();
        let started = Instant::now();
        let ended = tokio::time::timeout(
            Duration::from_secs(20),
            call_within(
                &operation,
                &endpoint,
                None,
                b"{}".to_vec(),
                &mut output,
                limit,
            ),
        )
        .await;
        holding.abort();
        let failure = err_of(ended.map_err(|_| "the operation was still waiting after 20 s")?)?;
        assert!(
            matches!(failure.error.code, ErrorCode::Unavailable),
            "{}",
            failure.error.message
        );
        assert!(started.elapsed() >= limit, "it ended before its limit");
        assert_eq!(output, b"");
        Ok(())
    }
}
