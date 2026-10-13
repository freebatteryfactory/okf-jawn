//! An answer is passed on as the service wrote it (the review's note N7): no value is read and
//! written again. The relay checks that an answer is one JSON value and a JSON-RPC message,
//! removes the line breaks between its tokens, and changes no other byte. A refusal that
//! lacks the request's id gets it put into its text, and an id is written back as the client
//! wrote it.

use okf_jawn_cli::relay::Body;
use serde_json::{Value, json};

use super::request;
use crate::check::{TestResult, some};
use crate::support::doubles::{FakeTransport, Script, Setup, broken, relay, status};

/// A `\u` escape of the UTF-16 code unit `code`, four hexadecimal digits.
fn escape(code: &str) -> String {
    format!("{}u{code}", '\\')
}

/// Run one request with id 1 against a service that answers with `body` and `status_code`.
async fn answered_with(
    status_code: u16,
    body: &str,
) -> crate::support::Outcome<crate::support::Ran> {
    let body = body.to_owned();
    let transport = FakeTransport::scripted(move |_| status(status_code, Body::Json(body.clone())));
    relay(
        Setup::new(&transport),
        request(&json!(1), "tools/call", &json!({})).as_bytes(),
    )
    .await
}

/// The review's `o32` cases: each comes out as it went in, byte for byte. Reading a value and
/// writing it again would turn the long integers into floats, `-0` into `-0.0`, `1E2` into
/// `100.0`, keep one of two equal keys, reorder the members, and drop the answers with a
/// number no float holds or with a lone surrogate.
#[tokio::test]
async fn an_answer_is_passed_on_byte_for_byte() -> TestResult {
    let raws = [
        r#"{"jsonrpc":"2.0","id":1,"result":{"n":12345678901234567890123}}"#.to_owned(),
        r#"{"jsonrpc":"2.0","id":1,"result":{"n":18446744073709551616,"m":-9223372036854775809,"u":18446744073709551615,"f":1.0,"g":0.1e1,"h":1E2,"z":-0,"p":3.141592653589793238462643383279}}"#.to_owned(),
        r#"{"jsonrpc":"2.0","id":1,"result":{"n":1e400}}"#.to_owned(),
        r#"{"jsonrpc":"2.0","id":1,"result":{"ok":false,"ok":true}}"#.to_owned(),
        r#"{"jsonrpc":"2.0","id":2,"result":{"first":true},"id":1,"result":{"second":true}}"#.to_owned(),
        r#"{"jsonrpc":"2.0","id":1,"result":{"isError":true,"content":[],"isError":false}}"#.to_owned(),
        r#"{"jsonrpc":"2.0","id":1,"error":{"code":-1,"message":"refused"},"result":{"ok":true}}"#.to_owned(),
        format!(r#"{{"jsonrpc":"2.0","id":1,"result":{{"s":"{}"}}}}"#, escape("d800")),
        format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":{{"s":"{}{} {} {} {} \/ \n"}}}}"#,
            escape("d83d"),
            escape("de00"),
            escape("0000"),
            escape("2028"),
            escape("007F")
        ),
        r#"{"jsonrpc":"2.0","result":{"z":1,"a":2},"id":1}"#.to_owned(),
        r#" { "jsonrpc" : "2.0" , "id" : 1 , "result" : [ ] } "#.to_owned(),
    ];
    for raw in raws {
        let ran = answered_with(200, &raw).await?;
        assert_eq!(
            (ran.stdout.as_str(), ran.stderr.as_str(), ran.code),
            (format!("{raw}\n").as_str(), "", 0)
        );
        // The same text as one event of a stream.
        let event = raw.clone();
        let transport = FakeTransport::scripted(move |_| Script {
            events: vec![event.clone()],
            ..status(200, Body::Events)
        });
        let streamed = relay(
            Setup::new(&transport),
            request(&json!(1), "tools/call", &json!({})).as_bytes(),
        )
        .await?;
        assert_eq!(
            (streamed.stdout.as_str(), streamed.stderr.as_str()),
            (format!("{raw}\n").as_str(), "")
        );
    }
    Ok(())
}

/// A body printed over several lines becomes one line: the line feeds and carriage returns
/// between its tokens go, and nothing else does. A line feed a string holds as an escape is
/// part of the string and stays.
#[tokio::test]
async fn a_pretty_printed_answer_becomes_one_line() -> TestResult {
    let pretty = "{\r\n  \"jsonrpc\": \"2.0\",\n  \"id\": 1,\n  \"result\": {\n    \"text\": \"first\\nsecond\\r\\n\",\n\t\"n\": [1,\n 2]\n  }\n}\n";
    let ran = answered_with(200, pretty).await?;
    assert_eq!(
        ran.stdout,
        "{  \"jsonrpc\": \"2.0\",  \"id\": 1,  \"result\": {    \"text\": \"first\\nsecond\\r\\n\",\t\"n\": [1, 2]  }}\n"
    );
    assert_eq!(
        some(ran.lines()?.first(), "the answer")?.pointer("/result/text"),
        Some(&json!("first\nsecond\r\n"))
    );
    assert_eq!(ran.stderr, "");
    Ok(())
}

/// What is not one JSON value, or not a JSON-RPC message, is never written: the request gets
/// the relay's own error with its id, once.
#[tokio::test]
async fn an_answer_that_is_not_json_is_an_error_for_its_request() -> TestResult {
    for body in [
        "not json",
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"a raw\nline feed in a string\"}",
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}} {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}",
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}x",
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":01}",
        "[{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}]",
        "{\"jsonrpc\":\"1.0\",\"id\":1,\"result\":{}}",
        "{\"id\":1,\"result\":{}}",
    ] {
        let ran = answered_with(200, body).await?;
        let lines = ran.lines()?;
        assert_eq!(lines.len(), 1, "{body}: {}", ran.stdout);
        let only = some(lines.first(), "an answer")?;
        assert_eq!(
            (only.get("id"), only.pointer("/error/code")),
            (Some(&json!(1)), Some(&json!(-32603))),
            "{body}"
        );
        assert!(
            ran.stdout.contains("not a JSON-RPC message"),
            "{}",
            ran.stdout
        );
    }
    Ok(())
}

/// A refusal the service sent without the request's id, or with `null` for it, is the
/// request's answer: the id is put into the text and no other byte changes, a number no float
/// holds and the order of the members included. A refusal that names another id is passed on
/// as it is, and the request still gets its own error.
#[tokio::test]
async fn a_refusal_gets_the_request_id_put_into_its_text() -> TestResult {
    let cases = [
        (
            r#"{"jsonrpc":"2.0","error":{"code":-32020,"message":"m","data":1e400}}"#,
            r#"{"id":1,"jsonrpc":"2.0","error":{"code":-32020,"message":"m","data":1e400}}"#,
        ),
        (
            r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32020,"message":"id: null"}}"#,
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32020,"message":"id: null"}}"#,
        ),
        (
            "\n {\n\"error\" : {\"code\":-1,\"message\":\"m\"},\r\n\"id\" :  null ,\"jsonrpc\":\"2.0\"}\n",
            " {\"error\" : {\"code\":-1,\"message\":\"m\"},\"id\" :  1 ,\"jsonrpc\":\"2.0\"}",
        ),
        (
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"m","data":{"n":12345678901234567890123}}}"#,
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"m","data":{"n":12345678901234567890123}}}"#,
        ),
    ];
    for (body, written) in cases {
        let ran = answered_with(400, body).await?;
        assert_eq!(
            (ran.stdout.as_str(), ran.stderr.as_str()),
            (format!("{written}\n").as_str(), ""),
            "{body}"
        );
    }
    let other = r#"{"jsonrpc":"2.0","id":99,"error":{"code":-32602,"message":"other"}}"#;
    let ran = answered_with(400, other).await?;
    let (first, second) = some(ran.stdout.trim_end().split_once('\n'), "two lines")?;
    assert_eq!(first, other);
    let own = serde_json::from_str::<Value>(second)?;
    assert_eq!(
        (own.get("id"), own.pointer("/error/code")),
        (Some(&json!(1)), Some(&json!(-32603)))
    );
    Ok(())
}

/// The relay's own error carries the request's id as the client wrote it: an integer beyond
/// 64 bits, `1.0` and a string with an escape are not read and spelled again.
#[tokio::test]
async fn a_request_id_is_written_back_as_the_client_wrote_it() -> TestResult {
    for id in [
        "12345678901234567890123",
        "1.0",
        "-0",
        "1E2",
        r#""a\/b""#,
        "7",
    ] {
        let transport = FakeTransport::scripted(|_| broken("gone"));
        let sent = format!("{{\"jsonrpc\":\"2.0\",  \"id\" : {id} ,\"method\":\"ping\"}}\n");
        let ran = relay(Setup::new(&transport), sent.as_bytes()).await?;
        let start = format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"error\":{{\"code\":-32603,");
        assert!(ran.stdout.starts_with(&start), "{id}: {}", ran.stdout);
        assert_eq!(ran.lines()?.len(), 1);
        // A line the relay refuses itself names the id the same way.
        let unsent = format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":7}}\n");
        let refused = relay(Setup::new(&transport), unsent.as_bytes()).await?;
        let start = format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"error\":{{\"code\":-32600,");
        assert!(
            refused.stdout.starts_with(&start),
            "{id}: {}",
            refused.stdout
        );
    }
    Ok(())
}
