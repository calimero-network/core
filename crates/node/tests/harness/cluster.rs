//! The in-process cluster the node's tests and the cluster throughput bench
//! share: `n` nodes on loopback ports, mock attestation, sealed channels, the
//! real kv-store fixture.
//!
//! Needs `kv_store.wasm`: `cargo build -p kv-store --target wasm32-unknown-unknown --profile app-release`,
//! or set `KV_STORE_WASM`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use calimero_client::{Client, Policy};
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::attest::Mode;
use calimero_node::{run, Config, Running};

pub const ORIGIN: &str = "http://localhost:3000";

pub fn kv_wasm() -> Vec<u8> {
    let path = std::env::var("KV_STORE_WASM")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/wasm32-unknown-unknown/app-release/kv_store.wasm")
        });
    std::fs::read(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; build kv-store for wasm32 first", path.display()))
}

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Node `id`'s configuration in a test cluster of `peers`, stored in `dir`.
pub fn node_config(id: u64, peers: &BTreeMap<u64, String>, dir: &std::path::Path) -> Config {
    let url = peers[&id].clone();
    let port = url::Url::parse(&url).unwrap().port().unwrap();
    Config {
        node_id: id,
        public_url: url,
        listen: format!("127.0.0.1:{port}").parse().unwrap(),
        data_dir: dir.to_owned(),
        peers: peers.clone(),
        bootstrap: id == 1,
        cluster: "test".into(),
        storage_key: [42; 32],
        attestation: Mode::Mock,
        allow_mock_peers: true,
        sealed_required: true,
        rp_id: "localhost".into(),
        origins: vec![ORIGIN.into()],
        release: "test".into(),
        ops_token: Some("ops-secret".into()),
        ops_token_sha256: None,
        metrics_listen: None,
        replicas: 3,
        backup_dir: Some(dir.join("backups")),
        raft: calimero_log::Config {
            heartbeat_ms: 100,
            election_min_ms: 500,
            election_max_ms: 1000,
            snapshot_every: 200,
        },
        session_key_rotation_secs: 6 * 3600,
        read_wait: Duration::from_secs(2),
        tee_release: None,
        tee_profile: None,
        peer_measurements: calimero_client::Measurements::None,
        clock_offset_ms: 0,
        budget: None,
        registry: None,
        allow_unsigned_bundles: true,
        allow_package_uploads: true,
        outbound: None,
        governance: Default::default(),
        apps_domain: None,
    }
}

/// Numbers the clusters this process starts (`Cluster::name`).
static NEXT_CLUSTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A cluster name of its own in this process and run (`test-{pid}-{n}`). The
/// peer secret is derived from it, so a node of another test's cluster that a
/// reused port leads to refuses this one's Raft messages instead of applying
/// them. Every test that starts nodes names their cluster with it.
pub fn cluster_name() -> String {
    format!(
        "test-{}-{}",
        std::process::id(),
        NEXT_CLUSTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// `n` nodes started in this process, each in its own temp dir.
pub struct Cluster {
    /// The cluster's name, its own in this process and run: the peer secret
    /// is derived from it, so a node of another test's cluster that a reused
    /// port leads to refuses this one's Raft messages instead of applying them.
    pub name: String,
    pub nodes: Vec<Option<Running>>,
    pub urls: Vec<String>,
    pub _dirs: Vec<tempfile::TempDir>,
}

impl Cluster {
    pub async fn start(n: u64) -> Self {
        Self::start_with(n, |_| {}).await
    }

    pub async fn start_with(n: u64, configure: impl Fn(&mut Config)) -> Self {
        // `free_port` hands the port back before a node binds it, so another
        // test can take it meanwhile: a cluster that loses that race starts
        // again on other ports.
        let mut attempt = 0;
        let (name, nodes, urls, dirs) = loop {
            attempt += 1;
            let ports: Vec<u16> = (0..n).map(|_| free_port()).collect();
            let urls: Vec<String> = ports
                .iter()
                .map(|p| format!("http://127.0.0.1:{p}/"))
                .collect();
            let peers: BTreeMap<u64, String> = urls
                .iter()
                .enumerate()
                .map(|(i, u)| (i as u64 + 1, u.clone()))
                .collect();
            let name = cluster_name();
            let mut nodes = Vec::new();
            let mut dirs = Vec::new();
            let mut failed = None;
            for i in 0..ports.len() {
                let dir = tempfile::tempdir().unwrap();
                let mut config = node_config(i as u64 + 1, &peers, dir.path());
                config.cluster.clone_from(&name);
                configure(&mut config);
                match run(config).await {
                    Ok(r) => nodes.push(Some(r)),
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                }
                dirs.push(dir);
            }
            match failed {
                None => break (name, nodes, urls, dirs),
                Some(e) if attempt < 3 => {
                    eprintln!("cluster start failed ({e:#}); retrying on other ports");
                    for r in nodes.into_iter().flatten() {
                        r.stop().await;
                    }
                }
                Some(e) => panic!("cluster did not start: {e:#}"),
            }
        };
        let c = Self {
            name,
            nodes,
            urls,
            _dirs: dirs,
        };
        // Wait for the system group to elect, register nodes and mint a session key.
        for _ in 0..100 {
            let ready = c.nodes[0]
                .as_ref()
                .unwrap()
                .node
                .sys(|s| s.nodes.len() == n as usize && !s.session_keys.is_empty());
            if ready {
                return c;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("cluster did not form");
    }

    pub fn client(&self, i: usize) -> Client {
        Client::connect(
            &self.urls[i],
            Policy {
                allow_mock: true,
                ..Policy::default()
            },
        )
        .unwrap()
    }

    pub async fn stop(&mut self, i: usize) {
        if let Some(r) = self.nodes[i].take() {
            r.stop().await;
        }
    }

    pub fn app_leader(&self, app: &str) -> Option<usize> {
        self.nodes.iter().enumerate().find_map(|(i, r)| {
            let r = r.as_ref()?;
            r.node
                .hosted(app)
                .filter(|g| g.status().is_leader)
                .map(|_| i)
        })
    }
}

pub async fn signed_up(c: &Cluster, i: usize, name: &str) -> (Client, SoftAuthenticator) {
    let client = c.client(i);
    let mut pk = SoftAuthenticator::new("localhost", ORIGIN);
    let _s = client.sign_up(&mut pk, name).await.unwrap();
    (client, pk)
}
