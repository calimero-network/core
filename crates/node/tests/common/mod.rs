//! Test support shared by the test binaries: the v2 app components (`apps/v2-fixtures/*`
//! and `apps/rooms`) built into components, and the in-process three-node cluster.

// Each test binary includes this module and uses a different subset of it.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use calimero_app_build::build_component_cached;
use calimero_client::{Client, Policy};
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::attest::Mode;
use calimero_node::{run, run_job, Config, JobConfig, Running, RunningJob};

/// The fixture `name` ("kv" or "org-gated") as a component, built on first use and cached
/// by source hash under `target/app-components` (the fixture, the SDK crates, the toolchain and
/// app-build's post-processing are hashed; see `calimero_app_build::cache_key`).
pub fn component(name: &str) -> Vec<u8> {
    build(&Path::new("apps/v2-fixtures").join(name))
}

/// The Rooms reference app (`apps/rooms`) as a component, cached like [`component`].
pub fn rooms() -> Vec<u8> {
    build(Path::new("apps/rooms"))
}

/// Builds the guest crate at `dir` (relative to the repository root), cached by source hash.
fn build(dir: &Path) -> Vec<u8> {
    // Tests run in parallel threads of one process; two builds of the same crate into
    // one target directory race, so builds are serialized.
    static BUILDING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one_at_a_time = BUILDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let sdk = root.join("crates/app-sdk");
    let macros = root.join("crates/app-sdk-macros");
    let wit = root.join("crates/app-wit");
    let cache: PathBuf = root.join("target/app-components");
    build_component_cached(&root.join(dir), &[&sdk, &macros, &wit], &cache)
        .unwrap_or_else(|e| panic!("build {}: {e}", dir.display()))
}

pub const ORIGIN: &str = "http://localhost:3000";

/// The operator key test clusters start with (`Config::operator_key`).
pub const OPERATOR_SEED: [u8; 32] = [9; 32];

/// [`OPERATOR_SEED`]'s public key, hex, as `--operator-key` takes it.
pub fn operator_public() -> String {
    hex::encode(
        ed25519_dalek::SigningKey::from_bytes(&OPERATOR_SEED)
            .verifying_key()
            .to_bytes(),
    )
}

/// POST an operator decision as mdma does: sent as is, and once the node
/// names what to sign (`428 OPERATOR_SIGNATURE_REQUIRED`), again with the
/// operator key's signature over it.
#[allow(
    clippy::result_large_err,
    reason = "the client's own error type, as its `post` returns it"
)]
pub async fn ops_signed(
    client: &Client,
    path: &str,
    mut body: serde_json::Value,
) -> Result<serde_json::Value, calimero_client::Error> {
    use ed25519_dalek::Signer;
    match client.post(path, body.clone()).await {
        Err(e) if e.code() == Some("OPERATOR_SIGNATURE_REQUIRED") => {
            let d = e.details().cloned().unwrap_or_default();
            let message = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                d["message"].as_str().unwrap_or_default(),
            )
            .unwrap_or_default();
            let signature = ed25519_dalek::SigningKey::from_bytes(&OPERATOR_SEED).sign(&message);
            body["operatorProof"] = serde_json::json!({
                "nonce": d["nonce"],
                "signature": hex::encode(signature.to_bytes()),
            });
            client.post(path, body).await
        }
        other => other,
    }
}

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

pub struct Cluster {
    pub nodes: Vec<Option<Running>>,
    pub urls: Vec<String>,
    pub configs: Vec<Config>,
    _dirs: Vec<tempfile::TempDir>,
}

/// Numbers the clusters this process starts: each gets a name of its own, as
/// `tests/harness/cluster.rs` does, so its peer secret is its own and a node of
/// another test's cluster that a reused port leads to refuses its Raft messages.
static NEXT_CLUSTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Cluster {
    pub async fn start(n: u64) -> Self {
        Self::start_with(n, |_| {}).await
    }

    pub async fn start_with(n: u64, configure: impl Fn(&mut Config)) -> Self {
        // `free_port` hands the port back before a node binds it, so another test
        // can take it meanwhile: a cluster that loses that race starts again on
        // other ports, as `tests/harness/cluster.rs` does.
        let mut attempt = 0;
        let (nodes, urls, configs, dirs) = loop {
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
            let mut nodes = Vec::new();
            let mut dirs = Vec::new();
            let mut configs = Vec::new();
            let name = format!(
                "test-{}-{}",
                std::process::id(),
                NEXT_CLUSTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            );
            let mut failed = None;
            for (i, port) in ports.iter().enumerate() {
                let dir = tempfile::tempdir().unwrap();
                let mut config = Config {
                    node_id: i as u64 + 1,
                    public_url: urls[i].clone(),
                    listen: format!("127.0.0.1:{port}").parse().unwrap(),
                    data_dir: dir.path().to_owned(),
                    peers: peers.clone(),
                    bootstrap: i == 0,
                    cluster: name.clone(),
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
                    backup_dir: Some(dir.path().join("backups")),
                    raft: calimero_log::Config {
                        heartbeat_ms: 100,
                        election_min_ms: 500,
                        election_max_ms: 1000,
                        snapshot_every: 200,
                    },
                    read_wait: Duration::from_secs(2),
                    tee_release: None,
                    tee_profile: None,
                    peer_measurements: calimero_client::Measurements::None,
                    clock_offset_ms: 0,
                    kms_keys: Default::default(),
                    budget: None,
                    registry: None,
                    // The component fixtures are raw uploads (`POST /bundles`), development only.
                    allow_unsigned_bundles: true,
                    allow_package_uploads: true,
                    outbound: None,
                    governance: Default::default(),
                    apps_domain: None,
                    operator_key: Some(operator_public()),
                };
                configure(&mut config);
                configs.push(config.clone());
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
                None => break (nodes, urls, configs, dirs),
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
            nodes,
            urls,
            configs,
            _dirs: dirs,
        };
        // Wait for the system group to elect and every node to know them all.
        for _ in 0..300 {
            let ready = c
                .nodes
                .iter()
                .flatten()
                .all(|r| r.node.sys(|s| s.nodes.len() == n as usize));
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

    /// Start node `i` again from its data directory, its configuration changed by `configure`.
    pub async fn restart(&mut self, i: usize, configure: impl Fn(&mut Config)) {
        self.stop(i).await;
        let mut config = self.configs[i].clone();
        config.bootstrap = false;
        configure(&mut config);
        // The stopped node's tasks may hold its log open for a moment.
        for _ in 0..50 {
            match run(config.clone()).await {
                Ok(r) => {
                    self.nodes[i] = Some(r);
                    return;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
        panic!("node {i} did not restart");
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

impl Cluster {
    /// Moves the app clock of every running node by `ms`.
    pub fn advance_all(&self, ms: i64) {
        for r in self.nodes.iter().flatten() {
            r.node.advance_clock(ms);
        }
    }

    /// The applied index of `app` on node `i`.
    pub fn applied_on(&self, i: usize, app: &str) -> u64 {
        self.nodes[i]
            .as_ref()
            .unwrap()
            .node
            .hosted(app)
            .unwrap()
            .applied()
    }
}

/// A job node started next to a cluster ([`Cluster::start_job_node`]).
pub struct JobNode {
    pub running: RunningJob,
    pub id: u64,
    pub url: String,
    _dir: tempfile::TempDir,
}

impl Cluster {
    /// Starts job node `id` with the cluster's settings, `configure` applied
    /// last. It is not in the directory until [`Cluster::register_job_node`].
    pub async fn start_job_node(&self, id: u64, configure: impl Fn(&mut JobConfig)) -> JobNode {
        let dir = tempfile::tempdir().unwrap();
        let from = &self.configs[0];
        let mut config = JobConfig {
            node_id: id,
            listen: format!("127.0.0.1:{}", free_port()).parse().unwrap(),
            data_dir: dir.path().to_owned(),
            cluster: from.cluster.clone(),
            attestation: from.attestation,
            sealed_required: from.sealed_required,
            release: from.release.clone(),
            tee_release: from.tee_release.clone(),
            tee_profile: from.tee_profile.clone(),
            cluster_identity: self.nodes[0].as_ref().unwrap().node.cluster_identity(),
        };
        configure(&mut config);
        let url = format!("http://127.0.0.1:{}/", config.listen.port());
        JobNode {
            running: run_job(config).await.unwrap(),
            id,
            url,
            _dir: dir,
        }
    }

    /// Registers `addr` as job node `id` through `/ops/nodes` and waits until
    /// the first node's directory shows it.
    pub async fn register_job_node(&self, id: u64, addr: &str) {
        let ops = self.client(0);
        ops.use_api_key("ops-secret").await;
        let _ = ops
            .post(
                "ops/nodes",
                serde_json::json!({ "id": id, "addr": addr, "role": "job" }),
            )
            .await
            .unwrap();
        let node = &self.nodes[0].as_ref().unwrap().node;
        for _ in 0..100 {
            if node.sys(|s| s.job_nodes().any(|n| n.id == id)) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("node 1 never applied job node {id}");
    }
}
