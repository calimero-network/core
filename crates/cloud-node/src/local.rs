//! A development cluster in one process (`merod-v2 local`): N nodes on
//! consecutive ports, mock attestation, a dev storage key. What merobox was
//! for v1, without containers: state persists in `dir` across restarts.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use crate::attest::Mode;
use crate::{run, Config, Running};

/// The cluster's name, which also seeds its dev storage key.
pub const CLUSTER: &str = "local";

/// A running local cluster.
pub struct LocalCluster {
    pub nodes: Vec<Running>,
    pub urls: Vec<String>,
    pub ops_token: String,
}

impl LocalCluster {
    /// Stop every node.
    pub async fn stop(self) {
        for n in self.nodes {
            n.stop().await;
        }
    }
}

/// Start `n` nodes on `127.0.0.1:base_port+1..=base_port+n`, keeping state under `dir`.
pub async fn start(
    n: u64,
    base_port: u16,
    dir: &Path,
    ops_token: &str,
    origins: Vec<String>,
    registry: Option<String>,
) -> eyre::Result<LocalCluster> {
    let ports: Vec<u16> = (1..=n)
        .map(|i| base_port + u16::try_from(i).unwrap_or(u16::MAX))
        .collect();
    let urls: Vec<String> = ports
        .iter()
        .map(|p| format!("http://127.0.0.1:{p}/"))
        .collect();
    let peers: BTreeMap<u64, String> = (1..=n).zip(urls.iter().cloned()).collect();
    let mut nodes = Vec::new();
    for (i, port) in (1..=n).zip(&ports) {
        let data_dir = dir.join(format!("node{i}"));
        std::fs::create_dir_all(&data_dir)?;
        let config = Config {
            node_id: i,
            public_url: urls[usize::try_from(i - 1)?].clone(),
            listen: format!("127.0.0.1:{port}").parse()?,
            data_dir,
            peers: peers.clone(),
            bootstrap: i == 1,
            cluster: CLUSTER.into(),
            storage_key: calimero_identity::sha256(
                format!("calimero-dev-storage-key|{CLUSTER}").as_bytes(),
            ),
            attestation: Mode::Mock,
            allow_mock_peers: true,
            sealed_required: false,
            rp_id: "localhost".into(),
            origins: origins.clone(),
            release: env!("CARGO_PKG_VERSION").into(),
            ops_token: Some(ops_token.to_owned()),
            replicas: usize::try_from(n.min(3))?,
            backup_dir: Some(dir.join("backups")),
            raft: calimero_log::Config::default(),
            session_key_rotation_secs: 6 * 3600,
            tee_release: None,
            tee_profile: None,
            peer_measurements: calimero_cloud_client::Measurements::None,
            registry: registry.clone(),
            allow_unsigned_bundles: true,
            outbound: None,
            upgrade_policy: crate::system::UpgradePolicy::Owner,
        };
        nodes.push(run(config).await?);
    }
    // Ready once the system group has every node and a session key.
    for _ in 0..200 {
        let ready = nodes[0]
            .node
            .sys(|s| s.nodes.len() == nodes.len() && !s.session_keys.is_empty());
        if ready {
            return Ok(LocalCluster {
                nodes,
                urls,
                ops_token: ops_token.to_owned(),
            });
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    eyre::bail!("the local cluster did not form within 20 s")
}
