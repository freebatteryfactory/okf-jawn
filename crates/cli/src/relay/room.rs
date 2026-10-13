//! The room: the one budget, counted in bytes, for what the relay holds on its way out.
//!
//! What is counted. A text of the service, a JSON body or the data of one event, holds room
//! from the moment its bytes are taken from the transport until the text has been written to
//! the output stream or dropped. The relay's own lines hold room from the moment they are
//! made, and so do its diagnostics, the fixed remarks apart ([`Room::remark`]). Room is counted
//! by the capacity of the buffer that holds a text, and it is taken before the buffer is given
//! that capacity: a [`Gathering`] grows only into room it holds, and a [`Held`] text gives its
//! room back when it is dropped, which the writer does once the text is written.
//!
//! How much. The budget has two parts. The shared part is `bytes` long and any number of texts
//! hold their capacity of it. A text that does not find its room there takes the turn instead:
//! one text at a time is held whole beside the shared part, and a text that takes the turn
//! gives back what it held of the shared part. So what is counted never exceeds the shared
//! part and one text, the longest being one answer or one line of the relay's own.
//!
//! Invariants:
//! - Nothing waits for the shared part. A text takes what is free there at once or waits for
//!   the turn, so a text that already holds room never keeps another from finishing: whoever
//!   has the turn needs nothing more, and gives the turn back when it is written.
//! - Whoever waits for the turn must not be what the holder of the turn waits for. A transport
//!   may wait while it gathers, because the relay goes on taking and writing what was handed
//!   over. The relay's own side of a message waits only when nothing of that message is still
//!   to come; what it has to say in the middle of an answer is a remark, which takes no room.
//!
//! Not counted here: while a buffer grows, the allocator may hold the old and the new one
//! together for a moment, so a growing text can take half as much again as it is counted for.
//! A transport that knows the length to come says so first ([`Gathering::expect`]) and then
//! never grows.

use std::ops::Range;
use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::json::{Noted, Reader};
use crate::credential::Credential;

/// Bytes of one text of the service while they arrive, holding room for their capacity.
#[derive(Debug)]
pub struct Gathering {
    bytes: Vec<u8>,
    grant: Grant,
    room: Room,
    /// The longest this text may become; the buffer is never given more capacity than that.
    most: usize,
}

/// The room one text holds.
#[derive(Debug)]
enum Grant {
    /// None: a remark of fixed words.
    Free,
    /// Of the shared part, one permit for each byte of capacity; `None` before the first.
    Shared(Option<OwnedSemaphorePermit>),
    /// The turn: the whole text is held beside the shared part until this is dropped.
    Turn { _held: OwnedSemaphorePermit },
}

/// A text with the room it holds; the room is given back when the text is dropped.
#[derive(Debug)]
pub struct Held {
    text: String,
    grant: Grant,
    room: Room,
}

/// The budget for what the relay holds on its way out.
#[derive(Debug, Clone)]
pub struct Room {
    shared: Arc<Semaphore>,
    turn: Arc<Semaphore>,
}

impl Gathering {
    /// The bytes gathered so far.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Take room for a text of `length` bytes in one step, because the service said how long
    /// its answer is. Never more than the longest text allowed: what the service says only
    /// spares the buffer its growing, and what arrives is checked by whoever gathers it.
    pub async fn expect(&mut self, length: usize) {
        self.grow(length.min(self.most), false).await;
    }

    /// Add `more` to the text, first taking the room the buffer needs to hold it; waits while
    /// there is none.
    pub async fn extend(&mut self, more: &[u8]) {
        self.grow(self.bytes.len().saturating_add(more.len()), true)
            .await;
        self.bytes.extend_from_slice(more);
    }

    /// Give the buffer the capacity `needed`, taking the room for it first.
    ///
    /// A buffer that is `spacious` grows to twice its capacity, so that gathering a long text
    /// stays linear in its length, but never past the longest text allowed.
    async fn grow(&mut self, needed: usize, spacious: bool) {
        let capacity = self.bytes.capacity();
        if needed <= capacity {
            return;
        }
        let doubled = if spacious {
            capacity.saturating_mul(2).min(self.most)
        } else {
            0
        };
        let target = doubled.max(needed);
        self.grant
            .more(&self.room, target.saturating_sub(capacity))
            .await;
        self.bytes
            .reserve_exact(target.saturating_sub(self.bytes.len()));
    }

    /// The text, when its bytes are UTF-8; it keeps the room it holds.
    #[must_use]
    pub fn into_text(self) -> Option<Held> {
        let Self {
            bytes, grant, room, ..
        } = self;
        Some(Held {
            text: String::from_utf8(bytes).ok()?,
            grant,
            room,
        })
    }

    /// Whether nothing was gathered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Keep the bytes `from..`, moved back to start at `to`, and nothing after them. `to` is
    /// not after `from`, so no byte is read after it was written over.
    pub fn keep_from(&mut self, from: usize, to: usize) {
        let end = self.bytes.len();
        let (from, to) = (from.min(end), to.min(from).min(end));
        self.bytes.copy_within(from..end, to);
        self.bytes
            .truncate(to.saturating_add(end.saturating_sub(from)));
    }

    /// How many bytes were gathered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Put `byte` in the place `at`, when the buffer has that place.
    pub fn put(&mut self, at: usize, byte: u8) {
        if let Some(place) = self.bytes.get_mut(at) {
            *place = byte;
        }
    }

    /// Hand over what was gathered, leaving this empty and holding no room.
    #[must_use]
    pub fn take(&mut self) -> Self {
        let fresh = self.room.gather(self.most);
        std::mem::replace(self, fresh)
    }

    /// Drop the bytes after the first `length`.
    pub fn truncate(&mut self, length: usize) {
        self.bytes.truncate(length);
    }
}

impl Grant {
    /// Take room for `bytes` more: of the shared part when it has them free, and otherwise the
    /// turn, waiting for it and giving back what was held of the shared part.
    async fn more(&mut self, room: &Room, bytes: usize) {
        let Self::Shared(held) = self else {
            // The turn covers the whole text, and a remark takes no room.
            return;
        };
        if bytes == 0 {
            return;
        }
        // More than a `u32` of bytes is more than the shared part ever has.
        let wanted = u32::try_from(bytes).unwrap_or(u32::MAX);
        match room.shared.clone().try_acquire_many_owned(wanted) {
            Ok(permit) => match held {
                Some(held) => held.merge(permit),
                None => *held = Some(permit),
            },
            Err(_) => {
                // The semaphore is never closed, so the turn is always given in the end.
                if let Ok(turn) = room.turn.clone().acquire_owned().await {
                    *self = Self::Turn { _held: turn };
                }
            }
        }
    }
}

impl Held {
    /// The text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The text without the credential, cleaned in the buffer it stands in, so it needs no
    /// more room than it holds. `None` when it cannot be cleaned
    /// ([`Credential::redact_line`]); its room is then given back.
    pub(super) fn cleaned(self, secret: &Credential) -> Option<Self> {
        let Self { text, grant, room } = self;
        Some(Self {
            text: secret.redact_line(text)?,
            grant,
            room,
        })
    }

    /// Whether the text is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// How long the text is, in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.text.len()
    }

    /// Read the text as JSON and make it one line where it stands: the line feeds and
    /// carriage returns between its tokens go, and the buffer never grows. `None` when the
    /// text is not one JSON value; what is left of it is then of no use.
    pub(super) fn one_line(&mut self) -> Option<Noted> {
        let mut reader = Reader::new(true);
        self.text.retain(|character| reader.step(character));
        reader.finish()
    }

    /// Put `with` in the place of the bytes `range`, taking the room a longer text needs
    /// first. False, and nothing changed, when the text would be longer than `most` or
    /// `range` is no place in it.
    pub(super) async fn splice(&mut self, range: Range<usize>, with: &str, most: usize) -> bool {
        if self.text.get(range.clone()).is_none() {
            return false;
        }
        let length = self
            .text
            .len()
            .saturating_sub(range.len())
            .saturating_add(with.len());
        if length > most {
            return false;
        }
        let capacity = self.text.capacity();
        if length > capacity {
            self.grant
                .more(&self.room, length.saturating_sub(capacity))
                .await;
            self.text
                .reserve_exact(length.saturating_sub(self.text.len()));
        }
        self.text.replace_range(range, with);
        true
    }
}

impl Room {
    /// How much of the shared part is free, in bytes.
    #[must_use]
    pub fn free(&self) -> usize {
        self.shared.available_permits()
    }

    /// An empty text of the service that may become `most` bytes long.
    #[must_use]
    pub fn gather(&self, most: usize) -> Gathering {
        Gathering {
            bytes: Vec::new(),
            grant: Grant::Shared(None),
            room: self.clone(),
            most,
        }
    }

    /// Take room for `text`, which already stands in memory: one of the relay's own lines or
    /// diagnostics, or a text a test double made. Its buffer is cut to its length first, so
    /// the room it takes is its length. Waits while there is none, so the caller must hold
    /// nothing that the turn's holder waits for.
    pub async fn hold(&self, mut text: String) -> Held {
        text.shrink_to_fit();
        let mut grant = Grant::Shared(None);
        grant.more(self, text.capacity()).await;
        Held {
            text,
            grant,
            room: self.clone(),
        }
    }

    /// A room whose shared part is `bytes` long.
    #[must_use]
    pub fn new(bytes: usize) -> Self {
        Self {
            shared: Arc::new(Semaphore::new(bytes.min(Semaphore::MAX_PERMITS))),
            turn: Arc::new(Semaphore::new(1)),
        }
    }

    /// A diagnostic of fixed words, which takes no room and never waits: what the relay has
    /// to say while an answer is still arriving. The queue it waits in bounds how many there
    /// are.
    #[must_use]
    pub fn remark(&self, text: &'static str) -> Held {
        Held {
            text: text.to_owned(),
            grant: Grant::Free,
            room: self.clone(),
        }
    }

    /// Wait until no text has the turn and none waits for it: until the shared part has been
    /// enough again for everything held.
    pub async fn vacancy(&self) {
        // The semaphore is never closed. The turn is given back at once: this only waits.
        drop(self.turn.acquire().await);
    }
}

#[cfg(test)]
mod mcp_stdio {
    use std::future::Future;
    use std::time::Duration;

    use super::Room;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// How often a test lets a future run before it says the future is waiting.
    const TURNS: usize = 200;

    /// `work` within ten seconds; a wait that never ends is a failed test, not an endless one.
    async fn soon<T>(work: impl Future<Output = T>) -> Result<T, &'static str> {
        tokio::time::timeout(Duration::from_secs(10), work)
            .await
            .map_err(|_| "a wait for room did not end")
    }

    /// Whether `work` is still waiting after it was given `TURNS` chances to finish.
    async fn waits<T>(work: &mut (impl Future<Output = T> + Unpin)) -> bool {
        for _ in 0..TURNS {
            tokio::select! {
                biased;
                _ = &mut *work => return false,
                () = tokio::task::yield_now() => {}
            }
        }
        true
    }

    /// Room is counted by capacity, taken as a text grows and given back when it is dropped,
    /// whether it was dropped as bytes or as a text.
    #[tokio::test]
    async fn room_is_taken_for_the_capacity_of_a_text_and_given_back() -> TestResult {
        let room = Room::new(100);
        let mut text = room.gather(1000);
        assert_eq!((room.free(), text.is_empty()), (100, true));
        soon(text.extend(b"0123456789")).await?;
        assert_eq!((room.free(), text.bytes().len()), (90, 10));
        // Thirty bytes are more than twice ten: the buffer is given what is needed.
        soon(text.extend(&[b'x'; 20])).await?;
        assert_eq!((room.free(), text.len()), (70, 30));
        // One byte more doubles it, so that gathering stays linear.
        soon(text.extend(b"y")).await?;
        assert_eq!(room.free(), 40);
        let held = text.into_text().ok_or("the bytes are text")?;
        assert_eq!((room.free(), held.len(), held.is_empty()), (40, 31, false));
        drop(held);
        assert_eq!(room.free(), 100);
        let mut dropped = room.gather(1000);
        soon(dropped.extend(b"abc")).await?;
        assert_eq!(room.free(), 97);
        drop(dropped);
        assert_eq!(room.free(), 100);
        Ok(())
    }

    /// A text that does not find its room in the shared part takes the turn and is held whole
    /// beside it; the next such text waits until the first is dropped. From both sides: a
    /// text that just fits takes no turn.
    #[tokio::test]
    async fn a_text_that_finds_no_room_takes_the_turn_and_the_next_one_waits() -> TestResult {
        let room = Room::new(10);
        let mut first = room.gather(1000);
        soon(first.extend(b"12345678")).await?;
        let mut fits = room.gather(1000);
        soon(fits.extend(b"12")).await?;
        assert_eq!(room.free(), 0);
        drop(fits);
        let mut second = room.gather(1000);
        soon(second.extend(b"123")).await?;
        // It has the turn: nothing of the shared part.
        assert_eq!((room.free(), second.len()), (2, 3));
        let mut third = room.gather(1000);
        {
            let mut waiting = Box::pin(third.extend(b"123"));
            assert!(waits(&mut waiting).await, "the turn was given twice");
            let mut watching = Box::pin(room.vacancy());
            assert!(waits(&mut watching).await, "the turn looked free");
            drop(second);
            soon(waiting).await?;
            assert!(waits(&mut watching).await, "the turn looked free");
        }
        assert_eq!((room.free(), third.len()), (2, 3));
        drop(third);
        soon(room.vacancy()).await?;
        Ok(())
    }

    /// A text that grows out of the shared part takes the turn and gives back what it held.
    #[tokio::test]
    async fn a_text_that_takes_the_turn_gives_its_shared_room_back() -> TestResult {
        let room = Room::new(10);
        let mut text = room.gather(1000);
        soon(text.extend(b"123456")).await?;
        assert_eq!(room.free(), 4);
        soon(text.extend(b"123456")).await?;
        assert_eq!((room.free(), text.len()), (10, 12));
        // The turn covers it whole: it grows on without asking.
        soon(text.extend(&[b'x'; 500])).await?;
        assert_eq!((room.free(), text.len()), (10, 512));
        Ok(())
    }

    /// The buffer of a text is never given more capacity than the longest text allowed, and
    /// a length the service announced is taken in one step, up to that and no further.
    #[tokio::test]
    async fn a_buffer_never_grows_past_the_longest_text() -> TestResult {
        let room = Room::new(1000);
        let mut text = room.gather(100);
        soon(text.extend(&[b'x'; 60])).await?;
        assert_eq!(room.free(), 940);
        soon(text.extend(b"y")).await?;
        assert_eq!(room.free(), 900);
        drop(text);
        let mut announced = room.gather(100);
        soon(announced.expect(40)).await?;
        assert_eq!(room.free(), 960);
        soon(announced.extend(&[b'x'; 40])).await?;
        assert_eq!(room.free(), 960);
        drop(announced);
        let mut overstated = room.gather(100);
        soon(overstated.expect(1_000_000)).await?;
        assert_eq!(room.free(), 900);
        Ok(())
    }

    /// A text that already stands in memory holds room for its length; a remark holds none.
    #[tokio::test]
    async fn a_line_of_the_relay_holds_room_and_a_remark_holds_none() -> TestResult {
        let room = Room::new(100);
        let mut line = String::with_capacity(40);
        line.push_str("a line");
        let held = soon(room.hold(line)).await?;
        assert_eq!((room.free(), held.as_str()), (94, "a line"));
        let remark = room.remark("said in passing");
        assert_eq!((room.free(), remark.as_str()), (94, "said in passing"));
        drop(held);
        assert_eq!(room.free(), 100);
        Ok(())
    }

    /// Bytes are moved and cut within the buffer, which is how an event's data is kept
    /// without a second copy.
    #[tokio::test]
    async fn bytes_are_moved_within_their_buffer() -> TestResult {
        let room = Room::new(100);
        let mut text = room.gather(100);
        soon(text.extend(b"keep|data: value")).await?;
        text.keep_from(11, 5);
        assert_eq!(text.bytes(), b"keep|value");
        text.truncate(4);
        assert_eq!(text.bytes(), b"keep");
        let taken = text.take();
        assert_eq!((taken.bytes(), text.len()), (b"keep".as_slice(), 0));
        assert_eq!(room.free(), 84);
        Ok(())
    }

    /// A text is spliced where it stands, takes room for what it grows by, and is left alone
    /// when it would become longer than allowed.
    #[tokio::test]
    async fn a_text_is_spliced_in_place_within_its_bound() -> TestResult {
        let room = Room::new(100);
        let mut held = soon(room.hold("{\"a\":null}".to_owned())).await?;
        let before = room.free();
        assert!(soon(held.splice(1..1, "\"id\":7,", 17)).await?);
        assert_eq!(held.as_str(), "{\"id\":7,\"a\":null}");
        assert_eq!(room.free(), before.saturating_sub(7));
        assert!(!soon(held.splice(1..1, "x", 17)).await?);
        assert!(soon(held.splice(12..16, "1", 17)).await?);
        assert_eq!(held.as_str(), "{\"id\":7,\"a\":1}");
        assert!(!soon(held.splice(40..41, "x", 100)).await?);
        Ok(())
    }
}
