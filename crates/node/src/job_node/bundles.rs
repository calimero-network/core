//! The components a job node holds, prepared once per content hash.

use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use calimero_app_host::{Engine, Prepared};
use serde_json::{json, Value};
use tokio::sync::Semaphore;

use super::admission::admit_bundle;
use super::{locked, JobNode};
use crate::error::ApiError;
use crate::job_wire::{is_hash, BUNDLE_SIGNATURE_HEADER};
use crate::node::MAX_BLOB_BYTES;

pub(super) const MAX_JOB_BUNDLES: usize = 8; // prepared components kept; the least recently used goes first
const MAX_CONCURRENT_PREPARES: usize = 1; // components compiled at once: anyone inside the sealed channel can offer one

pub(super) struct Bundles {
    /// Oldest use first.
    kept: Mutex<Vec<Arc<Prepared>>>,
    preparing: Semaphore,
}

impl Bundles {
    pub(super) fn new() -> Self {
        Self {
            kept: Mutex::default(),
            preparing: Semaphore::new(MAX_CONCURRENT_PREPARES),
        }
    }

    /// The prepared component with `hash`, made the most recently used.
    pub(super) fn get(&self, hash: &str) -> Option<Arc<Prepared>> {
        let mut kept = locked(&self.kept);
        let at = kept.iter().position(|p| p.hash == hash)?;
        let prepared = kept.remove(at);
        kept.push(Arc::clone(&prepared));
        Some(prepared)
    }

    fn insert(&self, prepared: Arc<Prepared>) {
        let mut kept = locked(&self.kept);
        kept.retain(|p| p.hash != prepared.hash);
        if kept.len() >= MAX_JOB_BUNDLES {
            let _evicted = kept.remove(0);
        }
        kept.push(prepared);
    }
}

/// `PUT /job/bundles/{hash}`: the component's bytes, which must hash to `hash`, with the
/// cluster identity's signature of the name.
pub(super) async fn put_bundle(
    State(job): State<Arc<JobNode>>,
    Path(hash): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    if !is_hash(&hash) {
        return Err(ApiError::bad_request(
            "BAD_HASH",
            "a bundle is named by its hex SHA-256",
        ));
    }
    let signature = headers
        .get(BUNDLE_SIGNATURE_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    admit_bundle(&job.config, &hash, signature)?;
    if body.len() > MAX_BLOB_BYTES {
        return Err(ApiError::new(413, "TOO_LARGE", "the bundle is too large"));
    }
    if hex::encode(calimero_identity::sha256(&body)) != hash {
        return Err(ApiError::bad_request(
            "HASH_MISMATCH",
            "the bytes are not the bundle named",
        ));
    }
    if job.bundles.get(&hash).is_some() {
        return Ok(Json(json!({ "hash": hash })));
    }
    let _permit = job
        .bundles
        .preparing
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e.to_string()))?;
    let engine = Arc::clone(&job.engine);
    let named = hash.clone();
    let prepared = tokio::task::spawn_blocking(move || engine.prepare(&named, &body))
        .await
        .map_err(|e| ApiError::internal(&e.to_string()))?
        .map_err(|e| ApiError::bad_request("BAD_BUNDLE", &e.to_string()))?;
    job.bundles.insert(Arc::new(prepared));
    Ok(Json(json!({ "hash": hash })))
}

#[cfg(test)]
mod tests {
    use calimero_app_host::Abi;

    use super::*;

    fn prepared(hash: &str) -> Arc<Prepared> {
        Arc::new(Prepared {
            hash: hash.to_owned(),
            abi: Abi::default(),
            inner: Arc::new(()),
        })
    }

    #[test]
    fn the_least_recently_used_bundle_is_the_one_dropped() {
        let bundles = Bundles::new();
        for i in 0..MAX_JOB_BUNDLES {
            bundles.insert(prepared(&format!("h{i}")));
        }
        // Using the oldest makes the next one the oldest.
        assert!(bundles.get("h0").is_some());
        bundles.insert(prepared("new"));
        assert!(bundles.get("h1").is_none());
        for kept in ["h0", "h2", "new"] {
            assert!(bundles.get(kept).is_some(), "{kept}");
        }
        assert_eq!(locked(&bundles.kept).len(), MAX_JOB_BUNDLES);
    }
}
