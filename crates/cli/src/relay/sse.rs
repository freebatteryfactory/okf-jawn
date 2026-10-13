//! Incremental framing of a `text/event-stream` answer into the data of its message events.
//!
//! This reads what the relay needs and no more. A line ends with a line feed, with a carriage
//! return, or with a carriage return and the line feed after it, as the event-stream format
//! says; a stream may use all three. `data` lines of one event are joined with line feeds, and
//! a blank line ends the event. An event is yielded when its type is absent or `message` and
//! its data is not blank; comments, `id`, `retry`, other event types and blank priming events
//! are dropped. An event that the stream never ends with a blank line is dropped, as the
//! event-stream format says.
//!
//! Invariants:
//! - A carriage return ends its line at once, whatever follows it. A line feed that is the
//!   next byte of the stream belongs to that line end and ends no line of its own, also when
//!   the two arrive in different chunks (the parser remembers the carriage return): counted
//!   twice, it would be the blank line that hands an event over early.
//! - One event's buffered bytes never exceed the limit. A line end is not buffered, so an
//!   event is bounded alike whichever way its lines end.
//! - What is buffered of an event stands in one buffer that holds room for its capacity
//!   ([`Gathering`]): the data read so far, then the line being read. A `data` line is moved
//!   back within that buffer, not copied to another, and an event is handed over as soon as
//!   its blank line is read, before anything more is taken from the bytes at hand.

use super::room::{Gathering, Held, Room};
use super::transport::TransportError;

/// Splits the bytes of an event stream into event data.
#[derive(Debug)]
pub struct EventParser {
    limit: usize,
    /// The data of the event being read, then the line being read.
    held: Gathering,
    /// Where the data ends and the line being read starts.
    data: usize,
    has_data: bool,
    /// Whether the event names a type other than `message`.
    other: bool,
    started: bool,
    /// What the byte read last was, which is kept from one chunk to the next.
    last: Last,
}

/// What the byte read last means for the byte that comes next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Last {
    /// A carriage return, which ended its line: a line feed that comes next is the rest of
    /// that line end.
    Return,
    /// Any other byte, or none yet: the next byte is read for what it is.
    Other,
}

/// The UTF-8 byte order mark a stream may begin with.
const BYTE_ORDER_MARK: &[u8] = b"\xEF\xBB\xBF";

impl EventParser {
    /// End the current event and yield its data when it is a message.
    fn end_event(&mut self) -> Result<Option<Held>, TransportError> {
        self.held.truncate(self.data);
        let data = self.held.take();
        let other = std::mem::take(&mut self.other);
        self.data = 0;
        self.has_data = false;
        if other {
            return Ok(None);
        }
        let text = data.into_text().ok_or_else(|| {
            TransportError("an event of the service's event stream is not UTF-8".to_owned())
        })?;
        Ok(Some(text).filter(|text| !text.as_str().trim().is_empty()))
    }

    /// Take one finished line, which stands after the data in the buffer: a field of the
    /// current event, or the blank line that ends it and yields its data.
    fn end_line(&mut self) -> Result<Option<Held>, TransportError> {
        // The line is read where it stands, and its places noted, before anything moves.
        let all = self.held.bytes();
        let end = all.len();
        let mut start = self.data.min(end);
        let mut line = all.get(start..end).unwrap_or_default();
        if !self.started {
            self.started = true;
            if let Some(rest) = line.strip_prefix(BYTE_ORDER_MARK) {
                start = start.saturating_add(BYTE_ORDER_MARK.len());
                line = rest;
            }
        }
        if line.is_empty() {
            return self.end_event();
        }
        let colon = line.iter().position(|byte| *byte == b':');
        let field = colon.map_or(line, |colon| line.get(..colon).unwrap_or_default());
        let (data, event) = (field == b"data", field == b"event");
        let mut value_at = colon.map_or(end, |colon| start.saturating_add(colon).saturating_add(1));
        if value_at < end && all.get(value_at) == Some(&b' ') {
            value_at = value_at.saturating_add(1);
        }
        let value = all.get(value_at..end).unwrap_or_default();
        let named_other = !(value.is_empty() || value == b"message");
        if data {
            // The value is moved back to stand after the data read so far, with a line feed
            // between them. It never moves forward: `data:` stood before it.
            self.held.truncate(end);
            let to = if self.has_data {
                self.held.put(self.data, b'\n');
                self.data.saturating_add(1)
            } else {
                self.data
            };
            self.held.keep_from(value_at, to);
            self.data = self.held.len();
            self.has_data = true;
        } else {
            if event {
                self.other = named_other;
            }
            self.held.truncate(self.data);
        }
        Ok(None)
    }

    /// A parser that refuses an event whose buffered bytes exceed `limit`, and takes the room
    /// for what it buffers from `room`.
    #[must_use]
    pub fn new(limit: usize, room: &Room) -> Self {
        Self {
            limit,
            held: room.gather(limit),
            data: 0,
            has_data: false,
            other: false,
            started: false,
            last: Last::Other,
        }
    }

    /// Take bytes of the stream from the front of `bytes` until they complete an event, and
    /// return its data; `None` when `bytes` are used up first. Call it again with what is
    /// left of `bytes` once the event is handed over. Waits while there is no room for what
    /// it has to buffer.
    ///
    /// # Errors
    /// Returns a [`TransportError`] when an event is larger than the limit or is not UTF-8;
    /// the events returned before it stay returned.
    pub async fn next(&mut self, bytes: &mut &[u8]) -> Result<Option<Held>, TransportError> {
        while !bytes.is_empty() {
            // Whatever follows a carriage return, it follows it only once.
            let last = std::mem::replace(&mut self.last, Last::Other);
            if last == Last::Return && bytes.first() == Some(&b'\n') {
                *bytes = bytes.get(1..).unwrap_or_default();
                continue;
            }
            let ended = bytes.iter().position(|byte| matches!(*byte, b'\n' | b'\r'));
            let part = bytes
                .get(..ended.unwrap_or(bytes.len()))
                .unwrap_or_default();
            if self.held.len().saturating_add(part.len()) > self.limit {
                return Err(TransportError(format!(
                    "one event of the service's event stream exceeds {} bytes",
                    self.limit
                )));
            }
            self.held.extend(part).await;
            let used = part.len().saturating_add(usize::from(ended.is_some()));
            if bytes.get(part.len()) == Some(&b'\r') {
                self.last = Last::Return;
            }
            *bytes = bytes.get(used..).unwrap_or_default();
            if ended.is_some()
                && let Some(event) = self.end_line()?
            {
                return Ok(Some(event));
            }
        }
        Ok(None)
    }
}
