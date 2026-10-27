//! The node: hosts the system group and its app groups, routes calls to each
//! group's leader, and talks to peers over attested sealed channels.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Duration;

use calimero_app_engine_wasmtime::WasmtimeEngine;
use calimero_app_host::{
    Budget, Engine as _, HostCore, Invocation, InvocationKind, MethodKind, Prepared, TimerCtx,
    TrapKind,
};

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
    AppCmd, AppMachine, Applied, CallEntry, Outcome, Status, SysDirectory, TickEntry,
};
use crate::attest::{HttpAttestor, Mode, Policy};
use crate::error::ApiError;
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
    /// Where app time starts relative to this host's clock, in ms (D19). `0` in
    /// production; tests move a node's app clock with it.
    pub clock_offset_ms: i64,
    /// The limits of every call and view; `None` is `Budget::call()`. Tests only.
    pub budget: Option<Budget>,
}

/// App time (D16, D19): this host's clock plus a shared offset. It stamps entries
/// and timer slots, nothing else; sessions, tokens and challenges use
/// `types::now_ms`/`now_secs`, and Raft timers use `Instant`.
#[derive(Clone, Debug, Default)]
pub struct Clock {
    offset_ms: Arc<AtomicI64>,
}

impl Clock {
    #[must_use]
    pub fn new(offset_ms: i64) -> Self {
        Self {
            offset_ms: Arc::new(AtomicI64::new(offset_ms)),
        }
    }

    /// Milliseconds since the epoch, on app time.
    #[must_use]
    pub fn now_ms(&self) -> u64 {
        let host = i64::try_from(now_ms()).unwrap_or(i64::MAX);
        let at = host.saturating_add(self.offset_ms.load(Ordering::Relaxed));
        u64::try_from(at).unwrap_or(0)
    }

    /// Moves app time by `ms` (negative: back).
    pub fn advance(&self, ms: i64) {
        let _prev = self.offset_ms.fetch_add(ms, Ordering::Relaxed);
    }
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
        }
    }
}

/// A method's JSON arguments as the guest takes them (`null`: none).
fn args_bytes(args: &Value) -> Vec<u8> {
    if args.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(args).unwrap_or_default()
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
    /// The view hit its deadline: answered `503 TIMEOUT` (D36).
    #[serde(default)]
    pub timeout: bool,
}

/// Shared node state.
pub struct Node {
    pub config: Config,
    pub engine: Arc<Engine>,
    pub system: Arc<Group<SystemMachine>>,
    apps: RwLock<HashMap<GroupId, Arc<Group<AppMachine>>>>,
    /// Runs every app's components (one engine for all apps, spec section 5).
    wasm: Arc<WasmtimeEngine>,
    /// Prepared components per bundle hash.
    prepared: Mutex<HashMap<String, Prepared>>,
    /// App time ([`Clock`]).
    clock: Clock,
    /// Per app group: the term this node last led it in, and its execution
    /// counter in that term (`id_seq`, D52).
    id_seqs: Mutex<HashMap<GroupId, (u64, u64)>>,
    /// App groups whose timer ticks are being run now (one round per app at a time).
    ticking: Mutex<HashSet<GroupId>>,
    pub transport: Arc<calimero_sealed::SealedTransport>,
    peers: Mutex<HashMap<String, Arc<calimero_sealed::client::SealedTransport>>>,
    http: reqwest::Client,
    cluster_secret: String,
    pub challenges: Mutex<HashMap<String, crate::api::Pending>>,
    /// Serializes starting app groups (reconcile and the RPC factory race otherwise).
    starting: tokio::sync::Mutex<()>,
    /// This node, for the directory a run asks ([`SysDirectory`]).
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

/// The error of a call whose caller no longer reaches the app (answered `404`).
pub const NOT_REACHED: &str = "no such app";

/// The code of a component the engine refuses.
pub const BAD_BUNDLE: &str = "BAD_BUNDLE";

/// The largest call or `init` arguments a node runs (Ruling 30): well under
/// `MAX_ENTRY_BYTES`, so the entry that logs a refused run always fits.
pub const MAX_ARGS_BYTES: usize = 512 * 1024;

/// `413 ARGS_TOO_LARGE`: arguments over [`MAX_ARGS_BYTES`], refused before running.
#[must_use]
pub fn args_too_large() -> ApiError {
    ApiError::new(
        413,
        "ARGS_TOO_LARGE",
        &format!("arguments are limited to {MAX_ARGS_BYTES} bytes"),
    )
}

/// `503 TIMEOUT`: the run hit its deadline; not logged, the key not spent (D23, D36).
#[must_use]
pub fn timeout_error() -> ApiError {
    ApiError::new(
        503,
        "TIMEOUT",
        "the run hit its deadline; retry with the same key",
    )
    .with_details(json!({ "retryAfter": 1 }))
}

/// A run whose entry the log refused as too large: a node bug (the host-denied
/// replacement always fits), never retried.
fn entry_refused(e: &WriteError) -> ApiError {
    ApiError::internal(&format!("the log refused the entry: {e}"))
}

/// The method name `POST /apps/{app}/upgrade` travels under.
pub const UPGRADE: &str = "__upgrade";

/// A refusal the host makes for the app, before running it (`422 APP_ERROR`).
#[must_use]
pub fn app_refusal(code: &str) -> ApiError {
    ApiError::new(422, "APP_ERROR", "the app refused the call")
        .with_details(json!({ "code": code, "fields": {} }))
}

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
            &config.data_dir.join("log.redb"),
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
        // Compiled components live in the node's own data directory (S8).
        let wasm = Arc::new(WasmtimeEngine::new(config.data_dir.join("components"))?);
        let clock = Clock::new(config.clock_offset_ms);
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
            wasm,
            prepared: Mutex::new(HashMap::new()),
            clock,
            id_seqs: Mutex::new(HashMap::new()),
            ticking: Mutex::new(HashSet::new()),
            transport: sealed,
            peers: Mutex::new(HashMap::new()),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()?,
            challenges: Mutex::new(HashMap::new()),
            starting: tokio::sync::Mutex::new(()),
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
                        Ok(v) => {
                            // Read-your-writes: the next request through this node
                            // (an access check) sees the write it just made.
                            self.sys_fresh().await;
                            return Ok(v);
                        }
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

    /// The engine every app runs on.
    #[must_use]
    pub fn app_engine(&self) -> &WasmtimeEngine {
        &self.wasm
    }

    /// App time: what entry stamps and timer slots read (D16, D19, D34).
    #[must_use]
    pub fn app_clock(&self) -> Clock {
        self.clock.clone()
    }

    /// Moves this node's app time by `ms` (negative: back). Tests only.
    #[cfg(feature = "testing")]
    pub fn advance_clock(&self, ms: i64) {
        self.clock.advance(ms);
    }

    /// The prepared component of `bundle`, preparing it on first use. A component
    /// the engine refuses is `422 BAD_BUNDLE`; a bundle not reachable yet is `503`.
    pub async fn prepared_for(&self, bundle: &str) -> Result<Prepared, ApiError> {
        if let Some(p) = self
            .prepared
            .lock()
            .ok()
            .and_then(|m| m.get(bundle).cloned())
        {
            return Ok(p);
        }
        let component = self.bundle_wasm(bundle).await?;
        let (wasm, hash) = (Arc::clone(&self.wasm), bundle.to_owned());
        let prepared = tokio::task::spawn_blocking(move || wasm.prepare(&hash, &component))
            .await
            .map_err(|e| ApiError::internal(&e.to_string()))?
            .map_err(|e| ApiError::new(422, BAD_BUNDLE, &e.to_string()))?;
        if let Ok(mut m) = self.prepared.lock() {
            let _prev = m.insert(bundle.to_owned(), prepared.clone());
        }
        Ok(prepared)
    }

    fn budget(&self) -> Budget {
        self.config.budget.unwrap_or_else(Budget::call)
    }

    /// Run one invocation of `app_id` against `state` at `applied`.
    fn run(
        &self,
        prepared: &Prepared,
        app_id: &str,
        state: Arc<dyn calimero_app_host::StateView>,
        applied: u64,
        invocation: Invocation,
    ) -> Outcome {
        let this = self.this.lock().map(|w| w.clone()).unwrap_or_default();
        let directory = Arc::new(SysDirectory::new(this, app_id.to_owned()));
        match HostCore::new(app_id, state, applied, invocation, directory) {
            Ok(host) => Outcome::from_host(self.wasm.invoke(prepared, host, self.budget())),
            Err(e) => Outcome::trap(TrapKind::Fault, &e.to_string()),
        }
    }

    /// The next `(id_term, id_seq)` for a run this node leads in `term` (D52,
    /// Ruling 29). `term` comes from the leadership check that admitted the
    /// proposal; `logged` is the highest pair the app's log carries (applied up to
    /// this node's last entry before the proposal was built). The counter continues
    /// from the larger of this node's memory and the log, so a node that leads the
    /// same term again after a restart never reissues an id.
    fn next_id(&self, group: GroupId, term: u64, logged: (u64, u64)) -> (u64, u64) {
        let Ok(mut ids) = self.id_seqs.lock() else {
            return (term, u64::MAX);
        };
        let entry = ids.entry(group).or_insert((term, 0));
        if entry.0 != term {
            *entry = (term, 0);
        }
        if logged.0 == term {
            entry.1 = entry.1.max(logged.1);
        }
        entry.1 += 1;
        (term, entry.1)
    }

    /// The timestamp of the next entry (D16): never before the last one applied.
    fn stamp(&self, m: &AppMachine) -> u64 {
        (m.state.last_ts_ms + 1).max(self.clock.now_ms())
    }

    /// An entry as bytes; one over `MAX_ENTRY_BYTES` becomes the same call
    /// trapped `HostDenied` with nothing in it, decided before proposing (D50).
    fn entry_bytes(entry: CallEntry) -> Vec<u8> {
        let mut cmd = AppCmd::Call(Box::new(entry));
        let bytes = serde_json::to_vec(&cmd).unwrap_or_default();
        if bytes.len() <= calimero_log::MAX_ENTRY_BYTES {
            return bytes;
        }
        if let AppCmd::Call(entry) = &mut cmd {
            entry.outcome = Outcome::trap(TrapKind::HostDenied, "entry too large");
        }
        serde_json::to_vec(&cmd).unwrap_or_default()
    }

    async fn init_app(&self, g: &Group<AppMachine>, app: &AppRecord) -> Result<Applied, ApiError> {
        // A component the engine refuses fails `init` for good (reported as
        // INIT_FAILED); anything else is retried by `reconcile`.
        let prepared = match self.prepared_for(&app.bundle).await {
            Ok(p) => Ok(p),
            Err(e) if e.code == BAD_BUNDLE => Err(format!("component refused: {}", e.message)),
            Err(e) => return Err(e),
        };
        let owner = Caller::Account {
            account: app.owner.clone(),
        };
        let args = args_bytes(&app.init_args);
        if args.len() > MAX_ARGS_BYTES {
            return Err(args_too_large());
        }
        let mut timed_out = false;
        let p = g
            .propose_led(|m, applied, leadership| {
                if m.state.initialized || m.state.bundle.is_some() {
                    return Proposal::Reply(Vec::new());
                }
                let ts_ms = self.stamp(m);
                let (id_term, id_seq) = self.next_id(g.id(), leadership.term, m.state.last_id);
                let outcome = match &prepared {
                    Ok(prepared) => self.run(
                        prepared,
                        &app.app_id,
                        m.view(),
                        applied,
                        Invocation {
                            kind: InvocationKind::Init,
                            method: "init".into(),
                            args: args.clone(),
                            caller: owner.to_host(),
                            entry_ts_ms: ts_ms,
                            id_term,
                            id_seq,
                            seed: calimero_identity::random::<32>(),
                            timer: None,
                        },
                    ),
                    Err(why) => Outcome::trap(TrapKind::Fault, why),
                };
                if outcome.is_timeout() {
                    // Not logged (D23): reconcile runs init again.
                    timed_out = true;
                    return Proposal::Reply(Vec::new());
                }
                let mut cmd = AppCmd::Init {
                    bundle: app.bundle.clone(),
                    base_index: applied,
                    ts_ms,
                    id_term,
                    id_seq,
                    outcome: Box::new(outcome),
                };
                let mut bytes = serde_json::to_vec(&cmd).unwrap_or_default();
                if bytes.len() > calimero_log::MAX_ENTRY_BYTES {
                    if let AppCmd::Init { outcome, .. } = &mut cmd {
                        **outcome = Outcome::trap(TrapKind::HostDenied, "entry too large");
                    }
                    bytes = serde_json::to_vec(&cmd).unwrap_or_default();
                }
                Proposal::Append(bytes)
            })
            .await
            .map_err(|e| status_of(&e))?;
        if timed_out {
            return Err(timeout_error());
        }
        if !p.appended {
            return Err(ApiError::conflict("already initialized"));
        }
        serde_json::from_slice(&p.output).map_err(|e| ApiError::internal(&e.to_string()))
    }

    /// One round of timer ticks (D34), every [`TIMER_POLL`]: for each app group
    /// this node leads, a task proposes the due ticks, unless the previous round
    /// for that app is still running (a slow tick never delays another app).
    pub fn tick_timers(self: &Arc<Self>) {
        let led: Vec<(GroupId, Arc<Group<AppMachine>>)> = self
            .apps
            .read()
            .map(|a| {
                a.iter()
                    .filter(|(_, g)| g.status().is_leader)
                    .map(|(id, g)| (*id, Arc::clone(g)))
                    .collect()
            })
            .unwrap_or_default();
        for (group, g) in led {
            let Ok(mut ticking) = self.ticking.lock() else {
                return;
            };
            if !ticking.insert(group) {
                continue;
            }
            drop(ticking);
            let node = Arc::clone(self);
            drop(tokio::spawn(async move {
                node.tick_app(&g).await;
                if let Ok(mut t) = node.ticking.lock() {
                    let _was = t.remove(&group);
                }
            }));
        }
    }

    /// Propose the latest due slot of each of the app's timers not yet applied
    /// (one catch-up tick for a gap). Nothing runs before the app is initialized.
    async fn tick_app(&self, g: &Group<AppMachine>) {
        let Some((app_id, bundle, install, applied)) = g.read_local(|m, _| {
            let bundle = m.state.bundle.clone().filter(|_| m.state.initialized)?;
            Some((
                m.app_id.clone(),
                bundle,
                m.state.install_ts_ms,
                m.state.timers.clone(),
            ))
        }) else {
            return;
        };
        let Ok(prepared) = self.prepared_for(&bundle).await else {
            return;
        };
        let now = self.clock.now_ms();
        for (timer, method) in &prepared.abi.methods {
            if method.kind != MethodKind::Timer {
                continue;
            }
            let Some(slot) = method
                .every
                .and_then(|every| due_slot(install, every, now))
                .filter(|slot| applied.get(timer).is_none_or(|last| last < slot))
            else {
                continue;
            };
            if let Err(e) = self.tick(g, &app_id, &prepared, timer, slot).await {
                debug!(app = %app_id, timer, slot, error = %e, "tick not proposed");
            }
        }
    }

    /// Run `timer` for `slot` on this node, the app group's leader, and log it.
    /// A tick that times out is not logged (D23): the next poll proposes the
    /// same slot again, unless a later one is due by then.
    async fn tick(
        &self,
        g: &Group<AppMachine>,
        app_id: &str,
        prepared: &Prepared,
        timer: &str,
        slot: u64,
    ) -> Result<(), WriteError> {
        let _p = g
            .propose_led(|m, applied, leadership| {
                if m.tick_applied(timer, slot) {
                    return Proposal::Reply(Vec::new());
                }
                let previous_ms = m.state.timers.get(timer).copied();
                let ts_ms = self.stamp(m);
                let (id_term, id_seq) = self.next_id(g.id(), leadership.term, m.state.last_id);
                let outcome = self.run(
                    prepared,
                    app_id,
                    m.view(),
                    applied,
                    Invocation {
                        kind: InvocationKind::Timer,
                        method: timer.to_owned(),
                        args: Vec::new(),
                        caller: calimero_app_host::Caller::System,
                        entry_ts_ms: ts_ms,
                        id_term,
                        id_seq,
                        seed: calimero_identity::random::<32>(),
                        timer: Some(TimerCtx {
                            scheduled_ms: slot,
                            previous_ms,
                        }),
                    },
                );
                if outcome.is_timeout() {
                    debug!(app = %app_id, timer, slot, "tick timed out; proposed again");
                    return Proposal::Reply(Vec::new());
                }
                let mut cmd = AppCmd::Tick(Box::new(TickEntry {
                    timer: timer.to_owned(),
                    scheduled_ms: slot,
                    previous_ms,
                    ts_ms,
                    base_index: applied,
                    id_term,
                    id_seq,
                    outcome,
                }));
                let mut bytes = serde_json::to_vec(&cmd).unwrap_or_default();
                if bytes.len() > calimero_log::MAX_ENTRY_BYTES {
                    // Too large to log (D50): the tick counts as trapped.
                    if let AppCmd::Tick(entry) = &mut cmd {
                        entry.outcome = Outcome::trap(TrapKind::HostDenied, "entry too large");
                    }
                    bytes = serde_json::to_vec(&cmd).unwrap_or_default();
                }
                Proposal::Append(bytes)
            })
            .await?;
        Ok(())
    }

    /// Run a call on this node, which must be the app group's leader.
    async fn call_local(
        &self,
        g: &Group<AppMachine>,
        req: &CallRequest,
    ) -> Result<Applied, WriteError> {
        if req.method == UPGRADE {
            return self.upgrade_local(g, req).await;
        }
        let bundle = g
            .read_local(|m, _| m.state.bundle.clone())
            .ok_or_else(|| WriteError::Unavailable("the app is not initialized yet".into()))?;
        let prepared = self
            .prepared_for(&bundle)
            .await
            .map_err(|e| WriteError::Unavailable(e.message))?;
        let app = self.sys(|s| s.apps.get(&req.app_id).cloned());
        // The node that took the request checked the caller reaches the app on its
        // own replica, which may not have applied a removal yet: the leader checks
        // again on its replica, and only when that refuses does it wait for a fresh
        // system state (a read index) and check once more, before anything runs.
        if let (Some(app), Some(account)) = (&app, req.caller.account()) {
            if !self.sys(|s| s.may_reach(app, Some(account))) {
                self.sys_fresh().await;
                if !self.sys(|s| s.may_reach(app, Some(account))) {
                    return Ok(Applied::unlogged(
                        g.applied(),
                        Outcome {
                            status: Status::AppError,
                            error: Value::String(NOT_REACHED.into()),
                            ..Outcome::default()
                        },
                    ));
                }
            }
        }
        // A read-only member's call runs, but one that would write is refused
        // (v1 discarded such writes; a client is told).
        let read_only = match &app {
            Some(app) => crate::api::read_only_in(self, app, req.caller.account()).await,
            None => false,
        };
        let args = args_bytes(&req.args);
        let p = g
            .propose_led(|m, applied, leadership| {
                if let Some(key) = &req.idempotency_key {
                    if let Some(prev) = m.replay(&req.caller, key) {
                        return Proposal::Reply(serde_json::to_vec(&prev).unwrap_or_default());
                    }
                }
                let ts_ms = self.stamp(m);
                let (id_term, id_seq) = self.next_id(g.id(), leadership.term, m.state.last_id);
                let outcome = self.run(
                    &prepared,
                    &req.app_id,
                    m.view(),
                    applied,
                    Invocation {
                        kind: InvocationKind::Call,
                        method: req.method.clone(),
                        args: args.clone(),
                        caller: req.caller.to_host(),
                        entry_ts_ms: ts_ms,
                        id_term,
                        id_seq,
                        seed: calimero_identity::random::<32>(),
                        timer: None,
                    },
                );
                // A timeout is not logged and does not spend the key (D23).
                if outcome.is_timeout() {
                    let a = Applied::unlogged(applied, outcome);
                    return Proposal::Reply(serde_json::to_vec(&a).unwrap_or_default());
                }
                if read_only && !outcome.diff.is_empty() {
                    let refused = Applied {
                        index: applied,
                        status: Status::AppError,
                        result: Value::Null,
                        error: Value::String(READ_ONLY.into()),
                        events: Vec::new(),
                        conflict: false,
                        timeout: false,
                    };
                    return Proposal::Reply(serde_json::to_vec(&refused).unwrap_or_default());
                }
                Proposal::Append(Self::entry_bytes(CallEntry {
                    method: req.method.clone(),
                    args: req.args.clone(),
                    caller: req.caller.clone(),
                    idempotency_key: req.idempotency_key.clone(),
                    consent: req.consent.clone(),
                    ts_ms,
                    base_index: applied,
                    id_term,
                    id_seq,
                    outcome,
                }))
            })
            .await?;
        serde_json::from_slice(&p.output).map_err(|e| WriteError::Unavailable(e.to_string()))
    }

    /// Switch an app to a new bundle, code only. A named migration is refused:
    /// migrations are rebuilds (D38), not in this milestone.
    async fn upgrade_local(
        &self,
        g: &Group<AppMachine>,
        req: &CallRequest,
    ) -> Result<Applied, WriteError> {
        let refuse = |code: &str, message: &str| Applied {
            index: g.applied(),
            status: Status::AppError,
            result: Value::Null,
            error: json!({ "code": code, "fields": { "message": message } }),
            events: Vec::new(),
            conflict: false,
            timeout: false,
        };
        if req.args.get("migrate").is_some_and(|m| !m.is_null()) {
            return Ok(refuse(
                "MigrationsNotSupported",
                "migrations are rebuilds (D38), not in this milestone",
            ));
        }
        let bundle = req
            .args
            .get("bundle")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        match self.prepared_for(&bundle).await {
            Ok(_) => {}
            Err(e) if e.code == BAD_BUNDLE => return Ok(refuse(BAD_BUNDLE, &e.message)),
            Err(e) => return Err(WriteError::Unavailable(e.message)),
        }
        let p = g
            .propose(|_, applied| {
                let cmd = AppCmd::Upgrade {
                    bundle: bundle.clone(),
                    base_index: applied,
                };
                Proposal::Append(serde_json::to_vec(&cmd).unwrap_or_default())
            })
            .await?;
        serde_json::from_slice(&p.output).map_err(|e| WriteError::Unavailable(e.to_string()))
    }

    /// The ABI checks the host makes before invoking (S5, D13), from the bundle's
    /// own ABI, default-deny (Ruling 31): a method the ABI does not name is
    /// `UnknownMethod`; `init` and timers are never reached this way (`WrongKind`);
    /// a `confirm` method never runs as a view (`ConfirmNotAView`) and a call to
    /// one needs a consent; only views run as views (`NotAView`); anonymous
    /// callers reach only `public` methods (`401 UNAUTHENTICATED`, Ruling 32: the
    /// platform's refusal, like the access gate's) and the System caller
    /// only `allowSystem` ones (`Forbidden`). `kind = action` never reaches here:
    /// `prepare` refuses it.
    pub async fn abi_gate(
        &self,
        app: &AppRecord,
        method: &str,
        caller: &Caller,
        view: bool,
        consented: bool,
    ) -> Result<(), ApiError> {
        if method == UPGRADE {
            return Ok(());
        }
        let prepared = self.prepared_for(&app.bundle).await?;
        let Some(m) = prepared.abi.methods.get(method) else {
            return Err(app_refusal("UnknownMethod"));
        };
        if !matches!(m.kind, MethodKind::Call | MethodKind::View) {
            return Err(app_refusal("WrongKind"));
        }
        if view && m.confirm {
            return Err(app_refusal("ConfirmNotAView"));
        }
        if view && m.kind != MethodKind::View {
            return Err(app_refusal("NotAView"));
        }
        match caller {
            Caller::Public if !m.public => {
                Err(ApiError::unauthenticated("sign in to call this method"))
            }
            Caller::System if !m.allow_system => Err(app_refusal("Forbidden")),
            _ if m.confirm && !view && !consented => Err(app_refusal("ConfirmRequired")),
            _ => Ok(()),
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
        if args_bytes(&req.args).len() > MAX_ARGS_BYTES {
            return Err(args_too_large());
        }
        let app = self.app(&req.app_id).await?;
        self.abi_gate(&app, &req.method, &req.caller, false, req.consent.is_some())
            .await?;
        let body = serde_json::to_value(req).map_err(|e| ApiError::internal(&e.to_string()))?;
        for _attempt in 0..30 {
            // A conflict (the diff was built on state a leader change replaced)
            // applied nothing and recorded no idempotency key: run it again.
            let target = if let Some(g) = self.replica_group(&app) {
                match self.call_local(&g, req).await {
                    Ok(a) if a.conflict => None,
                    Ok(a) => return Ok(a),
                    Err(e @ WriteError::TooLarge { .. }) => return Err(entry_refused(&e)),
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
        if args_bytes(&req.args).len() > MAX_ARGS_BYTES {
            return Err(args_too_large());
        }
        let app = self.app(&req.app_id).await?;
        self.abi_gate(&app, &req.method, &req.caller, false, req.consent.is_some())
            .await?;
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
            Err(e @ WriteError::TooLarge { .. }) => Err(entry_refused(&e)),
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
        let prepared = self.prepared_for(&bundle).await?;
        // The view holds its version of the state (D46) and runs outside the
        // machine's lock; it sees the last applied entry's time (D19).
        let (state, applied, ts_ms) =
            g.read_local(|m, applied| (m.view(), applied, m.state.last_ts_ms));
        let node = self
            .this
            .lock()
            .ok()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| ApiError::unavailable("the node is shutting down"))?;
        let (app_id, invocation) = (
            req.app_id.clone(),
            Invocation {
                kind: InvocationKind::View,
                method: req.method.clone(),
                args: args_bytes(&req.args),
                caller: req.caller.to_host(),
                entry_ts_ms: ts_ms,
                id_term: 0,
                id_seq: 0,
                seed: [0; 32],
                timer: None,
            },
        );
        // A view may run up to its deadline: off the async workers.
        let out = tokio::task::spawn_blocking(move || {
            node.run(&prepared, &app_id, state, applied, invocation)
        })
        .await
        .map_err(|e| ApiError::internal(&e.to_string()))?;
        Ok(ViewAnswer {
            index: applied,
            timeout: out.is_timeout(),
            status: out.status,
            result: out.result,
            error: out.error,
        })
    }

    /// Run a read-only method. Its writes, if any, are discarded.
    pub async fn view(&self, req: &ViewRequest) -> Result<ViewAnswer, ApiError> {
        let app = self.app(&req.app_id).await?;
        self.abi_gate(&app, &req.method, &req.caller, true, false)
            .await?;
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
            let (app_id, applied, data) = g.read_local(|m, applied| {
                (
                    m.app_id.clone(),
                    applied,
                    calimero_log::Machine::snapshot(m),
                )
            });
            let key = crate::backup::snapshot_key(&self.config.storage_key, &app_id);
            let sealer = calimero_log::store::Sealer::new(&key);
            let sealed = sealer.seal(app_id.as_bytes(), &data)?;
            let path = dir.join(&app_id);
            std::fs::create_dir_all(&path)?;
            let file = path.join(format!("{applied:020}.snap"));
            if !file.exists() {
                std::fs::write(&file, sealed)?;
                written += 1;
            }
        }
        Ok(written)
    }
}

/// How often an app group's leader looks for due timer ticks (D34).
pub const TIMER_POLL: Duration = Duration::from_millis(100);

/// The latest slot of a timer with interval `every`, installed at `install_ms`,
/// that is due at `now_ms`: `install_ms + k · every` for the largest `k ≥ 1` not
/// after `now_ms` (D34). Missed slots collapse into this one.
#[must_use]
pub fn due_slot(install_ms: u64, every: Duration, now_ms: u64) -> Option<u64> {
    let every = u64::try_from(every.as_millis()).ok().filter(|e| *e > 0)?;
    let k = now_ms.checked_sub(install_ms)? / every;
    (k >= 1).then(|| install_ms + k * every)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_due_slot_is_the_latest_one_aligned_to_the_install_time() {
        let every = Duration::from_secs(1);
        assert_eq!(due_slot(10_000, every, 9_000), None, "before install");
        assert_eq!(due_slot(10_000, every, 10_999), None, "no slot yet");
        assert_eq!(due_slot(10_000, every, 11_000), Some(11_000));
        assert_eq!(due_slot(10_000, every, 11_999), Some(11_000));
        // Five missed slots collapse into the latest one.
        assert_eq!(due_slot(10_000, every, 16_500), Some(16_000));
        assert_eq!(due_slot(10_000, Duration::ZERO, 16_500), None);
    }
}
