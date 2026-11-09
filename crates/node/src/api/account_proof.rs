//! `POST /auth/account-proof`: the node's signed statement that the passkey
//! session asking is an account's, binding a relying party's nonce and
//! audience ([`calimero_client::attest::AccountProof`]), for a console that
//! records which account a person controls without taking their word for it
//! (mdma's deployer and ownership grants). `GET /ops/nodes/{id}/receipt-key`:
//! the receipt key a proof names, as this node attested it, for the operator
//! who checks proofs over its attested channel.

use std::ops::RangeInclusive;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use super::{holder, operator, ApiResult};
use crate::error::ApiError;
use crate::node::Node;

/// How long a proof is valid, in seconds: long enough to carry it to the
/// relying party, short enough that one found later is spent.
pub const ACCOUNT_PROOF_TTL: u64 = 5 * 60;

/// The length of a relying party's nonce, in bytes of text.
pub const ACCOUNT_PROOF_NONCE: RangeInclusive<usize> = 16..=128;

/// The longest audience, in bytes of text.
pub const MAX_ACCOUNT_PROOF_AUDIENCE: usize = 128;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProofBody {
    nonce: String,
    audience: String,
}

/// Letters, digits and `-_.:/`: what a nonce (hex, base64url) and an audience
/// (a name, an origin) are written in, and the same text in every canonical
/// JSON encoder, so a verifier in any language rebuilds the signed bytes.
fn plain(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-_.:/".contains(&b))
}

/// Sign that the caller's passkey session is its account's, binding `nonce`
/// for `audience` for [`ACCOUNT_PROOF_TTL`]. A passkey session only, and not
/// one opened on an app's frontend (`holder`): an app's page cannot vouch
/// for its user to someone else, and an API key is not the person.
pub(super) async fn prove(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<ProofBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    if !ACCOUNT_PROOF_NONCE.contains(&body.nonce.len()) || !plain(&body.nonce) {
        return Err(ApiError::bad_request(
            "BAD_NONCE",
            "nonce must be 16 to 128 letters, digits or -_.:/",
        ));
    }
    if body.audience.is_empty()
        || body.audience.len() > MAX_ACCOUNT_PROOF_AUDIENCE
        || !plain(&body.audience)
    {
        return Err(ApiError::bad_request(
            "BAD_AUDIENCE",
            "audience must be 1 to 128 letters, digits or -_.:/",
        ));
    }
    let proof = node.account_proof(&account, &body.nonce, &body.audience, ACCOUNT_PROOF_TTL);
    let out = serde_json::to_value(proof).map_err(|e| ApiError::internal(&e.to_string()))?;
    Ok(Json(out))
}

/// The receipt key of node `id` as this node attested it (its own, or a
/// peer's under the peer policy), with the release the same quote named:
/// what the operator checks an [`calimero_client::attest::AccountProof`]
/// from that node against. `404` for a node that is not a peer, or that
/// could not be attested.
pub(super) async fn ops_receipt_key(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> ApiResult {
    operator(&node, &headers)?;
    let issuer = node
        .session_issuer(id, false)
        .await
        .ok_or_else(|| ApiError::not_found("no attested node with that id"))?;
    Ok(Json(json!({
        "node": id,
        "key": hex::encode(issuer.key),
        "release": issuer.release,
        "attestedAt": issuer.at,
    })))
}

#[cfg(test)]
mod tests {
    use super::plain;

    #[test]
    fn only_plain_text_is_signed() {
        assert!(plain("0123456789abcdefABCDEF-_"));
        assert!(plain("https://cloud.calimero.network"));
        assert!(!plain("a b"));
        assert!(!plain("a\"b"));
        assert!(!plain("é"));
        assert!(!plain("a\\u0000"));
    }
}
