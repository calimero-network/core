//! Client for Calimero nodes.
//!
//! Every request goes through a sealed channel to a node whose attestation was
//! verified first ([`attest`]). People sign in with a passkey ([`Passkey`]);
//! machines use an API key. Calls, views and events address one app instance.

use std::sync::Arc;

use calimero_identity::{Assertion, Registration};
use calimero_sealed::client::{SealedRefusal, SealedTransport};
use futures_util::{Stream, StreamExt};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use url::Url;

pub mod attest;

pub use attest::{AttestationDoc, HttpAttestor, Measurements, Policy};

/// An error from a node, or from getting to one.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The node answered with an error.
    #[error("{status} {code}: {message}")]
    Api {
        status: u16,
        code: String,
        message: String,
        details: Value,
    },
    /// The node runs a release the account's owners have not approved
    /// (`421 WRONG_NODE`): it neither issues nor takes the account's sessions.
    /// Send the request to another node of the cluster.
    #[error("421 WRONG_NODE: {message}")]
    WrongNode { message: String, details: Value },
    /// The node could not be reached, attested or understood.
    #[error(transparent)]
    Transport(#[from] eyre::Report),
}

/// The path a session is refreshed at; a `401` there is not refreshed again.
const REFRESH_PATH: &str = "auth/refresh";

/// What a request carries, kept so it can be sent again after a refresh.
enum Payload {
    Empty,
    Json(Value),
    Bytes {
        content_type: std::borrow::Cow<'static, str>,
        bytes: bytes::Bytes,
    },
}

/// The error a node's answer `status` with body `v` stands for.
fn api_error(status: u16, v: &Value) -> Error {
    let code = v["error"].as_str().unwrap_or("ERROR").to_owned();
    let message = v["message"].as_str().unwrap_or_default().to_owned();
    let details = v.get("details").cloned().unwrap_or(Value::Null);
    if status == 421 && code == "WRONG_NODE" {
        return Error::WrongNode { message, details };
    }
    Error::Api {
        status,
        code,
        message,
        details,
    }
}

/// `resp`'s JSON body, or the error its status stands for.
async fn answer(resp: reqwest::Response) -> Result<Value> {
    let status = resp.status().as_u16();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if status >= 400 {
        return Err(api_error(status, &v));
    }
    Ok(v)
}

/// How often [`Client::call`] sends a call again while the node is unavailable,
/// and how long it waits first: 200 ms, doubling, at most 2 s (about 9 s in
/// all, past a leader election).
const CALL_RETRIES: u32 = 6;
const CALL_RETRY_FIRST: std::time::Duration = std::time::Duration::from_millis(200);
const CALL_RETRY_MAX: std::time::Duration = std::time::Duration::from_secs(2);

/// A new idempotency key for a call the caller gave none. It only has to
/// outlive this client's own retries, so it takes the node's short-lived class:
/// a key starting `node:` (`calimero_node::app::NODE_KEY_PREFIX`) is kept
/// 10 minutes, a caller's own key 24 hours.
fn new_call_key() -> String {
    format!(
        "node:c-{}",
        calimero_identity::b64url(&calimero_identity::random::<16>())
    )
}

/// Runs `attempt` until it succeeds, fails for good, or [`CALL_RETRIES`] retries
/// are spent; only [`Error::is_unavailable`] is retried.
async fn retry_unavailable<T, F, Fut>(mut attempt: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut wait = CALL_RETRY_FIRST;
    for _ in 0..CALL_RETRIES {
        match attempt().await {
            Err(e) if e.is_unavailable() => {
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(CALL_RETRY_MAX);
            }
            done => return done,
        }
    }
    attempt().await
}

/// What a request sealed and sent became: a refusal of its envelope as too
/// large (`413`) is the answer a caller acts on, the same `TOO_LARGE` the node
/// gives a body past its own limits, so it is an [`Error::Api`]. It comes back
/// in the clear, as every refusal of an envelope does: the node refuses it
/// before the request can be opened, so there is no request to seal it to.
fn sealed_error(e: eyre::Report) -> Error {
    match e.downcast_ref::<SealedRefusal>() {
        Some(refusal) if refusal.status == 413 => Error::Api {
            status: 413,
            code: "TOO_LARGE".to_owned(),
            message: refusal.message.clone(),
            details: Value::Null,
        },
        _ => Error::Transport(e),
    }
}

impl Error {
    #[must_use]
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Api { code, .. } => Some(code),
            Self::WrongNode { .. } => Some("WRONG_NODE"),
            Self::Transport(_) => None,
        }
    }

    /// Whether the same request may succeed if sent again (D70): the node
    /// answered `503`, or it could not be reached or timed out. A refusal, an
    /// attestation failure or a bad answer will not change on a retry.
    #[must_use]
    pub fn is_unavailable(&self) -> bool {
        match self {
            Self::Api { status, .. } => *status == 503,
            Self::WrongNode { .. } => false,
            Self::Transport(e) => e.chain().any(|c| {
                c.downcast_ref::<reqwest::Error>()
                    .is_some_and(|r| r.is_connect() || r.is_timeout())
            }),
        }
    }

    #[must_use]
    pub fn details(&self) -> Option<&Value> {
        match self {
            Self::Api { details, .. } | Self::WrongNode { details, .. } => Some(details),
            Self::Transport(_) => None,
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Something that holds a passkey: a platform authenticator in a browser, a
/// software authenticator in tests.
pub trait Passkey: Send {
    /// Answer `navigator.credentials.create(options)`.
    fn create(&mut self, options: &Value) -> Registration;
    /// Answer `navigator.credentials.get(options)`, signing `challenge`.
    fn get(&mut self, challenge: &[u8]) -> Assertion;
}

#[cfg(feature = "testing")]
impl Passkey for calimero_identity::testing::SoftAuthenticator {
    fn create(&mut self, options: &Value) -> Registration {
        let challenge = options["challenge"]
            .as_str()
            .and_then(|c| calimero_identity::from_b64url(c).ok())
            .unwrap_or_default();
        self.register(&challenge)
    }

    fn get(&mut self, challenge: &[u8]) -> Assertion {
        self.assert(challenge)
    }
}

/// A signed-in session.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub account_id: String,
    pub session_token: String,
    pub expires_at: u64,
    pub refresh_token: String,
}

/// A committed call.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CallResult {
    pub index: u64,
    pub result: Value,
    #[serde(default)]
    pub events: Vec<Value>,
    /// The answering node's signed receipt for `result` ([`Client::verify_app_cluster`]
    /// gives the keys to check it with).
    #[serde(default)]
    pub receipt: Option<attest::Receipt>,
}

/// A read.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ViewResult {
    pub index: u64,
    pub result: Value,
    #[serde(default)]
    pub receipt: Option<attest::Receipt>,
}

/// How fresh a read must be.
#[derive(Clone, Copy, Debug, Default)]
pub enum Consistency {
    /// Linearizable, through the leader.
    #[default]
    Strong,
    /// From any replica, at least as new as `min_index`.
    Stale { min_index: u64 },
}

/// What [`Client::verify_app_cluster`] proved: every replica's attested
/// receipt key, and the governance they all attest.
#[derive(Clone, Debug)]
pub struct AttestedCluster {
    pub keys: std::collections::BTreeMap<u64, [u8; 32]>,
    pub governance: attest::Governance,
}

/// A connection to one node of a cluster.
pub struct Client {
    base: Url,
    http: reqwest::Client,
    policy: Policy,
    sealed: Arc<SealedTransport>,
    auth: RwLock<Option<String>>,
    session: RwLock<Option<Session>>,
    /// One refresh at a time: requests that got a `401` together refresh once
    /// (a refresh token presented twice ends the session).
    refreshing: tokio::sync::Mutex<()>,
}

impl Client {
    /// Connect to the node at `base`, trusting it only once `policy` accepts its attestation.
    pub fn connect(base: &str, policy: Policy) -> eyre::Result<Self> {
        let base: Url = if base.ends_with('/') {
            base.parse()?
        } else {
            format!("{base}/").parse()?
        };
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()?;
        let sealed = Arc::new(SealedTransport::attested(
            base.clone(),
            http.clone(),
            Arc::new(HttpAttestor {
                policy: policy.clone(),
                seen_release: Arc::default(),
            }),
        ));
        Ok(Self {
            base,
            http,
            policy,
            sealed,
            auth: RwLock::new(None),
            session: RwLock::new(None),
            refreshing: tokio::sync::Mutex::new(()),
        })
    }

    /// Verify every replica of `app`: each one's quote over a fresh nonce,
    /// held to this client's policy, binding its receipt key and the
    /// cluster's governance. Answers the attested receipt key per node (a
    /// receipt from any other key, or from a node not in this map, is not one
    /// to trust) and the governance every replica attests: who may move the
    /// app to new code.
    pub async fn verify_app_cluster(&self, app: &str) -> Result<AttestedCluster> {
        let nonce: [u8; 32] = calimero_identity::random::<32>();
        let v = self
            .get(&format!(
                "apps/{}/attestation?nonce={}",
                path_segment(app),
                hex::encode(nonce)
            ))
            .await?;
        let mut keys = std::collections::BTreeMap::new();
        let mut governance: Option<attest::Governance> = None;
        for r in v["replicas"].as_array().cloned().unwrap_or_default() {
            let node = r["node"].as_u64().unwrap_or_default();
            let doc: attest::AttestationDoc = serde_json::from_value(r["attestation"].clone())
                .map_err(|e| eyre::eyre!("replica {node} gave no attestation: {e}"))?;
            if doc.node != node {
                return Err(eyre::eyre!("replica {node} answered as node {}", doc.node).into());
            }
            let (key, rules) = attest::verify_receipt_key(&doc, &nonce, &self.policy)
                .await
                .map_err(|e| eyre::eyre!("replica {node}: {e:#}"))?;
            // One cluster, one set of rules: replicas that disagree are not it.
            if governance.as_ref().is_some_and(|g| *g != rules) {
                return Err(eyre::eyre!("replica {node} attests other governance").into());
            }
            governance = Some(rules);
            let _prev = keys.insert(node, key);
        }
        let governance = governance.ok_or_else(|| eyre::eyre!("no replica of {app} attested"))?;
        Ok(AttestedCluster { keys, governance })
    }

    /// Use an API key for every request.
    pub async fn use_api_key(&self, key: &str) {
        *self.auth.write().await = Some(key.to_owned());
    }

    /// Use a session token for every request.
    pub async fn use_session(&self, session: Session) {
        *self.auth.write().await = Some(session.session_token.clone());
        *self.session.write().await = Some(session);
    }

    pub async fn session(&self) -> Option<Session> {
        self.session.read().await.clone()
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<reqwest::Response> {
        self.send_payload(method, path, body.map_or(Payload::Empty, Payload::Json))
            .await
    }

    /// Send a request with this client's credential. A `401` to a session this
    /// client can refresh (it holds the refresh token) is refreshed once, on
    /// this node, and the request sent once more: the node that issued the
    /// session may be gone, or have been removed, and a refresh works on any
    /// node. Any other answer, or a refresh that fails, is returned as it is.
    async fn send_payload(
        &self,
        method: Method,
        path: &str,
        payload: Payload,
    ) -> Result<reqwest::Response> {
        let token = self.auth.read().await.clone();
        let resp = self
            .send_once(method.clone(), path, &payload, token.as_deref())
            .await?;
        if resp.status().as_u16() != 401 || path == REFRESH_PATH {
            return Ok(resp);
        }
        let Some(sent) = token else { return Ok(resp) };
        match self.refreshed_after(&sent).await {
            Some(fresh) => self.send_once(method, path, &payload, Some(&fresh)).await,
            None => Ok(resp),
        }
    }

    /// The session token to send again after `sent` was answered `401`: one a
    /// concurrent request already refreshed into, else a refresh's. `None` when
    /// `sent` is not a session this client can refresh (an API key, a session
    /// without its refresh token) or the refresh fails.
    async fn refreshed_after(&self, sent: &str) -> Option<String> {
        let _one = self.refreshing.lock().await;
        let session = self.session.read().await.clone()?;
        if session.session_token != sent {
            let current = self.auth.read().await.clone()?;
            return (current != sent && current == session.session_token).then_some(current);
        }
        if session.refresh_token.is_empty() {
            return None;
        }
        self.refresh().await.ok().map(|s| s.session_token)
    }

    async fn send_once(
        &self,
        method: Method,
        path: &str,
        payload: &Payload,
        token: Option<&str>,
    ) -> Result<reqwest::Response> {
        let url = self.base.join(path).map_err(eyre::Report::from)?;
        let mut req = self.http.request(method, url);
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }
        req = match payload {
            Payload::Empty => req,
            Payload::Json(b) => req.json(b),
            Payload::Bytes {
                content_type,
                bytes,
            } => req
                .header("content-type", &**content_type)
                .body(bytes.clone()),
        };
        let req = req.build().map_err(eyre::Report::from)?;
        self.execute(req).await
    }

    async fn execute(&self, req: reqwest::Request) -> Result<reqwest::Response> {
        self.sealed.execute(req).await.map_err(sealed_error)
    }

    async fn json(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        answer(self.send(method, path, body).await?).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.json(Method::POST, path, Some(body)).await
    }

    pub async fn put(&self, path: &str, body: Value) -> Result<Value> {
        self.json(Method::PUT, path, Some(body)).await
    }

    pub async fn get(&self, path: &str) -> Result<Value> {
        self.json(Method::GET, path, None).await
    }

    /// `GET path` answered as text (`/ops/metrics`).
    pub async fn get_text(&self, path: &str) -> Result<String> {
        let resp = self.send(Method::GET, path, None).await?;
        let status = resp.status().as_u16();
        let text = resp.text().await.map_err(eyre::Report::from)?;
        if status >= 400 {
            return Err(Error::Api {
                status,
                code: "ERROR".into(),
                message: text,
                details: Value::Null,
            });
        }
        Ok(text)
    }

    pub async fn delete(&self, path: &str) -> Result<Value> {
        self.json(Method::DELETE, path, None).await
    }

    /// DELETE `path` with a JSON body: a proof the caller made itself
    /// (`{confirm}`, `{operatorProof}`).
    ///
    /// # Errors
    /// As [`Self::delete`].
    pub async fn delete_with_body(&self, path: &str, body: Value) -> Result<Value> {
        self.json(Method::DELETE, path, Some(body)).await
    }

    // ------------------------------------------------------------ identity

    /// Create an account with a new passkey and sign in.
    pub async fn sign_up(&self, passkey: &mut dyn Passkey, name: &str) -> Result<Session> {
        let begin = self
            .post("auth/passkey/register/begin", json!({ "name": name }))
            .await?;
        let registration = passkey.create(&begin["options"]);
        let s: Session = serde_json::from_value(
            self.post(
                "auth/passkey/register/finish",
                json!({ "challengeId": begin["challengeId"], "registration": registration }),
            )
            .await?,
        )
        .map_err(eyre::Report::from)?;
        self.use_session(s.clone()).await;
        Ok(s)
    }

    /// Add another passkey (another device, `new`) to the signed-in
    /// account, approved with one it already has (`approver`, answering the
    /// node's `412 CONFIRM_REQUIRED`). Returns its credential id.
    pub async fn add_passkey(
        &self,
        new: &mut dyn Passkey,
        approver: &mut dyn Passkey,
    ) -> Result<String> {
        let begin = self.post("auth/passkeys/add/begin", json!({})).await?;
        let registration = new.create(&begin["options"]);
        let done = self
            .post_confirmed(
                "auth/passkeys/add/finish",
                json!({ "challengeId": begin["challengeId"], "registration": registration }),
                approver,
            )
            .await?;
        Ok(done["credentialId"].as_str().unwrap_or_default().to_owned())
    }

    /// Remove one of the account's passkeys, approved with a passkey it has.
    pub async fn remove_passkey(
        &self,
        credential: &str,
        passkey: &mut dyn Passkey,
    ) -> Result<Value> {
        self.delete_confirmed(
            &format!("auth/passkeys/{}", path_segment(credential)),
            passkey,
        )
        .await
    }

    /// Sign in with an existing passkey.
    pub async fn sign_in(&self, passkey: &mut dyn Passkey) -> Result<Session> {
        let begin = self.post("auth/passkey/login/begin", json!({})).await?;
        let challenge = calimero_identity::from_b64url(
            begin["options"]["challenge"].as_str().unwrap_or_default(),
        )
        .map_err(|e| eyre::eyre!("{e}"))?;
        let assertion = passkey.get(&challenge);
        let s: Session = serde_json::from_value(
            self.post(
                "auth/passkey/login/finish",
                json!({ "challengeId": begin["challengeId"], "assertion": assertion }),
            )
            .await?,
        )
        .map_err(eyre::Report::from)?;
        self.use_session(s.clone()).await;
        Ok(s)
    }

    /// Swap the refresh token for a new session, on this node (any node of
    /// the cluster takes it).
    pub async fn refresh(&self) -> Result<Session> {
        let current = self
            .session()
            .await
            .ok_or_else(|| eyre::eyre!("no session"))?;
        let token = self.auth.read().await.clone();
        let body = Payload::Json(json!({ "refreshToken": current.refresh_token }));
        let resp = self
            .send_once(Method::POST, REFRESH_PATH, &body, token.as_deref())
            .await?;
        let s: Session = serde_json::from_value(answer(resp).await?).map_err(eyre::Report::from)?;
        self.use_session(s.clone()).await;
        Ok(s)
    }

    pub async fn me(&self) -> Result<Value> {
        self.get("auth/me").await
    }

    /// Create an API key, approved with the account's passkey; returns
    /// `(key_id, key)`. The key is shown once.
    pub async fn create_api_key(
        &self,
        options: Value,
        passkey: &mut dyn Passkey,
    ) -> Result<(String, String)> {
        let v = self
            .post_confirmed("auth/apikeys", options, passkey)
            .await?;
        Ok((
            v["keyId"].as_str().unwrap_or_default().to_owned(),
            v["key"].as_str().unwrap_or_default().to_owned(),
        ))
    }

    // ------------------------------------------------------------ apps

    /// POST raw bytes (a bundle or a package) to `path`.
    async fn upload(
        &self,
        path: &str,
        content_type: impl Into<std::borrow::Cow<'static, str>>,
        bytes: Vec<u8>,
    ) -> Result<Value> {
        let payload = Payload::Bytes {
            content_type: content_type.into(),
            bytes: bytes.into(),
        };
        answer(self.send_payload(Method::POST, path, payload).await?).await
    }

    /// Upload a raw WASM bundle (development nodes only); returns its hash.
    pub async fn upload_bundle(&self, wasm: Vec<u8>) -> Result<String> {
        let v = self.upload("bundles", "application/wasm", wasm).await?;
        Ok(v["hash"].as_str().unwrap_or_default().to_owned())
    }

    /// Install a signed `.mpk` package file.
    pub async fn upload_package(&self, mpk: Vec<u8>) -> Result<Value> {
        self.upload("packages", "application/octet-stream", mpk)
            .await
    }

    /// The operator installs a signed `.mpk` (Ops API, the ops token as the key).
    pub async fn ops_upload_package(&self, mpk: Vec<u8>) -> Result<Value> {
        self.upload("ops/packages", "application/octet-stream", mpk)
            .await
    }

    /// The operator installs `package@version` from the node's registry.
    pub async fn ops_install_package(&self, package: &str, version: &str) -> Result<Value> {
        self.post(
            "ops/packages/install",
            json!({ "package": package, "version": version }),
        )
        .await
    }

    /// The operator uninstalls one version of a package it installed, or all of it.
    pub async fn ops_uninstall_package(
        &self,
        package: &str,
        version: Option<&str>,
    ) -> Result<Value> {
        let mut path = format!("ops/packages/{}", path_segment(package));
        if let Some(v) = version {
            path = format!("{path}/{}", path_segment(v));
        }
        self.delete(&path).await
    }

    /// Install `package@version` from the node's registry.
    pub async fn install_package(&self, package: &str, version: &str) -> Result<Value> {
        self.post(
            "packages/install",
            json!({ "package": package, "version": version }),
        )
        .await
    }

    /// Installed packages and their versions.
    pub async fn packages(&self) -> Result<Value> {
        Ok(self.get("packages").await?["packages"].clone())
    }

    /// One installed package and its versions.
    pub async fn package(&self, package: &str) -> Result<Value> {
        self.get(&format!("packages/{}", path_segment(package)))
            .await
    }

    /// Uninstall one version of a package, or all of it.
    pub async fn uninstall_package(&self, package: &str, version: Option<&str>) -> Result<Value> {
        match version {
            Some(v) => {
                self.delete(&format!(
                    "packages/{}/{}",
                    path_segment(package),
                    path_segment(v)
                ))
                .await
            }
            None => {
                self.delete(&format!("packages/{}", path_segment(package)))
                    .await
            }
        }
    }

    /// Delete an app instance.
    pub async fn delete_app(&self, app: &str) -> Result<Value> {
        self.delete(&format!("apps/{}", path_segment(app))).await
    }

    /// Upload a blob for app `app`; answers `{ blobId, id, size }` (`blobId`
    /// base58, as apps print it; `id` hex).
    pub async fn upload_blob(&self, app: &str, bytes: Vec<u8>) -> Result<Value> {
        self.upload(
            &format!("apps/{app}/blobs"),
            "application/octet-stream",
            bytes,
        )
        .await
    }

    /// Upload a blob with its content type; a component app records it (R8) and serves
    /// downloads with it.
    pub async fn upload_blob_as(&self, app: &str, mime: &str, bytes: Vec<u8>) -> Result<Value> {
        self.upload(&format!("apps/{app}/blobs"), mime.to_owned(), bytes)
            .await
    }

    /// Download a component app's blob with a grant one of its runs handed out
    /// (`blobs.grant`); no session needed.
    pub async fn download_blob_granted(&self, app: &str, id: &str, grant: &str) -> Result<Vec<u8>> {
        let resp = self
            .send(
                Method::GET,
                &format!(
                    "apps/{}/blobs/{}?grant={}",
                    path_segment(app),
                    path_segment(id),
                    path_segment(grant)
                ),
                None,
            )
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let v: Value = resp.json().await.unwrap_or(Value::Null);
            return Err(api_error(status, &v));
        }
        Ok(resp.bytes().await.map_err(eyre::Report::from)?.to_vec())
    }

    /// The app's blobs: `{ blobs: [{ blobId, id, size }] }`.
    pub async fn list_blobs(&self, app: &str) -> Result<Value> {
        self.get(&format!("apps/{}/blobs", path_segment(app))).await
    }

    /// One blob's id and size.
    pub async fn blob_info(&self, app: &str, id: &str) -> Result<Value> {
        self.get(&format!(
            "apps/{}/blobs/{}/info",
            path_segment(app),
            path_segment(id)
        ))
        .await
    }

    /// Delete one of the app's blobs (its owner's or an org admin's).
    pub async fn delete_blob(&self, app: &str, id: &str) -> Result<Value> {
        self.delete(&format!(
            "apps/{}/blobs/{}",
            path_segment(app),
            path_segment(id)
        ))
        .await
    }

    /// A component app's xcalls: `{ pending, deadLetters: [{ seq, app, method, reason,
    /// atMs }] }` (its owner or an org admin).
    pub async fn app_xcalls(&self, app: &str) -> Result<Value> {
        self.get(&format!("apps/{}/xcalls", path_segment(app)))
            .await
    }

    /// What the app's storage holds: `{ keys, bytes, blobs, blobBytes }`.
    pub async fn app_storage(&self, app: &str) -> Result<Value> {
        self.get(&format!("apps/{}/storage", path_segment(app)))
            .await
    }

    /// The ABI the app's bundle embeds: `{ bundle, abi }`.
    pub async fn app_abi(&self, app: &str) -> Result<Value> {
        self.get(&format!("apps/{}/abi", path_segment(app))).await
    }

    /// What this account uses, with the cluster's quotas.
    pub async fn usage(&self) -> Result<Value> {
        self.get("auth/usage").await
    }

    pub async fn download_blob(&self, app: &str, id: &str) -> Result<Vec<u8>> {
        let resp = self
            .send(Method::GET, &format!("apps/{app}/blobs/{id}"), None)
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let v: Value = resp.json().await.unwrap_or(Value::Null);
            return Err(api_error(status, &v));
        }
        Ok(resp.bytes().await.map_err(eyre::Report::from)?.to_vec())
    }

    /// Create an app instance from a package release (the newest without
    /// `version`; a package with several services names one), installed from
    /// the registry first if the cluster lacks it. `options` as [`Self::create_app`].
    pub async fn create_app_from_package(
        &self,
        name: &str,
        package: &str,
        version: Option<&str>,
        service: Option<&str>,
        options: Value,
    ) -> Result<String> {
        let mut body = json!({ "name": name, "package": package });
        if let Some(b) = body.as_object_mut() {
            if let Some(v) = version {
                let _prev = b.insert("version".into(), json!(v));
            }
            if let Some(s) = service {
                let _prev = b.insert("service".into(), json!(s));
            }
            for (k, v) in options.as_object().into_iter().flatten() {
                let _prev = b.insert(k.clone(), v.clone());
            }
        }
        let v = self.post("apps", body).await?;
        Ok(v["appId"].as_str().unwrap_or_default().to_owned())
    }

    /// Create an app instance from a bundle; `options` may set `org`, `access`, `confirmMethods`.
    pub async fn create_app(&self, name: &str, bundle: &str, options: Value) -> Result<String> {
        let mut body = json!({ "name": name, "bundle": bundle });
        if let (Some(b), Some(o)) = (body.as_object_mut(), options.as_object()) {
            for (k, v) in o {
                let _prev = b.insert(k.clone(), v.clone());
            }
        }
        let v = self.post("apps", body).await?;
        Ok(v["appId"].as_str().unwrap_or_default().to_owned())
    }

    /// POST `body` to `path`; when the node answers 412 CONFIRM_REQUIRED, sign
    /// its challenge with `passkey` and send it again with the approval. What
    /// an owner approves this way: confirm methods, upgrades, release approvals.
    pub async fn post_confirmed(
        &self,
        path: &str,
        body: Value,
        passkey: &mut dyn Passkey,
    ) -> Result<Value> {
        self.confirmed(Method::POST, path, body, passkey).await
    }

    /// End every session of this account, on every device.
    pub async fn sign_out_everywhere(&self) -> Result<Value> {
        self.post("auth/logout-all", json!({})).await
    }

    /// DELETE `path`, answering a `412 CONFIRM_REQUIRED` as
    /// [`Self::post_confirmed`] does.
    pub async fn delete_confirmed(&self, path: &str, passkey: &mut dyn Passkey) -> Result<Value> {
        self.confirmed(Method::DELETE, path, json!({}), passkey)
            .await
    }

    /// POST an operator decision (Ops API): when the node answers `428
    /// OPERATOR_SIGNATURE_REQUIRED`, sign the bytes it names (`message`,
    /// base64) with `sign` (the operator's Ed25519 key) and send it again
    /// with `operatorProof: { nonce, signature }`.
    pub async fn post_operator_signed(
        &self,
        path: &str,
        body: Value,
        sign: &(dyn Fn(&[u8]) -> Vec<u8> + Sync),
    ) -> Result<Value> {
        self.operator_signed(Method::POST, path, body, sign).await
    }

    /// DELETE an operator decision, signed as [`Self::post_operator_signed`]
    /// (the proof goes in a JSON body).
    pub async fn delete_operator_signed(
        &self,
        path: &str,
        sign: &(dyn Fn(&[u8]) -> Vec<u8> + Sync),
    ) -> Result<Value> {
        self.operator_signed(Method::DELETE, path, json!({}), sign)
            .await
    }

    async fn operator_signed(
        &self,
        method: Method,
        path: &str,
        mut body: Value,
        sign: &(dyn Fn(&[u8]) -> Vec<u8> + Sync),
    ) -> Result<Value> {
        let details = match self.json(method.clone(), path, Some(body.clone())).await {
            Err(e) if e.code() == Some("OPERATOR_SIGNATURE_REQUIRED") => {
                e.details().cloned().unwrap_or(Value::Null)
            }
            other => return other,
        };
        let message = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            details["message"].as_str().unwrap_or_default(),
        )
        .map_err(eyre::Report::from)?;
        if let Some(b) = body.as_object_mut() {
            let _prev = b.insert(
                "operatorProof".into(),
                json!({ "nonce": details["nonce"], "signature": hex::encode(sign(&message)) }),
            );
        }
        self.json(method, path, Some(body)).await
    }

    /// `method` on `path`, answering a `412 CONFIRM_REQUIRED` with a passkey
    /// assertion over the node's challenge.
    async fn confirmed(
        &self,
        method: Method,
        path: &str,
        body: Value,
        passkey: &mut dyn Passkey,
    ) -> Result<Value> {
        let details = match self.json(method.clone(), path, Some(body.clone())).await {
            Ok(v) => return Ok(v),
            Err(e) if e.code() == Some("CONFIRM_REQUIRED") => {
                e.details().cloned().unwrap_or(Value::Null)
            }
            Err(e) => return Err(e),
        };
        let challenge =
            calimero_identity::from_b64url(details["challenge"].as_str().unwrap_or_default())
                .map_err(|e| eyre::eyre!("{e}"))?;
        let assertion = passkey.get(&challenge);
        let mut body = body;
        if let Some(b) = body.as_object_mut() {
            let _prev = b.insert(
                "confirm".into(),
                json!({ "challengeId": details["challengeId"], "assertion": assertion }),
            );
        }
        self.json(method, path, Some(body)).await
    }

    /// Upgrade every app of `org` that runs `body.package` to
    /// `body.version` (optionally only `team`, with `cascade` its subtree):
    /// the org owner's, approved with their passkey. Answers the run, which
    /// [`Self::upgrade_run`] follows.
    pub async fn upgrade_org(
        &self,
        org: &str,
        body: Value,
        passkey: &mut dyn Passkey,
    ) -> Result<Value> {
        self.post_confirmed(
            &format!("orgs/{}/upgrade", path_segment(org)),
            body,
            passkey,
        )
        .await
    }

    /// An organization-wide upgrade run: its apps, done and failed.
    pub async fn upgrade_run(&self, org: &str, run: &str) -> Result<Value> {
        self.get(&format!(
            "orgs/{}/upgrades/{}",
            path_segment(org),
            path_segment(run)
        ))
        .await
    }

    /// Run the failed apps of an upgrade run again, as a new run.
    pub async fn retry_upgrade_run(
        &self,
        org: &str,
        run: &str,
        passkey: &mut dyn Passkey,
    ) -> Result<Value> {
        self.post_confirmed(
            &format!(
                "orgs/{}/upgrades/{}/retry",
                path_segment(org),
                path_segment(run)
            ),
            json!({}),
            passkey,
        )
        .await
    }

    /// Name an app for this account.
    pub async fn set_alias(&self, alias: &str, app: &str) -> Result<Value> {
        self.json(
            Method::PUT,
            &format!("auth/aliases/{}", path_segment(alias)),
            Some(json!({ "app": app })),
        )
        .await
    }

    /// The app an alias names: `{ alias, appId }`.
    pub async fn alias(&self, alias: &str) -> Result<Value> {
        self.get(&format!("auth/aliases/{}", path_segment(alias)))
            .await
    }

    /// Every alias of this account: `{ aliases: { alias: appId } }`.
    pub async fn aliases(&self) -> Result<Value> {
        self.get("auth/aliases").await
    }

    pub async fn delete_alias(&self, alias: &str) -> Result<Value> {
        self.delete(&format!("auth/aliases/{}", path_segment(alias)))
            .await
    }

    /// Delete an organization with its teams, invites and apps: the owner's,
    /// approved with their passkey.
    pub async fn delete_org(&self, org: &str, passkey: &mut dyn Passkey) -> Result<Value> {
        self.confirmed(
            Method::DELETE,
            &format!("orgs/{}", path_segment(org)),
            json!({}),
            passkey,
        )
        .await
    }

    /// Hand an app to another account (an org app only to one of the org's
    /// members): the owner's, approved with their passkey.
    pub async fn transfer_app(
        &self,
        app: &str,
        to: &str,
        passkey: &mut dyn Passkey,
    ) -> Result<Value> {
        self.post_confirmed(
            &format!("apps/{}/transfer", path_segment(app)),
            json!({ "to": to }),
            passkey,
        )
        .await
    }

    /// Set an organization's metadata (an admin's, or a holder of
    /// `MANAGE_METADATA`).
    pub async fn set_org_metadata(&self, org: &str, metadata: Value) -> Result<Value> {
        self.post(
            &format!("orgs/{}/metadata", path_segment(org)),
            json!({ "metadata": metadata }),
        )
        .await
    }

    /// Set a member's metadata (theirs, or an admin's); `null` clears it.
    pub async fn set_member_metadata(
        &self,
        org: &str,
        account: &str,
        metadata: Value,
    ) -> Result<Value> {
        self.post(
            &format!(
                "orgs/{}/members/{}/metadata",
                path_segment(org),
                path_segment(account)
            ),
            json!({ "metadata": metadata }),
        )
        .await
    }

    /// Set an app instance's metadata (its owner's, or an org admin's).
    pub async fn set_app_metadata(&self, app: &str, metadata: Value) -> Result<Value> {
        self.post(
            &format!("apps/{}/metadata", path_segment(app)),
            json!({ "metadata": metadata }),
        )
        .await
    }

    /// Name one of this account's passkeys; `""` clears the name.
    pub async fn label_passkey(&self, credential: &str, label: &str) -> Result<Value> {
        self.post(
            &format!("auth/passkeys/{}", path_segment(credential)),
            json!({ "label": label }),
        )
        .await
    }

    /// Upgrade an app (`body`: `{ version }`, `{ bundle }`, optional
    /// `migrate`), approved with the owner's passkey.
    pub async fn upgrade_app(
        &self,
        app: &str,
        body: Value,
        passkey: &mut dyn Passkey,
    ) -> Result<Value> {
        self.post_confirmed(
            &format!("apps/{}/upgrade", path_segment(app)),
            body,
            passkey,
        )
        .await
    }

    /// Approve a mero-tee release for an organization's apps (`org`), or for
    /// the account's own apps outside one (`None`). Until then, nodes on that
    /// release get none of those apps' data.
    pub async fn approve_release(
        &self,
        org: Option<&str>,
        version: &str,
        passkey: &mut dyn Passkey,
    ) -> Result<Value> {
        let path = match org {
            Some(o) => format!("orgs/{}/releases", path_segment(o)),
            None => "auth/releases".to_owned(),
        };
        self.post_confirmed(&path, json!({ "version": version }), passkey)
            .await
    }

    /// Approve URL prefixes `package`'s publisher may call out to, from an
    /// organization's apps (`org`) or the account's own (`None`): `prefixes`
    /// `None` approves every prefix its latest release declares; `replace`
    /// sets exactly `prefixes` (how one is revoked).
    pub async fn approve_outbound(
        &self,
        org: Option<&str>,
        package: &str,
        prefixes: Option<&[String]>,
        replace: bool,
        passkey: &mut dyn Passkey,
    ) -> Result<Value> {
        let path = match org {
            Some(o) => format!("orgs/{}/outbound", path_segment(o)),
            None => "auth/outbound".to_owned(),
        };
        let prefixes = prefixes.map_or_else(|| json!("manifest"), |p| json!(p));
        self.post_confirmed(
            &path,
            json!({ "package": package, "prefixes": prefixes, "replace": replace }),
            passkey,
        )
        .await
    }

    /// Call a method. A call always carries an idempotency key (`idempotency_key`,
    /// else a new short-lived one), and while the node answers `503` or cannot be reached
    /// it is sent again with that same key (D70): a call that did commit is
    /// answered from its record, never run twice.
    pub async fn call(
        &self,
        app: &str,
        method: &str,
        args: Value,
        idempotency_key: Option<&str>,
    ) -> Result<CallResult> {
        let key = idempotency_key.map_or_else(new_call_key, str::to_owned);
        let body = json!({ "method": method, "args": args, "idempotencyKey": key });
        let path = format!("apps/{app}/intents");
        let v = retry_unavailable(|| self.post(&path, body.clone())).await?;
        Ok(serde_json::from_value(v).map_err(eyre::Report::from)?)
    }

    /// Call a `confirm` method: the node asks for a passkey assertion over the
    /// exact call, which `passkey` provides.
    pub async fn call_confirmed(
        &self,
        app: &str,
        method: &str,
        args: Value,
        passkey: &mut dyn Passkey,
    ) -> Result<CallResult> {
        let first = self
            .post(
                &format!("apps/{app}/intents"),
                json!({ "method": method, "args": args }),
            )
            .await;
        let details = match first {
            Ok(v) => return Ok(serde_json::from_value(v).map_err(eyre::Report::from)?),
            Err(e) if e.code() == Some("CONFIRM_REQUIRED") => {
                e.details().cloned().unwrap_or(Value::Null)
            }
            Err(e) => return Err(e),
        };
        let challenge =
            calimero_identity::from_b64url(details["challenge"].as_str().unwrap_or_default())
                .map_err(|e| eyre::eyre!("{e}"))?;
        let assertion = passkey.get(&challenge);
        let v = self
            .post(
                &format!("apps/{app}/intents"),
                json!({
                    "method": method,
                    "args": args,
                    "confirm": { "challengeId": details["challengeId"], "assertion": assertion },
                }),
            )
            .await?;
        Ok(serde_json::from_value(v).map_err(eyre::Report::from)?)
    }

    /// Run a read-only method.
    pub async fn view(
        &self,
        app: &str,
        method: &str,
        args: Value,
        consistency: Consistency,
    ) -> Result<ViewResult> {
        let body = match consistency {
            Consistency::Strong => {
                json!({ "method": method, "args": args, "consistency": "strong" })
            }
            Consistency::Stale { min_index } => {
                json!({ "method": method, "args": args, "consistency": "stale", "minIndex": min_index })
            }
        };
        let v = self.post(&format!("apps/{app}/views"), body).await?;
        Ok(serde_json::from_value(v).map_err(eyre::Report::from)?)
    }

    /// Committed events from `from`, then live. Each item is `(id, kind, data)`.
    pub async fn events(
        &self,
        app: &str,
        from: u64,
    ) -> Result<impl Stream<Item = (String, String, Value)>> {
        self.sse(&format!("apps/{app}/events?from={from}")).await
    }

    /// Publish this device's presence in `app` (hex-encoded by the client;
    /// `None` retracts). Send the same `seq` and state again as a heartbeat
    /// (entries not refreshed within 7 s are gone).
    pub async fn publish_presence(
        &self,
        app: &str,
        seq: u64,
        state: Option<&[u8]>,
    ) -> Result<Value> {
        self.post(
            &format!("apps/{}/presence", path_segment(app)),
            json!({ "seq": seq, "state": state.map(hex::encode) }),
        )
        .await
    }

    /// Who is present in `app`: `{ present: [{ account, device, state }] }`.
    pub async fn presence(&self, app: &str) -> Result<Value> {
        self.get(&format!("apps/{}/presence", path_segment(app)))
            .await
    }

    /// Presence as a stream: `("", "snapshot", [...])` first, then
    /// `("", "presence", { account, device, state })` per change.
    pub async fn presence_stream(
        &self,
        app: &str,
    ) -> Result<impl Stream<Item = (String, String, Value)>> {
        self.sse(&format!("apps/{}/presence/stream", path_segment(app)))
            .await
    }

    /// A server-sent event stream: `(id, event, data)` per frame.
    async fn sse(&self, path: &str) -> Result<impl Stream<Item = (String, String, Value)>> {
        let resp = self.send(Method::GET, path, None).await?;
        if !resp.status().is_success() {
            return Err(Error::Api {
                status: resp.status().as_u16(),
                code: "EVENTS".into(),
                message: "event stream refused".into(),
                details: Value::Null,
            });
        }
        let bytes = resp.bytes_stream();
        let stream = futures_util::stream::unfold(
            (bytes, String::new()),
            |(mut bytes, mut buf)| async move {
                loop {
                    if let Some(pos) = buf.find("\n\n") {
                        let frame: String = buf.drain(..pos + 2).collect();
                        let (mut id, mut kind, mut data) =
                            (String::new(), String::new(), String::new());
                        for line in frame.lines() {
                            if let Some(v) = line.strip_prefix("id:") {
                                id = v.trim().to_owned();
                            } else if let Some(v) = line.strip_prefix("event:") {
                                kind = v.trim().to_owned();
                            } else if let Some(v) = line.strip_prefix("data:") {
                                data.push_str(v.trim());
                            }
                        }
                        if kind.is_empty() && data.is_empty() {
                            continue;
                        }
                        let value = serde_json::from_str(&data).unwrap_or(Value::String(data));
                        return Some(((id, kind, value), (bytes, buf)));
                    }
                    match bytes.next().await {
                        Some(Ok(chunk)) => buf.push_str(&String::from_utf8_lossy(&chunk)),
                        _ => return None,
                    }
                }
            },
        );
        Ok(Box::pin(stream))
    }

    /// The node's attestation, unverified (for display).
    pub async fn attestation(&self) -> Result<AttestationDoc> {
        let nonce = hex::encode([7_u8; 32]);
        let url = self
            .base
            .join(&format!("attestation?nonce={nonce}"))
            .map_err(eyre::Report::from)?;
        let doc = self
            .http
            .get(url)
            .send()
            .await
            .map_err(eyre::Report::from)?
            .json()
            .await
            .map_err(eyre::Report::from)?;
        Ok(doc)
    }
}

/// One URL path segment: everything but unreserved characters escaped, so an
/// id cannot reach another route.
fn path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod retry_tests {
    use std::sync::Mutex;

    use serde_json::Value;

    use super::{retry_unavailable, Error, CALL_RETRIES};

    fn api(status: u16) -> Error {
        Error::Api {
            status,
            code: "X".into(),
            message: String::new(),
            details: Value::Null,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_503_is_sent_again_until_it_succeeds() {
        let sent = Mutex::new(0);
        let out = retry_unavailable(|| {
            let n = {
                let mut s = sent.lock().unwrap();
                *s += 1;
                *s
            };
            async move {
                if n < 3 {
                    Err(api(503))
                } else {
                    Ok(n)
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(out, 3);
    }

    #[tokio::test(start_paused = true)]
    async fn a_refusal_is_not_sent_again() {
        let sent = Mutex::new(0);
        let out: Result<(), Error> = retry_unavailable(|| {
            *sent.lock().unwrap() += 1;
            async { Err(api(409)) }
        })
        .await;
        assert!(matches!(out, Err(Error::Api { status: 409, .. })));
        assert_eq!(*sent.lock().unwrap(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn the_retries_end() {
        let sent = Mutex::new(0);
        let out: Result<(), Error> = retry_unavailable(|| {
            *sent.lock().unwrap() += 1;
            async { Err(api(503)) }
        })
        .await;
        assert!(matches!(out, Err(Error::Api { status: 503, .. })));
        assert_eq!(*sent.lock().unwrap(), CALL_RETRIES + 1);
    }
}

#[cfg(test)]
mod error_tests {
    use serde_json::json;

    use super::{api_error, Error};

    #[test]
    fn a_421_wrong_node_is_its_own_error() {
        let e = api_error(
            421,
            &json!({ "error": "WRONG_NODE", "message": "use another node" }),
        );
        assert!(matches!(e, Error::WrongNode { .. }), "{e:?}");
        assert_eq!(e.code(), Some("WRONG_NODE"));
        assert!(!e.is_unavailable());
        assert_eq!(e.to_string(), "421 WRONG_NODE: use another node");
        // Another 421 (a leader elsewhere) stays an answer like any other.
        let e = api_error(421, &json!({ "error": "NOT_LEADER", "message": "" }));
        assert!(matches!(e, Error::Api { status: 421, .. }), "{e:?}");
    }
}
