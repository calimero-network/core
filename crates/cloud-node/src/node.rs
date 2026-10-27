//! The node: hosts the system group and its app groups, routes calls to each
//! group's leader, and talks to peers over attested sealed channels.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Duration;

use calimero_log::{
    BoxFuture, Engine, Group, GroupId, NodeId, Proposal, RpcRequest, RpcResponse, Transport,
    WriteError,
};
use ring::hmac;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{debug, info, warn};
use url::Url;

use crate::app::{
    AppCmd, AppMachine, Applied, CallEntry, Executor, FollowUp, Outcome, RunEnv, Status, Trigger,
};
use crate::attest::{HttpAttestor, Mode, Policy};
use crate::error::ApiError;
use crate::services::{AppBlobs, BundleFacts, Search};
use crate::system::{output, SystemCmd, SystemMachine, SystemState};
use crate::types::{now_ms, AppRecord, Caller, NodeRecord};

/// The system group's id.
pub const SYSTEM_GROUP: GroupId = 1;

/// Node configuration.
#[derive(Clone, Debug)]
pub struct Config {
    pub node_id: NodeId,
    /// This node's base URL as peers and clients reach it, e.g. `http://127.0.0.1:2501/`.
    pub public_url: String,
    pub listen: std::net::SocketAddr,
    pub data_dir: PathBuf,
    /// Every node of the cluster, this one included: id -> base URL.
    pub peers: BTreeMap<NodeId, String>,
    /// Form the system group with `peers` as voters (first start only).
    pub bootstrap: bool,
    pub cluster: String,
    /// Storage key K_r from the KMS (or a dev key).
    pub storage_key: [u8; 32],
    pub attestation: Mode,
    /// Accept mock quotes from peers (development only).
    pub allow_mock_peers: bool,
    /// Refuse requests that are not sealed (except attestation and health).
    pub sealed_required: bool,
    pub rp_id: String,
    pub origins: Vec<String>,
    pub release: String,
    /// Bearer token for the Ops API (mdma's operator credential).
    pub ops_token: Option<String>,
    /// Replicas per app.
    pub replicas: usize,
    /// Where encrypted snapshots go (object storage stand-in), if anywhere.
    pub backup_dir: Option<PathBuf>,
    pub raft: calimero_log::Config,
    /// How old the session-signing key K_s gets before the leader adds a new one.
    /// The previous key keeps verifying, so it must outlive a session (15 min).
    pub session_key_rotation_secs: u64,
    /// The mero-tee release and image profile this node runs (TDX only); its
    /// attestation names them so clients can fetch the signed measurements.
    pub tee_release: Option<String>,
    pub tee_profile: Option<String>,
    /// Which TD images a peer's real quote may come from.
    pub peer_measurements: calimero_cloud_client::Measurements,
    /// The package registry every node fetches releases from (`--registry-url`).
    pub registry: Option<String>,
    /// Take raw WASM uploads (`POST /bundles`), which carry no publisher
    /// signature. Development only; production installs signed packages.
    pub allow_unsigned_bundles: bool,
}

/// Request to run a method.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallRequest {
    pub app_id: String,
    pub method: String,
    #[serde(default)]
    pub args: Value,
    pub caller: Caller,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub consent: Option<Value>,
    /// Raw input instead of `args` (an event handler's event, an xcall's params).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<calimero_log::Bytes>,
    /// Why the node runs it itself; `None` for a request. Peers pass it along
    /// on `internal/call`, which only peers reach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<Trigger>,
}

impl CallRequest {
    /// A request a client made.
    #[must_use]
    pub fn new(app_id: String, method: String, args: Value, caller: Caller) -> Self {
        Self {
            app_id,
            method,
            args,
            caller,
            idempotency_key: None,
            consent: None,
            input: None,
            trigger: None,
        }
    }

    fn input_bytes(&self) -> Vec<u8> {
        match &self.input {
            Some(b) => b.0.clone(),
            None if self.args.is_null() => Vec::new(),
            None => serde_json::to_vec(&self.args).unwrap_or_default(),
        }
    }
}

/// Request to run a read-only method.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewRequest {
    pub app_id: String,
    pub method: String,
    #[serde(default)]
    pub args: Value,
    pub caller: Caller,
    #[serde(default)]
    pub strong: bool,
    #[serde(default)]
    pub min_index: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewAnswer {
    pub index: u64,
    pub status: Status,
    pub result: Value,
    pub error: Value,
}

/// Shared node state.
pub struct Node {
    pub config: Config,
    pub engine: Arc<Engine>,
    pub system: Arc<Group<SystemMachine>>,
    apps: RwLock<HashMap<GroupId, Arc<Group<AppMachine>>>>,
    pub executor: Executor,
    pub transport: Arc<calimero_sealed::SealedTransport>,
    peers: Mutex<HashMap<String, Arc<calimero_sealed::client::SealedTransport>>>,
    http: reqwest::Client,
    cluster_secret: String,
    pub challenges: Mutex<HashMap<String, crate::api::Pending>>,
    /// Event streams this node is serving (`/apps/{app}/events`).
    pub event_streams: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// Serializes starting app groups (reconcile and the RPC factory race otherwise).
    starting: tokio::sync::Mutex<()>,
    /// Full-text search over the apps hosted here.
    pub search: Search,
    /// ABI facts per bundle hash.
    facts: Mutex<HashMap<String, Arc<BundleFacts>>>,
    /// Wakes the follow-up driver: an entry may have queued work.
    pub kick: tokio::sync::Notify,
    /// This node, for what a run is handed (its blob host).
    this: Mutex<Weak<Node>>,
}

/// Raft RPCs to peers, over their sealed channels.
struct PeerTransport {
    node: Mutex<Weak<Node>>,
}

impl Transport for PeerTransport {
    fn send(
        &self,
        _target: NodeId,
        addr: String,
        group: GroupId,
        request: RpcRequest,
    ) -> BoxFuture<Result<RpcResponse, String>> {
        let node = self.node.lock().ok().and_then(|n| n.upgrade());
        Box::pin(async move {
            let node = node.ok_or("node is shutting down")?;
            let body = json!({ "group": group, "rpc": request });
            let resp = node
                .peer_post(&addr, "internal/raft", &body)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::from_value(resp).map_err(|e| e.to_string())
        })
    }
}

/// The error of a call a read-only member made that would have written.
pub const READ_ONLY: &str = "read-only in this team: the call would write";

/// How long a forwarded call or read waits for the leader before retrying.
pub const FORWARD_TIMEOUT: Duration = Duration::from_secs(5);

/// A command the system log refused at apply.
pub fn rejected(reason: &str) -> ApiError {
    if reason.starts_with(crate::system::QUOTA_PREFIX) {
        ApiError::new(429, "QUOTA_EXCEEDED", reason)
    } else {
        ApiError::bad_request("REJECTED", reason)
    }
}

fn status_of(e: &WriteError) -> ApiError {
    match e {
        WriteError::NotLeader { .. } => ApiError::unavailable("no leader yet; retry"),
        WriteError::Unavailable(m) => ApiError::unavailable(m),
        WriteError::TooLarge { .. } => ApiError::new(413, "TOO_LARGE", &e.to_string()),
    }
}

impl Node {
    /// Open storage, start the system group and every app group this node hosts.
    pub async fn start(config: Config) -> eyre::Result<Arc<Self>> {
        std::fs::create_dir_all(&config.data_dir)?;
        let peer_transport = Arc::new(PeerTransport {
            node: Mutex::new(Weak::new()),
        });
        let engine = Engine::open(
            config.node_id,
            &config.data_dir,
            &config.storage_key,
            Arc::clone(&peer_transport) as Arc<dyn Transport>,
            config.raft.clone(),
        )?;
        let system = engine
            .start_group(SYSTEM_GROUP, SystemMachine::default())
            .await?;
        let sealed = Arc::new(calimero_sealed::SealedTransport::generate(
            &calimero_sealed::SealedOptions {
                required: config.sealed_required,
                unsealed_paths: vec!["/attestation".into(), "/health".into()],
            },
        ));
        let secret = hmac::sign(
            &hmac::Key::new(hmac::HMAC_SHA256, &config.storage_key),
            format!("calimero.cluster.v1|{}", config.cluster).as_bytes(),
        );
        let node = Arc::new(Self {
            cluster_secret: calimero_identity::b64url(secret.as_ref()),
            config,
            engine: Arc::clone(&engine),
            system,
            apps: RwLock::new(HashMap::new()),
            executor: Executor::default(),
            transport: sealed,
            peers: Mutex::new(HashMap::new()),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()?,
            challenges: Mutex::new(HashMap::new()),
            event_streams: std::sync::Arc::default(),
            starting: tokio::sync::Mutex::new(()),
            search: Search::default(),
            facts: Mutex::new(HashMap::new()),
            kick: tokio::sync::Notify::new(),
            this: Mutex::new(Weak::new()),
        });
        *peer_transport.node.lock().map_err(|e| eyre::eyre!("{e}"))? = Arc::downgrade(&node);
        *node.this.lock().map_err(|e| eyre::eyre!("{e}"))? = Arc::downgrade(&node);

        let weak = Arc::downgrade(&node);
        engine.set_group_factory(move |gid| {
            let weak = weak.clone();
            Box::pin(async move {
                match weak.upgrade() {
                    Some(n) => n.start_app_group_if_replica(gid).await,
                    None => false,
                }
            })
        });

        if node.config.bootstrap {
            node.system.initialize(node.config.peers.clone()).await?;
        }
        Ok(node)
    }

    /// The secret peers present on internal routes (inside sealed channels).
    #[must_use]
    pub fn cluster_secret(&self) -> &str {
        &self.cluster_secret
    }

    fn peer(&self, addr: &str) -> eyre::Result<Arc<calimero_sealed::client::SealedTransport>> {
        let mut peers = self.peers.lock().map_err(|e| eyre::eyre!("{e}"))?;
        if let Some(p) = peers.get(addr) {
            return Ok(Arc::clone(p));
        }
        let url: Url = addr.parse()?;
        let attestor = HttpAttestor {
            policy: Policy {
                allow_mock: self.config.allow_mock_peers,
                app_hash: None,
                measurements: self.config.peer_measurements.clone(),
                allowed_tcb_statuses: Vec::new(),
                // A node trusts debug peers only when it runs a debug image itself.
                allow_debug_profiles: self
                    .config
                    .tee_profile
                    .as_deref()
                    .is_some_and(calimero_cloud_client::attest::is_debug_profile),
            },
        };
        let t = Arc::new(calimero_sealed::client::SealedTransport::attested(
            url,
            self.http.clone(),
            Arc::new(attestor),
        ));
        let _prev = peers.insert(addr.to_owned(), Arc::clone(&t));
        Ok(t)
    }

    /// POST JSON to a peer's internal route over its sealed channel.
    pub async fn peer_post(&self, addr: &str, path: &str, body: &Value) -> eyre::Result<Value> {
        let peer = self.peer(addr)?;
        let url = calimero_sealed::client::resolve_path(&addr.parse()?, path)?;
        let req = self
            .http
            .post(url)
            .header("x-cluster-auth", &self.cluster_secret)
            .json(body)
            .build()?;
        let resp = peer.execute(req).await?;
        let status = resp.status();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let err: ApiError = serde_json::from_value(v.clone())
                .unwrap_or_else(|_| ApiError::unavailable(&format!("peer answered {status}")));
            return Err(err.into());
        }
        Ok(v)
    }

    /// [`Self::peer_post`] for a request a peer should answer quickly: a call or
    /// read forwarded to an app's leader. A leader that stopped answering (frozen,
    /// partitioned) is replaced by an election in a few seconds, so the caller
    /// gives up after [`FORWARD_TIMEOUT`] and retries at the new leader instead of
    /// waiting out the HTTP timeout. Forwarded calls carry an idempotency key, so
    /// the retry replays one that did commit.
    pub async fn peer_forward(&self, addr: &str, path: &str, body: &Value) -> eyre::Result<Value> {
        tokio::time::timeout(FORWARD_TIMEOUT, self.peer_post(addr, path, body))
            .await
            .map_err(|_| eyre::eyre!("{addr} did not answer {path} within {FORWARD_TIMEOUT:?}"))?
    }

    /// Open a streaming GET to a peer (for events).
    pub async fn peer_stream(
        &self,
        addr: &str,
        path_and_query: &str,
    ) -> eyre::Result<reqwest::Response> {
        let peer = self.peer(addr)?;
        let base: Url = addr.parse()?;
        let url = base.join(path_and_query)?;
        let req = self
            .http
            .get(url)
            .header("x-cluster-auth", &self.cluster_secret)
            .build()?;
        peer.execute(req).await
    }

    /// Read the system state as applied here.
    pub fn sys<R>(&self, f: impl FnOnce(&SystemState) -> R) -> R {
        self.system.read_local(|m, _| f(&m.state))
    }

    /// Commit a system command through the system group's leader.
    pub async fn sys_write(&self, cmd: &SystemCmd) -> Result<Value, ApiError> {
        let bytes = serde_json::to_vec(cmd).map_err(|e| ApiError::internal(&e.to_string()))?;
        // Up to ~8 s: long enough to ride out a system leader election.
        for attempt in 0..50 {
            match self.system.write(bytes.clone()).await {
                Ok((index, out)) => {
                    let _ = index;
                    return output(&out).map_err(|e| rejected(&e));
                }
                Err(WriteError::NotLeader {
                    leader: Some((id, addr)),
                }) if id != self.config.node_id && !addr.is_empty() => {
                    match self
                        .peer_post(
                            &addr,
                            "internal/system",
                            &serde_json::to_value(cmd).unwrap_or_default(),
                        )
                        .await
                    {
                        Ok(v) => return Ok(v),
                        Err(e) => match e.downcast::<ApiError>() {
                            Ok(api) if api.code != "UNAVAILABLE" => return Err(api),
                            _ => {}
                        },
                    }
                }
                Err(e @ WriteError::TooLarge { .. }) => return Err(status_of(&e)),
                Err(e) => debug!(attempt, error = %e, "system write not yet possible"),
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        Err(ApiError::unavailable("the system group has no leader"))
    }

    fn app_group(&self, group: GroupId) -> Option<Arc<Group<AppMachine>>> {
        self.apps.read().ok()?.get(&group).cloned()
    }

    /// The app's group, if this node is one of its replicas. A node the app
    /// moved off may still hold the group until reconcile drops it; it must
    /// forward instead of answering from a group it no longer belongs to.
    fn replica_group(&self, app: &AppRecord) -> Option<Arc<Group<AppMachine>>> {
        if !app.replicas.contains(&self.config.node_id) {
            return None;
        }
        self.app_group(app.group)
    }

    /// The address of a replica of `app` other than this node.
    fn other_replica(&self, app: &AppRecord) -> Option<String> {
        self.sys(|s| {
            app.replicas
                .iter()
                .filter(|r| **r != self.config.node_id)
                .find_map(|r| s.nodes.get(r).map(|n| n.addr.clone()))
        })
    }

    /// Bring this node's system replica up to what the cluster has committed,
    /// so a read here sees every write acknowledged anywhere (a read index:
    /// the leader confirms it leads and names its applied index, this node
    /// waits to apply it). Best effort: on no answer the read goes ahead.
    pub async fn sys_fresh(&self) {
        let index = match self.system.read_strong(|_, applied| applied).await {
            Ok(i) => Some(i),
            Err(WriteError::NotLeader {
                leader: Some((id, addr)),
            }) if id != self.config.node_id && !addr.is_empty() => self
                .peer_forward(&addr, "internal/system/index", &json!({}))
                .await
                .ok()
                .and_then(|v| v["index"].as_u64()),
            Err(_) => None,
        };
        if let Some(i) = index {
            let _caught_up = self.system.wait_applied(i, Duration::from_secs(2)).await;
        }
    }

    /// Read the system state, waiting up to two seconds for `f` to find
    /// something: a write committed through another node may not have been
    /// applied here yet (read-your-writes across nodes).
    pub async fn sys_until<T>(&self, f: impl Fn(&SystemState) -> Option<T>) -> Option<T> {
        let mut applied = self.system.subscribe_applied();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(v) = self.sys(&f) {
                return Some(v);
            }
            if tokio::time::timeout_at(deadline, applied.changed())
                .await
                .is_err()
            {
                return self.sys(&f);
            }
        }
    }

    /// The app record, as applied here (waiting briefly for a fresh one).
    /// An app's id from either its id or its context id: what the app itself
    /// sees as `env::context_id()`, hex (`SHA-256(app_id)`). Apps hand context
    /// ids to each other and to their users, so either names the app here.
    #[must_use]
    pub fn canonical_app_id(&self, id: &str) -> String {
        if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return id.to_owned();
        }
        let wanted = id.to_ascii_lowercase();
        self.sys(|s| {
            s.apps
                .keys()
                .find(|a| hex::encode(calimero_identity::sha256(a.as_bytes())) == wanted)
                .cloned()
        })
        .unwrap_or_else(|| id.to_owned())
    }

    pub async fn app(&self, app_id: &str) -> Result<AppRecord, ApiError> {
        self.sys_until(|s| s.apps.get(app_id).cloned())
            .await
            .ok_or_else(|| ApiError::not_found("no such app"))
    }

    async fn start_app_group_if_replica(&self, group: GroupId) -> bool {
        let app = self.sys(|s| s.apps.values().find(|a| a.group == group).cloned());
        match app {
            Some(app) if app.replicas.contains(&self.config.node_id) => {
                self.ensure_app_group(&app).await.is_ok()
            }
            _ => false,
        }
    }

    async fn ensure_app_group(&self, app: &AppRecord) -> eyre::Result<Arc<Group<AppMachine>>> {
        if let Some(g) = self.app_group(app.group) {
            return Ok(g);
        }
        let _starting = self.starting.lock().await;
        if let Some(g) = self.app_group(app.group) {
            return Ok(g);
        }
        let g = self
            .engine
            .start_group(app.group, AppMachine::new(app.app_id.clone()))
            .await?;
        let _prev = self
            .apps
            .write()
            .map_err(|e| eyre::eyre!("{e}"))?
            .insert(app.group, Arc::clone(&g));
        if app.replicas.first() == Some(&self.config.node_id) {
            let voters: BTreeMap<NodeId, String> = self.sys(|s| {
                app.replicas
                    .iter()
                    .filter_map(|id| s.nodes.get(id).map(|n| (*id, n.addr.clone())))
                    .collect()
            });
            g.initialize(voters).await?;
        }
        info!(app = %app.app_id, group = app.group, "hosting app group");
        Ok(g)
    }

    /// Bring hosted groups in line with the system state.
    pub async fn reconcile(&self) {
        // The cluster's first leader registers the configured peers and the first session key.
        if self.system.status().is_leader {
            let missing: Vec<NodeRecord> = self.sys(|s| {
                self.config
                    .peers
                    .iter()
                    .filter(|(id, _)| !s.nodes.contains_key(id))
                    .map(|(id, addr)| NodeRecord {
                        id: *id,
                        addr: addr.clone(),
                        zone: String::new(),
                    })
                    .collect()
            });
            for node in missing {
                let _r = self.sys_write(&SystemCmd::RegisterNode { node }).await;
            }
            // The first session key, then a new one whenever the current one is old enough.
            let current = self.sys(|s| s.session_keys.last().map(|k| (k.version, k.created_at)));
            let now = crate::types::now_secs();
            let due = current.map_or(Some(1), |(version, created_at)| {
                (created_at + self.config.session_key_rotation_secs <= now).then_some(version + 1)
            });
            if let Some(version) = due {
                let key = calimero_identity::b64url(&calimero_identity::random::<32>());
                let _r = self
                    .sys_write(&SystemCmd::AddSessionKey {
                        version,
                        key,
                        created_at: now,
                    })
                    .await;
            }
        }
        let mine: Vec<AppRecord> = self.sys(|s| {
            s.apps
                .values()
                .filter(|a| a.replicas.contains(&self.config.node_id))
                .cloned()
                .collect()
        });
        for app in mine {
            let g = match self.ensure_app_group(&app).await {
                Ok(g) => g,
                Err(e) => {
                    warn!(app = %app.app_id, error = %e, "could not start app group");
                    continue;
                }
            };
            let needs_init = g.read_local(|m, _| !m.state.initialized && m.state.bundle.is_none());
            if needs_init && g.status().is_leader {
                if let Err(e) = self.init_app(&g, &app).await {
                    debug!(app = %app.app_id, error = %e.message, "init not yet possible");
                }
            }
            if g.status().is_leader {
                self.drive(&g, &app).await;
            }
        }
        // Stop groups this node no longer replicates.
        let hosted: Vec<GroupId> = self
            .apps
            .read()
            .map(|a| a.keys().copied().collect())
            .unwrap_or_default();
        for group in hosted {
            let keep = self.sys(|s| {
                s.apps
                    .values()
                    .any(|a| a.group == group && a.replicas.contains(&self.config.node_id))
            });
            if !keep {
                if let Some(app) = self.sys(|s| {
                    s.apps
                        .values()
                        .find(|a| a.group == group)
                        .map(|a| a.app_id.clone())
                }) {
                    self.search.drop_app(&app);
                }
                let _g = self.apps.write().ok().and_then(|mut a| a.remove(&group));
                let _r = self.engine.remove_group(group).await;
                info!(group, "stopped hosting app group");
            }
        }
    }

    /// Store a bundle here and on every other node of the fleet, before the
    /// system log records it. Succeeds once a majority holds it; a node that
    /// missed it fetches it from a peer on first use.
    pub async fn replicate_bundle(&self, hash: &str, wasm: &[u8]) -> Result<(), ApiError> {
        self.engine
            .put_blob(hash, wasm)
            .map_err(|e| ApiError::internal(&e.to_string()))?;
        let peers: Vec<String> = self.sys(|s| {
            s.nodes
                .values()
                .filter(|n| n.id != self.config.node_id)
                .map(|n| n.addr.clone())
                .collect()
        });
        let body = json!({ "hash": hash, "wasm": calimero_log::Bytes(wasm.to_vec()) });
        let results = futures_util::future::join_all(
            peers
                .iter()
                .map(|addr| self.peer_post(addr, "internal/blob", &body)),
        )
        .await;
        let held = 1 + results.iter().filter(|r| r.is_ok()).count();
        for (addr, r) in peers.iter().zip(&results) {
            if let Err(e) = r {
                warn!(%addr, error = %e, "bundle not stored on peer");
            }
        }
        if held * 2 <= peers.len() + 1 {
            return Err(ApiError::unavailable(
                "the bundle could not reach a majority of nodes",
            ));
        }
        Ok(())
    }

    /// A bundle's WASM: from this node's blob store, else from a peer.
    async fn bundle_wasm(&self, hash: &str) -> Result<Vec<u8>, ApiError> {
        if let Some(w) = self
            .engine
            .get_blob(hash)
            .map_err(|e| ApiError::internal(&e.to_string()))?
        {
            return Ok(w);
        }
        if !self.sys(|s| s.bundles.contains_key(hash)) {
            return Err(ApiError::not_found("bundle not replicated here yet"));
        }
        // A package release is fetched from where it lives and verified again.
        if let Some(release) = self.release_of(hash) {
            return self.release_wasm(hash, &release).await;
        }
        let peers: Vec<String> = self.sys(|s| {
            s.nodes
                .values()
                .filter(|n| n.id != self.config.node_id)
                .map(|n| n.addr.clone())
                .collect()
        });
        for addr in peers {
            let Ok(v) = self
                .peer_post(&addr, "internal/blob/get", &json!({ "hash": hash }))
                .await
            else {
                continue;
            };
            let Ok(wasm) = serde_json::from_value::<calimero_log::Bytes>(v["wasm"].clone()) else {
                continue;
            };
            if hex::encode(calimero_identity::sha256(&wasm.0)) == hash {
                self.engine
                    .put_blob(hash, &wasm.0)
                    .map_err(|e| ApiError::internal(&e.to_string()))?;
                return Ok(wasm.0);
            }
        }
        Err(ApiError::unavailable("no peer holds the bundle"))
    }

    /// What a run of `app_id` by `caller` is handed besides its call.
    fn env(&self, app_id: &str, caller: &Caller) -> RunEnv {
        let opener = caller.account().map(|a| {
            Arc::new(calimero_primitives::identity::PrivateKey::from(
                crate::backup::member_key(&self.config.storage_key, app_id, a),
            ))
        });
        RunEnv {
            org: self.sys(|s| crate::app::caller_org(s, app_id, caller)),
            blobs: Some(Arc::new(AppBlobs {
                node: self.this.lock().map(|w| w.clone()).unwrap_or_default(),
            })),
            sealing: calimero_runtime::logic::SealingContext {
                opener,
                ..Default::default()
            },
            ..RunEnv::default()
        }
    }

    /// The sealing keys of a run the node fires as the app's TEE: it opens
    /// with the app's TEE key, seals to it, and seals a member's share to the
    /// key standing for that member (every account that may reach the app).
    fn tee_sealing(&self, app_id: &str) -> calimero_runtime::logic::SealingContext {
        use calimero_primitives::identity::PrivateKey;
        let tee = PrivateKey::from(crate::backup::tee_key(&self.config.storage_key, app_id));
        let accounts: Vec<String> = self.sys(|s| {
            let Some(app) = s.apps.get(app_id) else {
                return Vec::new();
            };
            let mut out: Vec<String> = vec![app.owner.clone()];
            match &app.access {
                crate::types::Access::Accounts { accounts } => out.extend(accounts.iter().cloned()),
                crate::types::Access::Org => {
                    if let Some(org) = app.org.as_ref().and_then(|o| s.orgs.get(o)) {
                        out.extend(org.members.keys().cloned());
                    }
                }
                _ => {}
            }
            out.sort();
            out.dedup();
            out
        });
        // A member's one "device" is their account, which is what their runs
        // see as `env::device_id()`; sealing to it seals to their member key.
        let mut account_devices = BTreeMap::new();
        let mut stand_ins = BTreeMap::new();
        for a in &accounts {
            let key = PrivateKey::from(crate::backup::member_key(
                &self.config.storage_key,
                app_id,
                a,
            ));
            let id = Caller::Account { account: a.clone() }.account_bytes();
            let _prev = account_devices.insert(id, vec![id]);
            let _prev = stand_ins.insert(id, *AsRef::<[u8; 32]>::as_ref(&key.public_key()));
        }
        calimero_runtime::logic::SealingContext {
            tee_authority_keys: vec![*AsRef::<[u8; 32]>::as_ref(&tee.public_key())],
            opener: Some(Arc::new(tee)),
            vault_keys: Vec::new(),
            account_devices,
            stand_ins,
        }
    }

    /// What a bundle's ABI declares that the node acts on.
    pub async fn facts(&self, bundle: &str) -> Arc<BundleFacts> {
        if let Some(f) = self.facts.lock().ok().and_then(|f| f.get(bundle).cloned()) {
            return f;
        }
        let facts = Arc::new(
            self.bundle_wasm(bundle)
                .await
                .map(|w| BundleFacts::of(&w))
                .unwrap_or_default(),
        );
        if let Ok(mut f) = self.facts.lock() {
            let _prev = f.insert(bundle.to_owned(), Arc::clone(&facts));
        }
        facts
    }

    /// Store a blob here and on the other nodes; `Ok` once a majority holds it.
    /// Its id is `hex(SHA-256(bytes))`, the key the store files it under.
    pub async fn store_blob(&self, bytes: &[u8]) -> Result<String, ApiError> {
        let id = hex::encode(calimero_identity::sha256(bytes));
        self.engine
            .put_blob(&id, bytes)
            .map_err(|e| ApiError::internal(&e.to_string()))?;
        self.spread_blob(&id, bytes).await?;
        Ok(id)
    }

    /// Hand a blob this node holds to every other node; `Ok` once a majority holds it.
    async fn spread_blob(&self, id: &str, bytes: &[u8]) -> Result<(), ApiError> {
        let peers: Vec<String> = self.sys(|s| {
            s.nodes
                .values()
                .filter(|n| n.id != self.config.node_id)
                .map(|n| n.addr.clone())
                .collect()
        });
        let body = json!({ "hash": id, "wasm": calimero_log::Bytes(bytes.to_vec()) });
        let results = futures_util::future::join_all(
            peers
                .iter()
                .map(|addr| self.peer_post(addr, "internal/blob", &body)),
        )
        .await;
        let held = 1 + results.iter().filter(|r| r.is_ok()).count();
        if held * 2 <= peers.len() + 1 {
            return Err(ApiError::unavailable(
                "the blob could not reach a majority of nodes",
            ));
        }
        Ok(())
    }

    /// A blob's bytes from a peer (this node missed it), kept here after.
    pub async fn fetch_blob(&self, id: &str) -> Result<Vec<u8>, ApiError> {
        let peers: Vec<String> = self.sys(|s| {
            s.nodes
                .values()
                .filter(|n| n.id != self.config.node_id)
                .map(|n| n.addr.clone())
                .collect()
        });
        for addr in peers {
            let Ok(v) = self
                .peer_post(&addr, "internal/blob/get", &json!({ "hash": id }))
                .await
            else {
                continue;
            };
            let Ok(bytes) = serde_json::from_value::<calimero_log::Bytes>(v["wasm"].clone()) else {
                continue;
            };
            if hex::encode(calimero_identity::sha256(&bytes.0)) == id {
                self.engine
                    .put_blob(id, &bytes.0)
                    .map_err(|e| ApiError::internal(&e.to_string()))?;
                return Ok(bytes.0);
            }
        }
        Err(ApiError::unavailable("no peer holds the blob"))
    }

    /// Whether `id` is one of `app`'s blobs. A replica that has not applied
    /// the upload yet says no, so the others are asked too.
    pub async fn blob_held(&self, app: &AppRecord, id: &[u8; 32]) -> Result<bool, ApiError> {
        if let Some(g) = self.replica_group(app) {
            if g.read_local(|m, _| m.holds_blob(id)) {
                return Ok(true);
            }
        }
        let others: Vec<String> = self.sys(|s| {
            app.replicas
                .iter()
                .filter(|r| **r != self.config.node_id)
                .filter_map(|r| s.nodes.get(r).map(|n| n.addr.clone()))
                .collect()
        });
        let body = json!({ "app": app.app_id, "id": hex::encode(id) });
        for addr in others {
            if let Ok(v) = self.peer_forward(&addr, "internal/blob/held", &body).await {
                if v["held"].as_bool() == Some(true) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    async fn module_for(&self, bundle: &str) -> Result<Arc<calimero_runtime::Module>, ApiError> {
        let wasm = self.bundle_wasm(bundle).await?;
        self.executor
            .module(bundle, &wasm)
            .map_err(|e| ApiError::bad_request("BAD_BUNDLE", &e.to_string()))
    }

    async fn init_app(&self, g: &Group<AppMachine>, app: &AppRecord) -> Result<Applied, ApiError> {
        let module = self.module_for(&app.bundle).await?;
        let owner = Caller::Account {
            account: app.owner.clone(),
        };
        let bundle = app.bundle.clone();
        let app_id = app.app_id.clone();
        let init_args = app.init_args.clone();
        let env = self.env(&app.app_id, &owner);
        let p = g
            .propose(|m, applied| {
                if m.state.initialized || m.state.bundle.is_some() {
                    return Proposal::Reply(Vec::new());
                }
                let outcome = Executor::run(
                    &module,
                    &app_id,
                    &owner,
                    "init",
                    &init_args,
                    &m.state.kv,
                    &env,
                );
                self.keep_blobs(&outcome);
                let cmd = AppCmd::Init {
                    bundle: bundle.clone(),
                    base_index: applied,
                    outcome,
                };
                Proposal::Append(serde_json::to_vec(&cmd).unwrap_or_default())
            })
            .await
            .map_err(|e| status_of(&e))?;
        if !p.appended {
            return Err(ApiError::conflict("already initialized"));
        }
        self.kick.notify_one();
        serde_json::from_slice(&p.output).map_err(|e| ApiError::internal(&e.to_string()))
    }

    /// Keep the blobs a run wrote here, before its entry is proposed; the
    /// leader hands them to the other nodes once the entry commits.
    fn keep_blobs(&self, outcome: &Outcome) {
        for (id, bytes) in &outcome.new_blobs {
            if let Err(e) = self.engine.put_blob(id, bytes) {
                warn!(blob = %id, error = %e, "blob not stored");
            }
        }
    }

    /// After a run's entry committed: its blobs to the other nodes.
    async fn spread_blobs(&self, blobs: Vec<(String, Vec<u8>)>) {
        for (id, bytes) in blobs {
            if let Err(e) = self.spread_blob(&id, &bytes).await {
                warn!(blob = %id, error = %e.message, "blob held by too few nodes");
            }
        }
    }

    /// Run a call on this node, which must be the app group's leader.
    async fn call_local(
        &self,
        g: &Group<AppMachine>,
        req: &CallRequest,
    ) -> Result<Applied, WriteError> {
        // The caller's standing (org role, teams, read-only rows) is read from
        // the system state below; for a client's call it must include every
        // change committed before the call, wherever it was committed.
        if req.trigger.is_none() {
            self.sys_fresh().await;
        }
        let bundle = g.read_local(|m, _| m.state.bundle.clone());
        let module = match bundle.as_deref() {
            Some(b) => self.module_for(b).await.ok(),
            None => None,
        };
        let now = now_ms();
        if req.method == "__upgrade" {
            return self.upgrade_local(g, req).await;
        }
        if req.method == "__blob" {
            let cmd = AppCmd::Blob {
                id: req.args["id"].as_str().unwrap_or_default().to_owned(),
                size: req.args["size"].as_u64().unwrap_or_default(),
            };
            let bytes = serde_json::to_vec(&cmd).unwrap_or_default();
            let (_index, out) = g.write(bytes).await?;
            return serde_json::from_slice(&out)
                .map_err(|e| WriteError::Unavailable(e.to_string()));
        }
        let mut env = self.env(&req.app_id, &req.caller);
        // Who may reach this method, when the node runs it itself.
        let facts = match bundle.as_deref() {
            Some(b) => self.facts(b).await,
            None => Arc::new(BundleFacts::default()),
        };
        let refused: Option<String> = match &req.trigger {
            None => None,
            Some(Trigger::Timer { .. }) => {
                env.tee_trigger = true;
                env.sealing = self.tee_sealing(&req.app_id);
                None
            }
            Some(Trigger::FollowUp { seq }) => {
                let tee = g.read_local(|m, _| {
                    matches!(
                        m.state.pending.get(seq).map(|p| &p.followup),
                        Some(FollowUp::Handler { tee: true, .. })
                    )
                });
                env.tee_trigger = tee;
                if tee {
                    env.sealing = self.tee_sealing(&req.app_id);
                }
                (!tee && !facts.handlers.iter().any(|h| h == &req.method))
                    .then(|| format!("{} is not an #[app::handler]", req.method))
            }
            Some(Trigger::XCall { origin, .. }) => {
                env.xcall_origin = Some(calimero_identity::sha256(origin.as_bytes()));
                self.xcall_refusal(&facts, origin, &req.app_id, &req.method)
            }
        };
        let input = req.input_bytes();
        // A read-only member's call runs, but one that would write is refused
        // (v1 discarded such writes; a client is told).
        let read_only = match self.sys(|s| s.apps.get(&req.app_id).cloned()) {
            Some(app) => crate::api::read_only_in(self, &app, req.caller.account()).await,
            None => false,
        };
        let mut fresh_blobs = Vec::new();
        let p = g
            .propose(|m, applied| {
                if let Some(key) = &req.idempotency_key {
                    if let Some(prev) = m.replay(&req.caller, key) {
                        return Proposal::Reply(serde_json::to_vec(&prev).unwrap_or_default());
                    }
                }
                // A follow-up already run (a replayed driver pass) is not run twice.
                if let Some(Trigger::FollowUp { seq }) = &req.trigger {
                    if !m.state.pending.contains_key(seq) {
                        return Proposal::Reply(Vec::new());
                    }
                }
                let outcome = match (&module, &refused) {
                    (_, Some(why)) => Outcome {
                        status: Status::AppError,
                        error: Value::String(why.clone()),
                        ..Outcome::default()
                    },
                    (Some(module), None) => {
                        let (outcome, _ret) = Executor::run_bytes(
                            module,
                            &req.app_id,
                            &req.caller,
                            &req.method,
                            &input,
                            &m.state.kv,
                            &env,
                        );
                        if read_only && (!outcome.diff.is_empty() || !outcome.new_blobs.is_empty())
                        {
                            let refused = Applied {
                                index: applied,
                                status: Status::AppError,
                                result: Value::Null,
                                error: Value::String(READ_ONLY.into()),
                                events: Vec::new(),
                                conflict: false,
                            };
                            return Proposal::Reply(
                                serde_json::to_vec(&refused).unwrap_or_default(),
                            );
                        }
                        self.keep_blobs(&outcome);
                        fresh_blobs.clone_from(&outcome.new_blobs);
                        outcome
                    }
                    (None, None) => Outcome::trap("the app is not initialized yet"),
                };
                let cmd = AppCmd::Call(Box::new(CallEntry {
                    method: req.method.clone(),
                    args: req.args.clone(),
                    caller: req.caller.clone(),
                    idempotency_key: req.idempotency_key.clone(),
                    consent: req.consent.clone(),
                    ts_ms: now,
                    base_index: applied,
                    outcome,
                    trigger: req.trigger.clone(),
                }));
                Proposal::Append(serde_json::to_vec(&cmd).unwrap_or_default())
            })
            .await?;
        if p.output.is_empty() {
            // A follow-up someone already ran: nothing to answer.
            return Ok(Applied {
                index: 0,
                status: Status::Ok,
                result: Value::Null,
                error: Value::Null,
                events: Vec::new(),
                conflict: false,
            });
        }
        let applied: Applied = serde_json::from_slice(&p.output)
            .map_err(|e| WriteError::Unavailable(e.to_string()))?;
        if p.appended && !applied.conflict {
            self.kick.notify_one();
            self.spread_blobs(fresh_blobs).await;
        }
        Ok(applied)
    }

    /// Switch an app to a new bundle, running its migration export if named.
    async fn upgrade_local(
        &self,
        g: &Group<AppMachine>,
        req: &CallRequest,
    ) -> Result<Applied, WriteError> {
        let bundle = req
            .args
            .get("bundle")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let mut migrate = req
            .args
            .get("migrate")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let module = self
            .module_for(&bundle)
            .await
            .map_err(|e| WriteError::Unavailable(e.message))?;
        // Without a named migration, plan it from the two ABIs, as v1 does.
        if migrate.is_none() {
            let current = g.read_local(|m, _| m.state.bundle.clone());
            if let Some(current) = current.filter(|c| *c != bundle) {
                let (from, to) = (
                    self.bundle_wasm(&current).await,
                    self.bundle_wasm(&bundle).await,
                );
                if let (Ok(from), Ok(to)) = (from, to) {
                    match crate::services::plan_upgrade(&from, &to) {
                        Ok(planned) => migrate = planned,
                        Err(why) => {
                            return Ok(Applied {
                                index: g.applied(),
                                status: Status::AppError,
                                result: Value::Null,
                                error: Value::String(format!("upgrade refused: {why}")),
                                events: Vec::new(),
                                conflict: false,
                            })
                        }
                    }
                }
            }
        }
        let env = self.env(&req.app_id, &req.caller);
        let p = g
            .propose(|m, applied| {
                let outcome = match &migrate {
                    Some(method) => {
                        let outcome = Executor::migrate(
                            &module,
                            &req.app_id,
                            &req.caller,
                            method,
                            &m.state.kv,
                            &env,
                        );
                        self.keep_blobs(&outcome);
                        outcome
                    }
                    None => Outcome::default(),
                };
                let cmd = AppCmd::Upgrade {
                    bundle: bundle.clone(),
                    base_index: applied,
                    outcome,
                };
                Proposal::Append(serde_json::to_vec(&cmd).unwrap_or_default())
            })
            .await?;
        serde_json::from_slice(&p.output).map_err(|e| WriteError::Unavailable(e.to_string()))
    }

    /// Why an `xcall` from app `origin` may not run `method` of app `target`,
    /// if it may not (v1's rules): the method must be an `#[app::xcall]` entry
    /// point, `from_same_app` admits only the same bundle, and the two apps
    /// must share their organization (or, outside one, their owner).
    fn xcall_refusal(
        &self,
        target_facts: &BundleFacts,
        origin: &str,
        target: &str,
        method: &str,
    ) -> Option<String> {
        let Some(callers) = target_facts.xcalls.get(method) else {
            return Some(format!(
                "xcall denied: {method} is not an #[app::xcall] entry point"
            ));
        };
        let (source, dest) =
            self.sys(|s| (s.apps.get(origin).cloned(), s.apps.get(target).cloned()));
        let (Some(source), Some(dest)) = (source, dest) else {
            return Some("xcall denied: unknown app".into());
        };
        let same_boundary = match (&source.org, &dest.org) {
            (Some(a), Some(b)) => a == b,
            (None, None) => source.owner == dest.owner,
            _ => false,
        };
        if !same_boundary {
            return Some("xcall denied: the apps are not in the same organization".into());
        }
        if *callers == calimero_wasm_abi::schema::XCallCallers::SameApp
            && source.bundle != dest.bundle
        {
            return Some(format!("xcall denied: {method} admits only the same app"));
        }
        None
    }

    /// The leader's work for an app beyond requests: timer methods whose period
    /// came, and follow-ups (event handlers, xcalls) the log queued. Each runs
    /// as an entry that also records it done, so a new leader neither skips
    /// nor repeats one.
    pub async fn drive(&self, g: &Group<AppMachine>, app: &AppRecord) {
        let (bundle, initialized) =
            g.read_local(|m, _| (m.state.bundle.clone(), m.state.initialized));
        let (Some(bundle), true) = (bundle, initialized) else {
            return;
        };
        let facts = self.facts(&bundle).await;
        let app_caller = Caller::App {
            app: app.app_id.clone(),
        };
        let now = crate::types::now_secs();
        for (method, every) in &facts.timers {
            let tick = now / every;
            let last = g.read_local(|m, _| m.state.timer_ticks.get(method).copied());
            if last.is_some_and(|l| l >= tick) {
                continue;
            }
            let mut req = CallRequest::new(
                app.app_id.clone(),
                method.clone(),
                Value::Null,
                app_caller.clone(),
            );
            req.trigger = Some(Trigger::Timer {
                method: method.clone(),
                tick,
            });
            match self.call_local(g, &req).await {
                Ok(a) if a.status != Status::Ok => {
                    warn!(app = %app.app_id, %method, error = %a.error, "timer method failed");
                }
                Ok(_) => {}
                Err(e) => {
                    debug!(app = %app.app_id, %method, error = %e, "timer not fired");
                    return;
                }
            }
        }
        let pending: Vec<(u64, crate::app::Pending)> = g.read_local(|m, _| {
            m.state
                .pending
                .iter()
                .take(64)
                .map(|(k, v)| (*k, v.clone()))
                .collect()
        });
        for (seq, p) in pending {
            match p.followup {
                FollowUp::Handler { method, input, .. } => {
                    let mut req = CallRequest::new(
                        app.app_id.clone(),
                        method,
                        Value::Null,
                        app_caller.clone(),
                    );
                    req.input = Some(input);
                    req.trigger = Some(Trigger::FollowUp { seq });
                    match self.call_local(g, &req).await {
                        Ok(a) if a.status != Status::Ok => {
                            warn!(app = %app.app_id, method = %req.method, error = %a.error, "event handler failed");
                        }
                        Ok(_) => {}
                        Err(e) => {
                            debug!(app = %app.app_id, seq, error = %e, "handler not run");
                            return;
                        }
                    }
                }
                FollowUp::XCall {
                    context,
                    method,
                    input,
                } => {
                    let target = self.canonical_app_id(&context);
                    let mut req =
                        CallRequest::new(target.clone(), method, Value::Null, app_caller.clone());
                    req.input = Some(input);
                    req.idempotency_key = Some(format!("xcall:{}:{seq}", app.app_id));
                    req.trigger = Some(Trigger::XCall {
                        origin: app.app_id.clone(),
                        depth: p.depth + 1,
                    });
                    let known = self.sys(|s| s.apps.contains_key(&target));
                    if known {
                        match self.call(&req).await {
                            Ok(a) if a.status != Status::Ok => {
                                warn!(from = %app.app_id, to = %target, error = %a.error, "xcall refused");
                            }
                            Ok(_) => {}
                            Err(e) if e.code == "UNAVAILABLE" => {
                                debug!(to = %target, error = %e.message, "xcall waits for its target");
                                return;
                            }
                            Err(e) => warn!(to = %target, error = %e.message, "xcall failed"),
                        }
                    } else {
                        warn!(from = %app.app_id, to = %context, "xcall to an unknown app dropped");
                    }
                    let done =
                        serde_json::to_vec(&AppCmd::Done { seqs: vec![seq] }).unwrap_or_default();
                    if let Err(e) = g.write(done).await {
                        debug!(error = %e, "xcall not yet marked done");
                        return;
                    }
                }
            }
        }
    }

    /// Run a call wherever the app's leader is.
    pub async fn call(&self, req: &CallRequest) -> Result<Applied, ApiError> {
        // The loop below retries forwards that timed out and calls that raced a
        // leader change; a call that did commit must replay, not run again.
        let keyed;
        let req = if req.idempotency_key.is_some() {
            req
        } else {
            let mut r = req.clone();
            r.idempotency_key = Some(format!(
                "{}{}",
                crate::app::NODE_KEY_PREFIX,
                calimero_identity::b64url(&calimero_identity::random::<16>())
            ));
            keyed = r;
            &keyed
        };
        let app = self.app(&req.app_id).await?;
        let body = serde_json::to_value(req).map_err(|e| ApiError::internal(&e.to_string()))?;
        for _attempt in 0..30 {
            // A conflict (the diff was built on state a leader change replaced)
            // applied nothing and recorded no idempotency key: run it again.
            let target = if let Some(g) = self.replica_group(&app) {
                match self.call_local(&g, req).await {
                    Ok(a) if a.conflict => None,
                    Ok(a) => return Ok(a),
                    Err(WriteError::NotLeader {
                        leader: Some((id, addr)),
                    }) if id != self.config.node_id && !addr.is_empty() => Some(addr),
                    Err(e) => {
                        debug!(error = %e, "call not yet possible here");
                        None
                    }
                }
            } else {
                self.other_replica(&app)
            };
            if let Some(addr) = target {
                match self.peer_forward(&addr, "internal/call", &body).await {
                    Ok(v) => {
                        let a: Applied = serde_json::from_value(v)
                            .map_err(|e| ApiError::internal(&e.to_string()))?;
                        if !a.conflict {
                            return Ok(a);
                        }
                    }
                    Err(e) => match e.downcast::<ApiError>() {
                        Ok(api) if api.code != "UNAVAILABLE" => return Err(api),
                        _ => {}
                    },
                }
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        Err(ApiError::unavailable("the app has no leader"))
    }

    /// Answer an internal call (from a peer): run it here, without forwarding further.
    pub async fn call_here(&self, req: &CallRequest) -> Result<Applied, ApiError> {
        let app = self.app(&req.app_id).await?;
        let g = self
            .app_group(app.group)
            .ok_or_else(|| ApiError::unavailable("app not hosted here"))?;
        match self.call_local(&g, req).await {
            Ok(a) => Ok(a),
            Err(WriteError::NotLeader {
                leader: Some((id, addr)),
            }) if id != self.config.node_id && !addr.is_empty() => {
                let body =
                    serde_json::to_value(req).map_err(|e| ApiError::internal(&e.to_string()))?;
                let v = self
                    .peer_forward(&addr, "internal/call", &body)
                    .await
                    .map_err(|e| ApiError::unavailable(&e.to_string()))?;
                serde_json::from_value(v).map_err(|e| ApiError::internal(&e.to_string()))
            }
            Err(e) => Err(status_of(&e)),
        }
    }

    async fn view_local(
        &self,
        g: &Group<AppMachine>,
        req: &ViewRequest,
    ) -> Result<ViewAnswer, ApiError> {
        let bundle = g
            .read_local(|m, _| m.state.bundle.clone())
            .ok_or_else(|| ApiError::unavailable("the app is not initialized yet"))?;
        let module = self.module_for(&bundle).await?;
        let mut env = self.env(&req.app_id, &req.caller);
        Ok(g.read_local(|m, applied| {
            // Search answers from indexes brought up to this very state.
            env.search = self.search.for_view(&module, &bundle, m, applied);
            let out = Executor::run(
                &module,
                &req.app_id,
                &req.caller,
                &req.method,
                &req.args,
                &m.state.kv,
                &env,
            );
            ViewAnswer {
                index: applied,
                status: out.status,
                result: out.result,
                error: out.error,
            }
        }))
    }

    /// Run a read-only method. Its writes, if any, are discarded.
    pub async fn view(&self, req: &ViewRequest) -> Result<ViewAnswer, ApiError> {
        // Like calls, ride out an election (e.g. right after the leader moved off).
        let mut last = None;
        for _attempt in 0..30 {
            match self.view_once(req).await {
                Err(e) if e.code == "UNAVAILABLE" => last = Some(e),
                other => return other,
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        Err(last.unwrap_or_else(|| ApiError::unavailable("the app has no leader")))
    }

    async fn view_once(&self, req: &ViewRequest) -> Result<ViewAnswer, ApiError> {
        let app = self.app(&req.app_id).await?;
        if let Some(g) = self.replica_group(&app) {
            if req.strong {
                match g.read_strong(|_, applied| applied).await {
                    Ok(_) => return self.view_local(&g, req).await,
                    Err(WriteError::NotLeader {
                        leader: Some((id, addr)),
                    }) if id != self.config.node_id && !addr.is_empty() => {
                        let v = self
                            .peer_forward(
                                &addr,
                                "internal/view",
                                &serde_json::to_value(req).unwrap_or_default(),
                            )
                            .await
                            .map_err(|e| ApiError::unavailable(&e.to_string()))?;
                        return serde_json::from_value(v)
                            .map_err(|e| ApiError::internal(&e.to_string()));
                    }
                    Err(e) => return Err(status_of(&e)),
                }
            }
            if req.min_index > 0 && !g.wait_applied(req.min_index, Duration::from_secs(2)).await {
                return Err(ApiError::new(
                    503,
                    "NOT_CAUGHT_UP",
                    "this replica has not caught up to minIndex",
                ));
            }
            return self.view_local(&g, req).await;
        }
        let addr = self
            .other_replica(&app)
            .ok_or_else(|| ApiError::unavailable("no replica known"))?;
        let v = self
            .peer_forward(
                &addr,
                "internal/view",
                &serde_json::to_value(req).unwrap_or_default(),
            )
            .await
            .map_err(|e| ApiError::unavailable(&e.to_string()))?;
        serde_json::from_value(v).map_err(|e| ApiError::internal(&e.to_string()))
    }

    /// A hosted app group, for event streaming.
    #[must_use]
    pub fn hosted(&self, app_id: &str) -> Option<Arc<Group<AppMachine>>> {
        let group = self.sys(|s| s.apps.get(app_id).map(|a| a.group))?;
        self.app_group(group)
    }

    /// Pick replicas for a new app: the least-loaded nodes.
    #[must_use]
    pub fn place(&self) -> Vec<NodeId> {
        self.sys(|s| {
            let mut load: Vec<(usize, NodeId)> = s
                .nodes
                .keys()
                .map(|id| {
                    (
                        s.apps.values().filter(|a| a.replicas.contains(id)).count(),
                        *id,
                    )
                })
                .collect();
            load.sort_unstable();
            let mut picked: Vec<NodeId> = load
                .into_iter()
                .take(self.config.replicas)
                .map(|(_, id)| id)
                .collect();
            picked.sort_unstable();
            picked
        })
    }

    /// Write encrypted snapshots of the app groups this node leads.
    ///
    /// A backup is the group's snapshot file as the log wrote it: streamed
    /// from disk, sealed frame by frame, never the whole state in memory.
    pub async fn backup(&self) -> eyre::Result<usize> {
        let Some(dir) = self.config.backup_dir.clone() else {
            return Ok(0);
        };
        let groups: Vec<Arc<Group<AppMachine>>> = self
            .apps
            .read()
            .map(|a| a.values().cloned().collect())
            .unwrap_or_default();
        let mut written = 0;
        for g in groups {
            if !g.status().is_leader {
                continue;
            }
            let (app_id, applied) = g.read_local(|m, applied| (m.app_id.clone(), applied));
            // Bring the snapshot up to what this node has applied first.
            let fresh =
                |s: &Option<(u64, String, PathBuf)>| s.as_ref().is_some_and(|s| s.0 >= applied);
            let mut snap = self.engine.snapshot_file(g.id())?;
            if !fresh(&snap) {
                g.snapshot_now().await?;
                for _ in 0..100 {
                    snap = self.engine.snapshot_file(g.id())?;
                    if fresh(&snap) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
            let Some((index, id, file)) = snap else {
                continue;
            };
            let path = dir.join(&app_id);
            std::fs::create_dir_all(&path)?;
            let out = path.join(crate::backup::file_name(index, g.id(), &id));
            if !out.exists() {
                let _bytes = std::fs::copy(&file, &out)?;
                written += 1;
            }
        }
        Ok(written)
    }
}
