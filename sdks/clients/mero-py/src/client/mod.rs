//! `SealedClient` (`calimero_client_py._native`): the attested sealed
//! channel to the nodes of a Calimero 1.0 cluster, under the Python client.
//!
//! The routes are a client generated from core's OpenAPI document; the
//! Python package (`calimero_client_py`) runs it on [`request`]
//! (`SealedClient.request`) through an httpx transport, and answers the
//! node's step-ups (`412 CONFIRM_REQUIRED`, `428 OPERATOR_SIGNATURE_REQUIRED`)
//! there. What stays in Rust is what a generated client cannot be made of:
//! the attestation and the sealed channel ([`sealed`]), the recovery that
//! moves between nodes and refreshes a session ([`recover`], [`directory`]),
//! the event streams ([`streams`]), and the keys the client signs with
//! ([`account`], [`account_recovery`], [`org_recovery`], the operator key).
//! Every method blocks with the GIL released.
//!
//! [`request`]: PyCloudClient::request

mod account;
mod account_recovery;
mod directory;
mod org_recovery;
mod prelude;
mod recover;
mod sealed;
mod streams;
mod tls;

use std::collections::HashMap;

pub(crate) use directory::DirectoryError;

use crate::error::directory_err;
use prelude::*;
use recover::Nodes;
use sealed::{as_response, recoverable, RawRequest, RawResponse, SealedNode};
use tls::ExtraRoots;

/// The attested sealed channel to one node of a Calimero 1.0 cluster (and,
/// given `nodes`, the others it may move to).
#[pyclass(name = "SealedClient", module = "calimero_client_py._native")]
pub struct PyCloudClient {
    /// The nodes this client may use, the one in use, and the credential
    /// (the API key, the ops token for the Ops API, which a request sent on
    /// to the leader carries too). See [`recover`].
    nodes: Arc<Nodes<SealedNode>>,
    runtime: Arc<Runtime>,
    /// The trust policy this client was made with, so a request that must
    /// run on a leader (`NOT_LEADER`) is sent to a node attested the same way.
    policy: Policy,
    /// The roots trusted for TLS on top of the public ones (`ca_bundle`), for
    /// every node, leader and directory this client reaches.
    roots: ExtraRoots,
    /// The operator's Ed25519 key (`use_operator_key`): the Ops API's
    /// decisions answer `428 OPERATOR_SIGNATURE_REQUIRED` with it.
    operator_key: Arc<std::sync::Mutex<Option<OperatorKey>>>,
    /// Leaders a `421 NOT_LEADER` named (`request_at`), attested under
    /// `policy` once and kept.
    leaders: std::sync::Mutex<HashMap<String, Arc<SealedNode>>>,
}

/// The operator's signing key, as the node's `--operator-key` names its
/// verify half.
pub(crate) type OperatorKey = ed25519_dalek::SigningKey;

/// An operator key from its 32-byte seed, hex.
pub(crate) fn operator_key_from_hex(seed: &str) -> PyResult<OperatorKey> {
    hex::decode(seed.trim())
        .ok()
        .and_then(|v| <[u8; 32]>::try_from(v).ok())
        .map(|seed| OperatorKey::from_bytes(&seed))
        .ok_or_else(|| PyValueError::new_err("operator_key must be a 32-byte Ed25519 seed, hex"))
}

/// How often a request made with `retry_unavailable` is sent again to the
/// same node while it answers `503` or cannot be reached, and how long it
/// waits first: 200 ms, doubling, at most 2 s (about 9 s in all, past a
/// leader election), as core's `Client::call` does it.
const CALL_RETRIES: u32 = 6;
const CALL_RETRY_FIRST: Duration = Duration::from_millis(200);
const CALL_RETRY_MAX: Duration = Duration::from_secs(2);

/// How the client's recovery treats a request (`SealedClient.request`'s
/// `mode`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// A `401` refreshes once, a `421 WRONG_NODE` or a node out of reach
    /// moves on, and the request is sent again.
    Run,
    /// As `Run`, and a node answering `503` or timing out is left for the
    /// next too: a request safe to send twice (a read, a call with its
    /// idempotency key).
    Repeatable,
    /// Sent once (a sign-in or sign-up: its caller repeats it with the
    /// passkey); a `421 WRONG_NODE` still moves the client on.
    Once,
}

impl Mode {
    fn parse(mode: &str) -> PyResult<Self> {
        match mode {
            "run" => Ok(Self::Run),
            "repeatable" => Ok(Self::Repeatable),
            "once" => Ok(Self::Once),
            other => Err(PyValueError::new_err(format!(
                "mode is \"run\", \"repeatable\" or \"once\", not {other:?}"
            ))),
        }
    }
}

/// The credential a request carries: the API key, else the node's session.
async fn credential(api_key: Option<String>, node: &SealedNode) -> Option<String> {
    match api_key {
        Some(key) => Some(key),
        None => node.session().await.map(|s| s.session_token),
    }
}

/// Send `req` to `node` once, or, with `retry`, again while it answers `503`
/// or cannot be reached ([`CALL_RETRIES`]). An answer the recovery acts on
/// comes back as an error ([`recoverable`]).
async fn attempt(
    node: &SealedNode,
    req: &RawRequest,
    token: Option<&str>,
    repeatable: bool,
    retry: bool,
) -> Result<RawResponse, Error> {
    let once = async || match node.send(req, token).await {
        Ok(resp) => match recoverable(&resp, repeatable || retry) {
            Some(e) => Err(e),
            None => Ok(resp),
        },
        Err(e) => Err(e),
    };
    if !retry {
        return once().await;
    }
    let mut wait = CALL_RETRY_FIRST;
    for _ in 0..CALL_RETRIES {
        match once().await {
            Err(e) if e.is_unavailable() => {
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(CALL_RETRY_MAX);
            }
            done => return done,
        }
    }
    once().await
}

impl PyCloudClient {
    /// Run `op` on the node in use with the GIL released, answering a `401`
    /// once the issuing node is lost (refresh once, repeat) and a `421
    /// WRONG_NODE` (the next node, repeat) as [`Nodes::run`] does. The future
    /// runs on this thread (`block_on`), so it need not be `Send`.
    fn run<T: Send>(
        &self,
        py: Python<'_>,
        op: impl AsyncFn(Arc<SealedNode>) -> Result<T, Error> + Send,
    ) -> PyResult<T> {
        let (rt, nodes) = (Arc::clone(&self.runtime), Arc::clone(&self.nodes));
        py.allow_threads(move || rt.block_on(nodes.run(op)).map_err(cloud_err))
    }

    /// [`run`](Self::run) for a request that is safe to send twice (a read):
    /// a node that answers `503` or cannot be reached is also left for the
    /// next, as [`Nodes::run_repeatable`] does.
    fn run_repeatable<T: Send>(
        &self,
        py: Python<'_>,
        op: impl AsyncFn(Arc<SealedNode>) -> Result<T, Error> + Send,
    ) -> PyResult<T> {
        let (rt, nodes) = (Arc::clone(&self.runtime), Arc::clone(&self.nodes));
        py.allow_threads(move || rt.block_on(nodes.run_repeatable(op)).map_err(cloud_err))
    }

    /// Block on `f` with the GIL released, as it is: no recovery.
    fn block<T: Send>(
        &self,
        py: Python<'_>,
        f: impl std::future::Future<Output = Result<T, Error>> + Send,
    ) -> PyResult<T> {
        let rt = Arc::clone(&self.runtime);
        py.allow_threads(move || rt.block_on(f).map_err(cloud_err))
    }

    /// The node in use.
    fn client(&self) -> Arc<SealedNode> {
        self.nodes.current().1
    }

    /// The operator key `use_operator_key` set, if any.
    fn operator_key(&self) -> Option<OperatorKey> {
        self.operator_key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The leader at `url`, attested under this client's policy on first use.
    fn leader(&self, url: &str) -> PyResult<Arc<SealedNode>> {
        let url = url.trim_end_matches('/').to_owned();
        let mut leaders = self
            .leaders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(node) = leaders.get(&url) {
            return Ok(Arc::clone(node));
        }
        let _enter = self.runtime.enter();
        let node = Arc::new(
            SealedNode::connect(&url, self.policy.clone(), &self.roots)
                .map_err(|e| PyValueError::new_err(format!("{e:#}")))?,
        );
        let _prev = leaders.insert(url, Arc::clone(&node));
        Ok(node)
    }
}

/// The answer, as Python gets it: `(status, [(name, value)], body)`.
fn to_tuple(py: Python<'_>, r: RawResponse) -> (u16, Vec<(String, String)>, PyObject) {
    let body = pyo3::types::PyBytes::new_bound(py, &r.body)
        .into_any()
        .unbind();
    (r.status, r.headers, body)
}

/// The trust policy every node of the client is attested under.
fn policy(
    allow_mock: bool,
    app_hash: Option<&str>,
    tee_profiles: Option<Vec<String>>,
    min_release: Option<String>,
    allow_debug_profiles: bool,
) -> PyResult<Policy> {
    let app_hash = app_hash
        .map(|h| {
            hex::decode(h)
                .ok()
                .and_then(|v| <[u8; 32]>::try_from(v).ok())
                .ok_or_else(|| PyValueError::new_err("app_hash must be 32 bytes of hex"))
        })
        .transpose()?;
    Ok(Policy {
        allow_mock,
        app_hash,
        measurements: match tee_profiles {
            Some(allowed_profiles) if !allowed_profiles.is_empty() => Measurements::Release {
                allowed_profiles,
                min_version: min_release,
            },
            _ => Measurements::None,
        },
        allowed_tcb_statuses: Vec::new(),
        allow_debug_profiles,
    })
}

/// A client for each of `urls` (each once, in order), attested under `policy`
/// before anything is sent to it.
fn connect_all(
    runtime: &Runtime,
    urls: impl IntoIterator<Item = String>,
    policy: &Policy,
    roots: &ExtraRoots,
) -> PyResult<Vec<(String, SealedNode)>> {
    let mut unique: Vec<String> = Vec::new();
    for url in urls {
        let url = url.trim_end_matches('/').to_owned();
        if !unique.contains(&url) {
            unique.push(url);
        }
    }
    let _enter = runtime.enter();
    unique
        .into_iter()
        .map(|url| {
            let client = SealedNode::connect(&url, policy.clone(), roots)
                .map_err(|e| PyValueError::new_err(format!("{e:#}")))?;
            Ok((url, client))
        })
        .collect()
}

#[pymethods]
impl PyCloudClient {
    /// Connect to `base_url`. The node is trusted only once its attestation
    /// verifies; `allow_mock` accepts development (mock) quotes, `app_hash`
    /// (hex) pins the bundle the quote must bind.
    ///
    /// `nodes` are other nodes of the same cluster, in the order to move to
    /// them when a node answers `421 WRONG_NODE` (its release may not serve
    /// the account); each is attested under the same policy before anything
    /// is sent to it. Without them that answer is raised as `WrongNode`.
    /// A node that cannot be reached, or that keeps answering `503` to a read
    /// or a call, is left for the next too (each node once per request).
    /// A `401` to a request made with a session is answered by one refresh
    /// (any node takes it) and the request sent again; `LoginRequired` is
    /// raised only when that does not help.
    ///
    /// A real TDX quote is accepted only from an image the policy names:
    /// `tee_profiles` admits nodes whose signed mero-tee release lists their
    /// measurements under one of those profiles (`min_release` is a floor).
    /// Without it, only mock quotes (`allow_mock`) can be accepted. A
    /// release's debug profiles (whose memory the host can read) are refused
    /// unless `allow_debug_profiles`.
    ///
    /// `ca_bundle`: the path of a PEM file whose certificates are trusted as
    /// TLS roots on top of the built-in public ones (a private or staging CA),
    /// for every node, leader and directory the client reaches. None trusts
    /// the public roots only. A file that cannot be read raises the `OSError`
    /// it stands for, one with no usable certificate `ValueError`, here.
    #[new]
    #[pyo3(signature = (base_url, allow_mock = false, app_hash = None, tee_profiles = None, min_release = None, allow_debug_profiles = false, nodes = None, ca_bundle = None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        base_url: &str,
        allow_mock: bool,
        app_hash: Option<&str>,
        tee_profiles: Option<Vec<String>>,
        min_release: Option<String>,
        allow_debug_profiles: bool,
        nodes: Option<Vec<String>>,
        ca_bundle: Option<&str>,
    ) -> PyResult<Self> {
        let roots = ExtraRoots::load(ca_bundle)?;
        let policy = policy(
            allow_mock,
            app_hash,
            tee_profiles,
            min_release,
            allow_debug_profiles,
        )?;
        let runtime = Runtime::new().map_err(|e| PyValueError::new_err(e.to_string()))?;
        let urls = std::iter::once(base_url.to_owned()).chain(nodes.unwrap_or_default());
        let nodes = connect_all(&runtime, urls, &policy, &roots)?;
        Ok(Self {
            nodes: Arc::new(Nodes::new(nodes)),
            runtime: Arc::new(runtime),
            policy,
            roots: roots.clone(),
            operator_key: Arc::new(std::sync::Mutex::new(None)),
            leaders: std::sync::Mutex::new(HashMap::new()),
        })
    }

    /// Connect to the cluster named `cluster`, by the nodes the Calimero
    /// Cloud node directory lists for it (`GET {directory}/{cluster}`).
    ///
    /// The nodes in `zone` come first, then the rest, each group in a
    /// weighted random order; the first is the node in use and the others
    /// are `nodes`. The directory is not trusted: every node is attested
    /// under this policy (the arguments mean what they do for the
    /// constructor) before anything is sent to it. The listing is fetched
    /// again once it is due and after the client moved off an unavailable
    /// node. Raises `CloudError` (code `DIRECTORY`; the status is the
    /// directory's, `404` for an unknown cluster, 0 when it could not be
    /// reached) when the directory lists no usable node.
    /// `ca_bundle` is the constructor's, and covers the directory too.
    #[staticmethod]
    #[pyo3(signature = (cluster, *, tee_profiles = None, min_release = None, allow_mock = false, app_hash = None, allow_debug_profiles = false, directory = None, zone = None, ca_bundle = None))]
    #[allow(clippy::too_many_arguments)]
    fn connect(
        py: Python<'_>,
        cluster: &str,
        tee_profiles: Option<Vec<String>>,
        min_release: Option<String>,
        allow_mock: bool,
        app_hash: Option<&str>,
        allow_debug_profiles: bool,
        directory: Option<String>,
        zone: Option<String>,
        ca_bundle: Option<&str>,
    ) -> PyResult<Self> {
        let roots = ExtraRoots::load(ca_bundle)?;
        if cluster.is_empty() {
            return Err(PyValueError::new_err("cluster must not be empty"));
        }
        let policy = policy(
            allow_mock,
            app_hash,
            tee_profiles,
            min_release,
            allow_debug_profiles,
        )?;
        let runtime = Runtime::new().map_err(|e| PyValueError::new_err(e.to_string()))?;
        let source = directory::Remote {
            directory: directory.unwrap_or_else(|| directory::DEFAULT_DIRECTORY.to_owned()),
            cluster: cluster.to_owned(),
            zone,
            policy: policy.clone(),
            roots: roots.clone(),
        };
        let (urls, refresh_after) = py
            .allow_threads(|| runtime.block_on(source.listing()))
            .map_err(directory_err)?;
        let nodes = connect_all(&runtime, urls, &policy, &roots)?;
        Ok(Self {
            nodes: Arc::new(Nodes::new(nodes).with_directory(Box::new(source), refresh_after)),
            runtime: Arc::new(runtime),
            policy,
            roots: roots.clone(),
            operator_key: Arc::new(std::sync::Mutex::new(None)),
            leaders: std::sync::Mutex::new(HashMap::new()),
        })
    }

    /// Send one request through the sealed channel to the node in use, as
    /// the generated client made it (`path` relative to the node, query
    /// included; `body` as bytes, empty for none), with the client's
    /// credential, and answer it as it came: `(status, headers, body)`.
    ///
    /// `mode` is how the recovery treats it: `"run"` (a `401` refreshes the
    /// session once, a `421 WRONG_NODE` or a node out of reach moves to the
    /// next node, and the request is sent again), `"repeatable"` (as `"run"`,
    /// and a `503` or a timeout moves on too: a read, or a call with its
    /// idempotency key) or `"once"` (a sign-in or sign-up). With
    /// `retry_unavailable`, a node answering `503` or out of reach is sent
    /// the request again for a while first (a call: its idempotency key makes
    /// it run once). An answer the recovery gave up on comes back as the
    /// node's error answer. Raises `CloudError` (status 0, `TRANSPORT`) when
    /// no node could be reached or attested, and `413 TOO_LARGE` when the
    /// node refused the envelope as too large.
    #[pyo3(signature = (method, path, headers, body, mode = "run", retry_unavailable = false))]
    fn request(
        &self,
        py: Python<'_>,
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        mode: &str,
        retry_unavailable: bool,
    ) -> PyResult<(u16, Vec<(String, String)>, PyObject)> {
        let mode = Mode::parse(mode)?;
        let req = RawRequest {
            method,
            path,
            headers,
            body,
        };
        let repeatable = mode == Mode::Repeatable;
        let nodes = Arc::clone(&self.nodes);
        let op = async |node: Arc<SealedNode>| {
            let token = credential(nodes.api_key(), &node).await;
            attempt(&node, &req, token.as_deref(), repeatable, retry_unavailable).await
        };
        let (rt, n) = (Arc::clone(&self.runtime), Arc::clone(&self.nodes));
        let answer = py.allow_threads(move || {
            rt.block_on(async {
                let sent = match mode {
                    Mode::Run => n.run(op).await,
                    Mode::Repeatable => n.run_repeatable(op).await,
                    Mode::Once => n.run_once(op).await,
                };
                sent.or_else(as_response)
            })
        });
        Ok(to_tuple(py, answer.map_err(cloud_err)?))
    }

    /// Send one request to the leader at `url` that a `421 NOT_LEADER`
    /// named (an app group's for replica moves, the system group's for node
    /// removal), as `request` sends it but with no recovery. The leader is
    /// attested under this client's policy and gets the client's API key
    /// (the ops token); `ValueError` without one.
    fn request_at(
        &self,
        py: Python<'_>,
        url: &str,
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> PyResult<(u16, Vec<(String, String)>, PyObject)> {
        let Some(key) = self.nodes.api_key() else {
            return Err(PyValueError::new_err(
                "a leader is sent the request only with an API key",
            ));
        };
        let node = self.leader(url)?;
        let req = RawRequest {
            method,
            path,
            headers,
            body,
        };
        let answer = self.block(py, async move {
            node.send(&req, Some(&key)).await.or_else(as_response)
        })?;
        Ok(to_tuple(py, answer))
    }

    /// Whether requests carry an API key (`use_api_key`) rather than a session.
    #[getter]
    fn has_api_key(&self) -> bool {
        self.nodes.api_key().is_some()
    }

    /// Exchange the refresh token for a new session, on the node in use (any
    /// node of the cluster takes it). One at a time with the refreshes the
    /// client makes on its own after a `401`: a refresh token is used once.
    fn refresh(&self, py: Python<'_>) -> PyResult<PyObject> {
        let nodes = Arc::clone(&self.nodes);
        let s = self.block(py, async move { nodes.refresh().await })?;
        to_py(py, &s)
    }

    /// Authenticate every request with an API key (`cal_...`).
    fn use_api_key(&self, py: Python<'_>, key: &str) -> PyResult<()> {
        self.nodes.set_api_key(Some(key.to_owned()));
        let (c, key) = (self.client(), key.to_owned());
        self.block(py, async move {
            c.use_api_key(&key).await;
            Ok(())
        })
    }

    /// Authenticate with a session returned by `sign_up`/`sign_in`.
    fn use_session(&self, py: Python<'_>, session: &Bound<'_, PyAny>) -> PyResult<()> {
        let s: Session = serde_json::from_value(to_json(py, Some(session))?)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        let c = self.client();
        self.block(py, async move {
            recover::Node::use_session(&*c, s).await;
            Ok(())
        })?;
        self.nodes.set_api_key(None);
        Ok(())
    }

    /// The session in use (`sign_up`, `sign_in`, `refresh`, `use_session`),
    /// or `None` (none yet, or an API key instead).
    fn session(&self, py: Python<'_>) -> PyResult<PyObject> {
        let c = self.client();
        let s = self.block(py, async move { Ok(c.session().await) })?;
        to_py(py, &s)
    }

    /// Sign the Ops API's decisions with the operator's Ed25519 key (its
    /// 32-byte seed, hex; the nodes were started with `--operator-key` set to
    /// its verify key): a `428 OPERATOR_SIGNATURE_REQUIRED` from `ops_*` is
    /// then answered with the signature. `None` forgets it.
    #[pyo3(signature = (seed))]
    fn use_operator_key(&self, seed: Option<&str>) -> PyResult<()> {
        let key = seed.map(operator_key_from_hex).transpose()?;
        *self
            .operator_key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = key;
        Ok(())
    }

    /// The operator key's verify half (hex), what the nodes take as
    /// `--operator-key`, from its 32-byte seed (hex; `secrets.token_hex(32)`
    /// makes one).
    #[staticmethod]
    fn operator_public_key(seed: &str) -> PyResult<String> {
        Ok(hex::encode(
            operator_key_from_hex(seed)?.verifying_key().to_bytes(),
        ))
    }

    /// The operator key's signature (hex) over `message`, the bytes a `428
    /// OPERATOR_SIGNATURE_REQUIRED` names: with `seed` (hex), else the key
    /// `use_operator_key` set; `None` without either.
    #[pyo3(signature = (message, seed = None))]
    fn operator_sign(&self, message: &[u8], seed: Option<&str>) -> PyResult<Option<String>> {
        use ed25519_dalek::Signer;
        let key = match seed {
            Some(seed) => Some(operator_key_from_hex(seed)?),
            None => self.operator_key(),
        };
        Ok(key.map(|k| hex::encode(k.sign(message).to_bytes())))
    }

    /// The URL of the node in use: `base_url`, until a `421 WRONG_NODE` or
    /// an unavailable node moved the client to the next of `nodes`.
    #[getter]
    fn node_url(&self) -> String {
        self.nodes.url()
    }

    /// Every node this client may use, in the order it moves through them
    /// (the node in use among them).
    #[getter]
    fn node_urls(&self) -> Vec<String> {
        self.nodes.urls()
    }

    /// The node's attestation document (unverified, for display).
    fn attestation(&self, py: Python<'_>) -> PyResult<PyObject> {
        // Unverified and for display only, so a fixed nonce is enough: the
        // verifying paths (`verify_app_cluster`) pick their own.
        let nonce = hex::encode([7_u8; 32]);
        let v = self.run_repeatable(py, async |c| {
            c.public().attestation(&nonce, None, None, None).await
        })?;
        to_py(py, &v)
    }
}
