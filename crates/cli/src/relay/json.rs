//! Reading a JSON text in one pass, without building anything from it.
//!
//! A [`Reader`] is given a text one character at a time. It checks that the text is exactly
//! one JSON value by the grammar of RFC 8259, says of each character whether it stays, and
//! notes the few places the relay needs ([`Noted`]): where the top-level object opens, where
//! the values of its members `jsonrpc`, `id`, `method`, `result`, `error`, `code` and
//! `message` stand, and where `protocolVersion` stands inside `result` and `message` inside
//! `error`. That is all it keeps of the text: no record of any other member, no number or
//! string read into a value, and of a key only as many characters as the longest name it
//! looks for. Besides the notes it holds one bit for each array or object that is open.
//!
//! Invariants:
//! - A line feed or carriage return goes only where it stands between tokens. Inside a string
//!   a raw one is not JSON and the text is refused; a string holds them only as escapes. So
//!   removing them changes no value. Every other byte stays, blanks and tabs included.
//! - When a member occurs twice the last one counts, as it does for the JSON parsers a client
//!   is likely to use; what an earlier `result` or `error` held inside is forgotten with it.
//! - A key is the characters it spells, whatever escapes spell them.
//! - The places noted are places in the text that is kept.
//! - A number or a string is checked by the grammar alone, so a number of any size and an
//!   escape that stands for half a character pass, as they do for the grammar.
//!
//! Known limits: there is no limit on how deep a text may be nested. A text nested as deep as
//! it is long costs one bit for each level, an eighth of its length.

/// What is still to come of a number, by what was read last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Digits {
    /// The minus sign.
    Minus,
    /// A leading zero, after which no digit may follow.
    Zero,
    /// A digit of the whole part.
    Whole,
    /// The point.
    Point,
    /// A digit after the point.
    Fraction,
    /// The `e` or `E`.
    Exponent,
    /// The sign of the exponent.
    Sign,
    /// A digit of the exponent.
    Power,
}

/// A key while it is read, as far as it may still be one of the names looked for.
#[derive(Debug, Default)]
struct Key {
    spelled: String,
    /// Whether the key is looked at and has not yet turned out to be another.
    watched: bool,
}

/// The names the reader looks for among the members of the top-level object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Name {
    Code,
    Error,
    Id,
    Jsonrpc,
    Message,
    Method,
    Result,
}

/// What the reader noted of one JSON text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Noted {
    /// Where the members of the top-level object start, just after its opening brace; `None`
    /// when the text is not an object.
    pub(super) opening: Option<usize>,
    /// The value of the member `code`.
    pub(super) code: Option<Span>,
    /// The value of the member `error`.
    pub(super) error: Option<Span>,
    /// The value of `message` inside the `error` that counts, when that is an object.
    pub(super) error_message: Option<Span>,
    /// The value of the member `id`.
    pub(super) id: Option<Span>,
    /// The value of the member `jsonrpc`.
    pub(super) jsonrpc: Option<Span>,
    /// The value of the member `message`.
    pub(super) message: Option<Span>,
    /// The value of the member `method`.
    pub(super) method: Option<Span>,
    /// The value of the member `result`.
    pub(super) result: Option<Span>,
    /// The value of `protocolVersion` inside the `result` that counts, when that is an object.
    pub(super) version: Option<Span>,
}

/// Where the reader is inside a string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    /// Among its characters.
    Plain,
    /// After a backslash.
    Escape,
    /// Inside the four digits of a `\u` escape: how many are still to come, and the code so far.
    Hex { left: u8, code: u32 },
}

/// Reads one JSON text, a character at a time.
#[derive(Debug)]
pub(super) struct Reader {
    /// Whether the line feeds and carriage returns between tokens are dropped.
    one_line: bool,
    state: State,
    /// One bit for each array or object that is open, set for an object; the innermost last.
    open: Vec<u64>,
    depth: usize,
    /// How many bytes of the text were kept so far.
    kept: usize,
    key: Key,
    /// The name of the top-level member being read, when it is one looked for.
    member: Option<Name>,
    /// Where the value of the top-level member being read starts.
    member_at: usize,
    /// The name looked for inside the top-level member being read, when that is an object.
    inside: Option<&'static str>,
    /// Whether the member being read inside it has that name.
    inside_found: bool,
    /// Where the value of the member being read inside it starts.
    inside_at: usize,
    noted: Noted,
}

/// Where a value stands in the text that is kept: its first byte, and the byte after its last.
pub(super) type Span = (usize, usize);

/// What the reader expects next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// A value.
    Value,
    /// A value, or the end of an array that holds none.
    First,
    /// A key, or the end of an object that holds none.
    Opened,
    /// A key.
    Key,
    /// The colon after a key.
    Colon,
    /// After a value: a comma, the end of what holds it, or the end of the text.
    After,
    /// The rest of a string, which is a key or a value.
    Text { key: bool, part: Part },
    /// The rest of a number.
    Number(Digits),
    /// The rest of `true`, `false` or `null`.
    Word(&'static str),
    /// Nothing: the text has turned out not to be JSON.
    Failed,
}

/// The longest name the reader looks for, in bytes: `protocolVersion`.
const LONGEST_NAME: usize = 15;

impl Digits {
    /// The number after one more character; `None` when the character is no part of it.
    fn then(self, character: char) -> Option<Self> {
        Some(match (self, character) {
            (Self::Minus, '0') => Self::Zero,
            (Self::Minus, '1'..='9') | (Self::Whole, '0'..='9') => Self::Whole,
            (Self::Zero | Self::Whole, '.') => Self::Point,
            (Self::Point | Self::Fraction, '0'..='9') => Self::Fraction,
            (Self::Zero | Self::Whole | Self::Fraction, 'e' | 'E') => Self::Exponent,
            (Self::Exponent, '+' | '-') => Self::Sign,
            (Self::Exponent | Self::Sign | Self::Power, '0'..='9') => Self::Power,
            _ => return None,
        })
    }

    /// Whether what was read so far is a whole number by the grammar.
    fn whole(self) -> bool {
        matches!(
            self,
            Self::Zero | Self::Whole | Self::Fraction | Self::Power
        )
    }
}

impl Key {
    /// The key turned out to be none of the names.
    fn other(&mut self) {
        self.watched = false;
    }

    /// One more character of the key.
    fn push(&mut self, character: char) {
        if !self.watched {
            return;
        }
        if self.spelled.len() < LONGEST_NAME {
            self.spelled.push(character);
        } else {
            self.watched = false;
        }
    }

    /// The key, when it was looked at and is short enough to be one of the names.
    fn spelled(&self) -> Option<&str> {
        self.watched.then_some(self.spelled.as_str())
    }

    /// A new key starts; `watched` says whether it stands where a name is looked for.
    fn start(&mut self, watched: bool) {
        self.spelled.clear();
        self.watched = watched;
    }
}

impl Name {
    /// The name looked for inside this member, when it has one.
    fn inside(self) -> Option<&'static str> {
        match self {
            Self::Result => Some("protocolVersion"),
            Self::Error => Some("message"),
            _ => None,
        }
    }

    fn of(key: &str) -> Option<Self> {
        Some(match key {
            "code" => Self::Code,
            "error" => Self::Error,
            "id" => Self::Id,
            "jsonrpc" => Self::Jsonrpc,
            "message" => Self::Message,
            "method" => Self::Method,
            "result" => Self::Result,
            _ => return None,
        })
    }
}

impl Reader {
    /// A value starts with `character` at the place the kept text has reached.
    fn begin(&mut self, character: char) {
        match self.depth {
            0 if character == '{' => self.noted.opening = Some(self.kept.saturating_add(1)),
            1 if self.noted.opening.is_some() => {
                self.member_at = self.kept;
                self.inside = self
                    .member
                    .and_then(Name::inside)
                    .filter(|_| character == '{');
                // The last member of a name counts: what an earlier one held inside does not.
                match self.member {
                    Some(Name::Result) => self.noted.version = None,
                    Some(Name::Error) => self.noted.error_message = None,
                    _ => {}
                }
            }
            2 if self.inside.is_some() => self.inside_at = self.kept,
            _ => {}
        }
    }

    /// The array or object that is open ends; `object` says which the character closes.
    fn close(&mut self, object: bool) -> Option<()> {
        if self.innermost()? != object {
            return None;
        }
        self.depth = self.depth.checked_sub(1)?;
        self.end(self.kept.saturating_add(1));
        Some(())
    }

    /// A value has ended at `end` in the kept text.
    fn end(&mut self, end: usize) {
        self.state = State::After;
        match self.depth {
            1 if self.noted.opening.is_some() => {
                let place = Some((self.member_at, end));
                match self.member.take() {
                    Some(Name::Code) => self.noted.code = place,
                    Some(Name::Error) => self.noted.error = place,
                    Some(Name::Id) => self.noted.id = place,
                    Some(Name::Jsonrpc) => self.noted.jsonrpc = place,
                    Some(Name::Message) => self.noted.message = place,
                    Some(Name::Method) => self.noted.method = place,
                    Some(Name::Result) => self.noted.result = place,
                    None => {}
                }
                self.inside = None;
            }
            2 if self.inside_found => {
                let place = Some((self.inside_at, end));
                match self.inside {
                    Some("protocolVersion") => self.noted.version = place,
                    Some("message") => self.noted.error_message = place,
                    _ => {}
                }
                self.inside_found = false;
            }
            _ => {}
        }
    }

    /// What was noted, when the text was exactly one JSON value.
    pub(super) fn finish(mut self) -> Option<Noted> {
        // A number has no closing character: the end of the text ends it.
        if let State::Number(digits) = self.state {
            if !digits.whole() {
                return None;
            }
            self.end(self.kept);
        }
        (self.state == State::After && self.depth == 0).then_some(self.noted)
    }

    /// Whether the innermost array or object that is open is an object.
    fn innermost(&self) -> Option<bool> {
        let level = self.depth.checked_sub(1)?;
        let word = self.open.get(level / 64)?;
        Some(word & bit(level) != 0)
    }

    /// A key has ended: say what member it names.
    fn named(&mut self) {
        match self.depth {
            1 if self.noted.opening.is_some() => {
                self.member = self.key.spelled().and_then(Name::of);
            }
            2 if self.inside.is_some() => {
                self.inside_found =
                    self.key.spelled().is_some() && self.key.spelled() == self.inside;
            }
            _ => {}
        }
    }

    /// A reader of one text. `one_line` says whether the line feeds and carriage returns
    /// between its tokens are dropped; otherwise every character stays.
    pub(super) fn new(one_line: bool) -> Self {
        Self {
            one_line,
            state: State::Value,
            open: Vec::new(),
            depth: 0,
            kept: 0,
            key: Key::default(),
            member: None,
            member_at: 0,
            inside: None,
            inside_found: false,
            inside_at: 0,
            noted: Noted::default(),
        }
    }

    /// An array or object opens.
    fn open(&mut self, object: bool) {
        let level = self.depth;
        if level / 64 == self.open.len() {
            self.open.push(0);
        }
        if let Some(word) = self.open.get_mut(level / 64) {
            *word = if object {
                *word | bit(level)
            } else {
                *word & !bit(level)
            };
        }
        self.depth = level.saturating_add(1);
    }

    /// Take the next character of the text; returns whether it stays in the text.
    ///
    /// Once the text has turned out not to be JSON every character stays: the text is of no
    /// use then, and [`Reader::finish`] says so.
    pub(super) fn step(&mut self, character: char) -> bool {
        if self.state == State::Failed {
            return true;
        }
        let stays = self.take(character).unwrap_or_else(|| {
            self.state = State::Failed;
            true
        });
        if stays {
            self.kept = self.kept.saturating_add(character.len_utf8());
        }
        stays
    }

    /// One character inside a string.
    fn string(&mut self, character: char, key: bool, part: Part) -> Option<()> {
        let part = match (part, character) {
            (Part::Plain, '"') => {
                if key {
                    self.named();
                    self.state = State::Colon;
                } else {
                    self.end(self.kept.saturating_add(1));
                }
                return Some(());
            }
            (Part::Plain, '\\') => Part::Escape,
            // A raw control character, a line feed among them, is not JSON inside a string.
            (Part::Plain, raw) if raw < ' ' => return None,
            (Part::Plain, plain) | (Part::Escape, plain @ ('"' | '\\' | '/')) => {
                if key {
                    self.key.push(plain);
                }
                Part::Plain
            }
            (Part::Escape, 'u') => Part::Hex { left: 4, code: 0 },
            (Part::Escape, 'b' | 'f' | 'n' | 'r' | 't') => {
                // A control character: no name holds one.
                self.key.other();
                Part::Plain
            }
            (Part::Escape, _) => return None,
            (Part::Hex { left, code }, digit) => {
                let code = code.saturating_mul(16).saturating_add(digit.to_digit(16)?);
                match left.checked_sub(1) {
                    Some(0) | None => {
                        // Every name is ASCII: an escape for anything else spells another key.
                        match char::from_u32(code).filter(char::is_ascii) {
                            Some(spelled) if key => self.key.push(spelled),
                            _ => self.key.other(),
                        }
                        Part::Plain
                    }
                    Some(left) => Part::Hex { left, code },
                }
            }
        };
        self.state = State::Text { key, part };
        Some(())
    }

    /// Take one character; `None` when the text is not JSON, else whether the character stays.
    fn take(&mut self, character: char) -> Option<bool> {
        if let State::Number(digits) = self.state {
            if let Some(digits) = digits.then(character) {
                self.state = State::Number(digits);
                return Some(true);
            }
            // A number has no closing character: the first that is no part of it ends it.
            if !digits.whole() {
                return None;
            }
            self.end(self.kept);
        }
        match self.state {
            State::Text { key, part } => self.string(character, key, part).map(|()| true),
            State::Word(rest) => {
                let rest = rest.strip_prefix(character)?;
                if rest.is_empty() {
                    self.end(self.kept.saturating_add(1));
                } else {
                    self.state = State::Word(rest);
                }
                Some(true)
            }
            _ => match character {
                ' ' | '\t' => Some(true),
                '\n' | '\r' => Some(!self.one_line),
                _ => self.token(character).map(|()| true),
            },
        }
    }

    /// One character that starts or joins tokens.
    fn token(&mut self, character: char) -> Option<()> {
        match (self.state, character) {
            (State::Value | State::First, '{' | '[') => {
                self.begin(character);
                self.open(character == '{');
                self.state = if character == '{' {
                    State::Opened
                } else {
                    State::First
                };
            }
            (State::Value | State::First, '"') => {
                self.begin(character);
                self.state = State::Text {
                    key: false,
                    part: Part::Plain,
                };
            }
            (State::Value | State::First, '-' | '0'..='9') => {
                self.begin(character);
                self.state = State::Number(match character {
                    '-' => Digits::Minus,
                    '0' => Digits::Zero,
                    _ => Digits::Whole,
                });
            }
            (State::Value | State::First, 't' | 'f' | 'n') => {
                self.begin(character);
                self.state = State::Word(match character {
                    't' => "rue",
                    'f' => "alse",
                    _ => "ull",
                });
            }
            (State::Opened | State::Key, '"') => {
                let watched = match self.depth {
                    1 => self.noted.opening.is_some(),
                    2 => self.inside.is_some(),
                    _ => false,
                };
                self.key.start(watched);
                self.state = State::Text {
                    key: true,
                    part: Part::Plain,
                };
            }
            (State::Colon, ':') => self.state = State::Value,
            (State::After, ',') => {
                self.state = if self.innermost()? {
                    State::Key
                } else {
                    State::Value
                };
            }
            (State::First | State::After, ']') => self.close(false)?,
            (State::Opened | State::After, '}') => self.close(true)?,
            _ => return None,
        }
        Some(())
    }
}

/// The bit that stands for the array or object at `level`.
fn bit(level: usize) -> u64 {
    // A shift by less than 64 always has a value.
    1_u64
        .checked_shl(u32::try_from(level % 64).unwrap_or(0))
        .unwrap_or(0)
}

/// What a reader notes of `text` as it stands, when the text is one JSON value; the places
/// noted are places in `text`.
pub(super) fn noted(text: &str) -> Option<Noted> {
    let mut reader = Reader::new(false);
    for character in text.chars() {
        reader.step(character);
    }
    reader.finish()
}

#[cfg(test)]
mod mcp_stdio {
    use serde::de::IgnoredAny;

    use super::{Noted, Reader, noted};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn some<T>(value: Option<T>, what: &str) -> Result<T, String> {
        value.ok_or_else(|| format!("expected {what} to be present"))
    }

    /// `text` made one line, and what the reader noted of it.
    fn one_line(text: &str) -> (String, Option<Noted>) {
        let mut reader = Reader::new(true);
        let mut line = text.to_owned();
        line.retain(|character| reader.step(character));
        (line, reader.finish())
    }

    /// The text that stands at `place` in `line`.
    fn at(line: &str, place: Option<(usize, usize)>) -> Option<&str> {
        place.and_then(|(start, end)| line.get(start..end))
    }

    /// A step of a small generator of numbers, so the texts below are the same on every run.
    fn next(seed: &mut u64) -> usize {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        usize::try_from(*seed % 1_000_003).unwrap_or(0)
    }

    /// The members are found as written, whatever stands inside their values: a string that
    /// holds a brace, a quote or a backslash, nested objects and arrays, numbers of any size.
    /// The line breaks between tokens go and nothing else does, and every place noted is a
    /// place in the line that is kept.
    #[test]
    fn members_are_found_where_they_stand_in_the_line_that_is_kept() -> TestResult {
        let text = concat!(
            " {\n \"method\" : \"}\\\"]\\\\\" ,\r\n\"result\":{\"x\":[1,{\"y\":\"]\"}],\"z\":\"\\\\\"},",
            "\"error\":[\"[\",\"\\\"\"],\"id\":-1.5e400,\"jsonrpc\":true,\n\"code\"\n:\nnull\n}\r\n"
        );
        let (line, found) = one_line(text);
        let found = some(found, "one JSON value")?;
        assert_eq!(
            line,
            concat!(
                " { \"method\" : \"}\\\"]\\\\\" ,\"result\":{\"x\":[1,{\"y\":\"]\"}],\"z\":\"\\\\\"},",
                "\"error\":[\"[\",\"\\\"\"],\"id\":-1.5e400,\"jsonrpc\":true,\"code\":null}"
            )
        );
        assert_eq!(found.opening, Some(2));
        assert_eq!(at(&line, found.method), Some("\"}\\\"]\\\\\""));
        assert_eq!(
            at(&line, found.result),
            Some("{\"x\":[1,{\"y\":\"]\"}],\"z\":\"\\\\\"}")
        );
        assert_eq!(at(&line, found.error), Some("[\"[\",\"\\\"\"]"));
        assert_eq!(at(&line, found.id), Some("-1.5e400"));
        assert_eq!(at(&line, found.jsonrpc), Some("true"));
        assert_eq!(at(&line, found.code), Some("null"));
        assert_eq!(
            (found.message, found.version, found.error_message),
            (None, None, None)
        );
        // Read as it stands, the same members are found at their places in the text itself.
        let standing = some(noted(text), "one JSON value")?;
        assert_eq!(at(text, standing.id), Some("-1.5e400"));
        assert_eq!(at(text, standing.code), Some("null"));
        assert_eq!(standing.opening, Some(2));
        Ok(())
    }

    /// When a member occurs twice the last one counts, and so does what the last `result` or
    /// `error` holds inside, not what an earlier one held.
    #[test]
    fn the_last_of_two_equal_keys_counts() -> TestResult {
        let text = r#"{"id":1,"result":{"protocolVersion":"a","protocolVersion":"b"},"id":2,"error":{"message":"first"},"error":{"message":"m","data":{"message":"deeper"}}}"#;
        let found = some(noted(text), "one JSON value")?;
        assert_eq!(at(text, found.id), Some("2"));
        assert_eq!(at(text, found.version), Some("\"b\""));
        assert_eq!(at(text, found.error_message), Some("\"m\""));
        let replaced = r#"{"result":{"protocolVersion":"a"},"error":{"message":"m"},"result":[],"error":"no"}"#;
        let found = some(noted(replaced), "one JSON value")?;
        assert_eq!((found.version, found.error_message), (None, None));
        assert_eq!(
            (at(replaced, found.result), at(replaced, found.error)),
            (Some("[]"), Some("\"no\""))
        );
        // A name is looked for among the members of the top-level object and nowhere deeper.
        let deeper = r#"{"params":{"id":7,"result":{"protocolVersion":"x"}},"a":[{"id":8}]}"#;
        assert_eq!(
            some(noted(deeper), "one JSON value")?,
            Noted {
                opening: Some(1),
                ..Noted::default()
            }
        );
        // An array has no members.
        assert_eq!(
            some(noted(r#"[{"id":1}]"#), "one JSON value")?,
            Noted::default()
        );
        Ok(())
    }

    /// A key is the characters it spells: an escape for a letter spells that letter, and an
    /// escape for anything else, or a key that only starts like a name, is another key.
    #[test]
    fn a_key_spelled_with_escapes_is_the_key_it_spells() -> TestResult {
        let text = "{\"\\u0069d\":1,\"\\u006Aso\\u006erpc\":2,\"met\\/hod\":3,\"i\\u00e9\":4,\"id\\n\":5,\"idx\":6,\"protocolVersionX\":7,\"\\u0131d\":8}";
        let found = some(noted(text), "one JSON value")?;
        assert_eq!(at(text, found.id), Some("1"));
        assert_eq!(at(text, found.jsonrpc), Some("2"));
        assert_eq!(found.method, None);
        let inside = "{\"result\":{\"protocol\\u0056ersion\":\"v\",\"protocolVersio\":1}}";
        let found = some(noted(inside), "one JSON value")?;
        assert_eq!(at(inside, found.version), Some("\"v\""));
        Ok(())
    }

    /// A tab, a blank, a line feed and a carriage return are white space between tokens; the
    /// tab and the blank stay, the line breaks go, and nothing else counts as white space.
    #[test]
    fn white_space_between_tokens_is_the_four_characters_of_the_grammar() -> TestResult {
        let text = "\t{\t\"id\"\t:\t[\t1\t,\t2\t]\t,\r\n\"a\" : { } }\t\n";
        let (line, found) = one_line(text);
        assert_eq!(line, "\t{\t\"id\"\t:\t[\t1\t,\t2\t]\t,\"a\" : { } }\t");
        assert_eq!(
            at(&line, some(found, "one JSON value")?.id),
            Some("[\t1\t,\t2\t]")
        );
        for other in ["\u{b}", "\u{c}", "\u{a0}", "\u{feff}", "\u{2028}"] {
            assert_eq!(noted(&format!("{other}{{}}")), None, "{other:?} before");
            assert_eq!(noted(&format!("{{}}{other}")), None, "{other:?} after");
            assert_eq!(noted(&format!("[1,{other}2]")), None, "{other:?} between");
        }
        Ok(())
    }

    /// One JSON value and nothing else; the grammar decides, not what a number type can hold.
    #[test]
    fn one_json_value_is_told_from_everything_else() {
        for text in [
            "{\"n\":12345678901234567890123}",
            "{\"n\":1e400,\"m\":-0,\"k\":1E2}",
            "[0,-0,1.5,1e5,1E+5,1e-5,-1e-400,0.0,0e0,10,-10.01e+00]",
            "{\"a\":1,\"a\":2}",
            " [1, 2]\n",
            "\"a\\nb\"",
            "\"\\ud800 \\udc00\\ud800 \\u0000 \\/ \\b\\f\\n\\r\\t\\\\\\\"\"",
            "\"\u{7f}\u{2028}\u{85}\"",
            "0",
            "-0",
            "1e5",
            "true",
            "false",
            "null",
            "[]",
            "{}",
            "[[],{}]",
            "[{\"a\":[]},[{}]]",
        ] {
            assert!(noted(text).is_some(), "{text}");
        }
        for text in [
            "",
            " ",
            "{} {}",
            "{}{}",
            "{\"a\":1}x",
            "{\"a\":01}",
            "[+1]",
            "[.5]",
            "[1.]",
            "[1e]",
            "[1e+]",
            "[0x10]",
            "[NaN]",
            "[-Infinity]",
            "[-]",
            "[--1]",
            "-",
            "1.",
            "2e",
            "tru",
            "truex",
            "nul",
            "[1,]",
            "[,1]",
            "[1 2]",
            "{\"a\":1,}",
            "{,}",
            "{\"a\"}",
            "{\"a\":}",
            "{\"a\" 1}",
            "{1:2}",
            "{'a':1}",
            "[1}",
            "{\"a\":1]",
            "]",
            "}",
            "[",
            "[1",
            "{\"a\":[1,2}",
            "{\"a\":\"raw\nline feed\"}",
            "{\"a\":\"raw\rcarriage return\"}",
            "{\"a\":\"raw\u{1}control\"}",
            "{\"a\":\"bad \\x escape\"}",
            "{\"a\":\"short \\u12\"}",
            "{\"a\":\"not hex \\u12g4\"}",
            "{\"a\":\"open",
            "{\"a\":\"open\\\"}",
            "[1] // x",
            "\u{feff}{}",
            "not json",
        ] {
            assert_eq!(noted(text), None, "{text}");
        }
    }

    /// The reader and the JSON library agree on which texts are one JSON value: twenty
    /// thousand texts made by cutting, swapping and splicing the characters of valid ones.
    #[test]
    fn the_reader_agrees_with_the_json_library_on_what_is_json() {
        let seeds = [
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"a\":[1,-2.5e+3,true,false,null],\"s\":\"x\\n\\u00e9\\\"\"}}",
            "[{\"k\":{}},[],\"\",0,-0.0,1E9,{\"\\\\\":\"\\/\"}]",
            " {\t\"a\" :\r\n[ 10 , 0.1 ] , \"b\":\"\\ud83d\\ude00\" } ",
        ];
        let pieces: Vec<char> = "{}[]\",:\\ \n\t0123456789.-+eEtrufalsn/bx"
            .chars()
            .collect();
        let mut seed = 0x2545_F491_4F6C_DD1D_u64;
        let mut valid = 0_usize;
        for round in 0..20_000_usize {
            let from = seeds.get(round % seeds.len()).copied().unwrap_or("[]");
            let mut text: Vec<char> = from.chars().collect();
            for _ in 0..(next(&mut seed) % 3) {
                let length = text.len().max(1);
                let place = next(&mut seed) % length;
                match next(&mut seed) % 4 {
                    0 => {
                        if place < text.len() {
                            text.remove(place);
                        }
                    }
                    1 => {
                        let piece = pieces.get(next(&mut seed) % pieces.len()).copied();
                        text.insert(place.min(text.len()), piece.unwrap_or(' '));
                    }
                    2 => {
                        let other = next(&mut seed) % length;
                        if place < text.len() && other < text.len() {
                            text.swap(place, other);
                        }
                    }
                    _ => text.truncate(place),
                }
            }
            let text: String = text.into_iter().collect();
            let library = serde_json::from_str::<IgnoredAny>(&text).is_ok();
            assert_eq!(noted(&text).is_some(), library, "{text:?}");
            assert_eq!(one_line(&text).1.is_some(), library, "{text:?}");
            valid = valid.saturating_add(usize::from(library));
        }
        // The texts are of both kinds, so the agreement says something about each.
        assert!(
            valid > 2_000 && valid < 18_000,
            "{valid} of 20,000 were valid"
        );
    }

    /// Depth costs a bit for each level and no recursion: a text nested a hundred thousand
    /// deep, in arrays and objects by turns, is read, and one closed in the wrong order is
    /// refused.
    #[test]
    fn a_deeply_nested_text_is_read_and_its_brackets_are_matched() {
        let depth = 100_000;
        let deep = format!("{}1{}", "[{\"a\":".repeat(depth), "}]".repeat(depth));
        assert!(noted(&deep).is_some());
        let crossed = format!("{}1{}", "[{\"a\":".repeat(depth), "]}".repeat(depth));
        assert_eq!(noted(&crossed), None);
        // An object opened where an array stood before at that depth is still an object.
        assert!(noted("[[1],{\"a\":1},[2],{\"b\":[{\"c\":2}]}]").is_some());
        assert_eq!(noted("[[1],{\"a\":1,2}]"), None);
    }
}
