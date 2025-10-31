use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::TeeInfoResponse;
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

#[cfg(target_os = "linux")]
use tdx_workload_attestation::provider::AttestationProvider;
#[cfg(target_os = "linux")]
use tdx_workload_attestation::tdx::LinuxTdxProvider;

struct HostInfo {
    cloud_provider: String,
    os_image: String,
    mrtd: String,
}

#[cfg(target_os = "linux")]
fn get_mrtd() -> eyre::Result<String> {
    let provider = LinuxTdxProvider::new();
    let mrtd = provider.get_launch_measurement()?;
    Ok(hex::encode(mrtd))
}

#[cfg(not(target_os = "linux"))]
fn get_mrtd() -> eyre::Result<String> {
    // Mock MRTD for development on non-Linux platforms (e.g., macOS)
    tracing::warn!("Running on non-Linux platform - using mock MRTD");
    Ok("0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000".to_owned())
}

async fn detect_host_info() -> eyre::Result<HostInfo> {
    // Get MRTD first
    let mrtd = get_mrtd()?;

    // Try to detect GCP
    if let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    {
        if let Ok(response) = client
            .get("http://metadata.google.internal/computeMetadata/v1/instance/image")
            .header("Metadata-Flavor", "Google")
            .send()
            .await
        {
            if let Ok(image) = response.text().await {
                let image_name = image.split('/').last().unwrap_or(&image).to_owned();

                return Ok(HostInfo {
                    cloud_provider: "gcp".to_owned(),
                    os_image: image_name,
                    mrtd,
                });
            }
        }
    }

    // Fallback: Unknown platform
    Ok(HostInfo {
        cloud_provider: "unknown".to_owned(),
        os_image: "unknown".to_owned(),
        mrtd,
    })
}

pub async fn handler(Extension(_state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    info!("Getting TEE info");

    match detect_host_info().await {
        Ok(host_info) => {
            info!(
                cloud_provider=%host_info.cloud_provider,
                os_image=%host_info.os_image,
                "TEE info retrieved successfully"
            );

            ApiResponse {
                payload: TeeInfoResponse::new(
                    host_info.cloud_provider,
                    host_info.os_image,
                    host_info.mrtd,
                ),
            }
            .into_response()
        }
        Err(err) => {
            error!(error=?err, "Failed to get TEE info");

            ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("Failed to get TEE info: {}", err),
            }
            .into_response()
        }
    }
}
