//! Waiting for the cluster to get somewhere, instead of sleeping and hoping it has.
//!
//! A fixed `sleep` before an assertion is either too short on a loaded runner (a
//! flaky test) or longer than the cluster needs (a slow one). [`eventually`] polls
//! until the condition holds and fails with the last thing it saw. A sleep that
//! proves something does *not* happen within a window stays a sleep.

use std::fmt::Debug;
use std::future::Future;
use std::time::Duration;

/// How long a test waits for the cluster to converge: long enough for a loaded CI
/// runner, short enough to fail with its own message before nextest's `ci` profile
/// kills a test at 30 s (docs: contribute/testing, "No test runs past 15 seconds").
pub const EVENTUALLY: Duration = Duration::from_secs(10);

/// How often [`eventually`] polls.
const POLL: Duration = Duration::from_millis(50);

/// Polls `probe` until it returns `Ok`, and returns that value; once `timeout`
/// has passed, panics naming `what` and the last `Err` the probe returned.
pub async fn eventually<T, E, F, Fut>(timeout: Duration, what: &str, mut probe: F) -> T
where
    E: Debug,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let last = match probe().await {
            Ok(done) => return done,
            Err(last) => last,
        };
        if tokio::time::Instant::now() >= deadline {
            panic!("{what}: not within {timeout:?}; last: {last:?}");
        }
        tokio::time::sleep(POLL).await;
    }
}

/// [`eventually`] for a condition read without waiting (a node's applied state, a
/// group's status): polls `holds` until it is true.
pub async fn until(timeout: Duration, what: &str, mut holds: impl FnMut() -> bool) {
    eventually(timeout, what, || {
        std::future::ready(if holds() { Ok(()) } else { Err("not yet") })
    })
    .await;
}

/// [`eventually`] for a value read without waiting: polls `find` until it answers
/// `Some`, and returns that.
pub async fn found<T>(timeout: Duration, what: &str, mut find: impl FnMut() -> Option<T>) -> T {
    eventually(timeout, what, || {
        std::future::ready(find().ok_or("none yet"))
    })
    .await
}
