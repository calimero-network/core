//! Integration tests: `merod init` writes expected context client signer fields.

use std::process::Command;

use calimero_context::config::GroupGovernanceMode;
use camino::Utf8PathBuf;

#[tokio::test]
async fn local_init_omits_relayer_signer_in_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path();
    let status = Command::new(env!("CARGO_BIN_EXE_merod"))
        .args([
            "--home",
            home.to_str().expect("utf8 temp path"),
            "--node",
            "node1",
            "init",
            "--group-governance",
            "local",
        ])
        .status()
        .expect("spawn merod");

    assert!(status.success(), "merod init failed: {status:?}");

    let node_dir =
        Utf8PathBuf::from_path_buf(home.join("node1")).expect("node path is valid UTF-8");
    let cfg = calimero_config::ConfigFile::load(&node_dir)
        .await
        .expect("load config");

    assert_eq!(cfg.context.group_governance, GroupGovernanceMode::Local);
    assert!(
        cfg.context.client.signer.relayer.is_none(),
        "local governance init must omit relayer signer"
    );
}

#[tokio::test]
async fn external_init_includes_relayer_signer_in_config() {
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

    assert_eq!(cfg.context.group_governance, GroupGovernanceMode::External);
    assert!(
        cfg.context.client.signer.relayer.is_some(),
        "external governance init must include relayer signer"
    );
}
