use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use base64::{engine::general_purpose::STANDARD as base64_engine, Engine};
use calimero_server_primitives::admin::{TeeAttestRequest, TeeAttestResponse};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

#[cfg(target_os = "linux")]
use configfs_tsm::create_tdx_quote;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<TeeAttestRequest>,
) -> impl IntoResponse {
    info!(nonce=%req.nonce, application_id=?req.application_id, "Generating TEE attestation");

    // 1. Validate nonce
    let nonce = match hex::decode(&req.nonce) {
        Ok(n) => n,
        Err(_) => {
            error!("Invalid nonce format");
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid nonce format (must be 64 hex characters)".to_owned(),
            }
            .into_response();
        }
    };

    if nonce.len() != 32 {
        error!(nonce_len=%nonce.len(), "Invalid nonce length");
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Nonce must be exactly 32 bytes (64 hex characters)".to_owned(),
        }
        .into_response();
    }

    // 2. Get application bytecode hash (if requested)
    let app_hash = if let Some(application_id) = req.application_id {
        match state.node_client.get_application(&application_id) {
            Ok(Some(application)) => {
                // Use the bytecode BlobId (which is already a hash) directly
                // BlobId derefs to &[u8; 32]
                *application.blob.bytecode
            }
            Ok(None) => {
                error!(application_id=%application_id, "Application not found");
                return ApiError {
                    status_code: StatusCode::NOT_FOUND,
                    message: format!("Application '{}' not found", application_id),
                }
                .into_response();
            }
            Err(err) => {
                error!(application_id=%application_id, error=?err, "Failed to get application");
                return ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    message: format!("Failed to get application: {}", err),
                }
                .into_response();
            }
        }
    } else {
        [0u8; 32]
    };

    // 3. Build report_data: nonce[32] || app_hash[32]
    let mut report_data = [0u8; 64];
    report_data[..32].copy_from_slice(&nonce);
    report_data[32..].copy_from_slice(&app_hash);

    // 4. Generate TDX quote
    #[cfg(target_os = "linux")]
    let quote_bytes = match create_tdx_quote(report_data) {
        Ok(quote) => quote,
        Err(err) => {
            error!(error=?err, "Failed to generate TDX quote");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("Failed to generate TDX quote: {:?}", err),
            }
            .into_response();
        }
    };

    #[cfg(not(target_os = "linux"))]
    let quote_bytes = {
        // Mock quote for development on non-Linux platforms
        tracing::warn!("Generating mock TDX quote (non-Linux platform)");
        tracing::warn!("This quote will NOT pass cryptographic verification!");

        // Return a minimal mock quote that at least has the report_data in it
        // Real quotes are ~8KB, but this is just for local testing
        let mut mock_quote = vec![0u8; 128];
        // Put report_data at a known offset so client can still parse it (somewhat)
        mock_quote.extend_from_slice(&report_data);
        mock_quote
    };

    info!("TEE attestation generated successfully");

    ApiResponse {
        payload: TeeAttestResponse::new(base64_engine.encode(&quote_bytes)),
    }
    .into_response()
}
