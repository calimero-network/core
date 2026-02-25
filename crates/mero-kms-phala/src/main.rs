//! mero-kms-phala: Key management service for merod nodes running in Phala Cloud TEE.
//!
//! This service validates TDX attestations from merod nodes and releases deterministic
//! storage encryption keys based on peer ID using Phala's dstack key derivation.

mod handlers;

use std::net::SocketAddr;

use eyre::{bail, Result as EyreResult};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::handlers::create_router;

/// Attestation verification policy for key release.
#[derive(Debug, Clone)]
pub struct AttestationPolicy {
    /// Whether measurement checks are enforced.
    pub enforce_measurement_policy: bool,
    /// Allowed TCB statuses (normalized to lowercase).
    pub allowed_tcb_statuses: Vec<String>,
    /// Allowed MRTD values (hex, lowercase, no 0x prefix).
    pub allowed_mrtd: Vec<String>,
    /// Allowed RTMR0 values (hex, lowercase, no 0x prefix).
    pub allowed_rtmr0: Vec<String>,
    /// Allowed RTMR1 values (hex, lowercase, no 0x prefix).
    pub allowed_rtmr1: Vec<String>,
    /// Allowed RTMR2 values (hex, lowercase, no 0x prefix).
    pub allowed_rtmr2: Vec<String>,
    /// Allowed RTMR3 values (hex, lowercase, no 0x prefix).
    pub allowed_rtmr3: Vec<String>,
}

impl Default for AttestationPolicy {
    fn default() -> Self {
        Self {
            enforce_measurement_policy: true,
            allowed_tcb_statuses: vec!["uptodate".to_owned()],
            allowed_mrtd: Vec::new(),
            allowed_rtmr0: Vec::new(),
            allowed_rtmr1: Vec::new(),
            allowed_rtmr2: Vec::new(),
            allowed_rtmr3: Vec::new(),
        }
    }
}

/// Configuration for the key releaser service.
#[derive(Debug, Clone)]
pub struct Config {
    /// Socket address to listen on.
    pub listen_addr: SocketAddr,
    /// Path to the dstack Unix socket.
    pub dstack_socket_path: String,
    /// Whether to accept mock attestations (for development only).
    pub accept_mock_attestation: bool,
    /// Attestation policy used for key release decisions.
    pub attestation_policy: AttestationPolicy,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen_addr: SocketAddr::from(([0, 0, 0, 0], 8080)),
            dstack_socket_path: "/var/run/dstack.sock".to_string(),
            accept_mock_attestation: false,
            attestation_policy: AttestationPolicy::default(),
        }
    }
}

impl Config {
    /// Load configuration from environment variables.
    pub fn from_env() -> EyreResult<Self> {
        let listen_addr = std::env::var("LISTEN_ADDR")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 8080)));

        let dstack_socket_path = std::env::var("DSTACK_SOCKET_PATH")
            .unwrap_or_else(|_| "/var/run/dstack.sock".to_string());

        let accept_mock_attestation = std::env::var("ACCEPT_MOCK_ATTESTATION")
            .map(|v| parse_bool_flag(&v))
            .unwrap_or(false);

        let enforce_measurement_policy = std::env::var("ENFORCE_MEASUREMENT_POLICY")
            .map(|v| parse_bool_flag(&v))
            .unwrap_or(true);

        let allowed_tcb_statuses = parse_csv_env("ALLOWED_TCB_STATUSES")
            .unwrap_or_else(|| vec!["uptodate".to_owned()]);

        let allowed_mrtd = parse_measurement_list_env("ALLOWED_MRTD", "MRTD", 48)?;
        let allowed_rtmr0 = parse_measurement_list_env("ALLOWED_RTMR0", "RTMR0", 48)?;
        let allowed_rtmr1 = parse_measurement_list_env("ALLOWED_RTMR1", "RTMR1", 48)?;
        let allowed_rtmr2 = parse_measurement_list_env("ALLOWED_RTMR2", "RTMR2", 48)?;
        let allowed_rtmr3 = parse_measurement_list_env("ALLOWED_RTMR3", "RTMR3", 48)?;

        if enforce_measurement_policy && !accept_mock_attestation && allowed_tcb_statuses.is_empty()
        {
            bail!(
                "Measurement policy is enforced, but ALLOWED_TCB_STATUSES is empty. \
                 Configure at least one allowed status (recommended: UpToDate)."
            );
        }

        if enforce_measurement_policy && !accept_mock_attestation && allowed_mrtd.is_empty() {
            bail!(
                "Measurement policy is enforced, but ALLOWED_MRTD is empty. \
                 Configure at least one trusted MRTD to prevent releasing keys to arbitrary TEEs."
            );
        }

        Ok(Self {
            listen_addr,
            dstack_socket_path,
            accept_mock_attestation,
            attestation_policy: AttestationPolicy {
                enforce_measurement_policy,
                allowed_tcb_statuses,
                allowed_mrtd,
                allowed_rtmr0,
                allowed_rtmr1,
                allowed_rtmr2,
                allowed_rtmr3,
            },
        })
    }
}

fn parse_bool_flag(value: &str) -> bool {
    matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes")
}

fn parse_csv_env(name: &str) -> Option<Vec<String>> {
    std::env::var(name).ok().map(|value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_lowercase())
            .collect()
    })
}

fn parse_measurement_list_env(
    name: &str,
    label: &str,
    expected_bytes: usize,
) -> EyreResult<Vec<String>> {
    let Some(values) = parse_csv_env(name) else {
        return Ok(Vec::new());
    };

    let mut parsed = Vec::with_capacity(values.len());
    for value in values {
        let normalized = normalize_hex(&value);
        let bytes = hex::decode(&normalized).map_err(|e| {
            eyre::eyre!("{} value '{}' from {} is not valid hex: {}", label, value, name, e)
        })?;
        if bytes.len() != expected_bytes {
            bail!(
                "{} value '{}' from {} has invalid length: expected {} bytes, got {}",
                label,
                value,
                name,
                expected_bytes,
                bytes.len()
            );
        }
        parsed.push(normalized);
    }

    Ok(parsed)
}

fn normalize_hex(value: &str) -> String {
    value.trim().trim_start_matches("0x").to_ascii_lowercase()
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(true)
        .with_level(true)
        .init();

    // Load configuration
    let config = Config::from_env()?;

    info!("Starting mero-kms-phala");
    info!("Listen address: {}", config.listen_addr);
    info!("Dstack socket: {}", config.dstack_socket_path);
    info!(
        "Accept mock attestation: {}",
        config.accept_mock_attestation
    );
    info!(
        "Measurement policy enforced: {}",
        config.attestation_policy.enforce_measurement_policy
    );
    info!(
        "Policy entries: tcb_statuses={}, mrtd={}, rtmr0={}, rtmr1={}, rtmr2={}, rtmr3={}",
        config.attestation_policy.allowed_tcb_statuses.len(),
        config.attestation_policy.allowed_mrtd.len(),
        config.attestation_policy.allowed_rtmr0.len(),
        config.attestation_policy.allowed_rtmr1.len(),
        config.attestation_policy.allowed_rtmr2.len(),
        config.attestation_policy.allowed_rtmr3.len()
    );

    if config.accept_mock_attestation {
        tracing::warn!(
            "WARNING: Mock attestation acceptance is enabled. This should NEVER be used in production!"
        );
    }

    // Create router with handlers
    let app = create_router(config.clone())
        .layer(TraceLayer::new_for_http())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        );

    // Start server
    let listener = tokio::net::TcpListener::bind(config.listen_addr).await?;
    info!("Server listening on {}", config.listen_addr);

    axum::serve(listener, app).await?;

    Ok(())
}
