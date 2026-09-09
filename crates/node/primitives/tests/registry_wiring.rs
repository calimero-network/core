//! `[registry]` reaches the fetch path only through the builder call node
//! startup makes; dropping it reverts the node to P2P with nothing failing.

use calimero_app_downloader::registry::RegistryConfig;

mod common;

#[tokio::test]
async fn the_configured_registry_reaches_the_client() {
    let (node_client, _data, _blobs) = common::create_test_node_client(None).await;
    assert!(
        node_client.registry_config().base_url.is_none(),
        "resolution stays off until an operator configures it"
    );

    // The body of a real `[registry]` section, so this pins the whole seam:
    // what config.toml deserializes to is what the fetch path reads back.
    let configured: RegistryConfig =
        toml::from_str(r#"base_url = "https://apps.calimero.network/""#)
            .expect("registry section must deserialize");
    let wired = node_client.with_registry(configured);
    assert_eq!(
        wired.registry_config().base_url.map(String::from),
        Some("https://apps.calimero.network/".to_owned()),
        "the operator's base must be what the fetch path reads back"
    );
}
