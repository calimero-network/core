//! `POST /apps/{app}/files/{id}/export` and `GET /apps/{app}/files/{id}/segments/{k}`: taking a
//! sealed file out of the node as ciphertext plus its data key wrapped to a key the requester
//! holds. The node never answers plaintext, nor a key in the clear.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::Json;
use calimero_sealed::server::LowOrder;
use calimero_sealed::wrap::{wrap_export, ExportWrap};
use serde::Deserialize;
use serde_json::json;

use super::files::files_gate;
use super::{parse_blob_id, ApiResult};
use crate::error::ApiError;
use crate::files::unwrap_key;
use crate::node::Node;

const MAX_EXPORTS_PER_MINUTE: f64 = 30.0; // exports one account may ask of one node per minute
const MAX_TRACKED_ACCOUNTS: usize = 4096; // accounts whose bucket is kept at once

const TOO_MANY_EXPORTS: &str = "TOO_MANY_EXPORTS";
const BAD_RECIPIENT: &str = "BAD_RECIPIENT";
const NO_SUCH_SEGMENT: &str = "NO_SUCH_SEGMENT";

/// A token bucket per account: `MAX_EXPORTS_PER_MINUTE` tokens, refilled evenly over a minute.
#[derive(Default)]
pub struct ExportLimits(Mutex<HashMap<String, (f64, Instant)>>);

impl ExportLimits {
    /// Takes a token from `account`'s bucket; false when it is empty or the node tracks too many accounts.
    fn take(&self, account: &str) -> bool {
        let now = Instant::now();
        let mut buckets = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let refill = |(tokens, at): &(f64, Instant)| {
            (tokens + now.duration_since(*at).as_secs_f64() * MAX_EXPORTS_PER_MINUTE / 60.0)
                .min(MAX_EXPORTS_PER_MINUTE)
        };
        if buckets.len() >= MAX_TRACKED_ACCOUNTS && !buckets.contains_key(account) {
            buckets.retain(|_, b| refill(b) < MAX_EXPORTS_PER_MINUTE);
            if buckets.len() >= MAX_TRACKED_ACCOUNTS {
                return false;
            }
        }
        let bucket = buckets
            .entry(account.to_owned())
            .or_insert((MAX_EXPORTS_PER_MINUTE, now));
        let tokens = refill(bucket);
        if tokens < 1.0 {
            *bucket = (tokens, now);
            return false;
        }
        *bucket = (tokens - 1.0, now);
        true
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportBody {
    recipient: String,
}

/// The recipient key, 64 hex characters of an X25519 public key.
fn recipient_key(hex_key: &str) -> Result<[u8; 32], ApiError> {
    hex::decode(hex_key)
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .ok_or_else(|| ApiError::bad_request(BAD_RECIPIENT, "the recipient is 64 hex characters"))
}

/// The app's sealed file `id` as its leader holds it, else 404.
async fn sealed_file(
    node: &Node,
    app: &crate::types::AppRecord,
    raw: &[u8; 32],
) -> Result<(u64, crate::component::blobs::FileSeal), ApiError> {
    match node.file_record_strong(app, raw).await {
        Some(Some(record)) => Ok(record),
        Some(None) => Err(ApiError::not_found("no such file in this app")),
        None => Err(ApiError::unavailable("the app's leader did not answer")),
    }
}

/// Wrap the file's data key to `recipient`; answer what the client needs to fetch and open it.
pub async fn export(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, id)): Path<(String, String)>,
    Json(body): Json<ExportBody>,
) -> ApiResult {
    let (app, account) = files_gate(&node, &headers, &app_id, false).await?;
    let recipient = recipient_key(&body.recipient)?;
    if !node.export_limits.take(&account) {
        return Err(ApiError::new(
            429,
            TOO_MANY_EXPORTS,
            "too many exports; try again in a minute",
        ));
    }
    let raw = parse_blob_id(&id)?;
    let (size, seal) = sealed_file(&node, &app, &raw).await?;
    let data_key = unwrap_key(
        &node.file_key_of(&app.app_id),
        &app.app_id,
        &seal.wrapped_key.0,
    )
    .map_err(|e| ApiError::internal(&e.to_string()))?;
    let file_id = hex::encode(raw);
    let wrap = ExportWrap {
        app_id: &app.app_id,
        file_id: &file_id,
    };
    let (ephemeral, wrapped) = wrap_export(&recipient, &wrap, &data_key).map_err(|e| {
        if e.downcast_ref::<LowOrder>().is_some() {
            ApiError::bad_request(BAD_RECIPIENT, "the recipient is not a usable X25519 key")
        } else {
            ApiError::internal(&e.to_string())
        }
    })?;
    Ok(Json(json!({
        "app": app.app_id,
        "id": file_id,
        "size": size,
        "sha256": hex::encode(&seal.sha256.0),
        "segments": seal.segments,
        "ephemeral": hex::encode(ephemeral),
        "wrapped": hex::encode(wrapped),
    })))
}

/// Segment `k` of a sealed file's ciphertext, raw: from this node's store, else a replica's.
pub async fn segment(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, id, k)): Path<(String, String, String)>,
) -> Result<Response, ApiError> {
    let (app, _account) = files_gate(&node, &headers, &app_id, false).await?;
    let raw = parse_blob_id(&id)?;
    let (_, seal) = sealed_file(&node, &app, &raw).await?;
    let hash = k
        .parse::<usize>()
        .ok()
        .and_then(|k| seal.segments.get(k))
        .ok_or_else(|| ApiError::new(404, NO_SUCH_SEGMENT, "the file has no such segment"))?;
    let bytes = node.segment_from(hash, &node.other_replicas(&app)).await?;
    Ok(([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_gets_its_exports_and_no_more_and_others_are_not_affected() {
        let limits = ExportLimits::default();
        let allowed = (0..100).filter(|_| limits.take("a")).count();
        assert_eq!(allowed, MAX_EXPORTS_PER_MINUTE as usize);
        assert!(limits.take("b"));
    }

    #[test]
    fn a_node_tracking_too_many_busy_accounts_refuses_a_new_one() {
        let limits = ExportLimits::default();
        for n in 0..MAX_TRACKED_ACCOUNTS {
            assert!(limits.take(&format!("account-{n}")));
        }
        assert!(!limits.take("one-more"));
        assert!(limits.take("account-0"));
    }
}
