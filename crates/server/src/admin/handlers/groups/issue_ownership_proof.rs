use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use base64::{engine::general_purpose::STANDARD as base64_engine, Engine};
use calimero_context_client::group::IssueOwnershipProofRequest;
use calimero_server_primitives::admin::{
    IssueOwnershipProofApiRequest, IssueOwnershipProofApiResponse,
};
use tracing::{error, info};

use super::{parse_context_id, parse_group_id};
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<IssueOwnershipProofApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let context_id = match parse_context_id(&req.context_id) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(
        group_id=%group_id_str,
        context_id=%req.context_id,
        audience=%req.audience,
        "Issuing ownership proof"
    );

    let result = state
        .ctx_client
        .issue_ownership_proof(IssueOwnershipProofRequest {
            group_id,
            context_id,
            audience: req.audience,
            subject: req.subject,
            nonce: req.nonce,
            expires_at_ms: req.expires_at_ms,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(
                group_id=%group_id_str,
                signer=%resp.signer_public_key,
                "Ownership proof issued"
            );
            ApiResponse {
                payload: IssueOwnershipProofApiResponse {
                    // `PublicKey: Display` produces base58 (see calimero-primitives::identity).
                    signer_public_key: resp.signer_public_key.to_string(),
                    signed_payload: base64_engine.encode(&resp.signed_payload),
                    signature: base64_engine.encode(resp.signature),
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to issue ownership proof");
            err.into_response()
        }
    }
}
