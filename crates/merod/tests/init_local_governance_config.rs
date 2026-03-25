//! Integration tests: `merod init` writes local-only context client config.

use std::process::Command;

use camino::Utf8PathBuf;

#[tokio::test]
async fn init_writes_local_only_context_client_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path();
    let status = Command::new(env!("CARGO_BIN_EXE_merod"))
        .args([
            "--home",
            home.to_str().expect("utf8 temp path"),
            "--node",
            "node1",
            "init",
        ])
        .status()
        .expect("spawn merod");

    assert!(status.success(), "merod init failed: {status:?}");

    let node_dir =
        Utf8PathBuf::from_path_buf(home.join("node1")).expect("node path is valid UTF-8");
    let cfg = calimero_config::ConfigFile::load(&node_dir)
        .await
        .expect("load config");

    assert!(
        cfg.context.client.signer.local.protocols.is_empty(),
        "default init must start with empty protocol map under [context.config.signer.self]"
    );
}
