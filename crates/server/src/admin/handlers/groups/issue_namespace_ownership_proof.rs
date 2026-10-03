use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use base64::{engine::general_purpose::STANDARD as base64_engine, Engine};
use calimero_context_client::group::IssueNamespaceOwnershipProofRequest;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    IssueNamespaceOwnershipProofApiRequest, IssueOwnershipProofApiResponse,
};
use tracing::{error, info, warn};

use super::parse_group_id;
use crate::admin::handlers::namespaces::namespace_founding;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<IssueNamespaceOwnershipProofApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(
        group_id=%group_id_str,
        audience=%req.audience,
        "Issuing namespace ownership proof"
    );

    let result = state
        .ctx_client
        .issue_namespace_ownership_proof(IssueNamespaceOwnershipProofRequest {
            group_id,
            audience: req.audience,
            subject: req.subject,
            nonce: req.nonce,
            expires_at_ms: req.expires_at_ms,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            let founding = namespace_founding(&state.store, &group_id);
            let credential = presented_credential(&state.store, &resp.signer_public_key);
            info!(
                group_id=%group_id_str,
                signer=%resp.signer_public_key,
                founding=founding.is_some(),
                credential=credential.is_some(),
                "Namespace ownership proof issued"
            );
            ApiResponse {
                payload: IssueOwnershipProofApiResponse {
                    // `PublicKey: Display` produces 64 hex (see calimero-primitives::identity).
                    signer_public_key: resp.signer_public_key.to_string(),
                    signed_payload: base64_engine.encode(&resp.signed_payload),
                    signature: base64_engine.encode(resp.signature),
                    founding,
                    credential,
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to issue namespace ownership proof");
            err.into_response()
        }
    }
}

/// The credential this node presents for the key that signed the proof, as hex
/// of its borsh encoding — what lets a verifier holding no governance state
/// check that the founding account's root certifies that key.
///
/// Best effort, like [`namespace_founding`]: a node that cannot produce one
/// still issues the proof, and a verifier that requires it refuses it there,
/// where the reason is visible. Nothing is minted to produce it
/// (`join_credential::presented`).
fn presented_credential(store: &calimero_store::Store, signer: &PublicKey) -> Option<String> {
    match calimero_context::join_credential::presented(store, signer) {
        Ok(Some(credential)) => match borsh::to_vec(&*credential) {
            Ok(bytes) => Some(hex::encode(bytes)),
            Err(err) => {
                warn!(?err, "could not encode this node's credential");
                None
            }
        },
        Ok(None) => None,
        Err(err) => {
            warn!(?err, "could not read this node's credential");
            None
        }
    }
}
