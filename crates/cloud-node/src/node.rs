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

use crate::app::{AppCmd, AppMachine, Applied, CallEntry, Executor, Status};
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
    /// Serializes starting app groups (reconcile and the RPC factory race otherwise).
    starting: tokio::sync::Mutex<()>,
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
            starting: tokio::sync::Mutex::new(()),
        });
        *peer_transport.node.lock().map_err(|e| eyre::eyre!("{e}"))? = Arc::downgrade(&node);

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
                    return output(&out).map_err(|e| ApiError::bad_request("REJECTED", &e));
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
            if self.sys(|s| s.session_keys.is_empty()) {
                let key = calimero_identity::b64url(&calimero_identity::random::<32>());
                let _r = self
                    .sys_write(&SystemCmd::AddSessionKey { version: 1, key })
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
        let p = g
            .propose(|m, applied| {
                if m.state.initialized || m.state.bundle.is_some() {
                    return Proposal::Reply(Vec::new());
                }
                let outcome =
                    Executor::run(&module, &app_id, &owner, "init", &Value::Null, &m.state.kv);
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
        serde_json::from_slice(&p.output).map_err(|e| ApiError::internal(&e.to_string()))
    }

    /// Run a call on this node, which must be the app group's leader.
    async fn call_local(
        &self,
        g: &Group<AppMachine>,
        req: &CallRequest,
    ) -> Result<Applied, WriteError> {
        let bundle = g.read_local(|m, _| m.state.bundle.clone());
        let module = match bundle.as_deref() {
            Some(b) => self.module_for(b).await.ok(),
            None => None,
        };
        let now = now_ms();
        if req.method == "__upgrade" {
            return self.upgrade_local(g, req).await;
        }
        let p = g
            .propose(|m, applied| {
                if let Some(key) = &req.idempotency_key {
                    if let Some(prev) = m.replay(&req.caller, key, now) {
                        return Proposal::Reply(serde_json::to_vec(&prev).unwrap_or_default());
                    }
                }
                let outcome = match &module {
                    Some(module) => Executor::run(
                        module,
                        &req.app_id,
                        &req.caller,
                        &req.method,
                        &req.args,
                        &m.state.kv,
                    ),
                    None => crate::app::Outcome {
                        status: Status::Trap,
                        result: Value::Null,
                        error: Value::String("the app is not initialized yet".into()),
                        diff: Vec::new(),
                        events: Vec::new(),
                        logs: Vec::new(),
                    },
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
                }));
                Proposal::Append(serde_json::to_vec(&cmd).unwrap_or_default())
            })
            .await?;
        serde_json::from_slice(&p.output).map_err(|e| WriteError::Unavailable(e.to_string()))
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
        let migrate = req
            .args
            .get("migrate")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let module = self
            .module_for(&bundle)
            .await
            .map_err(|e| WriteError::Unavailable(e.message))?;
        let p = g
            .propose(|m, applied| {
                let outcome = match &migrate {
                    Some(method) => Executor::run(
                        &module,
                        &req.app_id,
                        &req.caller,
                        method,
                        &Value::Null,
                        &m.state.kv,
                    ),
                    None => crate::app::Outcome {
                        status: Status::Ok,
                        result: Value::Null,
                        error: Value::Null,
                        diff: Vec::new(),
                        events: Vec::new(),
                        logs: Vec::new(),
                    },
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

    /// Run a call wherever the app's leader is.
    pub async fn call(&self, req: &CallRequest) -> Result<Applied, ApiError> {
        let app = self.app(&req.app_id).await?;
        let body = serde_json::to_value(req).map_err(|e| ApiError::internal(&e.to_string()))?;
        for _attempt in 0..30 {
            let target = if let Some(g) = self.app_group(app.group) {
                match self.call_local(&g, req).await {
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
                self.sys(|s| {
                    app.replicas
                        .iter()
                        .find_map(|r| s.nodes.get(r).map(|n| n.addr.clone()))
                })
            };
            if let Some(addr) = target {
                match self.peer_post(&addr, "internal/call", &body).await {
                    Ok(v) => {
                        return serde_json::from_value(v)
                            .map_err(|e| ApiError::internal(&e.to_string()))
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
                    .peer_post(&addr, "internal/call", &body)
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
        Ok(g.read_local(|m, applied| {
            let out = Executor::run(
                &module,
                &req.app_id,
                &req.caller,
                &req.method,
                &req.args,
                &m.state.kv,
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
        let app = self.app(&req.app_id).await?;
        if let Some(g) = self.app_group(app.group) {
            if req.strong {
                match g.read_strong(|_, applied| applied).await {
                    Ok(_) => return self.view_local(&g, req).await,
                    Err(WriteError::NotLeader {
                        leader: Some((id, addr)),
                    }) if id != self.config.node_id && !addr.is_empty() => {
                        let v = self
                            .peer_post(
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
                return Err(ApiError::unavailable(
                    "this replica has not caught up to minIndex",
                ));
            }
            return self.view_local(&g, req).await;
        }
        let addr = self
            .sys(|s| {
                app.replicas
                    .iter()
                    .find_map(|r| s.nodes.get(r).map(|n| n.addr.clone()))
            })
            .ok_or_else(|| ApiError::unavailable("no replica known"))?;
        let v = self
            .peer_post(
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
