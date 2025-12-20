//! TEE host information retrieval.

use tracing::{error, warn};

use crate::error::AttestationError;

/// Information about the TEE host environment.
#[derive(Debug, Clone)]
pub struct TeeInfo {
    /// Cloud provider (e.g., "gcp", "azure", "unknown").
    pub cloud_provider: String,
    /// OS image name.
    pub os_image: String,
    /// MRTD (Measurement of the TDX module) - hex encoded.
    pub mrtd: String,
}

/// Get the MRTD (launch measurement) from TDX.
#[cfg(target_os = "linux")]
fn get_mrtd() -> eyre::Result<String> {
    use tdx_workload_attestation::provider::AttestationProvider;
    use tdx_workload_attestation::tdx::LinuxTdxProvider;

    let provider = LinuxTdxProvider::new();
    let mrtd = provider.get_launch_measurement()?;
    Ok(hex::encode(mrtd))
}

/// Get the MRTD (mock for non-Linux platforms).
#[cfg(not(target_os = "linux"))]
fn get_mrtd() -> eyre::Result<String> {
    warn!("Running on non-Linux platform - using mock MRTD");
    Ok("0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
        .to_owned())
}

/// Detect host information including cloud provider and OS image.
#[cfg(target_os = "linux")]
async fn detect_host_info() -> eyre::Result<TeeInfo> {
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

                return Ok(TeeInfo {
                    cloud_provider: "gcp".to_owned(),
                    os_image: image_name,
                    mrtd,
                });
            }
        }
    }

    // Fallback: Unknown platform
    Ok(TeeInfo {
        cloud_provider: "unknown".to_owned(),
        os_image: "unknown".to_owned(),
        mrtd,
    })
}

/// Detect host information (non-Linux fallback).
#[cfg(not(target_os = "linux"))]
async fn detect_host_info() -> eyre::Result<TeeInfo> {
    let mrtd = get_mrtd()?;
    Ok(TeeInfo {
        cloud_provider: "unknown".to_owned(),
        os_image: "unknown".to_owned(),
        mrtd,
    })
}

/// Get information about the TEE environment.
///
/// # Returns
/// A `TeeInfo` struct containing cloud provider, OS image, and MRTD.
///
/// # Errors
/// Returns an error if TEE information cannot be retrieved.
pub async fn get_tee_info() -> Result<TeeInfo, AttestationError> {
    detect_host_info().await.map_err(|err| {
        error!(error=?err, "Failed to get TEE info");
        AttestationError::InfoRetrievalFailed(err.to_string())
    })
}
