//! Validated wire identities distinguish requested state from resolved content.
//!
//! An `At::Latest` selector must be resolved once before a read returns `Revision`.

use std::fmt::{Display, Formatter};
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

macro_rules! uuid_id {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, ToSchema)]
        #[serde(transparent)]
        pub struct $name(#[doc = "Underlying UUID."] pub Uuid);
    };
}
uuid_id!(WorkspaceId, "Stable application identity for one user-organized workspace.");
uuid_id!(ItemId, "Stable item identity independent of its current relative path.");
uuid_id!(UploadId, "Identity of one authenticated source upload occurrence.");
uuid_id!(JobId, "Durable background work identity retained across retries.");
uuid_id!(ProposalId, "Identity of a suggested change set, not a review.");
uuid_id!(ReviewId, "Identity of an explicit revision-bound review action.");
uuid_id!(ReceiptId, "Identity of a durable application operation record.");
uuid_id!(ArtifactId, "Identity of a retained export or backup artifact.");


/// A resolved Git commit, never a branch name or the string `latest`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[serde(try_from = "String", into = "String")]
#[schema(value_type = String, pattern = "^([0-9a-f]{40}|[0-9a-f]{64})$")]
pub struct Revision(String);

/// A lower-case SHA-256 identifier for immutable bytes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[serde(try_from = "String", into = "String")]
#[schema(value_type = String, pattern = "^[0-9a-f]{64}$")]
pub struct Digest(String);

/// A normalized relative workspace path; this alone is not filesystem authorization.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[serde(try_from = "String", into = "String")]
#[schema(value_type = String, min_length = 1, max_length = 4096)]
pub struct WorkspacePath(String);

/// A requested version selector; resolved responses contain a `Revision` instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
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

impl JsonSchema for Revision {
    fn schema_name() -> std::borrow::Cow<'static, str> { "Revision".into() }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type": "string", "pattern": "^([0-9a-f]{40}|[0-9a-f]{64})$"})
    }
}
impl JsonSchema for Digest {
    fn schema_name() -> std::borrow::Cow<'static, str> { "Digest".into() }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type": "string", "pattern": "^[0-9a-f]{64}$"})
    }
}
impl JsonSchema for WorkspacePath {
    fn schema_name() -> std::borrow::Cow<'static, str> { "WorkspacePath".into() }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type": "string", "minLength": 1, "maxLength": 4096})
    }
}
impl Revision {
    /// Return the canonical lower-case commit identifier.
    #[must_use]
    pub fn as_str(&self) -> &str { &self.0 }
}
impl Digest {
    /// Return the canonical lower-case SHA-256 identifier.
    #[must_use]
    pub fn as_str(&self) -> &str { &self.0 }
}
impl WorkspacePath {
    /// Return the validated slash-separated relative path.
    #[must_use]
    pub fn as_str(&self) -> &str { &self.0 }
}
impl TryFrom<String> for Revision {
    type Error = IdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if matches!(value.len(), 40 | 64) && is_lower_hex(&value) {
            Ok(Self(value))
        } else {
            Err(IdentityError("revision must be a complete lower-case Git commit hash"))
        }
    }
}
impl TryFrom<String> for Digest {
    type Error = IdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() == 64 && is_lower_hex(&value) {
            Ok(Self(value))
        } else {
            Err(IdentityError("digest must be 64 lower-case SHA-256 hex characters"))
        }
    }
}
impl TryFrom<String> for WorkspacePath {
    type Error = IdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let invalid = value.is_empty() || value.len() > 4096 || value.contains('\\')
            || value.chars().any(char::is_control)
            || value.split('/').any(|part| part.is_empty() || part == "." || part == ".."
                || part.eq_ignore_ascii_case(".git") || part.contains(':'));
        if invalid { Err(IdentityError("path must be normalized, relative, and outside .git")) }
        else { Ok(Self(value)) }
    }
}
impl From<Revision> for String { fn from(value: Revision) -> Self { value.0 } }
impl From<Digest> for String { fn from(value: Digest) -> Self { value.0 } }
impl From<WorkspacePath> for String { fn from(value: WorkspacePath) -> Self { value.0 } }
impl FromStr for Revision { type Err = IdentityError; fn from_str(value: &str) -> Result<Self, Self::Err> { Self::try_from(value.to_owned()) } }
impl FromStr for Digest { type Err = IdentityError; fn from_str(value: &str) -> Result<Self, Self::Err> { Self::try_from(value.to_owned()) } }
impl FromStr for WorkspacePath { type Err = IdentityError; fn from_str(value: &str) -> Result<Self, Self::Err> { Self::try_from(value.to_owned()) } }
impl Display for IdentityError { fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result { f.write_str(self.0) } }
impl std::error::Error for IdentityError {}

fn is_lower_hex(value: &str) -> bool {
    value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
