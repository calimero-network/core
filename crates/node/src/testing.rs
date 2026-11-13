//! Test hooks (feature `testing`, never in a release build).

use std::sync::Mutex;
use std::time::Duration;

/// Nodes whose `/attestation` answers late, by public URL.
static SLOW_ATTESTATION: Mutex<Vec<(String, Duration)>> = Mutex::new(Vec::new());

/// Make the node at `public_url` answer `/attestation` only after `delay`, as
/// a TDX node does while it fetches Intel's collateral, so a test sees peers
/// attest each other more slowly than a Raft RPC may take.
pub fn slow_attestation(public_url: &str, delay: Duration) {
    if let Ok(mut s) = SLOW_ATTESTATION.lock() {
        s.retain(|(u, _)| u != public_url);
        s.push((public_url.to_owned(), delay));
    }
}

/// The delay [`slow_attestation`] set for the node at `public_url`.
pub(crate) fn attestation_delay(public_url: &str) -> Option<Duration> {
    SLOW_ATTESTATION
        .lock()
        .ok()?
        .iter()
        .find(|(u, _)| u == public_url)
        .map(|(_, d)| *d)
}

/// The timeout of the HTTP client nodes share for peers, job nodes and attestation, when a
/// test set one: nodes started after [`shared_http_timeout`] in this process use it instead of
/// [`crate::node::SHARED_HTTP_TIMEOUT`].
static SHARED_HTTP_TIMEOUT: Mutex<Option<Duration>> = Mutex::new(None);

/// Make nodes started from now on in this process give their shared HTTP client `timeout`, so
/// a test can show a transfer outlives it without waiting out the 30 s a cluster uses.
pub fn shared_http_timeout(timeout: Duration) {
    if let Ok(mut t) = SHARED_HTTP_TIMEOUT.lock() {
        *t = Some(timeout);
    }
}

/// The timeout [`shared_http_timeout`] set, if any.
pub(crate) fn shared_http_timeout_override() -> Option<Duration> {
    SHARED_HTTP_TIMEOUT.lock().ok().and_then(|t| *t)
}

/// How long creating an app waits for its `init` when a test set it: every node in this process
/// uses it from now on instead of [`crate::api::apps::APP_START_WAIT`].
static APP_START_WAIT: Mutex<Option<Duration>> = Mutex::new(None);

/// Make app creation in this process wait `wait` for the app's `init` instead of 30 s, so a
/// test of an app whose `init` never applies sees the `503` sooner.
pub fn app_start_wait(wait: Duration) {
    if let Ok(mut w) = APP_START_WAIT.lock() {
        *w = Some(wait);
    }
}

/// The wait [`app_start_wait`] set, if any.
pub(crate) fn app_start_wait_override() -> Option<Duration> {
    APP_START_WAIT.lock().ok().and_then(|w| *w)
}

/// How long an upload's `complete` waits for its segments to reach a majority when a test set
/// it: every node in this process uses it from now on instead of 20 s.
static UPLOAD_COMPLETE_WAIT: Mutex<Option<Duration>> = Mutex::new(None);

/// Make an upload's `complete` in this process wait `wait` for its segments instead of 20 s, so
/// a test whose pushes fail sees the `503 FILES_UNAVAILABLE` sooner.
pub fn upload_complete_wait(wait: Duration) {
    if let Ok(mut w) = UPLOAD_COMPLETE_WAIT.lock() {
        *w = Some(wait);
    }
}

/// The wait [`upload_complete_wait`] set, if any.
pub(crate) fn upload_complete_wait_override() -> Option<Duration> {
    UPLOAD_COMPLETE_WAIT.lock().ok().and_then(|w| *w)
}
