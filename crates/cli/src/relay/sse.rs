//! Incremental framing of a `text/event-stream` answer into the data of its message events.
//!
//! This reads what the relay needs and no more. Lines end with a line feed, with or without a
//! carriage return before it; a stream that ends its lines with a bare carriage return is not
//! split. `data` lines of one event are joined with line feeds, and a blank line ends the
//! event. An event is yielded when its type is absent or `message` and its data is not blank;
//! comments, `id`, `retry`, other event types and blank priming events are dropped. An event
//! that the stream never ends with a blank line is dropped, as the event-stream format says.
//! One event's buffered bytes never exceed the limit.

use super::transport::TransportError;

/// Splits the bytes of an event stream into event data.
#[derive(Debug)]
pub struct EventParser {
    limit: usize,
    line: Vec<u8>,
    data: Vec<u8>,
    kind: Vec<u8>,
    has_data: bool,
    started: bool,
}

/// The UTF-8 byte order mark a stream may begin with.
const BYTE_ORDER_MARK: &[u8] = b"\xEF\xBB\xBF";

impl EventParser {
    /// A parser that refuses an event whose buffered bytes exceed `limit`.
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            line: Vec::new(),
            data: Vec::new(),
            kind: Vec::new(),
            has_data: false,
            started: false,
        }
    }

    /// Take the next bytes of the stream and add the data of each event they complete to
    /// `events`.
    ///
    /// # Errors
    /// Returns a [`TransportError`] when an event is larger than the limit or is not UTF-8;
    /// the events completed before it are in `events` all the same.
    pub fn feed(&mut self, bytes: &[u8], events: &mut Vec<String>) -> Result<(), TransportError> {
        let mut parts = bytes.split(|byte| *byte == b'\n').peekable();
        while let Some(part) = parts.next() {
            if self
                .line
                .len()
                .saturating_add(self.data.len())
                .saturating_add(part.len())
                > self.limit
            {
                return Err(TransportError(format!(
                    "one event of the service's event stream exceeds {} bytes",
                    self.limit
                )));
            }
            self.line.extend_from_slice(part);
            if parts.peek().is_some() {
                self.end_line(events)?;
            }
        }
        Ok(())
    }

    /// Take one finished line: a field of the current event, or the blank line that ends it.
    fn end_line(&mut self, events: &mut Vec<String>) -> Result<(), TransportError> {
        let mut line = std::mem::take(&mut self.line);
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        let line = if self.started {
            line.as_slice()
        } else {
            self.started = true;
            line.strip_prefix(BYTE_ORDER_MARK).unwrap_or(&line)
        };
        if line.is_empty() {
            return self.end_event(events);
        }
        let (field, value) = match line.iter().position(|byte| *byte == b':') {
            Some(at) => {
                let value = line.get(at.saturating_add(1)..).unwrap_or_default();
                (
                    line.get(..at).unwrap_or_default(),
                    value.strip_prefix(b" ").unwrap_or(value),
                )
            }
            None => (line, b"".as_slice()),
        };
        match field {
            b"data" => {
                if self.has_data {
                    self.data.push(b'\n');
                }
                self.data.extend_from_slice(value);
                self.has_data = true;
            }
            b"event" => self.kind = value.to_vec(),
            _ => {}
        }
        Ok(())
    }

    /// End the current event and yield its data when it is a message.
    fn end_event(&mut self, events: &mut Vec<String>) -> Result<(), TransportError> {
        let data = std::mem::take(&mut self.data);
        let kind = std::mem::take(&mut self.kind);
        self.has_data = false;
        if !(kind.is_empty() || kind == b"message") {
            return Ok(());
        }
        let text = String::from_utf8(data).map_err(|_| {
            TransportError("an event of the service's event stream is not UTF-8".to_owned())
        })?;
        if !text.trim().is_empty() {
            events.push(text);
        }
        Ok(())
    }
}
