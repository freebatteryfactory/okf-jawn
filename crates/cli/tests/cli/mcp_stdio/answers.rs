//! An answer is passed on as the service wrote it (the review's note N7): no value is read and
//! written again. The relay checks that an answer is one JSON value and a JSON-RPC message,
//! removes the line breaks between its tokens, and changes no other byte. A refusal that
//! lacks the request's id gets it put into its text, and an id is written back as the client
//! wrote it.

use serde_json::{Value, json};

use super::{notification, request, response};
use crate::check::{TestResult, some};
use okf_jawn_cli::relay::Limits;

use crate::support::doubles::{
    Body, FakeTransport, Reply, Script, Setup, answered, broken, header, relay, status,
};
use crate::support::http::{Answering, answer, json as json_answer};
use crate::support::{TOKEN, cli};

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

/// One request with the id `id`, as written, against a service that answers with `body`.
async fn asked_as(id: &str, body: &str) -> crate::support::Outcome<crate::support::Ran> {
    let body = body.to_owned();
    let transport = FakeTransport::scripted(move |_| status(200, Body::Json(body.clone())));
    let sent = format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"ping\"}}\n");
    relay(Setup::new(&transport), sent.as_bytes()).await
}

/// An id is matched exactly (the re-review's note N8). A number is the text the client
/// wrote and is never read into a number: two long integers that differ in their last digit
/// are two ids, and `1.0` is not `1`. A string is the characters it spells, whatever escape
/// spells them. An answer that is not the request's is passed on, and the request still gets
/// one error of its own.
#[tokio::test]
async fn an_id_is_matched_exactly_as_the_client_wrote_it() -> TestResult {
    let answer = |id: &str| format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{}}}}");
    for (asked, answered, is_the_answer) in [
        ("12345678901234567890123", "12345678901234567890123", true),
        ("12345678901234567890123", "12345678901234567890124", false),
        ("9007199254740993", "9007199254740992", false),
        ("1", "1", true),
        ("1", "1.0", false),
        ("1.0", "1.0", true),
        ("1.0", "1", false),
        ("1", "\"1\"", false),
        ("\"1\"", "1", false),
        ("\"a\\/b\"", "\"a/b\"", true),
        ("\"a/b\"", "\"\\u0061\\/b\"", true),
        ("\"a/b\"", "\"a/c\"", false),
        ("\"a/b\"", "\"a/b \"", false),
        ("\"\"", "\"\"", true),
    ] {
        let ran = asked_as(asked, &answer(answered)).await?;
        let passed_on = format!("{}\n", answer(answered));
        if is_the_answer {
            assert_eq!(ran.stdout, passed_on, "{asked} answered by {answered}");
        } else {
            let own = some(ran.stdout.strip_prefix(&passed_on), "the answer passed on")?;
            let start =
                format!("{{\"jsonrpc\":\"2.0\",\"id\":{asked},\"error\":{{\"code\":-32603,");
            assert!(
                own.starts_with(&start),
                "{asked} answered by {answered}: {own}"
            );
            assert_eq!(ran.lines()?.len(), 2);
        }
    }
    Ok(())
}

/// The protocol version an `initialize` answer names travels in a header of every later
/// message, so it is bounded: 256 bytes as the service wrote it are put in force, and one
/// byte more is not.
#[tokio::test]
async fn a_protocol_version_is_put_in_force_up_to_its_bound() -> TestResult {
    for (length, in_force) in [(254_usize, true), (255, false)] {
        let version = "v".repeat(length);
        let named = version.clone();
        let transport = FakeTransport::scripted(move |message| {
            let id = message.get("id").unwrap_or(&Value::Null);
            match message.get("method").and_then(Value::as_str) {
                Some("initialize") => answered(&response(id, &json!({"protocolVersion": named}))),
                _ => answered(&response(id, &json!({}))),
            }
        });
        let input = format!(
            "{}{}",
            request(&json!(0), "initialize", &json!({})),
            request(&json!(1), "ping", &json!({}))
        );
        let ran = relay(Setup::new(&transport), input.as_bytes()).await?;
        assert_eq!(ran.lines()?.len(), 2, "{}", ran.stdout);
        let posts = transport.posts()?;
        let ping = some(posts.get(1), "the second message")?;
        assert_eq!(
            header(ping, "MCP-Protocol-Version"),
            in_force.then_some(version.as_str()),
            "a version of {length} characters"
        );
    }
    Ok(())
}

/// What the relay repeats of a refusal is bounded: a message of 8192 bytes as the service
/// wrote it is repeated, and one of a byte more is not, so the relay's own line stays short.
/// The refusal's status is said either way.
#[tokio::test]
async fn a_refusal_s_message_is_repeated_up_to_its_bound() -> TestResult {
    for (length, repeated) in [(8190_usize, true), (8191, false)] {
        let message = "m".repeat(length);
        // The service's own refusal of a request.
        let plain = format!("{{\"code\":\"internal\",\"message\":\"{message}\"}}");
        let ran = answered_with(500, &plain).await?;
        let lines = ran.lines()?;
        let said = some(
            some(lines.first(), "an answer")?
                .pointer("/error/message")
                .and_then(Value::as_str),
            "a message",
        )?;
        assert_eq!(said.contains(&message), repeated, "{length}");
        assert!(said.contains("HTTP 500"), "{said}");
        assert_eq!(
            said.len() < 300,
            !repeated,
            "{length}: {} bytes",
            said.len()
        );
        // A code is bounded the same way: one too long to repeat leaves the status alone.
        let coded = format!("{{\"code\":\"{message}\",\"message\":\"brief words\"}}");
        let ran = answered_with(500, &coded).await?;
        assert_eq!(ran.stdout.contains("brief words"), repeated, "{length}");
        // A JSON-RPC error that refuses a notification, which is said on the error stream.
        let error =
            format!("{{\"jsonrpc\":\"2.0\",\"error\":{{\"code\":-1,\"message\":\"{message}\"}}}}");
        let transport = FakeTransport::scripted(move |_| status(400, Body::Json(error.clone())));
        let ran = relay(
            Setup::new(&transport),
            notification("notifications/progress").as_bytes(),
        )
        .await?;
        assert_eq!(ran.stdout, "");
        assert_eq!(ran.stderr.contains(&message), repeated, "{length}");
        assert!(ran.stderr.contains("HTTP 400"), "{}", ran.stderr);
    }
    Ok(())
}

/// A refusal that is given the request's id grows by it, and no text of the service is held
/// longer than an answer may be: at the bound the id is put in and the refusal passed on,
/// and one byte over the request gets the relay's own error, which says why.
#[tokio::test]
async fn a_refusal_given_the_request_id_stays_within_the_answer_bound() -> TestResult {
    const BOUND: usize = 200;
    let member = "\"id\":1,".len();
    for (over, passed_on) in [(0_usize, true), (1, false)] {
        let frame = "{\"jsonrpc\":\"2.0\",\"error\":{\"code\":-1,\"message\":\"\"}}".len();
        let pad = BOUND
            .saturating_sub(frame)
            .saturating_sub(member)
            .saturating_add(over);
        let body = format!(
            "{{\"jsonrpc\":\"2.0\",\"error\":{{\"code\":-1,\"message\":\"{}\"}}}}",
            "m".repeat(pad)
        );
        assert_eq!(
            body.len().saturating_add(member),
            BOUND.saturating_add(over)
        );
        let sent = body.clone();
        let transport = FakeTransport::scripted(move |_| status(400, Body::Json(sent.clone())));
        let setup = Setup {
            limits: Limits {
                answer_bytes: BOUND,
                ..Limits::default()
            },
            ..Setup::new(&transport)
        };
        let ran = relay(
            setup,
            request(&json!(1), "tools/call", &json!({})).as_bytes(),
        )
        .await?;
        let lines = ran.lines()?;
        let only = some(lines.first(), "an answer")?;
        assert_eq!((lines.len(), only.get("id")), (1, Some(&json!(1))));
        if passed_on {
            assert_eq!(ran.stdout.len(), BOUND.saturating_add(1), "{}", ran.stdout);
            assert_eq!(only.pointer("/error/code"), Some(&json!(-1)));
        } else {
            assert_eq!(only.pointer("/error/code"), Some(&json!(-32603)));
            assert!(
                ran.stdout
                    .contains("too long to pass on with the request's id"),
                "{}",
                ran.stdout
            );
        }
        // An id put in the place of `null` is bounded the same way.
        let null = body.replacen('{', "{\"id\":null,", 1);
        let room = null.len().saturating_sub("null".len()).saturating_add(1);
        for (bound, passed_on) in [(room, true), (room.saturating_sub(1), false)] {
            let sent = null.clone();
            let transport = FakeTransport::scripted(move |_| status(400, Body::Json(sent.clone())));
            let setup = Setup {
                limits: Limits {
                    answer_bytes: bound,
                    ..Limits::default()
                },
                ..Setup::new(&transport)
            };
            let ran = relay(
                setup,
                request(&json!(1), "tools/call", &json!({})).as_bytes(),
            )
            .await?;
            assert_eq!(
                ran.stdout.contains("too long to pass on"),
                !passed_on,
                "{}",
                ran.stdout
            );
            assert_eq!(ran.lines()?.len(), 1);
        }
    }
    Ok(())
}

/// A tab between tokens is white space, as a blank is (the re-review's F5): an answer the
/// service indented with tabs and ended its lines with a carriage return and a line feed is
/// a message, and comes out as one line with its tabs.
#[tokio::test]
async fn an_answer_indented_with_tabs_is_passed_on() -> TestResult {
    let pretty = "{\r\n\t\"jsonrpc\": \"2.0\",\r\n\t\"id\": 1,\r\n\t\"result\": {\t}\r\n}\r\n";
    let ran = answered_with(200, pretty).await?;
    assert_eq!(
        (ran.stdout.as_str(), ran.stderr.as_str()),
        (
            "{\t\"jsonrpc\": \"2.0\",\t\"id\": 1,\t\"result\": {\t}}\n",
            ""
        )
    );
    let refusal =
        "{\n\t\"jsonrpc\"\t:\t\"2.0\",\n\t\"error\"\t:\t{\t\"code\":-1,\t\"message\":\"m\"\t}\n}";
    let ran = answered_with(400, refusal).await?;
    assert_eq!(
        (ran.stdout.as_str(), ran.stderr.as_str()),
        (
            "{\"id\":1,\t\"jsonrpc\"\t:\t\"2.0\",\t\"error\"\t:\t{\t\"code\":-1,\t\"message\":\"m\"\t}}\n",
            ""
        )
    );
    Ok(())
}

/// A key is the characters it spells (the re-review's F5): `"\u0069d"` is the member `id`.
/// An answer that names its id so is the request's answer, once; a refusal that names
/// another id so is not given a second one; and a `null` spelled so is replaced like any.
#[tokio::test]
async fn a_key_spelled_with_an_escape_is_the_key_it_spells() -> TestResult {
    let id = format!("\"{}d\"", escape("0069"));
    let answer = format!("{{\"jsonrpc\":\"2.0\",{id}:1,\"result\":{{}}}}");
    let ran = answered_with(200, &answer).await?;
    assert_eq!(
        (ran.stdout.as_str(), ran.stderr.as_str()),
        (format!("{answer}\n").as_str(), "")
    );
    let other =
        format!("{{\"jsonrpc\":\"2.0\",{id}:99,\"error\":{{\"code\":-1,\"message\":\"m\"}}}}");
    let ran = answered_with(400, &other).await?;
    let (first, second) = some(ran.stdout.trim_end().split_once('\n'), "two lines")?;
    assert_eq!(first, other);
    let own = serde_json::from_str::<Value>(second)?;
    assert_eq!(
        (own.get("id"), own.pointer("/error/code")),
        (Some(&json!(1)), Some(&json!(-32603)))
    );
    let unnamed =
        format!("{{\"jsonrpc\":\"2.0\",{id}:null,\"error\":{{\"code\":-1,\"message\":\"m\"}}}}");
    let ran = answered_with(400, &unnamed).await?;
    assert_eq!(
        ran.stdout,
        format!("{{\"jsonrpc\":\"2.0\",{id}:1,\"error\":{{\"code\":-1,\"message\":\"m\"}}}}\n")
    );
    // The version member spelled so is still the version, in its key and in its value.
    let version = format!(
        "{{\"{}sonrpc\":\"{}.0\",\"id\":1,\"result\":{{}}}}",
        escape("006a"),
        escape("0032")
    );
    let ran = answered_with(200, &version).await?;
    assert_eq!(ran.stdout, format!("{version}\n"));
    Ok(())
}

/// An `error` that is no object makes no JSON-RPC error (the re-review's F5): a refusal
/// whose `error` is a string, a number, an array or `null` is not passed on as the request's
/// answer. The request gets the relay's own error, which names the status.
#[tokio::test]
async fn an_error_that_is_no_object_is_not_a_json_rpc_error() -> TestResult {
    for error in ["\"m\"", "7", "[{\"code\":-1}]", "null", "true"] {
        let body = format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{error}}}");
        let ran = answered_with(400, &body).await?;
        let lines = ran.lines()?;
        let only = some(lines.first(), "an answer")?;
        assert_eq!(
            (lines.len(), only.get("id"), only.pointer("/error/code")),
            (1, Some(&json!(1)), Some(&json!(-32603))),
            "{error}"
        );
        assert!(ran.stdout.contains("HTTP 400"), "{error}: {}", ran.stdout);
    }
    Ok(())
}

/// An answer has no limit on depth (the re-review's note N9): one nested a million deep is a
/// message, and is passed on as the service wrote it.
#[tokio::test]
async fn an_answer_nested_a_million_deep_is_passed_on() -> TestResult {
    const DEPTH: usize = 1_000_000;
    let deep = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}{}}}",
        "[".repeat(DEPTH),
        "]".repeat(DEPTH)
    );
    let ran = answered_with(200, &deep).await?;
    assert_eq!(ran.stdout.len(), deep.len().saturating_add(1));
    assert!(ran.stdout.starts_with(&deep), "the answer was changed");
    assert_eq!((ran.stderr.as_str(), ran.code), ("", 0));
    Ok(())
}

/// An answer that holds text that is not ASCII before its `id` is the request's answer (the
/// second re-review's B2): it is passed on once, as it is, and the request gets no error of
/// the relay's besides. The places the reader notes are places in bytes, so the id is read
/// where it stands. The same for a refusal: the request's id goes where the `null` stands,
/// after such text.
#[tokio::test]
async fn an_answer_with_text_that_is_not_ascii_before_its_id_is_passed_on_once() -> TestResult {
    for raw in [
        "{\"jsonrpc\":\"2.0\",\"r\":\"\u{e9}\u{1f600}\",\"id\":1,\"result\":{}}",
        "{\"\u{4e16}\u{754c}\":\"\u{e9}\",\"jsonrpc\":\"2.0\",\"result\":{\"s\":\"\u{1f600}\"},\"id\":1}",
    ] {
        let ran = answered_with(200, raw).await?;
        assert_eq!(
            (ran.stdout.as_str(), ran.stderr.as_str(), ran.code),
            (format!("{raw}\n").as_str(), "", 0)
        );
    }
    let refusal = "{\"error\":{\"code\":-32020,\"message\":\"\u{e9}\u{1f600}\"},\"jsonrpc\":\"2.0\",\"id\":null}";
    let ran = answered_with(400, refusal).await?;
    assert_eq!(
        (ran.stdout.as_str(), ran.stderr.as_str()),
        (
            "{\"error\":{\"code\":-32020,\"message\":\"\u{e9}\u{1f600}\"},\"jsonrpc\":\"2.0\",\"id\":1}\n",
            ""
        )
    );
    Ok(())
}

/// No line that is not JSON reaches the output (the second re-review's B2): an answer with a
/// raw control character inside a string, each of U+0000 to U+001F and the last one
/// included, is dropped, and the request gets the relay's own error instead. Every line
/// written is JSON for the JSON library.
#[tokio::test]
async fn an_answer_with_a_raw_control_character_in_a_string_is_never_written() -> TestResult {
    for code in 0..0x20_u32 {
        let raw = some(char::from_u32(code), "a character")?;
        let body = format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"a{raw}b\"}}");
        let ran = answered_with(200, &body).await?;
        assert_eq!(
            ran.stdout.matches('\n').count(),
            1,
            "U+{code:04X}: {:?}",
            ran.stdout
        );
        let lines = ran.lines()?;
        let only = some(lines.first(), "an answer")?;
        assert_eq!(
            (lines.len(), only.get("id"), only.pointer("/error/code")),
            (1, Some(&json!(1)), Some(&json!(-32603))),
            "U+{code:04X}"
        );
        assert!(
            !ran.stdout.contains(&format!("a{raw}b")),
            "U+{code:04X}: {:?}",
            ran.stdout
        );
    }
    Ok(())
}

/// A key is matched whole (the second re-review's B2): `protocolVersionX` in the answer to
/// `initialize` is not the protocol version. With the version beside it, the version is put
/// in force; alone, none is.
#[tokio::test]
async fn a_key_that_only_starts_with_protocol_version_names_no_version() -> TestResult {
    for (result, in_force) in [
        (
            "{\"protocolVersion\":\"2025-06-18\",\"protocolVersionX\":\"wrong\"}",
            Some("2025-06-18"),
        ),
        (
            "{\"protocolVersionX\":\"wrong\",\"protocolVersion\":\"2025-06-18\"}",
            Some("2025-06-18"),
        ),
        ("{\"protocolVersionX\":\"wrong\"}", None),
        ("{\"protocolVersion2025\":\"wrong\"}", None),
    ] {
        let transport = FakeTransport::scripted(move |message| {
            let id = message.get("id").unwrap_or(&Value::Null);
            match message.get("method").and_then(Value::as_str) {
                Some("initialize") => status(
                    200,
                    Body::Json(format!(
                        "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{result}}}"
                    )),
                ),
                _ => answered(&response(id, &json!({}))),
            }
        });
        let input = format!(
            "{}{}",
            request(&json!(0), "initialize", &json!({})),
            request(&json!(1), "ping", &json!({}))
        );
        let ran = relay(Setup::new(&transport), input.as_bytes()).await?;
        assert_eq!(ran.lines()?.len(), 2, "{}", ran.stdout);
        let posts = transport.posts()?;
        let ping = some(posts.get(1), "the second message")?;
        assert_eq!(header(ping, "MCP-Protocol-Version"), in_force, "{result}");
    }
    Ok(())
}

/// The answer to `initialize` whose result names the protocol version `written`, a JSON value
/// as the service writes it, and no other member that could be taken for one.
fn naming(written: &str) -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{{\"protocolVersion\":{written},\"capabilities\":{{}}}}}}"
    )
}

/// A service that answers `initialize` with `body` and the session `session-7`, accepts a
/// notification, and answers every other request with an empty result.
fn initialized_with(body: &str) -> std::sync::Arc<FakeTransport> {
    let body = body.to_owned();
    FakeTransport::scripted(move |message| {
        let id = message.get("id").unwrap_or(&Value::Null);
        match message.get("method").and_then(Value::as_str) {
            Some("initialize") => {
                let mut script = status(200, Body::Json(body.clone()));
                script.reply = script.reply.map(|reply| Reply {
                    session: Some("session-7".to_owned()),
                    ..reply
                });
                script
            }
            Some(_) if id.is_null() => status(202, Body::Empty),
            _ => answered(&response(id, &json!({}))),
        }
    })
}

/// The protocol version the service names in its answer to `initialize` travels in a header
/// of every later message, so it passes the check a version in a request's `_meta` passes
/// (the pull request's finding 3). One that does not is not put in force: the answer is
/// still handed to the client as the service wrote it, the relay says so once on the error
/// stream in words of its own, which repeat nothing of the version, and the messages that
/// follow go out as they do when no version was negotiated, the session with them.
#[tokio::test]
async fn a_version_of_the_service_that_cannot_be_a_header_value_is_not_put_in_force() -> TestResult
{
    let long = format!("\"{}\"", "v".repeat(300));
    let escaped = format!("\"2025-06-18{}[31m\"", escape("001b"));
    let deleted = format!("\"2025-06-18{}\"", escape("007f"));
    let unusable = [
        ("a line feed", "\"2025-06-18\\n\""),
        ("an escape character", escaped.as_str()),
        ("a letter that is not ASCII", "\"2025-06-18\u{e9}\""),
        ("an empty string", "\"\""),
        ("300 characters", long.as_str()),
        ("a carriage return and a header", "\"1\\r\\nX-Injected: 1\""),
        ("a tab inside", "\"2025\\t06\""),
        ("a delete character", deleted.as_str()),
        ("a blank before it", "\" 2025-06-18\""),
        ("a blank after it", "\"2025-06-18 \""),
        ("the Base64 form", "\"=?base64?eA==?=\""),
        ("a number", "20250618"),
        ("null", "null"),
    ];
    let input = format!(
        "{}{}{}",
        request(&json!(0), "initialize", &json!({})),
        request(&json!(1), "ping", &json!({})),
        notification("notifications/initialized")
    );
    for (what, written) in unusable {
        let body = naming(written);
        let transport = initialized_with(&body);
        let ran = relay(Setup::new(&transport), input.as_bytes()).await?;
        assert_eq!(ran.code, 0, "{what}");
        // The relay is not the judge of the answer's content.
        assert_eq!(ran.stdout.split('\n').next(), Some(body.as_str()), "{what}");
        assert_eq!(ran.lines()?.len(), 2, "{what}: {}", ran.stdout);
        let posts = transport.posts()?;
        assert_eq!(posts.len(), 3, "{what}");
        for later in posts.iter().skip(1) {
            assert_eq!(
                later.headers,
                [("Mcp-Session-Id", "session-7".to_owned())],
                "{what}: a later message carries the session and no version"
            );
        }
        assert_eq!(
            ran.stderr,
            "okf-jawn mcp: the okf-jawn service's answer to initialize names a protocol \
             version the relay cannot use: it is not a short string of printable ASCII that \
             can stand in an HTTP header as it is. No protocol version is in force for the \
             messages that follow, as if the service had named none\n",
            "{what}"
        );
    }
    // The other side: a version that can be a header value is put in force, and nothing is
    // said.
    let transport = initialized_with(&naming("\"2025-06-18\""));
    let ran = relay(Setup::new(&transport), input.as_bytes()).await?;
    assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
    for later in transport.posts()?.iter().skip(1) {
        assert_eq!(header(later, "MCP-Protocol-Version"), Some("2025-06-18"));
        assert_eq!(header(later, "Mcp-Session-Id"), Some("session-7"));
    }
    // An answer that names no version at all, as a result or as a refusal, names none the
    // relay cannot use: none is in force, and nothing is said.
    for unnamed in [
        "{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{\"capabilities\":{}}}",
        "{\"jsonrpc\":\"2.0\",\"id\":0,\"error\":{\"code\":-32602,\"message\":\"no\"}}",
    ] {
        let transport = initialized_with(unnamed);
        let ran = relay(Setup::new(&transport), input.as_bytes()).await?;
        assert_eq!((ran.code, ran.stderr.as_str()), (0, ""), "{unnamed}");
        let posts = transport.posts()?;
        assert_eq!(posts.len(), 3, "{unnamed}");
        for later in posts.iter().skip(1) {
            assert_eq!(
                later.headers,
                [("Mcp-Session-Id", "session-7".to_owned())],
                "{unnamed}"
            );
        }
    }
    Ok(())
}

/// A version that is not put in force leaves none in force: not the one an earlier handshake
/// negotiated either.
#[tokio::test]
async fn a_version_that_is_not_put_in_force_does_not_leave_an_earlier_one() -> TestResult {
    let transport = FakeTransport::scripted(|message| {
        let id = message.get("id").unwrap_or(&Value::Null);
        match message.get("method").and_then(Value::as_str) {
            Some("initialize") if message.pointer("/params/second").is_some() => {
                status(200, Body::Json(naming("\"2025-06-18\\n\"")))
            }
            Some("initialize") => status(200, Body::Json(naming("\"2025-06-18\""))),
            _ => answered(&response(id, &json!({}))),
        }
    });
    let input = format!(
        "{}{}{}{}",
        request(&json!(0), "initialize", &json!({})),
        request(&json!(1), "ping", &json!({})),
        request(&json!(0), "initialize", &json!({"second": true})),
        request(&json!(2), "ping", &json!({})),
    );
    let ran = relay(Setup::new(&transport), input.as_bytes()).await?;
    assert_eq!(ran.lines()?.len(), 4, "{}", ran.stdout);
    let posts = transport.posts()?;
    let versions: Vec<Option<&str>> = posts
        .iter()
        .map(|post| header(post, "MCP-Protocol-Version"))
        .collect();
    assert_eq!(versions, [None, Some("2025-06-18"), None, None]);
    assert_eq!(ran.stderr.matches("names a protocol version").count(), 1);
    Ok(())
}

/// The same through the relay as shipped and real HTTP, where a header value is built: after
/// a handshake whose answer names `2025-06-18` and a line feed, the notification and the
/// request that follow reach the service, without a version header, and the request gets the
/// service's answer.
#[tokio::test]
async fn the_relay_as_shipped_goes_on_after_a_version_it_cannot_send() -> TestResult {
    let handshake = naming("\"2025-06-18\\n\"");
    let answer_to_it = handshake.clone();
    let listener = Answering::routed(move |heard| {
        let body = String::from_utf8_lossy(&heard.body).into_owned();
        if heard.line().starts_with("GET ") {
            json_answer("200 OK", "{}")
        } else if body.contains("\"initialize\"") {
            json_answer("200 OK", &answer_to_it)
        } else if body.contains("notifications/initialized") {
            answer("202 Accepted", &[], "")
        } else {
            json_answer("200 OK", "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}")
        }
    })
    .await?;
    let input = format!(
        "{}{}{}",
        request(&json!(0), "initialize", &json!({})),
        notification("notifications/initialized"),
        request(&json!(1), "ping", &json!({})),
    );
    let ran = cli(
        &["mcp", "--stdio", "--server", &listener.origin()],
        Some(TOKEN),
        input.as_bytes(),
    )
    .await?;
    let heard = listener.heard()?;
    assert_eq!(
        (ran.code, heard.len()),
        (0, 4),
        "the liveness check and three messages; stdout: {} stderr: {}",
        ran.stdout,
        ran.stderr
    );
    assert_eq!(
        ran.stdout,
        format!("{handshake}\n{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{}}}}\n")
    );
    for later in heard.iter().skip(2) {
        assert_eq!(later.header("mcp-protocol-version"), None, "{}", later.head);
    }
    assert_eq!(
        (
            ran.stderr.lines().count(),
            ran.stderr.matches("names a protocol version").count()
        ),
        (1, 1),
        "{}",
        ran.stderr
    );
    Ok(())
}
