//! The live launch link of a local installation: one single-use launch URL is always published.
//!
//! SPEC section 11: "startup prints a single-use launch URL". The first link is returned by
//! [`LaunchLink::publish_first`] for startup to print, and written to `<data dir>/launch-url`
//! ([`LAUNCH_FILE`]). [`LaunchLink::keep`] then publishes the next link each time a sign-in
//! completes with a launch token and each time the published one's lifetime ends, so the file
//! always holds a link that works: a desktop launcher that finds a server already running, or a
//! person whose server the CLI's relay started with its output closed, reads it there. Only the
//! first link is printed; the printed one ends with its lifetime. The owner decided on
//! 2026-10-18 to keep the file (server-m2 rulings, Q2).
//!
//! The file holds the URL and a line end. It is written to `launch-url.tmp` first and renamed
//! over the old one, so a reader never sees half a link; on Unix it is created owner-only
//! (`0600`); on Windows it takes the data directory's access list, which for the default
//! per-user directory is the user's own. Whoever can read the data directory can already read
//! and write every store in it, so a live owner link there grants nothing more. It is removed
//! when `keep` stops at shutdown. A file that cannot be written is reported on stderr and does
//! not stop the service: the printed link still works.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use okf_jawn_contract::error::ApiError;
use tokio_util::sync::CancellationToken;

use crate::local::{LaunchTokens, SecretSource};

/// The launch link a local installation publishes.
pub struct LaunchLink {
    tokens: Arc<LaunchTokens>,
    secrets: Arc<dyn SecretSource>,
    origin: String,
    file: PathBuf,
}

/// The file in the data directory that holds the live launch link.
pub const LAUNCH_FILE: &str = "launch-url";

/// The launch route a link names (`api/transports.json` `begin_local_session`).
const LAUNCH_PATH: &str = "/auth/local";

impl LaunchLink {
    /// Links of the service at `origin` for `tokens`, drawn from `secrets`, published in
    /// `data_dir`'s [`LAUNCH_FILE`].
    #[must_use]
    pub fn new(
        tokens: Arc<LaunchTokens>,
        secrets: Arc<dyn SecretSource>,
        origin: &str,
        data_dir: &Path,
    ) -> Self {
        Self {
            tokens,
            secrets,
            origin: origin.to_owned(),
            file: data_dir.join(LAUNCH_FILE),
        }
    }

    /// Issue the first link, write it to the file, and answer it for startup to print.
    ///
    /// # Errors
    /// Returns the random source's error. A file that cannot be written is reported on stderr
    /// and is no error: the answered link works.
    pub fn publish_first(&self) -> Result<String, ApiError> {
        let link = self.issue()?;
        self.write(&link);
        Ok(link)
    }

    /// Keep one live link published until `stop`: the next after each completed sign-in and
    /// each time the published one's lifetime ends. Removes the file when it stops.
    pub async fn keep(self, stop: CancellationToken) {
        loop {
            tokio::select! {
                () = stop.cancelled() => break,
                () = self.tokens.signed_in() => {}
                () = tokio::time::sleep(self.tokens.lifetime()) => {}
            }
            match self.issue() {
                Ok(link) => self.write(&link),
                Err(error) => {
                    tracing::warn!(code = ?error.code, "the next launch link could not be issued");
                }
            }
        }
        match std::fs::remove_file(&self.file) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(kind = ?error.kind(), "the launch link file could not be removed");
            }
        }
    }

    /// A fresh link.
    fn issue(&self) -> Result<String, ApiError> {
        let token = self.tokens.issue(self.secrets.as_ref())?;
        Ok(format!("{}{LAUNCH_PATH}?token={token}", self.origin))
    }

    /// Publish `link` in the file, reporting a failure without stopping anything.
    fn write(&self, link: &str) {
        if let Err(error) = replace(&self.file, link) {
            tracing::warn!(
                kind = ?error.kind(),
                "the launch link could not be written to the data directory; the printed link \
                 still works"
            );
        }
    }
}

/// Write `link` and a line end to `file` through a temporary file renamed over it.
fn replace(file: &Path, link: &str) -> std::io::Result<()> {
    let partial = file.with_file_name(format!("{LAUNCH_FILE}.tmp"));
    // A leftover partial file would keep its old permissions: start from none.
    if let Err(error) = std::fs::remove_file(&partial)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(error);
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut written = options.open(&partial)?;
    written.write_all(format!("{link}\n").as_bytes())?;
    written.sync_all()?;
    drop(written);
    std::fs::rename(&partial, file)
}
