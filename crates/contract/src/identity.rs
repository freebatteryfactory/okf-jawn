//! Validated wire identities distinguish requested state from resolved content.
//!
//! An `At::Latest` selector must be resolved once before a read returns `Revision`. A
//! `Timestamp` has exactly one spelling, so comparing two as strings compares the instants.
//! A `WorkspacePath` is stored in Unicode NFC and refuses what Windows cannot store; two paths
//! collide when their `collision_key`s are equal.

use std::fmt::{Display, Formatter};
use std::num::NonZero;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::format_description::well_known::iso8601::{Config, EncodedConfig, TimePrecision};
use time::format_description::well_known::{Iso8601, Rfc3339};
use time::{OffsetDateTime, UtcOffset};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

macro_rules! uuid_id {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
            JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(#[doc = "Underlying UUID."] pub Uuid);
    };
}
uuid_id!(
    WorkspaceId,
    "Stable application identity for one user-organized workspace."
);
uuid_id!(
    ItemId,
    "Stable item identity independent of its current relative path."
);
uuid_id!(
    UploadId,
    "Identity of one authenticated source upload occurrence."
);
uuid_id!(
    JobId,
    "Durable background work identity retained across retries."
);
uuid_id!(
    ProposalId,
    "Identity of a suggested change set, not a review."
);
uuid_id!(
    ReviewId,
    "Identity of an explicit revision-bound review action."
);
uuid_id!(
    ReceiptId,
    "Identity of a durable application operation record."
);
uuid_id!(
    ArtifactId,
    "Identity of a retained export or backup artifact."
);
uuid_id!(
    ConnectorId,
    "Identity of one local MCP connector credential, never the owner's browser session."
);
uuid_id!(
    IdempotencyKey,
    "Caller-chosen retry identity for one write, scoped to tenant, subject and operation."
);
uuid_id!(
    MutationId,
    "Durable server-assigned identity of one write; every store that creates a row enforces it as unique."
);
uuid_id!(
    ConfirmationId,
    "Identity of one session-bound human confirmation challenge."
);
uuid_id!(
    PurgeId,
    "Identity of one purge; its record outlives what it removed."
);

/// Tenant boundary: the WorkOS organization id when hosted, a fixed installation id locally.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TenantId(String);

/// A resolved Git commit, never a branch name or the string `latest`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Revision(String);

/// A lower-case SHA-256 identifier for immutable bytes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Digest(String);

/// A normalized relative workspace path; this alone is not filesystem authorization.
///
/// Every `/`-separated segment is non-empty, is not `.`, `..` or `.git` (any case), holds no
/// control character and none of `\ : < > " | ? *`, does not end in `.` or a space, and has a
/// stem (the segment up to its first `.`) that is not a Windows reserved device name such as
/// `CON`, `NUL`, `COM1` or `CONIN$`, compared ASCII case-insensitively. The whole value is in
/// Unicode NFC and at most 4096 bytes. A non-NFC value is refused, never changed on parse;
/// `from_supplied` normalizes a supplied filename first.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct WorkspacePath(String);

/// A UTC instant in one canonical RFC 3339 spelling: `YYYY-MM-DDTHH:MM:SS.sssZ`, always three
/// fraction digits and the `Z` offset, such as `2026-10-08T14:03:07.250Z`.
///
/// Every value has the same width, so comparing the strings compares the instants. Any other
/// spelling of the same instant is refused: no fraction, another fraction width, an offset
/// other than `Z` (`+00:00` included), a lower-case `t` or `z`, a leap second (`:60`), or a
/// date that does not exist. Producers write with `Timestamp::from_utc`, which truncates to the
/// millisecond, so a value is never later than the instant it records.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Timestamp(String);

/// A requested version selector; resolved responses contain a `Revision` instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum At {
    /// Resolve the current accepted workspace head exactly once.
    Latest,
    /// Read an explicitly identified historical snapshot.
    Revision {
        /// Exact resolved commit identity.
        revision: Revision,
    },
}

/// A typed identity parsing error suitable for a validation response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityError(pub &'static str);

/// The part of the workspace-path rule a regular expression can state: `/`-separated
/// segments, none empty, none containing `\`, `:`, `<`, `>`, `"`, `|`, `?`, `*` or a C0
/// control. Not expressible by the pattern, and enforced by `TryFrom<String>`: the `.`, `..`
/// and `.git` segment rules, the trailing `.` and space rule, the reserved device names, NFC,
/// DEL and C1 controls, and the byte length.
const WORKSPACE_PATH_PATTERN: &str = r#"^[^/\\:<>"|?*\x00-\x1f]+(/[^/\\:<>"|?*\x00-\x1f]+)*$"#;
/// Characters Windows refuses in a file name, besides controls and `/`.
const RESERVED_CHARACTERS: [char; 8] = ['\\', ':', '<', '>', '"', '|', '?', '*'];
/// Windows device names: a segment whose stem is one of these, in any ASCII case, is refused.
const RESERVED_STEMS: [&str; 32] = [
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "COM0",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9",
    "LPT0",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9",
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
    "CONIN$",
    "CONOUT$",
];
/// The schema pattern of a `Timestamp`. Minutes and seconds are `00` to `59`, so a leap second
/// (`:60`) is refused by the pattern as well as by the parse.
const TIMESTAMP_PATTERN: &str =
    r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-5][0-9]:[0-5][0-9]\.[0-9]{3}Z$";
/// ISO 8601 with separators, a four-digit year, seconds with three fraction digits, and `Z` for
/// the UTC offset; the fraction is truncated, never rounded.
const TIMESTAMP_FORMAT: EncodedConfig = Config::DEFAULT
    .set_time_precision(TimePrecision::Second {
        decimal_digits: NonZero::new(3),
    })
    .encode();

impl JsonSchema for Revision {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Revision".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type": "string", "pattern": "^([0-9a-f]{40}|[0-9a-f]{64})$"})
    }
}
impl JsonSchema for Digest {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Digest".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type": "string", "pattern": "^[0-9a-f]{64}$"})
    }
}
impl JsonSchema for WorkspacePath {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "WorkspacePath".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "minLength": 1,
            "maxLength": 4096,
            "pattern": WORKSPACE_PATH_PATTERN
        })
    }
}
impl JsonSchema for Timestamp {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Timestamp".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "format": "date-time",
            "pattern": TIMESTAMP_PATTERN
        })
    }
}
impl JsonSchema for TenantId {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "TenantId".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type": "string", "pattern": "^[A-Za-z0-9_-]{1,128}$"})
    }
}
impl TenantId {
    /// Return the validated tenant identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for TenantId {
    type Error = IdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let valid = (1..=128).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
        if valid {
            Ok(Self(value))
        } else {
            Err(IdentityError(
                "tenant id must be 1 to 128 ASCII letters, digits, '_' or '-'",
            ))
        }
    }
}
impl From<TenantId> for String {
    fn from(value: TenantId) -> Self {
        value.0
    }
}
impl FromStr for TenantId {
    type Err = IdentityError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value.to_owned())
    }
}
impl Revision {
    /// Return the canonical lower-case commit identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl Digest {
    /// Return the canonical lower-case SHA-256 identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl WorkspacePath {
    /// Return the validated slash-separated relative path.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Build a path from a supplied filename or folder: convert it to NFC, then apply every
    /// rule of `TryFrom<String>`. The supplied spelling itself is kept elsewhere unchanged.
    ///
    /// # Errors
    /// Returns the rule the normalized value breaks.
    pub fn from_supplied(value: &str) -> Result<Self, IdentityError> {
        Self::try_from(value.nfc().collect::<String>())
    }

    /// The key two paths collide on: the NFC value with every segment lower-cased by
    /// locale-independent Unicode lowercasing. Two items may not hold paths with one key.
    #[must_use]
    pub fn collision_key(&self) -> String {
        self.0.nfc().collect::<String>().to_lowercase()
    }
}
impl Timestamp {
    /// Return the canonical spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Write an instant in the canonical spelling: converted to UTC and truncated to the
    /// millisecond, never rounded.
    ///
    /// # Errors
    /// Returns an error when the UTC year lies outside 0000 to 9999, which RFC 3339 cannot write.
    pub fn from_utc(instant: OffsetDateTime) -> Result<Self, IdentityError> {
        let utc = instant
            .checked_to_offset(UtcOffset::UTC)
            .ok_or(IdentityError(
                "timestamp lies outside the representable range",
            ))?;
        utc.format(&Iso8601::<TIMESTAMP_FORMAT>)
            .map(Self)
            .map_err(|_| IdentityError("timestamp year must be between 0000 and 9999"))
    }

    /// The instant this value records.
    ///
    /// # Errors
    /// Returns an error only if the stored spelling stopped parsing, which validation prevents.
    pub fn instant(&self) -> Result<OffsetDateTime, IdentityError> {
        OffsetDateTime::parse(&self.0, &Rfc3339)
            .map_err(|_| IdentityError("timestamp is not an RFC 3339 instant"))
    }
}
impl TryFrom<String> for Revision {
    type Error = IdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if matches!(value.len(), 40 | 64) && is_lower_hex(&value) {
            Ok(Self(value))
        } else {
            Err(IdentityError(
                "revision must be a complete lower-case Git commit hash",
            ))
        }
    }
}
impl TryFrom<String> for Digest {
    type Error = IdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() == 64 && is_lower_hex(&value) {
            Ok(Self(value))
        } else {
            Err(IdentityError(
                "digest must be 64 lower-case SHA-256 hex characters",
            ))
        }
    }
}
impl TryFrom<String> for WorkspacePath {
    type Error = IdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
            return Err(IdentityError(
                "path must be normalized, relative, and outside .git",
            ));
        }
        if let Some(error) = value.split('/').find_map(segment_error) {
            return Err(error);
        }
        if !unicode_normalization::is_nfc(&value) {
            return Err(IdentityError("path must be in Unicode NFC"));
        }
        Ok(Self(value))
    }
}
impl TryFrom<String> for Timestamp {
    type Error = IdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        const SPELLING: IdentityError =
            IdentityError("timestamp must be a UTC instant spelled YYYY-MM-DDTHH:MM:SS.sssZ");
        if !has_timestamp_shape(&value) {
            return Err(SPELLING);
        }
        let instant = OffsetDateTime::parse(&value, &Rfc3339)
            .map_err(|_| IdentityError("timestamp names a date or time that does not exist"))?;
        // One spelling: writing the parsed instant must give back exactly the input.
        if Self::from_utc(instant)?.0 == value {
            Ok(Self(value))
        } else {
            Err(SPELLING)
        }
    }
}
impl From<Revision> for String {
    fn from(value: Revision) -> Self {
        value.0
    }
}
impl From<Digest> for String {
    fn from(value: Digest) -> Self {
        value.0
    }
}
impl From<WorkspacePath> for String {
    fn from(value: WorkspacePath) -> Self {
        value.0
    }
}
impl From<Timestamp> for String {
    fn from(value: Timestamp) -> Self {
        value.0
    }
}
impl FromStr for Revision {
    type Err = IdentityError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value.to_owned())
    }
}
impl FromStr for Digest {
    type Err = IdentityError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value.to_owned())
    }
}
impl FromStr for WorkspacePath {
    type Err = IdentityError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value.to_owned())
    }
}
impl FromStr for Timestamp {
    type Err = IdentityError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value.to_owned())
    }
}
impl Display for Timestamp {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl Display for IdentityError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for IdentityError {}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The rule one `/`-separated segment breaks, if any.
fn segment_error(segment: &str) -> Option<IdentityError> {
    if segment.is_empty()
        || segment == "."
        || segment == ".."
        || segment.eq_ignore_ascii_case(".git")
    {
        return Some(IdentityError(
            "path must be normalized, relative, and outside .git",
        ));
    }
    if segment.contains(RESERVED_CHARACTERS) {
        return Some(IdentityError(
            "path segment holds a character Windows refuses: \\ : < > \" | ? *",
        ));
    }
    if segment.ends_with('.') || segment.ends_with(' ') {
        return Some(IdentityError(
            "path segment must not end with a dot or a space",
        ));
    }
    let stem = segment.split('.').next().unwrap_or(segment);
    if RESERVED_STEMS
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
    {
        return Some(IdentityError(
            "path segment is a name Windows reserves for a device",
        ));
    }
    None
}

/// Whether `value` matches `TIMESTAMP_PATTERN`, checked byte by byte.
fn has_timestamp_shape(value: &str) -> bool {
    const SHAPE: &[u8; 24] = b"dddd-dd-ddTdd:5d:5d.dddZ";
    let bytes = value.as_bytes();
    bytes.len() == SHAPE.len()
        && bytes
            .iter()
            .zip(SHAPE)
            .all(|(byte, expected)| match expected {
                b'd' => byte.is_ascii_digit(),
                b'5' => (b'0'..=b'5').contains(byte),
                literal => byte == literal,
            })
}
