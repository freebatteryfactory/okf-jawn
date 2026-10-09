//! Connectors, browser sessions and the local installation identity (`CredentialStore`).
//!
//! A connector secret is generated here, returned once, and stored only as
//! `core::credentials::secret_hash` of it; `create_connector` is unique on its `MutationId`
//! and a repeat reports `Existing`, never a second secret.

use okf_jawn_contract::access::{Connector, IssuedConnector, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{ConnectorId, MutationId};
use okf_jawn_core::credentials::{
    ConnectorIssue, CredentialStore, InstallationIdentity, NewConnector, SessionRecord, secret_hash,
};
use okf_jawn_core::ports::PortFuture;
use rusqlite::{Connection, OptionalExtension, params};

use crate::confirmations::canonical_expiry;
use crate::db::{Db, conflict, json, not_found, sql};
use crate::seam;

/// `CredentialStore` over the records database.
#[derive(Debug, Clone)]
pub struct SqliteCredentials {
    db: Db,
}

/// Prefix that makes a connector secret recognizable in a configuration file.
const SECRET_PREFIX: &str = "okfj_";

impl SqliteCredentials {
    pub(crate) const fn new(db: Db) -> Self {
        Self { db }
    }
}

impl CredentialStore for SqliteCredentials {
    fn installation_identity(&self) -> PortFuture<'_, InstallationIdentity> {
        Box::pin(self.db.transaction(|transaction| installation(transaction)))
    }

    fn create_connector(
        &self,
        mutation_id: MutationId,
        connector: NewConnector,
    ) -> PortFuture<'_, ConnectorIssue> {
        Box::pin(self.db.transaction(move |transaction| {
            let id: ConnectorId = new_id!()?;
            let mut permissions = vec![Permission::Read];
            if connector.allow_propose {
                permissions.push(Permission::Propose);
            }
            let record = Connector {
                connector_id: id,
                label: connector.label,
                workspace_ids: connector.workspace_ids,
                permissions,
                created_at: seam::now()?,
                revoked_at: None,
            };
            let secret = new_secret()?;
            let inserted = transaction
                .execute(
                    "INSERT INTO connectors (connector_id, mutation_id, secret_hash, record)
                     VALUES (?1, ?2, ?3, ?4) ON CONFLICT (mutation_id) DO NOTHING",
                    params![
                        id.0.to_string(),
                        mutation_id.0.to_string(),
                        secret_hash(&secret).as_slice(),
                        serde_json::to_string(&record).map_err(|error| json(&error))?
                    ],
                )
                .map_err(|error| sql(&error))?;
            if inserted == 1 {
                return Ok(ConnectorIssue::Issued(IssuedConnector {
                    connector: record,
                    secret,
                }));
            }
            let existing: String = transaction
                .query_row(
                    "SELECT record FROM connectors WHERE mutation_id = ?1",
                    [mutation_id.0.to_string()],
                    |row| row.get(0),
                )
                .map_err(|error| sql(&error))?;
            Ok(ConnectorIssue::Existing(
                serde_json::from_str(&existing).map_err(|error| json(&error))?,
            ))
        }))
    }

    fn rotate_connector_secret(&self, connector: ConnectorId) -> PortFuture<'_, IssuedConnector> {
        Box::pin(self.db.transaction(move |transaction| {
            let record = read(transaction, connector)?.ok_or_else(|| not_found("connector"))?;
            if record.revoked_at.is_some() {
                return Err(conflict("a revoked connector cannot be given a new secret"));
            }
            let secret = new_secret()?;
            transaction
                .execute(
                    "UPDATE connectors SET secret_hash = ?1 WHERE connector_id = ?2",
                    params![secret_hash(&secret).as_slice(), connector.0.to_string()],
                )
                .map_err(|error| sql(&error))?;
            Ok(IssuedConnector {
                connector: record,
                secret,
            })
        }))
    }

    fn list_connectors(&self, include_revoked: bool) -> PortFuture<'_, Vec<Connector>> {
        Box::pin(self.db.call(move |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT record FROM connectors WHERE ?1 OR revoked = 0 ORDER BY rowid DESC",
                )
                .map_err(|error| sql(&error))?;
            let records = statement
                .query_map([include_revoked], |row| row.get::<_, String>(0))
                .map_err(|error| sql(&error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| sql(&error))?;
            records
                .iter()
                .map(|record| serde_json::from_str(record).map_err(|error| json(&error)))
                .collect()
        }))
    }

    fn revoke_connector(&self, connector: ConnectorId) -> PortFuture<'_, Connector> {
        Box::pin(self.db.transaction(move |transaction| {
            let mut record = read(transaction, connector)?.ok_or_else(|| not_found("connector"))?;
            if record.revoked_at.is_some() {
                return Ok(record);
            }
            record.revoked_at = Some(seam::now()?);
            transaction
                .execute(
                    "UPDATE connectors SET revoked = 1, record = ?1 WHERE connector_id = ?2",
                    params![
                        serde_json::to_string(&record).map_err(|error| json(&error))?,
                        connector.0.to_string()
                    ],
                )
                .map_err(|error| sql(&error))?;
            Ok(record)
        }))
    }

    fn lookup_connector(&self, hash: [u8; 32]) -> PortFuture<'_, Option<Connector>> {
        Box::pin(self.db.call(move |connection| {
            let record: Option<String> = connection
                .query_row(
                    "SELECT record FROM connectors WHERE secret_hash = ?1 AND revoked = 0",
                    [hash.as_slice()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| sql(&error))?;
            record
                .map(|record| serde_json::from_str(&record).map_err(|error| json(&error)))
                .transpose()
        }))
    }

    fn insert_session(&self, session: SessionRecord) -> PortFuture<'_, SessionRecord> {
        Box::pin(self.db.transaction(move |transaction| {
            let expires_at = canonical_expiry(&session.expires_at)?;
            transaction
                .execute(
                    "INSERT INTO sessions (session_id, principal, expires_at) VALUES (?1, ?2, ?3)",
                    params![
                        session.session_id,
                        serde_json::to_string(&session.principal).map_err(|error| json(&error))?,
                        expires_at.as_str()
                    ],
                )
                .map_err(|error| sql(&error))?;
            Ok(session)
        }))
    }

    fn get_session<'a>(&'a self, session_id: &'a str) -> PortFuture<'a, Option<SessionRecord>> {
        let session_id = session_id.to_owned();
        Box::pin(self.db.call(move |connection| {
            let found: Option<(String, String)> = connection
                .query_row(
                    "SELECT principal, expires_at FROM sessions
                     WHERE session_id = ?1 AND revoked = 0 AND expires_at > ?2",
                    params![session_id, seam::now()?.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| sql(&error))?;
            found
                .map(|(principal, expires_at)| {
                    let principal: Principal =
                        serde_json::from_str(&principal).map_err(|error| json(&error))?;
                    Ok(SessionRecord {
                        session_id: session_id.clone(),
                        principal,
                        expires_at,
                    })
                })
                .transpose()
        }))
    }

    fn revoke_session<'a>(&'a self, session_id: &'a str) -> PortFuture<'a, ()> {
        let session_id = session_id.to_owned();
        Box::pin(self.db.transaction(move |transaction| {
            transaction
                .execute(
                    "UPDATE sessions SET revoked = 1 WHERE session_id = ?1",
                    [session_id],
                )
                .map_err(|error| sql(&error))?;
            Ok(())
        }))
    }
}

/// The installation identity, created on first use.
pub(crate) fn installation(connection: &Connection) -> Result<InstallationIdentity, ApiError> {
    let found: Option<(String, String)> = connection
        .query_row(
            "SELECT subject, created_at FROM installation WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    if let Some((subject, created_at)) = found {
        return Ok(InstallationIdentity {
            subject,
            created_at,
        });
    }
    let identity = InstallationIdentity {
        subject: format!("local-owner-{}", seam::hex(&seam::random_bytes::<8>()?)),
        created_at: seam::now()?.to_string(),
    };
    connection
        .execute(
            "INSERT INTO installation (singleton, subject, created_at) VALUES (1, ?1, ?2)",
            params![identity.subject, identity.created_at],
        )
        .map_err(|error| sql(&error))?;
    Ok(identity)
}

fn read(connection: &Connection, connector: ConnectorId) -> Result<Option<Connector>, ApiError> {
    let record: Option<String> = connection
        .query_row(
            "SELECT record FROM connectors WHERE connector_id = ?1",
            [connector.0.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    record
        .map(|record| serde_json::from_str(&record).map_err(|error| json(&error)))
        .transpose()
}

/// A new connector secret: 32 random bytes, hex-spelled after a recognizable prefix.
fn new_secret() -> Result<String, ApiError> {
    let bytes = seam::random_bytes::<32>()?;
    if bytes.iter().all(|byte| *byte == 0) {
        return Err(ApiError::new(
            ErrorCode::Internal,
            "the random source returned no randomness",
        ));
    }
    Ok(format!("{SECRET_PREFIX}{}", seam::hex(&bytes)))
}
