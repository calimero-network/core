//! The collateral `/tee/attest` hands out with a quote (`includeCollateral`).
//!
//! A quote is verified against Intel-signed collateral: TCB info, QE identity,
//! CRLs and their chains. Serving it with the quote lets a client that has only
//! the node — a browser, a mobile app — verify the quote offline, and it is
//! safe to take from the node because Intel signs it: the node can choose only
//! which valid collateral to serve, not forge it.
//!
//! The attest endpoint needs no credential, so collateral is fetched at most
//! once per [`CACHE_FOR`] and shared: a flood of attestations cannot turn into
//! a flood of requests to the collateral source. It depends only on the
//! platform, which does not change while the process runs.

use core::time::Duration;
use std::time::Instant;

use calimero_tee_attestation::{fetch_collateral, AttestationError};
use tokio::sync::Mutex;

/// How long fetched collateral is served before it is fetched again. Intel
/// reissues TCB info and QE identity monthly; an hour keeps a verifier's view
/// current well within that.
const CACHE_FOR: Duration = Duration::from_secs(60 * 60);

/// The last collateral fetched, and when. The lock is held across a fetch, so
/// concurrent attestations wait for one fetch rather than each starting one.
static CACHE: Mutex<Option<(Instant, serde_json::Value)>> = Mutex::const_new(None);

/// The collateral for `quote_bytes`, fetched if none was within [`CACHE_FOR`].
pub(super) async fn for_quote(quote_bytes: &[u8]) -> Result<serde_json::Value, AttestationError> {
    let mut cache = CACHE.lock().await;
    if let Some((fetched, collateral)) = cache.as_ref() {
        if fetched.elapsed() < CACHE_FOR {
            return Ok(collateral.clone());
        }
    }
    let collateral = serde_json::to_value(fetch_collateral(quote_bytes).await?)
        .map_err(|err| AttestationError::CollateralFetchFailed(err.to_string()))?;
    *cache = Some((Instant::now(), collateral.clone()));
    Ok(collateral)
}
