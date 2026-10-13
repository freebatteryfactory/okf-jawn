//! Bounded line framing of the relay's input: one JSON-RPC message per line.
//!
//! A line is the bytes before a line feed; one carriage return before the line feed is not
//! part of it. A line longer than the limit is never held: once it has grown past the limit
//! its bytes are dropped, the rest of it is read and dropped up to the line feed, and it is
//! reported as [`Frame::TooLong`]. At most `limit + 1` bytes of one line are buffered, beside
//! the reader's own fixed buffer. Input that ends without a line feed still yields its last
//! line.

use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// What the next read of the input produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// One line without its terminator; it may be empty.
    Line(Vec<u8>),
    /// A line longer than the limit; its bytes were dropped.
    TooLong,
    /// The input ended.
    End,
}

/// Reads [`Frame`]s from a buffered reader.
#[derive(Debug)]
pub struct Frames<R> {
    reader: R,
    limit: usize,
}

/// The bytes of the line being read: never more than `room` of them.
#[derive(Debug)]
struct Held {
    bytes: Vec<u8>,
    room: usize,
    over: bool,
}

impl<R: AsyncBufRead + Unpin> Frames<R> {
    /// Frame `reader` into lines of at most `limit` bytes.
    pub fn new(reader: R, limit: usize) -> Self {
        Self { reader, limit }
    }

    /// Read the next frame.
    ///
    /// # Errors
    /// Returns the reader's error.
    pub async fn next(&mut self) -> std::io::Result<Frame> {
        // One byte over the limit is kept so that a carriage return ending a line of exactly
        // `limit` bytes can still be told from a line that is too long.
        let mut held = Held::new(self.limit.saturating_add(1));
        loop {
            let available = self.reader.fill_buf().await?;
            if available.is_empty() {
                return Ok(if !held.over && held.bytes.is_empty() {
                    Frame::End
                } else {
                    held.finish(self.limit)
                });
            }
            let end = available.iter().position(|byte| *byte == b'\n');
            let part = end.and_then(|at| available.get(..at)).unwrap_or(available);
            held.take(part);
            let used = part.len().saturating_add(usize::from(end.is_some()));
            self.reader.consume(used);
            if end.is_some() {
                return Ok(held.finish(self.limit));
            }
        }
    }
}

impl Held {
    /// The whole line as a frame: without its carriage return, and within `limit`.
    fn finish(mut self, limit: usize) -> Frame {
        if self.bytes.last() == Some(&b'\r') {
            self.bytes.pop();
        }
        if self.over || self.bytes.len() > limit {
            Frame::TooLong
        } else {
            Frame::Line(self.bytes)
        }
    }

    fn new(room: usize) -> Self {
        Self {
            bytes: Vec::new(),
            room,
            over: false,
        }
    }

    /// Add the next bytes of the line, or give the line up when they would pass `room`.
    fn take(&mut self, part: &[u8]) {
        if self.over {
            return;
        }
        if self.bytes.len().saturating_add(part.len()) > self.room {
            self.over = true;
            self.bytes = Vec::new();
        } else {
            self.bytes.extend_from_slice(part);
        }
    }
}

#[cfg(test)]
mod mcp_stdio {
    use super::{Frame, Held};

    /// However long a line grows and however it arrives, no more than `room` bytes are held.
    #[test]
    fn a_line_is_never_held_past_its_room() {
        for size in [1_usize, 7, 1024] {
            let mut held = Held::new(17);
            let part = vec![b'a'; size];
            for _ in 0..4096 {
                held.take(&part);
                assert!(held.bytes.len() <= 17, "{} bytes held", held.bytes.len());
            }
            assert!(held.over);
            assert_eq!(held.finish(16), Frame::TooLong);
        }
    }

    /// The room is one byte more than the limit, and that byte may only be a carriage return.
    #[test]
    fn the_spare_byte_of_room_is_only_for_a_carriage_return() {
        let mut ended = Held::new(17);
        ended.take(b"0123456789abcdef\r");
        assert_eq!(ended.finish(16), Frame::Line(b"0123456789abcdef".to_vec()));
        let mut long = Held::new(17);
        long.take(b"0123456789abcdefg");
        assert_eq!(long.finish(16), Frame::TooLong);
    }
}
