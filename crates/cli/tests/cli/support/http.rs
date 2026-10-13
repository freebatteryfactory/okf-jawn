//! A loopback HTTP/1.1 listener answered by hand.
//!
//! It reads each request whole, records it, writes the next scripted answer as raw bytes and
//! closes the connection. Nothing here knows the service: a test states the exact bytes the
//! service would send and asserts the exact request the command line sent.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use super::Outcome;
use crate::check::some;

/// A listener that answers each connection with the next scripted answer.
pub struct Answering {
    pub address: SocketAddr,
    heard: Arc<Mutex<Vec<Heard>>>,
    task: JoinHandle<()>,
}

/// One request the listener received.
#[derive(Debug, Clone)]
pub struct Heard {
    pub head: String,
    pub body: Vec<u8>,
}

/// The answer once the script is used up.
const UNSCRIPTED: &str =
    "HTTP/1.1 500 Unscripted\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";

impl Answering {
    /// The requests received so far, in order.
    pub fn heard(&self) -> Outcome<Vec<Heard>> {
        Ok(self
            .heard
            .lock()
            .map_err(|_| "the listener's record is poisoned")?
            .clone())
    }

    /// The listener's origin, as a `--server` value.
    pub fn origin(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Listen on a free loopback port and answer each request with what `route` gives for it.
    pub async fn routed(mut route: impl FnMut(&Heard) -> String + Send + 'static) -> Outcome<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let heard = Arc::new(Mutex::new(Vec::new()));
        let record = heard.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let Ok(request) = receive(&mut stream).await else {
                    return;
                };
                let answer = route(&request);
                if !remember(&record, request) {
                    return;
                }
                if stream.write_all(answer.as_bytes()).await.is_err()
                    || stream.shutdown().await.is_err()
                {
                    return;
                }
            }
        });
        Ok(Self {
            address,
            heard,
            task,
        })
    }

    /// Listen on a free loopback port and answer the requests in order with `answers`.
    pub async fn with(answers: Vec<String>) -> Outcome<Self> {
        let mut answers = answers.into_iter();
        Self::routed(move |_| answers.next().unwrap_or_else(|| UNSCRIPTED.to_owned())).await
    }
}

impl Drop for Answering {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Heard {
    /// The value of the header `name`, whatever its case.
    pub fn header(&self, name: &str) -> Option<String> {
        self.head.lines().skip(1).find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name)
                .then(|| value.trim().to_owned())
        })
    }

    /// The request line, such as `POST /mcp HTTP/1.1`.
    pub fn line(&self) -> &str {
        self.head.lines().next().unwrap_or_default()
    }
}

/// An answer with a body of known length.
pub fn answer(status: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut text = format!("HTTP/1.1 {status}\r\n");
    for (name, value) in headers {
        text.push_str(name);
        text.push_str(": ");
        text.push_str(value);
        text.push_str("\r\n");
    }
    format!(
        "{text}content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// An address on which nothing listens: a port that was free a moment ago.
pub async fn closed() -> Outcome<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    Ok(listener.local_addr()?)
}

/// A JSON answer.
pub fn json(status: &str, body: &str) -> String {
    answer(status, &[("content-type", "application/json")], body)
}

/// Read one request whole.
async fn receive(
    stream: &mut TcpStream,
) -> Result<Heard, Box<dyn std::error::Error + Send + Sync>> {
    let mut bytes = Vec::new();
    let mut chunk = vec![0_u8; 65_536];
    let split = loop {
        if let Some(at) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break at;
        }
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err("the connection closed before the request head ended".into());
        }
        bytes.extend_from_slice(some(chunk.get(..read), "the bytes read")?);
    };
    let head = String::from_utf8(some(bytes.get(..split), "the head")?.to_vec())?;
    let mut body = some(bytes.get(split.saturating_add(4)..), "the body")?.to_vec();
    let length = head
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    while body.len() < length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err("the connection closed before the request body ended".into());
        }
        body.extend_from_slice(some(chunk.get(..read), "the bytes read")?);
    }
    Ok(Heard { head, body })
}

/// Record one request; false when the record can no longer be written.
fn remember(record: &Mutex<Vec<Heard>>, request: Heard) -> bool {
    record.lock().map(|mut all| all.push(request)).is_ok()
}
