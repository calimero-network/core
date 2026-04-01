use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_network_primitives::specialized_node_invite::SpecializedNodeType;
use calimero_node_primitives::sync::BroadcastMessage;
use calimero_server_primitives::admin::FleetJoinRequest;
use calimero_tee_attestation::{build_report_data, generate_attestation};
use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use tracing::{error, info, warn};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<FleetJoinRequest>,
) -> impl IntoResponse {
    let group_id_bytes = match hex::decode(&req.group_id) {
        Ok(b) if b.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&b);
            arr
        }
        _ => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "group_id must be 64 hex chars (32 bytes)".to_owned(),
            }
            .into_response();
        }
    };

    info!(
        group_id = %req.group_id,
        "Fleet join: generating attestation and subscribing to group"
    );

    // Create identity first (before subscribe, so we can clean up if needed)
    let our_public_key = match state.ctx_client.new_identity() {
        Ok(pk) => pk,
        Err(err) => {
            error!(error=?err, "Failed to create identity for fleet join");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to create identity".to_owned(),
            }
            .into_response();
        }
    };

    // Generate attestation BEFORE subscribing -- this is the expensive step
    // and if it fails we haven't leaked a subscription
    let pk_hash: [u8; 32] = Sha256::digest(&*our_public_key).into();
    let nonce: [u8; 32] = rand::random();
    let report_data = build_report_data(&nonce, Some(&pk_hash));

    let attestation = match generate_attestation(report_data) {
        Ok(result) => {
            if result.is_mock {
                error!("Mock attestation generated -- fleet-join requires real TDX hardware");
                return ApiError {
                    status_code: StatusCode::NOT_IMPLEMENTED,
                    message: "TDX attestation required -- mock not accepted for fleet join"
                        .to_owned(),
                }
                .into_response();
            }
            result
        }
        Err(err) => {
            error!(error=?err, "Failed to generate TDX attestation");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to generate attestation".to_owned(),
            }
            .into_response();
        }
    };

    // Serialize the broadcast payload BEFORE subscribing
    let broadcast = BroadcastMessage::TeeAttestationAnnounce {
        quote_bytes: attestation.quote_bytes,
        public_key: our_public_key,
        nonce,
        node_type: SpecializedNodeType::ReadOnly,
    };

    let payload = match borsh::to_vec(&broadcast) {
        Ok(p) => p,
        Err(err) => {
            error!(error=?err, "Failed to serialize TeeAttestationAnnounce");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to serialize announcement".to_owned(),
            }
            .into_response();
        }
    };

    // NOW subscribe -- all preparation succeeded
    if let Err(err) = state.node_client.subscribe_group(group_id_bytes).await {
        error!(error=?err, "Failed to subscribe to group topic");
        return ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Failed to subscribe to group".to_owned(),
        }
        .into_response();
    }

    match state
        .node_client
        .publish_on_group(group_id_bytes, payload)
        .await
    {
        Ok(_) => {
            info!(
                group_id = %req.group_id,
                %our_public_key,
                "TeeAttestationAnnounce broadcast on group topic"
            );
            ApiResponse {
                payload: serde_json::json!({
                    "status": "announced",
                    "group_id": req.group_id,
                    "public_key": our_public_key.to_string(),
                }),
            }
            .into_response()
        }
        Err(err) => {
            warn!(error=?err, "Failed to broadcast, unsubscribing from group");
            let _ = state
                .node_client
                .unsubscribe_group(group_id_bytes)
                .await;
            ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to broadcast attestation".to_owned(),
            }
            .into_response()
        }
    }
}
