//! The node: hosts the system group and its app groups, routes calls to each
//! group's leader, and talks to peers over attested sealed channels.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Duration;

use calimero_log::{
    BoxFuture, Engine, Group, GroupId, NodeId, Proposal, RpcRequest, RpcResponse, Transport,
    WriteError,
};
use futures_util::StreamExt;
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
use crate::membership::{grow_voters, PROMOTE_MAX_LAG, SYSTEM_VOTERS};
use crate::services::{AppBlobs, BundleFacts, Search};
use crate::system::{output, SystemCmd, SystemMachine, SystemState};
use crate::types::{now_ms, AppRecord, Caller, Executor as Kind, NodeRecord};
use crate::usage::{Kind as RunKind, Meter, Usage};

mod component;

pub use component::{
    app_refusal, args_bytes, args_too_large, due_slot, timeout_error, Clock, BAD_BUNDLE,
    MAX_ARGS_BYTES, NOT_REACHED, TIMER_POLL,
};

/// The system group's id.
pub const SYSTEM_GROUP: GroupId = 1;

/// [`crate::app::StorageStats`] as a peer sends it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatsWire {
    keys: u64,
    bytes: u64,
    blob_bytes: u64,
    blobs: Vec<(String, u64)>,
}

impl From<StatsWire> for crate::app::StorageStats {
    fn from(w: StatsWire) -> Self {
        Self {
            keys: w.keys,
            bytes: w.bytes,
            blob_bytes: w.blob_bytes,
            blobs: w
                .blobs
                .into_iter()
                .filter_map(|(id, size)| {
                    let raw: [u8; 32] = hex::decode(id).ok()?.try_into().ok()?;
                    Some((raw, size))
                })
                .collect(),
        }
    }
}

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
    /// Storage key K_r from this release's KMS (or a dev key). It only wraps
    /// the cluster key K_c on disk (`cluster_key`), which seals the data.
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
    /// Hex SHA-256 of the ops token, when only that is given
    /// (`--ops-token-sha256`); ignored when `ops_token` is set.
    pub ops_token_sha256: Option<String>,
    /// A plain-HTTP listener serving only `GET /metrics` (what `/ops/metrics`
    /// answers), for a scraper that cannot open the sealed channel. Off by default.
    pub metrics_listen: Option<std::net::SocketAddr>,
    /// Replicas per app.
    pub replicas: usize,
    /// Where encrypted snapshots go (object storage stand-in), if anywhere.
    pub backup_dir: Option<PathBuf>,
    pub raft: calimero_log::Config,
    /// How long a lookup of a system object (`Node::sys_until`) waits for a
    /// write committed elsewhere to reach this replica before answering.
    pub read_wait: Duration,
    /// The mero-tee release and image profile this node runs (TDX only); its
    /// attestation names them so clients can fetch the signed measurements.
    pub tee_release: Option<String>,
    pub tee_profile: Option<String>,
    /// Which TD images a peer's real quote may come from.
    pub peer_measurements: calimero_client::Measurements,
    /// The package registry every node fetches releases from (`--registry-url`).
    pub registry: Option<String>,
    /// Take raw WASM uploads (`POST /bundles`), which carry no publisher
    /// signature. Development only; production installs signed packages.
    pub allow_unsigned_bundles: bool,
    /// Take uploaded `.mpk` releases (`POST /packages`, `/ops/packages`).
    /// Development only: in production every release comes from the registry
    /// (`/packages/install`), so any node that lacks one fetches it there again
    /// and no release lives only on the nodes it was uploaded to.
    pub allow_package_uploads: bool,
    /// The domain app frontends are served under (`--apps-domain`): a
    /// package's at `{label}.{apps_domain}` (`app_host`). `None`: not served.
    pub apps_domain: Option<String>,
    /// Who decides releases, app upgrades and outbound hosts
    /// (`--release-approvals`, `--app-upgrades`, `--outbound-approvals`).
    /// Written to the system state at genesis by the first leader; after that
    /// the state's value holds, whatever a restarted node is given.
    pub governance: crate::system::Governance,
    /// The operator's outbound HTTPS policy (`[outbound]` in `--settings`);
    /// `None`: `http_request` traps in every run.
    pub outbound: Option<Arc<calimero_outbound::OutboundService>>,
    /// Where app time starts relative to this host's clock, in ms (D19), for
    /// component apps. `0` in production; tests move a node's app clock with it.
    pub clock_offset_ms: i64,
    /// The limits of every component call and view; `None` is `Budget::call()`.
    /// Tests only.
    pub budget: Option<calimero_app_host::Budget>,
    /// The operator's Ed25519 verify key, hex (`--operator-key`): recorded at
    /// genesis, and what every operator decision must be signed with.
    pub operator_key: Option<String>,
    /// What this node verified of the KMS replicas its keys came from (the
    /// storage key, and the data disk's when the boot script recorded it).
    /// Reported, bound into the quote, by `GET /attestation?purpose=kms`.
    pub kms_keys: calimero_client::attest::KmsKeys,
}

impl Config {
    /// How this node judges a peer's quote: its transport key on every peer
    /// channel, and a new node's request for the cluster key.
    #[must_use]
    pub fn peer_policy(&self) -> Policy {
        Policy {
            allow_mock: self.allow_mock_peers,
            app_hash: None,
            measurements: self.peer_measurements.clone(),
            allowed_tcb_statuses: Vec::new(),
            // A node trusts debug peers only when it runs a debug image itself.
            allow_debug_profiles: self
                .tee_profile
                .as_deref()
                .is_some_and(calimero_client::attest::is_debug_profile),
        }
    }

    /// The release this node's attestations name.
    #[must_use]
    pub fn attested_release(&self) -> crate::attest::Release {
        crate::attest::Release {
            version: self
                .tee_release
                .clone()
                .unwrap_or_else(|| self.release.clone()),
            profile: match (&self.tee_profile, self.attestation) {
                (_, Mode::Mock) => "mock".into(),
                (Some(p), _) => p.clone(),
                (None, _) => "tdx".into(),
            },
        }
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
    /// Raw input instead of `args` (an event handler's event, an xcall's params).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<calimero_log::Bytes>,
    /// Why the node runs it itself; `None` for a request. Peers pass it along
    /// on `internal/call`, which only peers reach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<Trigger>,
    /// One of the node's own commands ([`HOST_METHODS`]), which the node
    /// issues itself once the API has judged who may. Never set from a
    /// client's request; peers pass it along on `internal/call`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub host: bool,
}

/// The node's own commands, run by [`Node::call`] only for a [`CallRequest::host`]
/// request: they change what the app is (its bundle, its blob marks), so
/// authority over them is the API's (`run_upgrade`, the blob routes), never
/// the app's or a caller's.
pub const HOST_METHODS: [&str; 3] = ["__upgrade", "__blob", "__blob_forget"];

/// Whether a method name is reserved: the node's commands and the runtime's
/// own exports (`__calimero_*`) are not methods a client, an xcall or a
/// follow-up may name.
#[must_use]
pub fn reserved_method(method: &str) -> bool {
    method.starts_with("__")
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
            host: false,
        }
    }

    /// One of the node's own commands, after the API judged `caller` may.
    #[must_use]
    pub fn host(app_id: String, method: &str, args: Value, caller: Caller) -> Self {
        debug_assert!(HOST_METHODS.contains(&method));
        Self {
            host: true,
            ..Self::new(app_id, method.to_owned(), args, caller)
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
    /// A component's view hit its deadline: answered `503 TIMEOUT` (D36).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub timeout: bool,
}

/// A replica move of one app in progress on this node ([`Node::begin_move`]).
pub struct MoveGuard {
    node: Arc<Node>,
    app: String,
}

impl Drop for MoveGuard {
    fn drop(&mut self) {
        let _was = self
            .node
            .moving
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.app);
    }
}

/// The header a node names itself with on a peer request. A claim only: the
/// receiving node believes it once the session's proven key is the one that
/// node's address attests ([`Node::identify_peer`]).
pub const PEER_NODE_HEADER: &str = "x-calimero-node";

/// A peer that called identified, as this node verified it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerIdentity {
    pub node: NodeId,
    /// The release its attestation named.
    pub release: Option<String>,
}

/// A node that signs sessions, as this node attested it: its receipt key and
/// release, from one quote ([`Node::session_issuer`]).
#[derive(Clone, Debug)]
pub struct Issuer {
    pub key: [u8; 32],
    pub release: Option<String>,
    /// When it was attested (Unix seconds).
    pub at: u64,
}

/// How often a token's verification attests its issuer again (a token under
/// another key than the one attested: the issuer's data directory was
/// replaced), or tries one this node holds no key of: at most once per
/// issuer in this many seconds, so tokens naming an unreachable issuer are
/// refused at once instead of each waiting on it.
pub const ISSUER_RETRY_SECS: u64 = 5;

/// How long one attestation of a session issuer may take: a frozen issuer
/// would otherwise hold it for the HTTP client's 30 s.
pub const ISSUER_ATTEST_WAIT: Duration = Duration::from_secs(5);

/// At most this many issuers are attested at once ([`Node::warm_issuers`]).
pub const MAX_CONCURRENT_ISSUER_ATTESTS: usize = 8;

/// How often [`Node::warm_issuers`] runs besides on each system change: a
/// peer that could not be attested is tried again.
pub const ISSUER_WARM_EVERY: Duration = Duration::from_secs(2);

/// Shared node state.
pub struct Node {
    pub config: Config,
    pub engine: Arc<Engine>,
    pub system: Arc<Group<SystemMachine>>,
    apps: RwLock<HashMap<GroupId, Arc<Group<AppMachine>>>>,
    /// Runs the apps whose bundle is a v1 core module (wasmer).
    pub executor: Executor,
    /// Runs the apps whose bundle is a `calimero:app` component (one wasmtime
    /// engine for all of them, spec section 5); compiled artifacts cached under
    /// `<data_dir>/components`, each with a MAC under a key derived from K_c (S8).
    wasm: Arc<calimero_app_engine_wasmtime::WasmtimeEngine>,
    /// Prepared components per bundle hash.
    prepared: Mutex<HashMap<String, calimero_app_host::Prepared>>,
    /// One lock per bundle being prepared, so concurrent first uses wait for one
    /// prepare instead of each compiling it (Ruling 44).
    preparing: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// How many times this node ran the engine's `prepare` (feature `testing`).
    #[cfg(feature = "testing")]
    prepares: std::sync::atomic::AtomicU64,
    /// App time of component apps ([`Clock`]).
    clock: Clock,
    /// Per component app group: the term this node last led it in, and its
    /// execution counter in that term (`id_seq`, D52).
    id_seqs: Mutex<HashMap<GroupId, (u64, u64)>>,
    /// Component app groups whose timer ticks are being run now (one round per
    /// app at a time).
    ticking: Mutex<HashSet<GroupId>>,
    pub transport: Arc<calimero_sealed::SealedTransport>,
    /// Signs receipts for the results this node returns and the sessions it
    /// issues. Made on the node's first start and kept in its data directory
    /// wrapped under K_r (`signing_key`), so a restart keeps it; its public
    /// half is attested (`purpose=receipts`).
    receipt_key: ed25519_dalek::SigningKey,
    peers: Mutex<HashMap<String, Arc<calimero_sealed::client::SealedTransport>>>,
    pub(crate) http: reqwest::Client,
    cluster_secret: String,
    /// K_c (`cluster_key`): what the cluster's data is sealed under.
    cluster_key: [u8; 32],
    /// Bounds the cluster-key releases this node answers at once.
    pub(crate) key_releases: tokio::sync::Semaphore,
    pub challenges: Mutex<HashMap<String, crate::api::Pending>>,
    /// The release each peer's verified attestation named, by address.
    peer_releases: Mutex<HashMap<String, Arc<Mutex<Option<String>>>>>,
    /// Peers that called this node identified, by the transport key they
    /// proved: the node registered at the address that attests that key, and
    /// the release that attestation named ([`Self::identify_peer`]).
    callers: Mutex<HashMap<[u8; 32], PeerIdentity>>,
    /// One attestation of a caller at a time.
    identifying: Arc<tokio::sync::Mutex<()>>,
    /// Attestations of callers under way, by key and claimed node: each runs
    /// in a task of its own, so a caller that hangs up does not abort it.
    identifications: Mutex<HashMap<CallerClaim, Detached<Option<PeerIdentity>>>>,
    /// Channels to peers being opened, by address: each in a task of its own
    /// ([`Self::connected`]).
    connecting: Mutex<HashMap<String, Detached<Result<(), String>>>>,
    /// The issuers of node-signed sessions this node has attested, by node id
    /// ([`Self::session_issuer`]), every peer's ahead of time
    /// ([`Self::warm_issuers`]).
    issuers: Mutex<HashMap<NodeId, Issuer>>,
    /// When a token's verification last attested each issuer
    /// ([`ISSUER_RETRY_SECS`]).
    issuer_tries: Mutex<HashMap<NodeId, u64>>,
    /// Event streams this node is serving (`/apps/{app}/events`).
    pub event_streams: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// Every app's ephemeral presence, in memory only.
    pub presence: crate::presence::Presence,
    /// Release frontends opened for `app_host`.
    pub frontends: crate::app_host::FrontendCache,
    /// Consent requests this node issued, and the facts their pages show.
    pub consent: crate::consent_page::ConsentRequests,
    /// Nodes being added to the system group now.
    joining: Mutex<std::collections::HashSet<NodeId>>,
    /// Apps whose replicas this node is moving now ([`Node::begin_move`]).
    moving: Mutex<HashSet<String>>,
    /// A promotion of learners to system voters is under way.
    promoting: std::sync::atomic::AtomicBool,
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
    /// The system group's read-index rounds, shared by concurrent `sys_fresh`.
    sys_reads: ReadRounds,
    /// Entries this node appended that applied as conflicts (built on state
    /// that changed before they applied) and were run again.
    pub conflicts: std::sync::atomic::AtomicU64,
    /// Per-app usage of the runs this node made, for `/metrics` ([`crate::usage`]).
    pub usage: Usage,
}

/// One read-index round on the system group at a time. A caller may take the
/// answer of a round that began after it arrived, never of one already under
/// way (that round could predate a write the caller must see), so callers
/// that arrive together share the next round instead of each running one.
#[derive(Default)]
struct ReadRounds {
    /// Rounds begun so far.
    begun: std::sync::atomic::AtomicU64,
    /// The last round that finished, and the index it named.
    finished: Mutex<(u64, Option<u64>)>,
    /// Held for the length of a round.
    turn: tokio::sync::Mutex<()>,
}

/// Work that runs to the end in a task of its own, whoever still waits on it.
type Detached<T> = futures_util::future::Shared<futures_util::future::BoxFuture<'static, T>>;

/// A caller's proven transport key and the node it claims to be.
type CallerClaim = ([u8; 32], NodeId);

/// Most detached attestations of one kind under way at once (callers being
/// identified, peer channels being opened); past it the request is refused
/// and its caller tries again.
const MAX_DETACHED: usize = 64;

/// Start `work` in a task of its own, unless `map` holds unfinished work under
/// `key`; either way, the work to wait on. Finished work is dropped from `map`.
fn detach<K, T>(
    map: &Mutex<HashMap<K, Detached<T>>>,
    key: K,
    work: impl FnOnce() -> futures_util::future::BoxFuture<'static, T>,
) -> Option<Detached<T>>
where
    K: std::hash::Hash + Eq,
    T: Clone + Send + Sync + 'static,
{
    use futures_util::FutureExt as _;
    let mut map = map.lock().ok()?;
    map.retain(|_, w| w.peek().is_none());
    if let Some(w) = map.get(&key) {
        return Some(w.clone());
    }
    if map.len() >= MAX_DETACHED {
        return None;
    }
    let w = work().shared();
    drop(tokio::spawn(w.clone()));
    let _prev = map.insert(key, w.clone());
    Some(w)
}

/// Raft RPCs to peers, over their sealed channels.
struct PeerTransport {
    node: Mutex<Weak<Node>>,
}

impl Transport for PeerTransport {
    fn send(
        &self,
        target: NodeId,
        addr: String,
        group: GroupId,
        request: RpcRequest,
    ) -> BoxFuture<Result<RpcResponse, String>> {
        let node = self.node.lock().ok().and_then(|n| n.upgrade());
        Box::pin(async move {
            let node = node.ok_or("node is shutting down")?;
            if !node.may_send(&addr, group).await {
                return Err(format!(
                    "{addr} runs a release the app's owner has not approved; it gets none of the app's data"
                ));
            }
            // The addressee, so a node that answers at `addr` but is not
            // `target` refuses the message (`api::internal_raft`).
            let body = json!({ "group": group, "target": target, "rpc": request });
            let resp = node
                .peer_post(&addr, "internal/raft", &body)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::from_value(resp).map_err(|e| e.to_string())
        })
    }
}

/// What the wasmer path answers a component app that reaches it: its
/// commands (follow-ups, xcalls) are the wasmer executor's; a component's blobs
/// are its own host records (R8) and it has no xcalls until D25.
pub const COMPONENT_REFUSES: &str =
    "this app is a component: the wasmer executor's commands (xcalls, follow-ups) do not reach it";

/// The error of a call a read-only member made that would have written.
pub const READ_ONLY: &str = "read-only in this team: the call would write";

/// How long a forwarded call or read waits for the leader before retrying.
pub const FORWARD_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a system write may take, election and forwarding included, before
/// it answers `503 UNAVAILABLE`. A system group that cannot commit (too few
/// voters answering, or a membership change waiting on members that do not)
/// otherwise left the write waiting until the proxy in front of merod cut the
/// connection at 30 s, so the caller saw a dropped request and not the reason.
pub const SYSTEM_WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// A command the system log refused at apply.
/// The largest blob (an app's file, a package's `.mpk`) a node keeps: it
/// reaches the other nodes base64-encoded inside one sealed request
/// (`calimero_sealed::server::MAX_SEALED_BYTES`), with 1 MiB left for the rest.
pub const MAX_BLOB_BYTES: usize = (calimero_sealed::server::MAX_SEALED_BYTES - (1 << 20)) / 4 * 3;

/// The code of a refusal of new work past a release deadline.
pub const RELEASE_DEADLINE: &str = "RELEASE_DEADLINE";

/// New work refused past a release deadline: `409 RELEASE_DEADLINE`, naming
/// the release to approve and its deadline in `details`.
pub fn stalled(stall: &crate::system::Stall) -> ApiError {
    ApiError::new(409, RELEASE_DEADLINE, &stall.message()).with_details(json!({
        "release": stall.release,
        "after": crate::system::rfc3339(stall.after),
        "afterSecs": stall.after,
    }))
}

/// The code of an install refused where deployments are granted and the
/// installer holds no live deployer grant.
pub const DEPLOY_NOT_GRANTED: &str = "DEPLOY_NOT_GRANTED";

/// An install refused for want of a deployer grant: `403 DEPLOY_NOT_GRANTED`.
pub fn not_a_deployer() -> ApiError {
    ApiError::new(403, DEPLOY_NOT_GRANTED, crate::system::NOT_A_DEPLOYER)
}

pub fn rejected(reason: &str) -> ApiError {
    if reason.starts_with(crate::system::QUOTA_PREFIX) {
        ApiError::new(429, "QUOTA_EXCEEDED", reason)
    } else if reason.starts_with(crate::system::DEADLINE_PREFIX) {
        ApiError::new(409, RELEASE_DEADLINE, reason)
    } else if reason == crate::system::NOT_A_DEPLOYER {
        not_a_deployer()
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
    ///
    /// Everything the cluster shares or keeps across releases is keyed by
    /// `cluster_key` (K_c, `cluster_key::resolve`), not by this release's K_r.
    pub async fn start(config: Config, cluster_key: [u8; 32]) -> eyre::Result<Arc<Self>> {
        std::fs::create_dir_all(&config.data_dir)?;
        let receipt_key =
            crate::signing_key::load_or_create(&config.data_dir, &config.storage_key)?;
        let peer_transport = Arc::new(PeerTransport {
            node: Mutex::new(Weak::new()),
        });
        let engine = Engine::open(
            config.node_id,
            &config.data_dir,
            &cluster_key,
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
        // Compiled components live in the node's data directory, which the host can
        // write: each artifact carries a MAC under a key derived from K_c, so only this
        // cluster's own compilations are ever loaded (S8, Ruling 38).
        let wasm = Arc::new(calimero_app_engine_wasmtime::WasmtimeEngine::new(
            config.data_dir.join("components"),
            &component::component_cache_key(&cluster_key),
        )?);
        let clock = Clock::new(config.clock_offset_ms);
        let secret = hmac::sign(
            &hmac::Key::new(hmac::HMAC_SHA256, &cluster_key),
            format!("calimero.cluster.v1|{}", config.cluster).as_bytes(),
        );
        let node = Arc::new(Self {
            cluster_secret: calimero_identity::b64url(secret.as_ref()),
            cluster_key,
            key_releases: tokio::sync::Semaphore::new(crate::api::MAX_CONCURRENT_KEY_RELEASES),
            config,
            engine: Arc::clone(&engine),
            system,
            apps: RwLock::new(HashMap::new()),
            executor: Executor::default(),
            wasm,
            prepared: Mutex::new(HashMap::new()),
            preparing: Mutex::new(HashMap::new()),
            #[cfg(feature = "testing")]
            prepares: std::sync::atomic::AtomicU64::new(0),
            clock,
            id_seqs: Mutex::new(HashMap::new()),
            ticking: Mutex::new(HashSet::new()),
            transport: sealed,
            receipt_key,
            peers: Mutex::new(HashMap::new()),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()?,
            challenges: Mutex::new(HashMap::new()),
            event_streams: std::sync::Arc::default(),
            presence: crate::presence::Presence::default(),
            frontends: crate::app_host::FrontendCache::default(),
            consent: crate::consent_page::ConsentRequests::default(),
            joining: Mutex::default(),
            moving: Mutex::default(),
            promoting: std::sync::atomic::AtomicBool::new(false),
            peer_releases: Mutex::new(HashMap::new()),
            callers: Mutex::new(HashMap::new()),
            identifying: Arc::default(),
            identifications: Mutex::default(),
            connecting: Mutex::default(),
            issuers: Mutex::new(HashMap::new()),
            issuer_tries: Mutex::new(HashMap::new()),
            sys_reads: ReadRounds::default(),
            conflicts: std::sync::atomic::AtomicU64::new(0),
            usage: Usage::default(),
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

    /// K_c: the key the cluster's data, backups included, is sealed under.
    #[must_use]
    pub const fn cluster_key(&self) -> &[u8; 32] {
        &self.cluster_key
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
        let seen_release = Arc::new(Mutex::new(None));
        if let Ok(mut r) = self.peer_releases.lock() {
            let _prev = r.insert(addr.to_owned(), Arc::clone(&seen_release));
        }
        let attestor = HttpAttestor {
            seen_release,
            policy: self.config.peer_policy(),
        };
        // Identified (Noise IK, with this node's transport key), so the peer
        // knows which node calls and can judge its release.
        let t = Arc::new(calimero_sealed::client::SealedTransport::identified_as(
            url,
            self.http.clone(),
            Arc::new(attestor),
            self.transport.static_key(),
        ));
        let _prev = peers.insert(addr.to_owned(), Arc::clone(&t));
        Ok(t)
    }

    /// The sealed channel to the peer at `addr`, opened first if it is not.
    /// The opening (attesting the peer, then the handshake) runs in a task of
    /// its own: attesting a TDX peer takes about a second, Intel's collateral
    /// included, and openraft drops an RPC after one heartbeat (200 ms). Run
    /// inside the RPC, every attempt was dropped halfway and the next began
    /// again, so the channel never opened (eu-shared-2's system group kept no
    /// leader). Now the attempt outlives the RPC and the next one finds the
    /// channel open; one opening per peer runs at a time.
    async fn connected(
        &self,
        addr: &str,
    ) -> eyre::Result<Arc<calimero_sealed::client::SealedTransport>> {
        use futures_util::FutureExt as _;
        let peer = self.peer(addr)?;
        if peer.is_open() {
            return Ok(peer);
        }
        let opening = detach(&self.connecting, addr.to_owned(), || {
            let peer = Arc::clone(&peer);
            async move { peer.connect().await.map_err(|e| format!("{e:#}")) }.boxed()
        })
        .ok_or_else(|| eyre::eyre!("too many peer channels are being opened"))?;
        opening.await.map_err(|e| eyre::eyre!(e))?;
        Ok(peer)
    }

    /// POST JSON to a peer's internal route over its sealed channel.
    pub async fn peer_post(&self, addr: &str, path: &str, body: &Value) -> eyre::Result<Value> {
        let peer = self.connected(addr).await?;
        let url = calimero_sealed::client::resolve_path(&addr.parse()?, path)?;
        let req = self
            .http
            .post(url)
            .header("x-cluster-auth", &self.cluster_secret)
            .header(PEER_NODE_HEADER, self.config.node_id.to_string())
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
            .header(PEER_NODE_HEADER, self.config.node_id.to_string())
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
        // Long enough to ride out a system leader election, and never longer
        // than `SYSTEM_WRITE_TIMEOUT` in all: bounded by the deadline, not by a
        // count of tries, which a leaderless group used up in about 7.5 s
        // (`a_system_write_waits_for_a_leader_until_its_deadline`).
        let deadline = tokio::time::Instant::now() + SYSTEM_WRITE_TIMEOUT;
        for attempt in 0_u32.. {
            let Ok(written) =
                tokio::time::timeout_at(deadline, self.system.write(bytes.clone())).await
            else {
                return Err(self.system_stalled());
            };
            match written {
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
            if tokio::time::timeout_at(deadline, tokio::time::sleep(Duration::from_millis(150)))
                .await
                .is_err()
            {
                return Err(self.system_stalled());
            }
        }
        Err(self.system_stalled())
    }

    /// The `503` of a system write that did not commit in
    /// [`SYSTEM_WRITE_TIMEOUT`], naming the system group as this node sees it:
    /// its leader, voters and learners, and on the leader the members whose
    /// log is behind. The write may still commit later; nothing here undoes it.
    fn system_stalled(&self) -> ApiError {
        let status = self.system.status();
        let ids = |v: &[NodeId]| {
            v.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut why = format!(
            "the system group did not commit this write in {} s (leader {}, voters [{}], learners [{}]",
            SYSTEM_WRITE_TIMEOUT.as_secs(),
            status.leader.map_or_else(|| "none".to_owned(), |l| l.to_string()),
            ids(&status.voters),
            ids(&status.learners),
        );
        if status.is_leader {
            let metrics = self.system.raft().metrics().borrow().clone();
            let last = metrics.last_log_index.unwrap_or(0);
            let behind: Vec<NodeId> = metrics
                .replication
                .iter()
                .flatten()
                .filter(|(_, log)| log.map_or(0, |l| l.index) < last)
                .map(|(peer, _)| *peer)
                .collect();
            why.push_str(&format!(", behind the leader: [{}]", ids(&behind)));
        }
        why.push_str("); it may still apply once the group can commit");
        warn!(%why, "system write timed out");
        ApiError::unavailable(&why)
    }

    /// The voters and learners of app group `group` as this node's replica
    /// knows them; `None` when it does not host the group.
    #[must_use]
    pub fn group_members(&self, group: GroupId) -> Option<std::collections::BTreeSet<NodeId>> {
        let status = self.app_group(group)?.status();
        Some(
            status
                .voters
                .iter()
                .chain(status.learners.iter())
                .copied()
                .collect(),
        )
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
    /// waits to apply it, at most `Config::read_wait`). Best effort: on no
    /// answer the read goes ahead; the result says whether this replica did
    /// catch up, for a caller that must not answer from a stale one.
    /// Callers that wait at the same moment share one round (`ReadRounds`).
    pub async fn sys_fresh(&self) -> bool {
        use std::sync::atomic::Ordering;

        let rounds = &self.sys_reads;
        let arrived = rounds.begun.load(Ordering::SeqCst);
        let index = {
            let _turn = rounds.turn.lock().await;
            let (round, index) = *rounds
                .finished
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if round > arrived {
                index
            } else {
                let round = rounds.begun.fetch_add(1, Ordering::SeqCst) + 1;
                let index = self.sys_read_index().await;
                *rounds
                    .finished
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = (round, index);
                index
            }
        };
        match index {
            Some(i) => self.system.wait_applied(i, self.config.read_wait).await,
            None => false,
        }
    }

    /// One read-index round: the system leader's applied index, once it has
    /// confirmed it leads. `None` when no leader answers.
    async fn sys_read_index(&self) -> Option<u64> {
        match self.system.read_strong(|_, applied| applied).await {
            Ok(i) => Some(i),
            Err(WriteError::NotLeader {
                leader: Some((id, addr)),
            }) if id != self.config.node_id && !addr.is_empty() => self
                .peer_forward(&addr, "internal/system/index", &json!({}))
                .await
                .ok()
                .and_then(|v| v["index"].as_u64()),
            Err(_) => None,
        }
    }

    /// The release node `id` runs, as its verified attestation names it (this
    /// node's own for itself), attesting the node first if this one has not
    /// yet. `None` when it cannot be attested.
    pub async fn release_of_node(&self, id: NodeId) -> Option<String> {
        if id == self.config.node_id {
            return Some(self.own_release());
        }
        let addr = self.sys(|s| s.nodes.get(&id).map(|n| n.addr.clone()))?;
        if self.release_at(&addr).is_none() {
            // Any sealed request attests it.
            let _ = self
                .peer_post(&addr, "internal/system/index", &json!({}))
                .await;
        }
        self.release_at(&addr)
    }

    /// Which node holds `key`, the transport key a caller proved on an
    /// identified session, claiming to be node `claimed`: the node registered
    /// (or in `--peers`) at an address whose attestation names that key, and
    /// the release that same attestation names. Attested here, with a fresh
    /// nonce, at the address the cluster has for `claimed`; cached by key (a
    /// key lives as long as the process holding it). `None` when the claim
    /// does not hold: no such node, removed, or another key there.
    ///
    /// The attestation runs in a task of its own ([`detach`]): it takes
    /// longer than a Raft RPC may (about a second on TDX), and a caller that
    /// gives up must not abort it, or no RPC from that peer is ever taken.
    pub async fn identify_peer(&self, key: [u8; 32], claimed: NodeId) -> Option<PeerIdentity> {
        use futures_util::FutureExt as _;
        if let Some(p) = self.cached_caller(&key, claimed) {
            return self.is_peer(p.node).then_some(p);
        }
        if !self.is_peer(claimed) {
            return None;
        }
        let addr = self.addr_of(claimed)?;
        let url: Url = addr.parse().ok()?;
        // What the task needs, and the node only weakly: a stopping node must
        // not be kept alive (its store open) by an attestation under way.
        let (http, policy, turn) = (
            self.http.clone(),
            self.config.peer_policy(),
            Arc::clone(&self.identifying),
        );
        let node = self.this.lock().ok()?.clone();
        let identifying = detach(&self.identifications, (key, claimed), || {
            async move {
                let _one = turn.lock().await;
                if let Some(p) = node.upgrade()?.cached_caller(&key, claimed) {
                    return Some(p);
                }
                let release = Arc::new(Mutex::new(None));
                let attestor = HttpAttestor {
                    seen_release: Arc::clone(&release),
                    policy,
                };
                let attested =
                    calimero_sealed::client::Attest::attest_transport_key(&attestor, &http, &url)
                        .await;
                let attested = match attested {
                    Ok(k) => k,
                    Err(e) => {
                        warn!(node = claimed, error = %format!("{e:#}"), "could not attest a calling peer");
                        return None;
                    }
                };
                if attested != key {
                    warn!(
                        node = claimed,
                        "a peer called with a key its node's address does not attest"
                    );
                    return None;
                }
                let identity = PeerIdentity {
                    node: claimed,
                    release: release.lock().ok()?.clone(),
                };
                if let Ok(mut callers) = node.upgrade()?.callers.lock() {
                    // A restarted node has a new key: forget its old one.
                    callers.retain(|_, p| p.node != claimed);
                    let _prev = callers.insert(key, identity.clone());
                }
                Some(identity)
            }
            .boxed()
        })?;
        identifying.await.filter(|p| self.is_peer(p.node))
    }

    fn cached_caller(&self, key: &[u8; 32], claimed: NodeId) -> Option<PeerIdentity> {
        self.callers
            .lock()
            .ok()?
            .get(key)
            .filter(|p| p.node == claimed)
            .cloned()
    }

    /// Whether `id` is a node of this cluster: registered (or in `--peers`)
    /// and not removed, or still a voter or learner of the system group (a
    /// node being removed takes part until its membership change commits).
    #[must_use]
    pub fn is_peer(&self, id: NodeId) -> bool {
        id != self.config.node_id
            && (self.sys(|s| {
                !s.removed_nodes.contains(&id)
                    && (s.nodes.contains_key(&id) || self.config.peers.contains_key(&id))
            }) || self.system.member_addr(id).is_some())
    }

    /// Where node `id` is reached: its registered address, its `--peers`
    /// entry, or the address the system group's membership has for it.
    fn addr_of(&self, id: NodeId) -> Option<String> {
        self.sys(|s| s.nodes.get(&id).map(|n| n.addr.clone()))
            .or_else(|| self.config.peers.get(&id).cloned())
            .or_else(|| self.system.member_addr(id))
    }

    /// The node that issued a session (`claims.node`), as this node attested
    /// it: its receipt key and release, from one receipt-key quote fetched at
    /// the address the cluster has for it (this node itself: its own).
    /// Answered from what this node attested before ([`Self::warm_issuers`]
    /// attests every peer ahead of time), so a session outlives its issuer.
    /// Attested here only when this node holds no key for it, or with
    /// `refresh` (a token under another key), each at most every
    /// [`ISSUER_RETRY_SECS`] and within [`ISSUER_ATTEST_WAIT`]; an attestation
    /// that fails leaves the key held before. `None` for a node that is not
    /// (or no longer) a peer, and for one never attested.
    pub async fn session_issuer(&self, id: NodeId, refresh: bool) -> Option<Issuer> {
        if id == self.config.node_id {
            return Some(Issuer {
                key: self.receipt_public(),
                release: Some(self.own_release()),
                at: crate::types::now_secs(),
            });
        }
        if !self.is_peer(id) {
            // Removed: its sessions stop verifying with the removal.
            if let Ok(mut issuers) = self.issuers.lock() {
                let _gone = issuers.remove(&id);
            }
            return None;
        }
        let known = self.known_issuer(id);
        if known.is_some() && !refresh {
            return known;
        }
        let now = crate::types::now_secs();
        let may_try = self.issuer_tries.lock().is_ok_and(|mut tries| {
            let due = tries
                .get(&id)
                .is_none_or(|at| at + ISSUER_RETRY_SECS <= now);
            if due {
                let _prev = tries.insert(id, now);
            }
            due
        });
        if !may_try {
            return known;
        }
        self.attest_issuer(id).await.or(known)
    }

    /// The issuer `id` as this node last attested it, without reaching it.
    #[must_use]
    pub fn known_issuer(&self, id: NodeId) -> Option<Issuer> {
        self.issuers.lock().ok()?.get(&id).cloned()
    }

    /// Attest peer `id`'s receipt key at the address the cluster has for it,
    /// within [`ISSUER_ATTEST_WAIT`], under the peer policy, and keep it with
    /// the release the same quote names. `None` (and the key held before kept)
    /// when it cannot be attested.
    async fn attest_issuer(&self, id: NodeId) -> Option<Issuer> {
        let addr = self.addr_of(id)?;
        let nonce = calimero_identity::random::<32>();
        let mut url =
            calimero_sealed::client::resolve_path(&addr.parse().ok()?, "attestation").ok()?;
        let _q = url
            .query_pairs_mut()
            .append_pair("nonce", &hex::encode(nonce))
            .append_pair("purpose", "receipts");
        let attested = tokio::time::timeout(ISSUER_ATTEST_WAIT, async {
            let doc: crate::attest::AttestationDoc = self
                .http
                .get(url)
                .send()
                .await
                .ok()?
                .error_for_status()
                .ok()?
                .json()
                .await
                .ok()?;
            let (key, _governance) = crate::attest::verify_receipt_key(
                &doc,
                &nonce,
                &self.config.peer_policy(),
            )
            .await
            .map_err(|e| {
                warn!(node = id, error = %format!("{e:#}"), "could not attest a session issuer");
            })
            .ok()?;
            Some((key, doc.release.version))
        })
        .await
        .ok()
        .flatten();
        let (key, release) = attested?;
        let issuer = Issuer {
            key,
            release: Some(release),
            at: crate::types::now_secs(),
        };
        if let Ok(mut issuers) = self.issuers.lock() {
            let _prev = issuers.insert(id, issuer.clone());
        }
        Some(issuer)
    }

    /// Attest every peer's session-signing key this node holds none of yet
    /// (at most [`MAX_CONCURRENT_ISSUER_ATTESTS`] at once, each within
    /// [`ISSUER_ATTEST_WAIT`]), and drop the keys of nodes that are no longer
    /// peers. Run at start and on every system change (`lib.rs`), so a token is
    /// verified without reaching its issuer, which may be stopped or frozen by
    /// then.
    pub async fn warm_issuers(&self) {
        let candidates: BTreeSet<NodeId> = self
            .sys(|s| s.nodes.keys().copied().collect::<BTreeSet<_>>())
            .into_iter()
            .chain(self.config.peers.keys().copied())
            .collect();
        let peers: BTreeSet<NodeId> = candidates
            .into_iter()
            .filter(|id| self.is_peer(*id))
            .collect();
        let missing: Vec<NodeId> = match self.issuers.lock() {
            Ok(mut issuers) => {
                issuers.retain(|id, _| peers.contains(id));
                peers
                    .iter()
                    .filter(|id| !issuers.contains_key(id))
                    .copied()
                    .collect()
            }
            Err(_) => return,
        };
        futures_util::stream::iter(missing)
            .for_each_concurrent(MAX_CONCURRENT_ISSUER_ATTESTS, |id| async move {
                if self.attest_issuer(id).await.is_none() {
                    debug!(node = id, "session issuer not attested yet");
                }
            })
            .await;
    }

    fn release_at(&self, addr: &str) -> Option<String> {
        let slot = self.peer_releases.lock().ok()?.get(addr).cloned()?;
        let r = slot.lock().ok()?.clone();
        r
    }

    /// Whether the peer at `addr` may be sent `group`'s data: its attested
    /// release must be one the app's owner approved (requirement: no
    /// operator-driven upgrade reaches an app's data without that approval).
    /// The system group goes to every attested peer.
    async fn may_send(&self, addr: &str, group: GroupId) -> bool {
        if group == SYSTEM_GROUP {
            return true;
        }
        let Some(app) = self.sys(|s| s.apps.values().find(|a| a.group == group).cloned()) else {
            return true;
        };
        if self.release_at(addr).is_none() {
            // Not attested yet: any sealed request attests it.
            let _ = self
                .peer_post(addr, "internal/system/index", &json!({}))
                .await;
        }
        let release = self.release_at(addr);
        self.sys(|s| s.release_may_hold(&app, release.as_deref()))
    }

    /// A session token for `claims`, signed with this node's receipt key
    /// (attested with `purpose=receipts`), naming this node as its issuer.
    #[must_use]
    pub fn mint_session(&self, mut claims: calimero_identity::SessionClaims) -> String {
        claims.node = self.config.node_id;
        claims.key = calimero_identity::b64url(&self.receipt_public());
        calimero_identity::mint_session(&self.receipt_key.to_bytes(), &claims)
    }

    /// This node's receipt key (Ed25519 public).
    #[must_use]
    pub fn receipt_public(&self) -> [u8; 32] {
        self.receipt_key.verifying_key().to_bytes()
    }

    /// A receipt for `result`, entry `index` of `app_id`, signed by this node.
    #[must_use]
    pub fn receipt(
        &self,
        app_id: &str,
        index: u64,
        result: &Value,
    ) -> calimero_client::attest::Receipt {
        use calimero_client::attest::Receipt;
        use ed25519_dalek::Signer;
        let result_hash = Receipt::hash_result(result);
        let msg = Receipt::message(app_id, index, &result_hash, self.config.node_id);
        Receipt {
            app: app_id.to_owned(),
            index,
            result_hash,
            node: self.config.node_id,
            key: hex::encode(self.receipt_public()),
            signature: hex::encode(self.receipt_key.sign(&msg).to_bytes()),
        }
    }

    /// The mero-tee release this node runs, as its attestation names it.
    #[must_use]
    pub fn own_release(&self) -> String {
        self.config
            .tee_release
            .clone()
            .unwrap_or_else(|| self.config.release.clone())
    }

    /// Read the system state, waiting up to two seconds for `f` to find
    /// something: a write committed through another node may not have been
    /// applied here yet (read-your-writes across nodes).
    pub async fn sys_until<T>(&self, f: impl Fn(&SystemState) -> Option<T>) -> Option<T> {
        let mut applied = self.system.subscribe_applied();
        let deadline = tokio::time::Instant::now() + self.config.read_wait;
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

    /// Claim `app` for a replica move: `None` while another move of it runs
    /// here. The claim ends when the guard drops.
    #[must_use]
    pub fn begin_move(self: &Arc<Self>, app: &str) -> Option<MoveGuard> {
        let mut moving = self
            .moving
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !moving.insert(app.to_owned()) {
            return None;
        }
        Some(MoveGuard {
            node: Arc::clone(self),
            app: app.to_owned(),
        })
    }

    /// On the system leader: every registered node that is not yet in the
    /// system group joins it as a learner (a node `/ops/nodes` added to a
    /// running cluster), and catches up from the log or a snapshot; then it
    /// knows the directory and can host app replicas. Runs in the background,
    /// one join per node at a time.
    fn admit_registered_nodes(&self) {
        let Some(me) = self.this.lock().ok().and_then(|w| w.upgrade()) else {
            return;
        };
        let status = self.system.status();
        let members: std::collections::BTreeSet<NodeId> = status
            .voters
            .iter()
            .chain(status.learners.iter())
            .copied()
            .collect();
        let newcomers: Vec<(NodeId, String)> = self.sys(|s| {
            s.nodes
                .values()
                .filter(|n| !members.contains(&n.id))
                .map(|n| (n.id, n.addr.clone()))
                .collect()
        });
        for (id, addr) in newcomers {
            let fresh = self
                .joining
                .lock()
                .map(|mut j| j.insert(id))
                .unwrap_or(false);
            if !fresh {
                continue;
            }
            let node = Arc::clone(&me);
            drop(tokio::spawn(async move {
                match node.system.add_learner(id, addr).await {
                    Ok(()) => info!(node = id, "a registered node joined the system group"),
                    Err(e) => warn!(node = id, error = %e, "a registered node could not join yet"),
                }
                if let Ok(mut j) = node.joining.lock() {
                    let _was = j.remove(&id);
                }
            }));
        }
    }

    /// On the system leader: while the system group has fewer than
    /// [`SYSTEM_VOTERS`] voters, promote caught-up registered learners so it
    /// has exactly that many, in one membership change (joint consensus). A
    /// cluster started on one node never passes through 2 voters, which
    /// tolerate no failure and make every write wait on both: it waits until
    /// enough learners exist to reach 3. Never grows past 3. Runs in the
    /// background, one change at a time.
    fn promote_learners(&self) {
        let status = self.system.status();
        if !status.is_leader || status.voters.len() >= SYSTEM_VOTERS {
            return;
        }
        let Some(me) = self.this.lock().ok().and_then(|w| w.upgrade()) else {
            return;
        };
        let metrics = self.system.raft().metrics().borrow().clone();
        let last = metrics.last_log_index.unwrap_or(0);
        let matched: BTreeMap<NodeId, u64> = metrics
            .replication
            .iter()
            .flatten()
            .map(|(peer, log)| (*peer, log.map_or(0, |l| l.index)))
            .collect();
        let (voter_zones, candidates) = self.sys(|s| {
            let zone = |id: &NodeId| s.nodes.get(id).map(|n| n.zone.clone()).unwrap_or_default();
            let voter_zones: Vec<String> = status.voters.iter().map(zone).collect();
            let candidates: Vec<(NodeId, String)> = status
                .learners
                .iter()
                .filter(|l| s.nodes.contains_key(l) && !s.removed_nodes.contains(l))
                .filter(|l| {
                    matched
                        .get(l)
                        .is_some_and(|m| last.saturating_sub(*m) <= PROMOTE_MAX_LAG)
                })
                .map(|l| (*l, zone(l)))
                .collect();
            (voter_zones, candidates)
        });
        let Some(voters) = grow_voters(&status.voters, &voter_zones, &candidates) else {
            return;
        };
        if self
            .promoting
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        drop(tokio::spawn(async move {
            match me.system.change_voters(voters.clone()).await {
                Ok(()) => info!(?voters, "learners became system voters"),
                Err(e) => warn!(error = %e, "could not promote learners to system voters yet"),
            }
            me.promoting
                .store(false, std::sync::atomic::Ordering::SeqCst);
        }));
    }

    /// The operator key this node was configured with, as the state records it.
    fn operator_key(&self) -> Option<crate::proof::OperatorKey> {
        self.config
            .operator_key
            .as_ref()
            .map(|k| crate::proof::OperatorKey {
                key: k.to_ascii_lowercase(),
                cluster: self.config.cluster.clone(),
            })
    }

    /// Bring hosted groups in line with the system state.
    pub async fn reconcile(&self) {
        // The cluster's first leader fixes its governance and registers the
        // configured peers, with genesis. Every later node joins only through
        // the operator's signed `/ops/nodes` (`RegisterNode` carries the
        // operator's signature, checked where it applies).
        if self.system.status().is_leader {
            if self.sys(|s| s.governance.is_none() && s.accounts.is_empty()) {
                let nodes: Vec<NodeRecord> = self
                    .config
                    .peers
                    .iter()
                    .map(|(id, addr)| NodeRecord {
                        id: *id,
                        addr: addr.clone(),
                        zone: String::new(),
                    })
                    .collect();
                let _r = self
                    .sys_write(&SystemCmd::Genesis {
                        governance: self.config.governance.clone(),
                        release: Some(self.own_release()),
                        operator_key: self.operator_key(),
                        nodes,
                    })
                    .await;
            }
            self.admit_registered_nodes();
            self.promote_learners();
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
            // A wasmer app is initialized once it has a bundle; a component app may
            // be given one by an upgrade before its init applied (its init timed
            // out), and still needs its init then, on that bundle.
            let needs_init = g.read_local(|m, _| {
                !m.state.initialized
                    && (m.state.bundle.is_none()
                        || (m.state.executor() == Some(Kind::Component)
                            && m.state.init_error.is_none()))
            });
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
    pub(crate) async fn bundle_wasm(&self, hash: &str) -> Result<Vec<u8>, ApiError> {
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

    /// The outbound client for a writing run of `app_id` on `bundle`: `None`
    /// when the operator turned outbound off, or the bundle is no signed
    /// release declaring a prefix. A declared prefix the owner has not approved
    /// still gets a client, whose refusal (`AWAITING_APPROVAL`, `NOT_APPROVED`)
    /// the app can show.
    fn outbound_for(
        &self,
        app_id: &str,
        bundle: &str,
        sent: Arc<std::sync::atomic::AtomicU64>,
    ) -> Option<Arc<dyn calimero_runtime::logic::OutboundHost>> {
        let service = self.config.outbound.as_ref()?;
        let release = self.release_of(bundle)?;
        let (identity, approval) = self.sys(|s| {
            let signer = s.packages.get(&release.at.package)?.signer_id.clone();
            let approval = s.apps.get(app_id).and_then(|app| {
                s.outbound_approval(app, &signer)
                    .map(|a| calimero_outbound::Approval {
                        prefixes: a.prefixes.clone(),
                        reviewed: a.reviewed.clone(),
                    })
            });
            Some((
                calimero_outbound::AppIdentity {
                    package: release.at.package.clone(),
                    signer,
                },
                approval,
            ))
        })?;
        service.for_run_counted(
            identity,
            release.version.outbound.clone(),
            approval,
            app_id.to_owned(),
            sent,
        )
    }

    /// What a run of `app_id` by `caller` is handed besides its call.
    fn env(&self, app_id: &str, caller: &Caller) -> RunEnv {
        let opener = caller.account().map(|a| {
            Arc::new(calimero_primitives::identity::PrivateKey::from(
                crate::backup::member_key(&self.cluster_key, app_id, a),
            ))
        });
        RunEnv {
            org: self.sys(|s| crate::app::caller_org(s, app_id, caller)),
            blobs: Some(Arc::new(AppBlobs {
                node: self.this.lock().map(|w| w.clone()).unwrap_or_default(),
            })),
            sealing: calimero_runtime::logic::SealingContext {
                opener,
                namespace_sealing_key: Some(self.tee_sealing_key(app_id)),
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
        let tee = PrivateKey::from(crate::backup::tee_key(&self.cluster_key, app_id));
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
            let key = PrivateKey::from(crate::backup::member_key(&self.cluster_key, app_id, a));
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
            namespace_sealing_key: Some(self.tee_sealing_key(app_id)),
        }
    }

    /// The app's TEE key, public half, as `env::tee_sealing_key` reads it:
    /// what a value only the app's TEE may read is sealed to (the
    /// cluster is every app's TEE).
    fn tee_sealing_key(&self, app_id: &str) -> calimero_runtime::logic::SealingKeyResolver {
        let tee = calimero_primitives::identity::PrivateKey::from(crate::backup::tee_key(
            &self.cluster_key,
            app_id,
        ));
        let public = *AsRef::<[u8; 32]>::as_ref(&tee.public_key());
        calimero_runtime::logic::SealingKeyResolver(Arc::new(move || Some(public)))
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
    /// `app` names the app it is a blob of (`None`: code, a package release):
    /// an app's blob goes only to nodes whose release may hold its data.
    pub async fn store_blob(&self, bytes: &[u8], app: Option<&str>) -> Result<String, ApiError> {
        if bytes.len() > MAX_BLOB_BYTES {
            return Err(ApiError::new(
                413,
                "TOO_LARGE",
                &format!(
                    "a blob is limited to {} MiB (what reaches the other nodes)",
                    MAX_BLOB_BYTES >> 20
                ),
            ));
        }
        let id = hex::encode(calimero_identity::sha256(bytes));
        self.engine
            .put_blob(&id, bytes)
            .map_err(|e| ApiError::internal(&e.to_string()))?;
        self.spread_blob(&id, bytes, app).await?;
        Ok(id)
    }

    /// Hand a blob this node holds to every other node that may hold it (for
    /// `app`'s blob, a node whose release `app`'s owner approved, as
    /// [`Self::may_send`] judges a Raft entry); `Ok` once a majority of those
    /// holds it.
    async fn spread_blob(&self, id: &str, bytes: &[u8], app: Option<&str>) -> Result<(), ApiError> {
        let peers: Vec<String> = self.sys(|s| {
            s.nodes
                .values()
                .filter(|n| n.id != self.config.node_id)
                .map(|n| n.addr.clone())
                .collect()
        });
        let group = app.and_then(|a| self.sys(|s| s.apps.get(a).map(|r| r.group)));
        let mut eligible = Vec::with_capacity(peers.len());
        for addr in peers {
            if group.is_none() || self.may_send(&addr, group.unwrap_or(SYSTEM_GROUP)).await {
                eligible.push(addr);
            }
        }
        let body = json!({ "hash": id, "wasm": calimero_log::Bytes(bytes.to_vec()) });
        let results = futures_util::future::join_all(
            eligible
                .iter()
                .map(|addr| self.peer_post(addr, "internal/blob", &body)),
        )
        .await;
        let held = 1 + results.iter().filter(|r| r.is_ok()).count();
        if held * 2 <= eligible.len() + 1 {
            return Err(ApiError::unavailable(
                "the blob could not reach a majority of nodes",
            ));
        }
        Ok(())
    }

    /// Hand `to` (nodes about to become replicas of `app`) the bytes the app
    /// needs that live outside its log: its bundle and every blob its state
    /// marks. They would otherwise be fetched from a peer on first use, and a
    /// cluster grown from one node holds them only on that node: once it is
    /// gone, nobody could run the app. Returns how many it handed over.
    pub async fn hand_over_bytes(
        &self,
        app: &AppRecord,
        g: &Group<AppMachine>,
        to: &[(NodeId, String)],
    ) -> Result<usize, ApiError> {
        if to.is_empty() {
            return Ok(0);
        }
        let bundle = g
            .read_local(|m, _| m.state.bundle.clone())
            .unwrap_or_else(|| app.bundle.clone());
        let wasm = self.bundle_wasm(&bundle).await?;
        self.hand_to(&bundle, wasm, to).await?;
        // One blob in memory at a time: an app may hold many large ones.
        let blobs = g.read_local(|m, _| m.storage_stats().blobs);
        for (id, _size) in &blobs {
            let id = hex::encode(id);
            let bytes = match self
                .engine
                .get_blob(&id)
                .map_err(|e| ApiError::internal(&e.to_string()))?
            {
                Some(b) => b,
                None => self.fetch_blob(&id).await?,
            };
            self.hand_to(&id, bytes, to).await?;
        }
        Ok(1 + blobs.len())
    }

    /// Store `bytes` (named by their hash) on each of `to`.
    async fn hand_to(
        &self,
        hash: &str,
        bytes: Vec<u8>,
        to: &[(NodeId, String)],
    ) -> Result<(), ApiError> {
        let body = json!({ "hash": hash, "wasm": calimero_log::Bytes(bytes) });
        for (id, addr) in to {
            if let Err(e) = self.peer_post(addr, "internal/blob", &body).await {
                return Err(ApiError::unavailable(&format!(
                    "node {id} could not take the app's bytes: {e}"
                )));
            }
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

    /// What `app`'s storage holds, from a replica: this node's if it hosts the
    /// app, else one that does.
    pub async fn storage_stats(
        &self,
        app: &AppRecord,
    ) -> Result<crate::app::StorageStats, ApiError> {
        if let Some(g) = self.hosted(&app.app_id) {
            return Ok(g.read_local(|m, _| m.storage_stats()));
        }
        let body = json!({ "app": app.app_id });
        for id in &app.replicas {
            let Some(addr) = self.sys(|s| s.nodes.get(id).map(|n| n.addr.clone())) else {
                continue;
            };
            if let Ok(v) = self.peer_forward(&addr, "internal/storage", &body).await {
                if let Ok(stats) = serde_json::from_value::<StatsWire>(v) {
                    return Ok(stats.into());
                }
            }
        }
        Err(ApiError::unavailable("no replica answered"))
    }

    /// After an app forgot blob `id`: delete its bytes on every node, unless
    /// another app still marks it or it is a bundle or a package release.
    pub async fn collect_blob(&self, id: &[u8; 32]) {
        let hex_id = hex::encode(id);
        let (code, apps) = self.sys(|s| {
            let code = s.bundles.contains_key(&hex_id)
                || s.packages
                    .values()
                    .any(|p| p.versions.values().any(|v| v.mpk == hex_id));
            (code, s.apps.values().cloned().collect::<Vec<_>>())
        });
        if code {
            return;
        }
        for app in &apps {
            // Read on each app's leader: a lagging replica could miss a mark
            // committed a moment ago (the same content uploaded to another
            // app). Unknown counts as held: never delete what an app may need.
            if self.blob_held_strong(app, id).await.unwrap_or(true) {
                return;
            }
        }
        if let Err(e) = self.engine.delete_blob(&hex_id) {
            warn!(blob = %hex_id, error = %e, "blob not deleted");
        }
        let peers: Vec<String> = self.sys(|s| {
            s.nodes
                .values()
                .filter(|n| n.id != self.config.node_id)
                .map(|n| n.addr.clone())
                .collect()
        });
        let body = json!({ "hash": hex_id });
        let _r = futures_util::future::join_all(
            peers
                .iter()
                .map(|addr| self.peer_post(addr, "internal/blob/delete", &body)),
        )
        .await;
    }

    /// Whether `id` is one of `app`'s blobs. A replica that has not applied
    /// the upload yet says no, so the others are asked too.
    /// [`Self::blob_held`] as `app`'s leader answers it (a linearizable
    /// read); `None` when no replica could answer as leader.
    pub async fn blob_held_strong(&self, app: &AppRecord, id: &[u8; 32]) -> Option<bool> {
        if let Some(g) = self.replica_group(app) {
            if let Ok(held) = g.read_strong(|m, _| m.holds_blob(id)).await {
                return Some(held);
            }
        }
        let others: Vec<String> = self.sys(|s| {
            app.replicas
                .iter()
                .filter(|r| **r != self.config.node_id)
                .filter_map(|r| s.nodes.get(r).map(|n| n.addr.clone()))
                .collect()
        });
        let body = json!({ "app": app.app_id, "id": hex::encode(id), "strong": true });
        for addr in others {
            if let Ok(v) = self.peer_forward(&addr, "internal/blob/held", &body).await {
                if let Some(held) = v["held"].as_bool() {
                    return Some(held);
                }
            }
        }
        None
    }

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
        self.usage.learn(bundle, &wasm);
        self.executor
            .module(bundle, &wasm)
            .map_err(|e| ApiError::bad_request("BAD_BUNDLE", &e.to_string()))
    }

    async fn init_app(&self, g: &Group<AppMachine>, app: &AppRecord) -> Result<Applied, ApiError> {
        if app.executor == Kind::Component {
            return self.init_component(g, app).await;
        }
        let module = self.module_for(&app.bundle).await?;
        let owner = Caller::Account {
            account: app.owner.clone(),
        };
        let bundle = app.bundle.clone();
        let app_id = app.app_id.clone();
        let init_args = app.init_args.clone();
        let env = self.env(&app.app_id, &owner);
        let meter = self.meter(&app.app_id, &app.bundle);
        let p = g
            .propose(|m, _| {
                if m.state.initialized || m.state.bundle.is_some() {
                    return Proposal::Reply(Vec::new());
                }
                let outcome = self.metered(&meter, || {
                    Executor::run(
                        &module,
                        &app_id,
                        &owner,
                        "init",
                        &init_args,
                        &m.state.kv,
                        &env,
                    )
                });
                self.keep_blobs(&outcome);
                let cmd = AppCmd::Init {
                    bundle: bundle.clone(),
                    base_index: m.state.last_index,
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
        let applied: Applied =
            serde_json::from_slice(&p.output).map_err(|e| ApiError::internal(&e.to_string()))?;
        self.count_conflict(&applied);
        if !applied.conflict {
            self.usage.called(&meter, RunKind::Mutation);
        }
        Ok(applied)
    }

    /// Where a run of `app_id` on `bundle` is billed ([`crate::usage`]). Read
    /// before the run's proposal, outside the app group's locks.
    pub(crate) fn meter(&self, app_id: &str, bundle: &str) -> Meter {
        Meter {
            app: app_id.to_owned(),
            payer: self
                .sys(|s| s.apps.get(app_id).map(crate::usage::payer))
                .unwrap_or_default(),
            bundle: bundle.to_owned(),
        }
    }

    /// Run `f`, one WASM run, off the async workers ([`off_workers`]), and add
    /// the CPU and memory it took to `meter`'s app: on the thread that runs
    /// it, on this node, whatever becomes of its result.
    pub(crate) fn metered<R>(&self, meter: &Meter, f: impl FnOnce() -> R) -> R {
        let (out, run) = off_workers(|| crate::usage::measure(f));
        self.usage.ran(meter, run);
        out
    }

    /// Sample the state and blob bytes of every app hosted here
    /// (`calimero_app_state_bytes`, `calimero_app_blob_bytes`). Each app's scan
    /// holds its group's read lock, as `GET /apps/{app}/storage` does.
    pub fn sample_usage(&self) {
        let hosted: Vec<(String, String)> = self.sys(|s| {
            s.apps
                .values()
                .map(|a| (a.app_id.clone(), crate::usage::payer(a)))
                .collect()
        });
        for (app, payer) in hosted {
            let Some(g) = self.hosted(&app) else {
                continue;
            };
            let stats = g.read_local(|m, _| m.storage_stats());
            self.usage
                .stored(&app, &payer, stats.bytes, stats.blob_bytes);
        }
    }

    /// Count an appended entry that applied as a conflict.
    fn count_conflict(&self, applied: &Applied) {
        if applied.conflict {
            let _prev = self
                .conflicts
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
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

    /// Apply a presence update here and pass it to every other node; it
    /// does not wait for them (presence is best effort, and refreshed).
    pub fn publish_presence(self: &Arc<Self>, u: crate::presence::Update) {
        if !self.presence.apply(&u, crate::types::now_ms()).spreads() {
            return;
        }
        let peers: Vec<String> = self.sys(|s| {
            s.nodes
                .values()
                .filter(|n| n.id != self.config.node_id)
                .map(|n| n.addr.clone())
                .collect()
        });
        let node = Arc::clone(self);
        let body = serde_json::to_value(&u).unwrap_or_default();
        let group = self.sys(|s| s.apps.get(&u.app).map(|a| a.group));
        drop(tokio::spawn(async move {
            // Only to nodes whose release may hold the app's data.
            let mut eligible = Vec::with_capacity(peers.len());
            for addr in peers {
                if let Some(g) = group {
                    if node.may_send(&addr, g).await {
                        eligible.push(addr);
                    }
                }
            }
            let _r = futures_util::future::join_all(
                eligible
                    .iter()
                    .map(|addr| node.peer_post(addr, "internal/presence", &body)),
            )
            .await;
        }));
    }

    /// After a run's entry committed: its blobs to the other nodes.
    async fn spread_blobs(&self, app: &str, blobs: Vec<(String, Vec<u8>)>) {
        for (id, bytes) in blobs {
            if let Err(e) = self.spread_blob(&id, &bytes, Some(app)).await {
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
        if req.host && req.method == "__upgrade" {
            return self.upgrade_local(g, req).await;
        }
        // Everything below runs a wasmer app's code or writes its keyspace; a
        // component's calls are `call_component`'s, and its blobs its own host records.
        let component = g.read_local(|m, _| m.state.executor()) == Some(Kind::Component);
        if component && req.host && matches!(req.method.as_str(), "__blob" | "__blob_forget") {
            return self.component_blob_cmd(g, req).await;
        }
        if component {
            return Ok(Applied {
                index: g.applied(),
                status: Status::AppError,
                result: Value::Null,
                error: Value::String(COMPONENT_REFUSES.into()),
                events: Vec::new(),
                conflict: false,
            });
        }
        let bundle = g.read_local(|m, _| m.state.bundle.clone());
        let module = match bundle.as_deref() {
            Some(b) => self.module_for(b).await.ok(),
            None => None,
        };
        let now = now_ms();
        if req.host && req.method == "__blob_forget" {
            let cmd = AppCmd::BlobForget {
                id: req.args["id"].as_str().unwrap_or_default().to_owned(),
            };
            let bytes = serde_json::to_vec(&cmd).unwrap_or_default();
            let (_index, out) = g.write(bytes).await?;
            return serde_json::from_slice(&out)
                .map_err(|e| WriteError::Unavailable(e.to_string()));
        }
        if req.host && req.method == "__blob" {
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
            // The API refuses these first; an xcall or follow-up naming one ends here.
            _ if reserved_method(&req.method) => Some(format!("{} is not a method", req.method)),
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
        // A call writes: it may call out, under what its app declares and its
        // owner approved. Views, inits, migrations and a read-only member's
        // calls get no client.
        let meter = self.meter(&req.app_id, bundle.as_deref().unwrap_or_default());
        if !read_only {
            env.outbound = bundle.as_deref().and_then(|b| {
                self.outbound_for(
                    &req.app_id,
                    b,
                    self.usage.outbound(&meter.app, &meter.payer),
                )
            });
        }
        let mut fresh_blobs = Vec::new();
        let mut ran = false;
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
                // The module (and the facts and outbound client) came from the
                // bundle read before the lock; an upgrade may have applied since.
                if let Some(stale) = m.stale_module(bundle.as_deref()) {
                    return Proposal::Reply(serde_json::to_vec(&stale).unwrap_or_default());
                }
                let outcome = match (&module, &refused) {
                    (_, Some(why)) => Outcome {
                        status: Status::AppError,
                        error: Value::String(why.clone()),
                        ..Outcome::default()
                    },
                    (Some(module), None) => {
                        ran = true;
                        let (outcome, _ret) = self.metered(&meter, || {
                            Executor::run_bytes(
                                module,
                                &req.app_id,
                                &req.caller,
                                &req.method,
                                &input,
                                &m.state.kv,
                                &env,
                            )
                        });
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
                    // The app's last entry, not the log's applied index: blank and
                    // membership entries change no app state, and `apply` compares
                    // with the last entry the app applied (`AppState::last_index`).
                    base_index: m.state.last_index,
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
        if p.appended {
            self.count_conflict(&applied);
        }
        // Counted once, by the run whose answer stood: a conflict runs again.
        if ran && !applied.conflict {
            let kind = match req.trigger {
                None => RunKind::Mutation,
                Some(_) => RunKind::FollowUp,
            };
            self.usage.called(&meter, kind);
        }
        if p.appended && !applied.conflict {
            self.kick.notify_one();
            self.spread_blobs(&req.app_id, fresh_blobs).await;
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
        // An upgrade never moves an app between executors (either way, state or
        // none) until rebuild migrations (D38) exist: the two keep different
        // state formats. Judged from the target's WASM against the executor
        // the app's state names, or, before its `init` applied (none named yet),
        // the one the replicated app record names (decided once, at creation).
        let recorded = self
            .sys(|s| s.apps.get(&req.app_id).map(|a| a.executor))
            .unwrap_or(Kind::Wasmer);
        let from = g.read_local(|m, _| m.state.executor()).unwrap_or(recorded);
        let to = self
            .executor_of(&bundle)
            .await
            .map_err(|e| WriteError::Unavailable(e.message))?;
        if from != to {
            return Ok(Applied {
                index: g.applied(),
                status: Status::AppError,
                result: Value::Null,
                error: Value::String(format!(
                    "upgrade refused: {} runs on {}, the app on {}; an upgrade across executors waits for rebuild migrations (D38)",
                    bundle,
                    executor_name(to),
                    executor_name(from),
                )),
                events: Vec::new(),
                conflict: false,
            });
        }
        if from == Kind::Component {
            return self.upgrade_component(g, &bundle, migrate.as_deref()).await;
        }
        let module = self
            .module_for(&bundle)
            .await
            .map_err(|e| WriteError::Unavailable(e.message))?;
        // The bundle the migration is planned from; the build checks it is
        // still the state's, as a call's does.
        let current = g.read_local(|m, _| m.state.bundle.clone());
        // Without a named migration, plan it from the two ABIs.
        if migrate.is_none() {
            if let Some(current) = current.clone().filter(|c| *c != bundle) {
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
        let meter = self.meter(&req.app_id, &bundle);
        let mut ran = false;
        let p = g
            .propose(|m, _| {
                if let Some(stale) = m.stale_module(current.as_deref()) {
                    return Proposal::Reply(serde_json::to_vec(&stale).unwrap_or_default());
                }
                let outcome = match &migrate {
                    Some(method) => {
                        ran = true;
                        let outcome = self.metered(&meter, || {
                            Executor::migrate(
                                &module,
                                &req.app_id,
                                &req.caller,
                                method,
                                &m.state.kv,
                                &env,
                            )
                        });
                        self.keep_blobs(&outcome);
                        outcome
                    }
                    None => Outcome::default(),
                };
                let cmd = AppCmd::Upgrade {
                    bundle: bundle.clone(),
                    base_index: m.state.last_index,
                    outcome,
                };
                Proposal::Append(serde_json::to_vec(&cmd).unwrap_or_default())
            })
            .await?;
        let applied: Applied = serde_json::from_slice(&p.output)
            .map_err(|e| WriteError::Unavailable(e.to_string()))?;
        if p.appended {
            self.count_conflict(&applied);
        }
        if ran && !applied.conflict {
            self.usage.called(&meter, RunKind::Mutation);
        }
        Ok(applied)
    }

    /// Why an `xcall` from app `origin` may not run `method` of app `target`,
    /// if it may not: the method must be an `#[app::xcall]` entry
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
        let (bundle, initialized, executor) = g.read_local(|m, _| {
            (
                m.state.bundle.clone(),
                m.state.initialized,
                m.state.executor(),
            )
        });
        // A component's timers are `tick_timers`'s; it has no follow-ups.
        let (Some(bundle), true, Some(Kind::Wasmer)) = (bundle, initialized, executor) else {
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
        // A component's calls are `call_component`'s (the API routes them there);
        // what reaches here for one is a wasmer app's xcall or follow-up, which
        // a component does not take (D25).
        if app.executor == Kind::Component && !req.host {
            return Err(ApiError::new(409, "NOT_SUPPORTED", COMPONENT_REFUSES));
        }
        // The entry carries the arguments, so arguments past the cap never
        // fit: refuse here rather than forward a megabyte to the leader first.
        let args_len = serde_json::to_vec(&req.args).map_or(0, |a| a.len())
            + req.input.as_ref().map_or(0, |i| i.0.len());
        if args_len > calimero_log::MAX_ENTRY_BYTES {
            return Err(status_of(&WriteError::TooLarge {
                size: args_len,
                max: calimero_log::MAX_ENTRY_BYTES,
            }));
        }
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
                    // No leader will take it either: answer now, as a forward does.
                    Err(e @ WriteError::TooLarge { .. }) => return Err(status_of(&e)),
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

    /// Answer an internal call (from a peer), for either executor: run it here,
    /// without forwarding further. The node's own commands (`req.host`) are the
    /// node's whatever the executor.
    pub async fn call_here_any(&self, req: &CallRequest) -> Result<Value, ApiError> {
        let app = self.app(&req.app_id).await?;
        if app.executor == Kind::Component && !req.host {
            let a = self.call_here_component(&app, req).await?;
            return serde_json::to_value(a).map_err(|e| ApiError::internal(&e.to_string()));
        }
        let a = self.call_here(req).await?;
        serde_json::to_value(a).map_err(|e| ApiError::internal(&e.to_string()))
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
        if g.read_local(|m, _| m.state.executor()) == Some(Kind::Component) {
            return self.view_local_component(g, req).await;
        }
        let bundle = g
            .read_local(|m, _| m.state.bundle.clone())
            .ok_or_else(|| ApiError::unavailable("the app is not initialized yet"))?;
        let module = self.module_for(&bundle).await?;
        let mut env = self.env(&req.app_id, &req.caller);
        let meter = self.meter(&req.app_id, &bundle);
        let answer = g.read_local(|m, applied| {
            // Search answers from indexes brought up to this very state.
            env.search = self.search.for_view(&module, &bundle, m, applied);
            let out = self.metered(&meter, || {
                Executor::run(
                    &module,
                    &req.app_id,
                    &req.caller,
                    &req.method,
                    &req.args,
                    &m.state.kv,
                    &env,
                )
            });
            ViewAnswer {
                index: applied,
                status: out.status,
                result: out.result,
                error: out.error,
                timeout: false,
            }
        });
        self.usage.called(&meter, RunKind::View);
        Ok(answer)
    }

    /// Run a read-only method. Its writes, if any, are discarded.
    pub async fn view(&self, req: &ViewRequest) -> Result<ViewAnswer, ApiError> {
        let app = self.app(&req.app_id).await?;
        if app.executor == Kind::Component {
            self.abi_gate(&app, &req.method, &req.caller, true, false)
                .await?;
        }
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

    #[must_use]
    /// Pick replicas for a new app among the nodes whose release is in
    /// `approved` (when the app's owner restricted releases) and not
    /// [retired](crate::system::SystemState::retired) at `now`: spread over
    /// as many zones as those nodes are in, least loaded first
    /// ([`crate::placement::replicas`]).
    pub async fn place(
        &self,
        approved: Option<&std::collections::BTreeSet<String>>,
        now: u64,
    ) -> Vec<NodeId> {
        // Past a release's deadline, a node on an older one keeps its apps
        // but takes no new one.
        let deadlines = self.sys(|s| s.any_deadline_passed(now));
        let mut allowed = Vec::new();
        for id in self.sys(|s| s.nodes.keys().copied().collect::<Vec<_>>()) {
            let ok = match (approved, deadlines) {
                (None, false) => true,
                (approved, _) => self.release_of_node(id).await.is_some_and(|r| {
                    approved.is_none_or(|set| set.contains(&r)) && !self.sys(|s| s.retired(&r, now))
                }),
            };
            if ok {
                allowed.push(id);
            }
        }
        self.sys(|s| crate::placement::replicas(s, &allowed, self.config.replicas))
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

/// The executor's name, as an error names it.
const fn executor_name(e: Kind) -> &'static str {
    match e {
        Kind::Wasmer => "wasmer (a core module)",
        Kind::Component => "wasmtime (a component)",
    }
}

/// Runs `f`, a WASM run that may take up to its budget, without holding an
/// async worker: on the multi-threaded runtime, `block_in_place` hands this
/// worker's queued tasks to another worker for the run, so Raft heartbeats,
/// the system group and HTTP keep going beside it. The run cannot move to
/// `spawn_blocking` itself: it builds its proposal on the machine's read lock,
/// under the group's proposal lock, from borrowed state. Elsewhere (a
/// current-thread runtime, which `block_in_place` refuses) `f` runs in place.
fn off_workers<R>(f: impl FnOnce() -> R) -> R {
    match tokio::runtime::Handle::try_current() {
        Ok(h) if h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(f)
        }
        _ => f(),
    }
}

#[cfg(test)]
mod off_workers_tests {
    use std::time::{Duration, Instant};

    use super::off_workers;

    /// A run that holds its thread for 300 ms leaves the one async worker
    /// free: a task ticking every 10 ms never waits much longer than that.
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn a_long_run_does_not_hold_an_async_worker() {
        let ticker = tokio::spawn(async {
            let mut worst = Duration::ZERO;
            let mut last = Instant::now();
            for _ in 0..40 {
                tokio::time::sleep(Duration::from_millis(10)).await;
                worst = worst.max(last.elapsed());
                last = Instant::now();
            }
            worst
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        // On the worker, as a request handler's run is (the test's own body
        // runs on the thread that blocks on it, not on a worker).
        tokio::spawn(async { off_workers(|| std::thread::sleep(Duration::from_millis(300))) })
            .await
            .unwrap();
        let worst = ticker.await.unwrap();
        assert!(
            worst < Duration::from_millis(150),
            "a task waited {worst:?}"
        );
    }

    #[test]
    fn outside_a_runtime_it_runs_in_place() {
        assert_eq!(off_workers(|| 7), 7);
    }
}
