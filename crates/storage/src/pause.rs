//! Test-only pause points. A write calls `point` at a named place between two of its steps; a
//! test that called `hold` for that name first is told when the write arrives there, and the
//! write waits until the test releases it. A window between two steps is then exercised in a
//! fixed order instead of by timing. Each hold catches one arrival; with no hold, `point` does
//! nothing. Names carry a path unique to each test's data directory, so parallel tests never
//! meet.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};

use okf_jawn_contract::error::ApiError;

use crate::db::internal;

/// Each held point's arrival signal and release, by name.
type Held = BTreeMap<String, (Sender<()>, Receiver<()>)>;

static HELD: Mutex<Held> = Mutex::new(BTreeMap::new());

/// Hold the next arrival at the point `name`; returns (told when it arrives, its release).
pub(crate) fn hold(name: String) -> Result<(Receiver<()>, Sender<()>), ApiError> {
    let (arrived, arrival) = channel();
    let (release, released) = channel();
    HELD.lock()
        .map_err(|_| internal("the pause table was poisoned"))?
        .insert(name, (arrived, released));
    Ok((arrival, release))
}

/// Wait, off the async threads, until a held write has arrived at its point.
pub(crate) async fn arrived(arrival: Receiver<()>) -> Result<(), ApiError> {
    tokio::task::spawn_blocking(move || arrival.recv())
        .await
        .map_err(|error| internal(error.to_string()))?
        .map_err(|error| internal(error.to_string()))
}

/// Where a write waits while a test holds the point `name` (once per hold).
pub(crate) async fn point(name: &str) -> Result<(), ApiError> {
    let held = HELD
        .lock()
        .map_err(|_| internal("the pause table was poisoned"))?
        .remove(name);
    if let Some((arrived, released)) = held {
        arrived
            .send(())
            .map_err(|error| internal(error.to_string()))?;
        tokio::task::spawn_blocking(move || released.recv())
            .await
            .map_err(|error| internal(error.to_string()))?
            .map_err(|error| internal(error.to_string()))?;
    }
    Ok(())
}
