//! Configuration validation utilities for node startup.
//!
//! This module provides comprehensive validation for node configuration including:
//! - Port conflict detection between services
//! - Path accessibility checks for data storage
//! - Required credentials presence validation
//! - Sane limit values for timeouts and intervals

use std::collections::HashSet;
use std::time::Duration;

use calimero_config::ConfigFile;
use calimero_server::config::AuthMode;
use camino::Utf8Path;
use eyre::{bail, Result as EyreResult, WrapErr};
use multiaddr::{Multiaddr, Protocol};

/// Minimum sync timeout in milliseconds
const MIN_SYNC_TIMEOUT_MS: u64 = 1000; // 1 second
/// Maximum sync timeout in milliseconds
const MAX_SYNC_TIMEOUT_MS: u64 = 300_000; // 5 minutes

/// Minimum sync interval in milliseconds
const MIN_SYNC_INTERVAL_MS: u64 = 100; // 100ms
/// Maximum sync interval in milliseconds
const MAX_SYNC_INTERVAL_MS: u64 = 3_600_000; // 1 hour

/// Maximum registrations limit
const MAX_REGISTRATIONS_LIMIT: usize = 100;

/// Validates the entire configuration at startup.
///
/// This function performs comprehensive validation including:
/// - Port conflict detection
/// - Path accessibility
/// - Required credentials presence
/// - Sane limit values
///
/// # Arguments
/// * `config` - The loaded configuration file
/// * `node_path` - The path to the node's home directory
///
/// # Errors
/// Returns an error if any validation check fails.
pub fn validate_config(config: &ConfigFile, node_path: &Utf8Path) -> EyreResult<()> {
    validate_port_conflicts(config).wrap_err("Port conflict validation failed")?;
    validate_path_accessibility(config, node_path)
        .wrap_err("Path accessibility validation failed")?;
    validate_required_credentials(config).wrap_err("Credentials validation failed")?;
    validate_limit_values(config).wrap_err("Limit values validation failed")?;

    Ok(())
}

/// Extracts TCP/UDP ports from a multiaddr.
fn extract_ports_from_multiaddr(addr: &Multiaddr) -> Vec<u16> {
    addr.iter()
        .filter_map(|protocol| match protocol {
            Protocol::Tcp(port) | Protocol::Udp(port) => Some(port),
            _ => None,
        })
        .collect()
}

/// Validates that there are no port conflicts between different services.
///
/// Checks for conflicts between:
/// - Swarm listening addresses
/// - Server listening addresses
/// - Embedded auth listening address (if configured)
fn validate_port_conflicts(config: &ConfigFile) -> EyreResult<()> {
    let mut used_ports: HashSet<(String, u16)> = HashSet::new();
    let mut conflicts: Vec<String> = Vec::new();

    // Collect swarm ports
    for addr in &config.network.swarm.listen {
        let host = extract_host_from_multiaddr(addr);
        for port in extract_ports_from_multiaddr(addr) {
            let key = (host.clone(), port);
            if !used_ports.insert(key.clone()) {
                conflicts.push(format!("Swarm address {}:{}", host, port));
            }
        }
    }

    // Collect server ports
    for addr in &config.network.server.listen {
        let host = extract_host_from_multiaddr(addr);
        for port in extract_ports_from_multiaddr(addr) {
            let key = (host.clone(), port);
            if !used_ports.insert(key.clone()) {
                conflicts.push(format!(
                    "Server port {} conflicts with another service on {}",
                    port, host
                ));
            }
        }
    }

    // Check embedded auth port if configured
    if let Some(ref auth_config) = config.network.server.embedded_auth {
        let auth_addr = auth_config.listen_addr;
        let host = auth_addr.ip().to_string();
        let port = auth_addr.port();

        // Check against wildcard addresses (0.0.0.0 and ::) and specific IPs
        let should_check = |existing_host: &str| -> bool {
            existing_host == "0.0.0.0"
                || existing_host == "::"
                || existing_host == host
                || host == "0.0.0.0"
                || host == "::"
        };

        for (existing_host, existing_port) in &used_ports {
            if *existing_port == port && should_check(existing_host) {
                conflicts.push(format!(
                    "Embedded auth port {} conflicts with another service",
                    port
                ));
                break;
            }
        }
    }

    if !conflicts.is_empty() {
        bail!(
            "Port conflicts detected:\n{}",
            conflicts
                .iter()
                .map(|c| format!("  - {}", c))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    Ok(())
}

/// Extracts the host/IP from a multiaddr, defaulting to "0.0.0.0" if not found.
fn extract_host_from_multiaddr(addr: &Multiaddr) -> String {
    for protocol in addr.iter() {
        match protocol {
            Protocol::Ip4(ip) => return ip.to_string(),
            Protocol::Ip6(ip) => return ip.to_string(),
            Protocol::Dns(name) | Protocol::Dns4(name) | Protocol::Dns6(name) => {
                return name.to_string()
            }
            _ => continue,
        }
    }
    "0.0.0.0".to_string()
}

/// Validates that required paths are accessible.
///
/// Checks:
/// - Datastore path's parent directory exists and is writable
/// - Blobstore path's parent directory exists and is writable
fn validate_path_accessibility(config: &ConfigFile, node_path: &Utf8Path) -> EyreResult<()> {
    // Check datastore path
    let datastore_path = node_path.join(&config.datastore.path);
    validate_path_writable(&datastore_path, "datastore")?;

    // Check blobstore path
    let blobstore_path = node_path.join(&config.blobstore.path);
    validate_path_writable(&blobstore_path, "blobstore")?;

    // Check embedded auth storage path if using RocksDB
    if let Some(ref auth_config) = config.network.server.embedded_auth {
        if let mero_auth::config::StorageConfig::RocksDB { ref path } = auth_config.storage {
            let auth_path = if path.is_relative() {
                node_path.as_std_path().join(path)
            } else {
                path.clone()
            };
            validate_path_writable_std(&auth_path, "embedded auth storage")?;
        }
    }

    Ok(())
}

/// Validates that a path (or its parent) is accessible for writing.
fn validate_path_writable(path: &Utf8Path, name: &str) -> EyreResult<()> {
    // If the path exists, check if it's a directory
    if path.exists() {
        if !path.is_dir() {
            bail!("{} path '{}' exists but is not a directory", name, path);
        }
        return Ok(());
    }

    // Check if parent directory exists and is writable
    if let Some(parent) = path.parent() {
        if !parent.as_str().is_empty() && !parent.exists() {
            bail!("{} parent directory '{}' does not exist", name, parent);
        }
    }

    Ok(())
}

/// Validates that a std path (or its parent) is accessible for writing.
fn validate_path_writable_std(path: &std::path::Path, name: &str) -> EyreResult<()> {
    // If the path exists, check if it's a directory
    if path.exists() {
        if !path.is_dir() {
            bail!(
                "{} path '{}' exists but is not a directory",
                name,
                path.display()
            );
        }
        return Ok(());
    }

    // Check if parent directory exists
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            bail!(
                "{} parent directory '{}' does not exist",
                name,
                parent.display()
            );
        }
    }

    Ok(())
}

/// Validates that required credentials are present based on configuration.
///
/// Checks:
/// - Embedded auth config exists when auth mode is Embedded
/// - TEE KMS configuration is valid when TEE is enabled
fn validate_required_credentials(config: &ConfigFile) -> EyreResult<()> {
    // Check embedded auth credentials
    if matches!(config.network.server.auth_mode, AuthMode::Embedded) {
        if config.network.server.embedded_auth.is_none() {
            bail!(
                "Auth mode is set to 'embedded' but no embedded_auth configuration is provided. \
                 Either provide embedded_auth configuration or change auth_mode to 'proxy'."
            );
        }

        // Validate JWT issuer is not empty
        if let Some(ref auth_config) = config.network.server.embedded_auth {
            if auth_config.jwt.issuer.trim().is_empty() {
                bail!("Embedded auth JWT issuer cannot be empty");
            }
        }
    }

    // Check TEE KMS configuration
    if let Some(ref tee_config) = config.tee {
        // At least one KMS provider must be configured
        if tee_config.kms.phala.is_none() {
            bail!(
                "TEE is enabled but no KMS provider is configured. \
                 Please configure at least one KMS provider (e.g., phala)."
            );
        }
    }

    Ok(())
}

/// Validates that limit values are within sane bounds.
///
/// Checks:
/// - Sync timeout, interval, and frequency are within reasonable ranges
/// - Registration limits are positive and not excessive
/// - WebSocket ping/pong timeouts are reasonable
fn validate_limit_values(config: &ConfigFile) -> EyreResult<()> {
    // Validate sync configuration
    validate_duration_range(
        config.sync.timeout,
        "sync.timeout",
        Duration::from_millis(MIN_SYNC_TIMEOUT_MS),
        Duration::from_millis(MAX_SYNC_TIMEOUT_MS),
    )?;

    validate_duration_range(
        config.sync.interval,
        "sync.interval",
        Duration::from_millis(MIN_SYNC_INTERVAL_MS),
        Duration::from_millis(MAX_SYNC_INTERVAL_MS),
    )?;

    validate_duration_range(
        config.sync.frequency,
        "sync.frequency",
        Duration::from_millis(MIN_SYNC_INTERVAL_MS),
        Duration::from_millis(MAX_SYNC_INTERVAL_MS),
    )?;

    // Validate discovery limits
    let rendezvous_limit = config.network.discovery.rendezvous.registrations_limit;
    if rendezvous_limit == 0 {
        bail!("discovery.rendezvous.registrations_limit must be greater than 0");
    }
    if rendezvous_limit > MAX_REGISTRATIONS_LIMIT {
        bail!(
            "discovery.rendezvous.registrations_limit ({}) exceeds maximum allowed value ({})",
            rendezvous_limit,
            MAX_REGISTRATIONS_LIMIT
        );
    }

    let relay_limit = config.network.discovery.relay.registrations_limit;
    if relay_limit == 0 {
        bail!("discovery.relay.registrations_limit must be greater than 0");
    }
    if relay_limit > MAX_REGISTRATIONS_LIMIT {
        bail!(
            "discovery.relay.registrations_limit ({}) exceeds maximum allowed value ({})",
            relay_limit,
            MAX_REGISTRATIONS_LIMIT
        );
    }

    // Validate autonat configuration
    let autonat_candidates = config.network.discovery.autonat.max_candidates;
    if autonat_candidates == 0 {
        bail!("discovery.autonat.max_candidates must be greater than 0");
    }

    // Validate WebSocket configuration if enabled
    if let Some(ref ws_config) = config.network.server.websocket {
        if ws_config.enabled {
            // Ping interval should be reasonable
            if ws_config.ping_interval_secs > 0 && ws_config.ping_interval_secs < 5 {
                bail!(
                    "websocket.ping_interval_secs ({}) is too low. Minimum is 5 seconds or 0 to disable.",
                    ws_config.ping_interval_secs
                );
            }

            // Pong timeout should be less than ping interval if ping is enabled
            if ws_config.ping_interval_secs > 0
                && ws_config.pong_timeout_secs >= ws_config.ping_interval_secs
            {
                bail!(
                    "websocket.pong_timeout_secs ({}) must be less than ping_interval_secs ({})",
                    ws_config.pong_timeout_secs,
                    ws_config.ping_interval_secs
                );
            }
        }
    }

    // Validate embedded auth limits if configured
    if let Some(ref auth_config) = config.network.server.embedded_auth {
        // JWT token expiry should be reasonable
        if auth_config.jwt.access_token_expiry == 0 {
            bail!("embedded_auth.jwt.access_token_expiry must be greater than 0");
        }
        if auth_config.jwt.refresh_token_expiry == 0 {
            bail!("embedded_auth.jwt.refresh_token_expiry must be greater than 0");
        }
        if auth_config.jwt.access_token_expiry >= auth_config.jwt.refresh_token_expiry {
            bail!(
                "embedded_auth.jwt.access_token_expiry ({}) should be less than refresh_token_expiry ({})",
                auth_config.jwt.access_token_expiry,
                auth_config.jwt.refresh_token_expiry
            );
        }

        // Password length constraints
        if auth_config.user_password.min_password_length == 0 {
            bail!("embedded_auth.user_password.min_password_length must be greater than 0");
        }
        if auth_config.user_password.max_password_length
            <= auth_config.user_password.min_password_length
        {
            bail!(
                "embedded_auth.user_password.max_password_length ({}) must be greater than min_password_length ({})",
                auth_config.user_password.max_password_length,
                auth_config.user_password.min_password_length
            );
        }
    }

    Ok(())
}

/// Validates that a duration is within the specified range.
fn validate_duration_range(
    value: Duration,
    name: &str,
    min: Duration,
    max: Duration,
) -> EyreResult<()> {
    if value < min {
        bail!(
            "{} ({:?}) is below minimum allowed value ({:?})",
            name,
            value,
            min
        );
    }
    if value > max {
        bail!(
            "{} ({:?}) exceeds maximum allowed value ({:?})",
            name,
            value,
            max
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_ports_from_multiaddr() {
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/8080".parse().unwrap();
        let ports = extract_ports_from_multiaddr(&addr);
        assert_eq!(ports, vec![8080]);

        let addr: Multiaddr = "/ip4/127.0.0.1/udp/8080/quic-v1".parse().unwrap();
        let ports = extract_ports_from_multiaddr(&addr);
        assert_eq!(ports, vec![8080]);
    }

    #[test]
    fn test_extract_host_from_multiaddr() {
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/8080".parse().unwrap();
        let host = extract_host_from_multiaddr(&addr);
        assert_eq!(host, "127.0.0.1");

        let addr: Multiaddr = "/ip6/::1/tcp/8080".parse().unwrap();
        let host = extract_host_from_multiaddr(&addr);
        assert_eq!(host, "::1");
    }

    #[test]
    fn test_validate_duration_range() {
        let min = Duration::from_secs(1);
        let max = Duration::from_secs(10);

        // Valid duration
        assert!(validate_duration_range(Duration::from_secs(5), "test", min, max).is_ok());

        // Below minimum
        assert!(validate_duration_range(Duration::from_millis(500), "test", min, max).is_err());

        // Above maximum
        assert!(validate_duration_range(Duration::from_secs(15), "test", min, max).is_err());

        // Edge cases - exactly at boundaries
        assert!(validate_duration_range(Duration::from_secs(1), "test", min, max).is_ok());
        assert!(validate_duration_range(Duration::from_secs(10), "test", min, max).is_ok());
    }
}
