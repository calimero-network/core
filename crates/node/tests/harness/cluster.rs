//! The in-process cluster the node's tests and the cluster throughput bench
//! share: `n` nodes on loopback ports, mock attestation, sealed channels, the
//! kv fixture (`apps/v2-fixtures/kv`, a `calimero:app` component).

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use calimero_client::{Client, Policy};
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::attest::Mode;
use calimero_node::{run, Config, Running};

pub const ORIGIN: &str = "http://localhost:3000";

/// The kv fixture (`apps/v2-fixtures/kv`: `set`, `get`, `entries`, `remove`, ...) as a
/// component, built on first use and cached by source hash under
/// `target/app-components`, as `tests/common/mod.rs` builds its fixtures.
pub fn kv_wasm() -> Vec<u8> {
    fixture("apps/v2-fixtures/kv")
}

/// The component at `dir` (relative to the repository root), built and cached.
pub fn fixture(dir: &str) -> Vec<u8> {
    // Tests run in parallel threads of one process; two builds of the same crate into
    // one target directory race, so builds are serialized.
    static BUILDING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one_at_a_time = BUILDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let sdk = root.join("crates/sdk");
    let macros = root.join("crates/sdk-macros");
    let alias = root.join("crates/app-sdk");
    let wit = root.join("crates/app-wit");
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| root.join("target"), std::path::PathBuf::from);
    calimero_app_build::build_component_cached(
        &root.join(dir),
        &[&sdk, &macros, &alias, &wit],
        &target.join("app-components"),
    )
    .unwrap_or_else(|e| panic!("build {dir}: {e}"))
}

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
    body: serde_json::Value,
) -> Result<serde_json::Value, calimero_client::Error> {
    let req = calimero_client::Request::new(reqwest::Method::POST, path).json(body);
    client
        .transport()
        .approved(req, None, Some(&operator_sign))
        .await
}

/// The operator key's signature over `message`.
pub fn operator_sign(message: &[u8]) -> Vec<u8> {
    use ed25519_dalek::Signer;
    ed25519_dalek::SigningKey::from_bytes(&OPERATOR_SEED)
        .sign(message)
        .to_bytes()
        .to_vec()
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
        restore_from: None,
        raft: calimero_log::Config {
            heartbeat_ms: 100,
            election_min_ms: 500,
            election_max_ms: 1000,
            snapshot_every: 200,
        },
        read_wait: Duration::from_secs(2),
        // Every refresh exchanges its token in the log, as the tests of
        // rotation and reuse expect; `tests/refresh.rs` runs the production
        // `api::REFRESH_ROTATE_AFTER`.
        refresh_rotate_after: Duration::ZERO,
        tee_release: None,
        tee_profile: None,
        peer_measurements: calimero_client::Measurements::None,
        clock_offset_ms: 0,
        kms_keys: Default::default(),
        rollback: None,
        accept_rollback: false,
        budget: None,
        registry: None,
        allow_unsigned_bundles: true,
        allow_package_uploads: true,
        outbound: None,
        governance: Default::default(),
        apps_domain: None,
        operator_key: Some(operator_public()),
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
        let seen: Vec<String> = c
            .nodes
            .iter()
            .flatten()
            .map(|r| r.node.sys(|s| format!("nodes {}", s.nodes.len())))
            .collect();
        panic!("cluster did not form: {seen:?}");
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

/// A generated route's typed answer as the JSON the node sent, for tests that
/// assert on the wire shape: the answer's types keep every field the node sent
/// (named, or in `extra`), so nothing is lost on the way back.
pub fn wire<T: serde::Serialize>(answer: T) -> serde_json::Value {
    // SAFETY: the generated answer types serialize to JSON objects; nothing
    // in them (maps keyed by strings, plain values) can fail to serialize.
    serde_json::to_value(answer).expect("a generated answer is JSON")
}

/// A generated route's typed request, written as the JSON a test sends. It
/// must say all of it: a key the type would drop changes the request, so it
/// panics rather than send less than the test wrote.
pub fn typed<T: serde::Serialize + serde::de::DeserializeOwned>(body: serde_json::Value) -> T {
    let typed: T = serde_json::from_value(body.clone())
        .unwrap_or_else(|e| panic!("not the route's request type: {e}: {body}"));
    let sent = serde_json::to_value(&typed).ok();
    assert_eq!(
        sent.as_ref(),
        Some(&body),
        "the request type drops part of {body}"
    );
    typed
}
