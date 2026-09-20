//! Prove to an outside verifier that this node's account is the caller's.
//!
//! The online counterpart to `merod account link-proof`, and the one a UI can
//! drive: the offline command reads the root from a stopped node or a recovery
//! phrase, which is right for a root in cold storage and impossible for a button
//! in a desktop app talking to a running node.
//!
//! # Why this signs with the account root when almost nothing online does
//!
//! Every other root signature the node makes while running is *about a device* —
//! `pair-complete` certifies one, `revoke` withdraws one, `rescope` narrows one.
//! The claim an outside service needs before it files an account under somebody's
//! login is a different one, and the only one a device genuinely cannot make:
//! **this account is mine**. A device certificate is also root-signed and is the
//! credential such a service is most likely to be offered instead, but it asserts
//! that a device was certified, at some past moment, and it is a static blob
//! whoever obtained a copy can replay.
//!
//! So the root signs, and the statement is bounded and addressed to keep that a
//! once-per-verifier act: one `audience`, one `challenge`, and an expiry clamped
//! to [`MAX_LINK_PROOF_VALIDITY_SECS`].
//!
//! # No op, no publish
//!
//! Nothing here reaches the context actor, because nothing is published. A link
//! proof is a local signature over local facts, verified by a party that holds no
//! Calimero state at all — so this reads the store directly, as
//! `GET /admin-api/identity` does, rather than paying an actor round trip to
//! produce bytes no replica will ever see.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::response::IntoResponse;
use axum::Extension;
use calimero_account::{AccountLink, Audience, SignedAccountLink};
use calimero_governance_store::NodeDeviceRepository;
use calimero_server_primitives::admin::{
    AccountLinkProofApiRequest, AccountLinkProofApiResponse, AccountLinkProofApiResponseData,
    MAX_LINK_PROOF_VALIDITY_SECS,
};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::handlers::account::{decode32, no_account_error};
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<AccountLinkProofApiRequest>,
) -> impl IntoResponse {
    let challenge = match decode32(&req.challenge, "challenge") {
        Ok(bytes) => bytes,
        Err(err) => return err.into_response(),
    };

    // Read, never minted. `provision_account_root` here would sign with a key
    // that owns nothing: the proof would verify against itself and assert an
    // account nobody has ever been a member of — the same reason
    // `merod account link-proof` refuses rather than mints.
    let root = match NodeDeviceRepository::new(&state.store).account_root() {
        Ok(Some(root)) => root,
        Ok(None) => return no_account_error().into_response(),
        Err(err) => {
            error!(error=?err, "link proof: could not read this node's account root");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "could not read this node's account root".to_owned(),
            }
            .into_response();
        }
    };

    let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        error!("link proof: the system clock is before the unix epoch");
        return ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: "the system clock is before the unix epoch".to_owned(),
        }
        .into_response();
    };
    let issued_at = now.as_secs();
    let expires_at = issued_at.saturating_add(req.valid_for_secs.min(MAX_LINK_PROOF_VALIDITY_SECS));

    let account = root.account();
    // One spelling of "who is this addressed to", shared with `merod`: see
    // `Audience::from_spelling`.
    let audience = Audience::from_spelling(&req.audience);

    // Epoch 0 with an empty chain, matching every other node-side minter: nothing
    // in the tree rotates a node's own account root, so there are no handoffs for
    // a verifier to walk. When rotation arrives this reads the epoch instead.
    let link = match AccountLink::sign(
        root.signing_key(),
        account,
        audience,
        challenge,
        issued_at,
        expires_at,
        0,
    ) {
        Ok(link) => link,
        Err(err) => {
            error!(%err, "link proof: failed to sign");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "failed to sign the account link".to_owned(),
            }
            .into_response();
        }
    };

    let proof = SignedAccountLink {
        genesis: root.genesis(),
        chain: vec![],
        statement: link,
    };
    let encoded = match borsh::to_vec(&proof) {
        Ok(bytes) => hex::encode(bytes),
        Err(err) => {
            error!(%err, "link proof: failed to encode");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "failed to encode the account link proof".to_owned(),
            }
            .into_response();
        }
    };

    // The audience is logged and the proof is not. The proof is not a secret —
    // it authorises nothing — but it is the artifact, and a log is a second place
    // it would sit around past its expiry for no benefit.
    info!(%account, audience = %req.audience, %expires_at, "signed an account link proof");

    ApiResponse {
        payload: AccountLinkProofApiResponse {
            data: AccountLinkProofApiResponseData {
                account_id: hex::encode(account.as_bytes()),
                proof: encoded,
                expires_at,
            },
        },
    }
    .into_response()
}
