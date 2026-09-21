//! Sign a verifier-specified payload with this node's account root.
//!
//! # Why core does not define the format here
//!
//! Every other credential this crate emits is a statement *core* designed: a
//! borsh struct over a core domain, checked by `calimero_account`'s own
//! verifier. This is the opposite case. An outside verifier — mdma — already
//! specified what the account root must sign, it shipped before core did, and a
//! signature core finds tidier is a signature that fails at the far end.
//!
//! So the caller supplies the payload and core supplies only the domain. For
//! mdma the payload *is* the challenge it sealed and issued, which means core
//! learns nothing about that format beyond which domain it belongs to.
//!
//! # Why the domain is a name and not bytes
//!
//! "Sign these bytes with the account root" is a signing oracle, and the root is
//! the one key that can certify a device — that is, take over the account.
//!
//! A length guard does not make it safe. Core's own credentials sign a 32-byte
//! `domain_hash` digest, so refusing 32-byte payloads looks sufficient, but
//! `calimero_governance_types::admitter_endorsement_payload` signs a raw
//! concatenation of variable length, and nothing stops a future signing site
//! from doing the same. Naming the reachable domains inverts the problem: a new
//! signing site elsewhere cannot become a target here, because its domain is not
//! in [`ExternalSigningDomain`]. `no_external_domain_shares_a_prefix_with_a_core_domain`
//! in `calimero-account` is what keeps that true as either set grows.
//!
//! # What this does not establish
//!
//! Possession of the root, and nothing else. It does not check that the payload
//! is a challenge anyone issued, that it is fresh, or that the caller is entitled
//! to link this account anywhere — all three belong to the verifier, which is the
//! party that issued the challenge and knows who it issued it to.
//!
//! # No op, no publish
//!
//! Nothing here reaches the context actor. This is a local signature over bytes a
//! third party supplied, verified by someone holding no Calimero state, so it
//! reads the store directly rather than paying an actor round trip for bytes no
//! replica will ever see.

use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use calimero_account::{sign_external, ExternalSigningDomain};
use calimero_governance_store::NodeDeviceRepository;
use calimero_server_primitives::admin::{
    AccountSignWithRootApiRequest, AccountSignWithRootApiResponse,
    AccountSignWithRootApiResponseData,
};
use reqwest::StatusCode;
use tracing::{debug, error};

use crate::admin::handlers::account::no_account_error;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<AccountSignWithRootApiRequest>,
) -> impl IntoResponse {
    // Validation already rejected an unknown name with a 400 naming the accepted
    // set; this is the authoritative resolution, so the allowlist is enforced
    // here even if a future caller path skips validation.
    let Some(domain) = ExternalSigningDomain::from_name(&req.domain) else {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!(
                "unknown signing domain '{}'; expected one of: {}",
                req.domain,
                ExternalSigningDomain::names().join(", ")
            ),
        }
        .into_response();
    };

    let payload = match hex::decode(&req.payload) {
        Ok(bytes) => bytes,
        Err(_) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "payload is not valid hex".to_owned(),
            }
            .into_response()
        }
    };

    // Read, never minted. Provisioning a root here would sign with a key that
    // owns nothing: the signature would verify against itself and assert an
    // account nobody has ever been a member of.
    let root = match NodeDeviceRepository::new(&state.store).account_root() {
        Ok(Some(root)) => root,
        Ok(None) => return no_account_error().into_response(),
        Err(err) => {
            error!(error=?err, "sign-with-root: could not read this node's account root");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "could not read this node's account root".to_owned(),
            }
            .into_response();
        }
    };

    let (public_key, signature) = match sign_external(root.signing_key(), domain, &payload) {
        Ok(signed) => signed,
        Err(err) => {
            error!(%err, "sign-with-root: failed to sign");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "failed to sign with the account root".to_owned(),
            }
            .into_response();
        }
    };

    // The payload is not logged: it is a third party's challenge, and a node log
    // is a wider audience than the exchange it belongs to. The ACCOUNT is not
    // logged at info for the same reason carried one step further -- a fleet
    // node ships its journal to a store operators can read, so naming the
    // account here records which user proved themselves to which verifier, and
    // when. `debug!` keeps it for a node someone is actively debugging.
    debug!(
        account = %root.account(),
        domain = %req.domain,
        payload_len = payload.len(),
        "signed a payload with the account root for an outside verifier"
    );

    ApiResponse {
        payload: AccountSignWithRootApiResponse {
            data: AccountSignWithRootApiResponseData {
                root_public_key: hex::encode(public_key.digest()),
                signature: BASE64.encode(signature),
                account_id: hex::encode(root.account().as_bytes()),
            },
        },
    }
    .into_response()
}
