//! Runtime mode tests
//!
//! Tests RuntimeMode enum and configuration.

use std::collections::BTreeMap;

use calimero_blobstore::config::BlobStoreConfig;
use calimero_context::config::ContextConfig;
use calimero_context_config::client::config::{ClientConfig, ClientRelayerSigner, ClientSigner};
use calimero_network_primitives::config::{
    AutonatConfig, BootstrapConfig, BootstrapNodes, DiscoveryConfig, NetworkConfig, RelayConfig,
    RendezvousConfig, SwarmConfig,
};
use calimero_node::sync::SyncConfig;
use calimero_node::{NodeConfig, RuntimeMode};
use calimero_server::config::ServerConfig;
use calimero_store::config::StoreConfig;
use camino::Utf8PathBuf;
use libp2p::identity::Keypair;
use libp2p::multiaddr::Multiaddr;

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

#[test]
#[should_panic(expected = "Desktop build must not bind HTTP listeners")]
fn test_desktop_mode_rejects_http_listeners() {
    // This test verifies the defensive guard catches misconfiguration
    let identity = Keypair::generate_ed25519();
    let temp_dir = std::env::temp_dir();
    let test_path = temp_dir.join(format!("calimero-test-{}", rand::random::<u64>()));
    let test_path = Utf8PathBuf::from_path_buf(test_path).unwrap();

    // Create config with HTTP listener (should panic in Desktop mode)
    let http_listener: Multiaddr = "/ip4/127.0.0.1/tcp/2528".parse().unwrap();

    let cfg = NodeConfig {
        home: test_path.clone(),
        identity: identity.clone(),
        network: NetworkConfig::new(
            identity.clone(),
            SwarmConfig::new(vec![]),
            BootstrapConfig::new(BootstrapNodes::new(vec![])),
            DiscoveryConfig::new(
                false,
                false,
                RendezvousConfig::new(0),
                RelayConfig::new(0),
                AutonatConfig::new(0),
            ),
        ),
        sync: SyncConfig::default(),
        datastore: StoreConfig::new(test_path.join("data")),
        blobstore: BlobStoreConfig::new(test_path.join("blobs")),
        context: ContextConfig {
            client: ClientConfig {
                signer: ClientSigner {
                    relayer: ClientRelayerSigner {
                        url: "http://localhost:1234".parse().unwrap(),
                    },
                    local: calimero_context_config::client::config::LocalConfig {
                        protocols: BTreeMap::new(),
                    },
                },
                params: BTreeMap::new(),
            },
        },
        server: ServerConfig::new(
            vec![http_listener], // ← Non-empty listeners
            identity.clone(),
            None,
            None,
            None,
            None,
        ),
        gc_interval_secs: Some(3600),
        runtime_mode: RuntimeMode::Desktop, // ← Desktop mode with HTTP listener = panic
    };

    // This should panic due to the defensive guard
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let _ = calimero_node::start(cfg).await;
    });
}
