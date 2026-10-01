//! `POST /admin-api/contexts/{context_id}/presence-intents`: an account's
//! ephemeral presence, published through this node as its relay.
//!
//! Public, like the other intents routes: the device's signature over the
//! statement is the credential, as a warrant is for `/intents`. Replay is
//! bounded by the 7 s freshness window and the seq the relay's presence store
//! keeps per device. It is not `/intents` because presence runs nothing,
//! spends no warrant nonce and changes no state: the relay checks the update
//! and forwards it (`NodeClient::publish_delegated_ephemeral`).

use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_account::{AccountProof, DeviceCert};
use calimero_node_primitives::presence::{
    DelegatedPresenceError, PresenceStatement, PresenceUpdate,
};
use calimero_primitives::context::ContextId;
use tracing::debug;

use crate::AdminState;

/// One presence update as a browser sends it.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresenceIntentRequest {
    /// Hex of the slice, or `null` to retract.
    pub state: Option<String>,
    pub seq: u64,
    pub sent_at_ms: u64,
    /// Hex, 64 bytes: the device key's signature over the statement.
    pub signature: String,
    /// Hex of the borsh `AccountProof<DeviceCert>` tying the device to its account.
    pub author_proof: String,
}

/// Build the update a request describes. The context comes from the path and
/// the author is the certificate's key, so a client cannot name another.
pub(crate) fn update_from_request(
    context_id: ContextId,
    req: &PresenceIntentRequest,
) -> Result<PresenceUpdate, String> {
    let proof_bytes = hex::decode(req.author_proof.trim())
        .map_err(|err| format!("authorProof is not hex: {err}"))?;
    let proof: AccountProof<DeviceCert> = borsh::from_slice(&proof_bytes)
        .map_err(|err| format!("authorProof is not a credential: {err}"))?;
    let state = req
        .state
        .as_deref()
        .map(hex::decode)
        .transpose()
        .map_err(|err| format!("state is not hex: {err}"))?;
    let signature: [u8; 64] = hex::decode(req.signature.trim())
        .map_err(|err| format!("signature is not hex: {err}"))?
        .try_into()
        .map_err(|_| "signature is not 64 bytes".to_owned())?;
    let statement = PresenceStatement::new(
        context_id,
        proof.statement.sign_pk,
        req.seq,
        req.sent_at_ms,
        &state,
    );
    Ok(PresenceUpdate {
        statement,
        signature,
        certificate: Some(proof),
        state,
    })
}

/// One status per refusal.
pub(crate) fn status_for(err: &DelegatedPresenceError) -> StatusCode {
    match err {
        DelegatedPresenceError::Refused(_) | DelegatedPresenceError::Stale => {
            StatusCode::BAD_REQUEST
        }
        DelegatedPresenceError::NotAnAccount
        | DelegatedPresenceError::NotAMember
        | DelegatedPresenceError::DeviceRevoked => StatusCode::FORBIDDEN,
        DelegatedPresenceError::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
        DelegatedPresenceError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        DelegatedPresenceError::NoGroupKey => StatusCode::SERVICE_UNAVAILABLE,
        DelegatedPresenceError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn refusal(status: StatusCode, reason: String) -> axum::response::Response {
    (status, Json(serde_json::json!({ "error": reason }))).into_response()
}

pub async fn handler(
    Path(context_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<PresenceIntentRequest>,
) -> impl IntoResponse {
    let context_id: ContextId = match context_id_str.parse() {
        Ok(id) => id,
        Err(err) => {
            return refusal(
                StatusCode::BAD_REQUEST,
                format!("context id '{context_id_str}' is not valid: {err}"),
            )
        }
    };
    let update = match update_from_request(context_id, &req) {
        Ok(update) => update,
        Err(reason) => return refusal(StatusCode::BAD_REQUEST, reason),
    };
    match state
        .node_client
        .publish_delegated_ephemeral(context_id, update)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => {
            debug!(%context_id, %err, "refusing a presence intent");
            refusal(status_for(&err), err.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use calimero_account::{AccountGenesis, AccountProof, DeviceCert, DeviceId, KemPublicKey};
    use calimero_node_primitives::presence::DelegatedPresenceError;
    use calimero_primitives::identity::PrivateKey;

    use super::*;

    fn device_and_proof() -> (PrivateKey, AccountProof<DeviceCert>) {
        let root = PrivateKey::from([0x55u8; 32]);
        let genesis = AccountGenesis::new(root.public_key());
        let device = PrivateKey::from([0x66u8; 32]);
        let cert = DeviceCert::sign(
            &root,
            genesis.account_id(),
            DeviceId::from([0x77u8; 32]),
            &device.public_key(),
            &KemPublicKey::from([0x88u8; 32]),
            0,
            1,
        )
        .expect("the root signs its device");
        (
            device,
            AccountProof {
                genesis,
                chain: vec![],
                statement: cert,
            },
        )
    }

    fn request_for(state: Option<&[u8]>) -> (ContextId, PresenceIntentRequest) {
        let (device, proof) = device_and_proof();
        let context_id = ContextId::from([0x11u8; 32]);
        let update = PresenceUpdate::signed(
            &device,
            context_id,
            7,
            1_700_000_000_000,
            state.map(<[u8]>::to_vec),
            Some(proof.clone()),
        )
        .expect("sign");
        let req = PresenceIntentRequest {
            state: state.map(hex::encode),
            seq: 7,
            sent_at_ms: 1_700_000_000_000,
            signature: hex::encode(update.signature),
            author_proof: hex::encode(borsh::to_vec(&proof).expect("borsh")),
        };
        (context_id, req)
    }

    #[test]
    fn a_request_signed_by_the_certificate_key_rebuilds_a_verifying_update() {
        let (context_id, req) = request_for(Some(b"{\"typing\":true}"));
        let update = update_from_request(context_id, &req).expect("parses");
        let verified = update.verify(context_id).expect("verifies");
        assert!(verified.account.is_some());
    }

    #[test]
    fn a_retract_parses_with_no_state() {
        let (context_id, req) = request_for(None);
        let update = update_from_request(context_id, &req).expect("parses");
        assert_eq!(update.state, None);
        assert!(update.verify(context_id).is_ok());
    }

    #[test]
    fn the_path_context_is_the_one_signed() {
        let (_context_id, req) = request_for(Some(b"x"));
        let other = ContextId::from([0x99u8; 32]);
        // Rebuilt for the path's context, the device's signature no longer covers it.
        let update = update_from_request(other, &req).expect("parses");
        assert!(update.verify(other).is_err());
    }

    #[test]
    fn refuses_non_hex_state_and_a_short_signature() {
        let (context_id, mut req) = request_for(Some(b"x"));
        req.state = Some("zz".to_owned());
        assert!(update_from_request(context_id, &req).is_err());
        let (context_id, mut req) = request_for(Some(b"x"));
        req.signature = "00".to_owned();
        assert!(update_from_request(context_id, &req).is_err());
    }

    #[test]
    fn each_refusal_has_its_status() {
        use DelegatedPresenceError as E;
        for (err, status) in [
            (E::Refused("x".to_owned()), StatusCode::BAD_REQUEST),
            (E::Stale, StatusCode::BAD_REQUEST),
            (E::NotAnAccount, StatusCode::FORBIDDEN),
            (E::NotAMember, StatusCode::FORBIDDEN),
            (E::DeviceRevoked, StatusCode::FORBIDDEN),
            (E::TooLarge(1), StatusCode::PAYLOAD_TOO_LARGE),
            (E::RateLimited, StatusCode::TOO_MANY_REQUESTS),
            (E::NoGroupKey, StatusCode::SERVICE_UNAVAILABLE),
            (
                E::Internal("x".to_owned()),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ] {
            assert_eq!(status_for(&err), status, "{err:?}");
        }
    }
}
