//! Managed backups (Stage 1b design section 8): their layout on disk.
//!
//! A pre-migration backup set is one directory under `<data>/backups/pre-migration`, holding
//! the installation archive of each tenant and every workspace archive at
//! `workspaces/<tenant>/<workspace>.zip`. A purge deletes a workspace's archive from every set
//! (`purge::copies`).

/// The directory of a pre-migration backup set that holds its workspace archives.
pub(crate) const PRE_MIGRATION_WORKSPACES: &str = "workspaces";
