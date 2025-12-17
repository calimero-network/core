/// Docker host detection utilities
///
/// Provides functions to detect Docker environment and resolve host URLs
/// for services running on the host machine from within Docker containers.

/// Check if a hostname can be resolved at runtime
pub fn can_resolve_host(host: &str) -> bool {
    use std::net::ToSocketAddrs;
    format!("{}:80", host).to_socket_addrs().is_ok()
}

/// Get the Docker host URL for a given port
///
/// When running inside Docker:
/// - Uses `host.docker.internal` (works on Mac/Windows Docker Desktop)
/// - Falls back to `172.17.0.1` on Linux (default Docker bridge gateway)
///
/// When not in Docker:
/// - Returns `http://127.0.0.1:{port}`
pub fn get_docker_host_for_port(port: u16) -> String {
    // Check if we're in Docker
    if !std::path::Path::new("/.dockerenv").exists() {
        return format!("http://127.0.0.1:{}", port);
    }

    // Try to resolve host.docker.internal at runtime (works on Mac/Windows Docker Desktop)
    // If it doesn't resolve, fall back to default Docker bridge gateway (Linux native Docker)
    if can_resolve_host("host.docker.internal") {
        format!("http://host.docker.internal:{}", port)
    } else {
        format!("http://172.17.0.1:{}", port)
    }
}
