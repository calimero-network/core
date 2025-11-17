use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{Quote, TeeAttestRequest, TeeAttestResponse};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

#[cfg(target_os = "linux")]
use base64::{engine::general_purpose::STANDARD as base64_engine, Engine};
#[cfg(target_os = "linux")]
use configfs_tsm::create_tdx_quote;
#[cfg(target_os = "linux")]
use tdx_quote::Quote as TdxQuote;

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

    // 4. Generate attestation (platform-specific)
    match generate_attestation(report_data).await {
        Ok((quote_b64, quote)) => {
            info!("TEE attestation generated successfully");
            ApiResponse {
                payload: TeeAttestResponse::new(quote_b64, quote),
            }
            .into_response()
        }
        Err(err) => err.into_response(),
    }
}

#[cfg(target_os = "linux")]
async fn generate_attestation(report_data: [u8; 64]) -> Result<(String, Quote), ApiError> {
    // Generate TDX quote using configfs-tsm
    let quote_bytes = create_tdx_quote(report_data).map_err(|err| {
        error!(error=?err, "Failed to generate TDX quote");
        ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("Failed to generate TDX quote: {:?}", err),
        }
    })?;

    // Parse the generated quote
    let tdx_quote = TdxQuote::from_bytes(&quote_bytes).map_err(|err| {
        error!(error=?err, "Failed to parse generated TDX quote");
        ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("Failed to parse generated TDX quote: {:?}", err),
        }
    })?;

    let quote = Quote::try_from(tdx_quote).map_err(|err| {
        error!(error=%err, "Failed to convert TDX quote to serializable format");
        ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("Failed to convert TDX quote: {}", err),
        }
    })?;

    let quote_b64 = base64_engine.encode(&quote_bytes);

    Ok((quote_b64, quote))
}

#[cfg(not(target_os = "linux"))]
async fn generate_attestation(_report_data: [u8; 64]) -> Result<(String, Quote), ApiError> {
    error!("TDX attestation generation is only supported on Linux");
    Err(ApiError {
        status_code: StatusCode::NOT_IMPLEMENTED,
        message: "TDX attestation generation is only supported on Linux with TDX hardware"
            .to_owned(),
    })
}
