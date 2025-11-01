//! Runtime mode tests
//!
//! Tests RuntimeMode enum and configuration.

use calimero_node::RuntimeMode;

#[test]
fn test_runtime_mode_default() {
    // Server mode should be the default
    assert_eq!(RuntimeMode::default(), RuntimeMode::Server);
}

#[test]
fn test_runtime_mode_variants() {
    // Verify both variants exist and can be compared
    let server = RuntimeMode::Server;
    let desktop = RuntimeMode::Desktop;

    assert_ne!(server, desktop);
    assert_eq!(server, RuntimeMode::default());
}

#[test]
fn test_runtime_mode_debug() {
    // Verify Debug formatting works
    let server = RuntimeMode::Server;
    let desktop = RuntimeMode::Desktop;

    let server_str = format!("{:?}", server);
    let desktop_str = format!("{:?}", desktop);

    assert!(server_str.contains("Server"));
    assert!(desktop_str.contains("Desktop"));
}

#[test]
fn test_runtime_mode_clone_copy() {
    // Verify Copy and Clone traits work
    let mode = RuntimeMode::Server;
    let mode2 = mode; // Copy
    let mode3 = mode.clone(); // Clone

    assert_eq!(mode, mode2);
    assert_eq!(mode, mode3);
}
