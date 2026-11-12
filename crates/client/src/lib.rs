//! Client for Calimero nodes.
//!
//! Every request goes through a sealed channel to a node whose attestation was
//! verified first ([`attest`]). People sign in with a passkey ([`Passkey`]);
//! machines use an API key. Calls, views and events address one app instance.

use calimero_identity::{Assertion, Registration};
use futures_util::Stream;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub mod attest;
mod files;
mod generated;
mod route;
pub mod transport;

pub use attest::{AttestationDoc, HttpAttestor, Measurements, Policy};
pub use generated::{routes, types};
pub use route::OperatorSigner;
use transport::{answer, api_error};
pub use transport::{Body, Credential, Request, Transport};
use types::AppAttestationResponseReplicasItem;

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

/// A connection to one node of a cluster: the generated route methods
/// ([`routes`], `client.orgs()`, `client.apps()`, ...) and the hand-written
/// flows over them (passkey ceremonies, cluster verification, file uploads,
/// event streams), all sent through one [`Transport`].
pub struct Client {
    transport: Transport,
    policy: Policy,
}

impl Client {
    /// Connect to the node at `base`, trusting it only once `policy` accepts its attestation.
    pub fn connect(base: &str, policy: Policy) -> eyre::Result<Self> {
        Ok(Self {
            transport: Transport::connect(base, policy.clone())?,
            policy,
        })
    }

    /// The runtime every route of this client is sent through: the generated
    /// route layer and a raw [`Request`] (`client.transport().json(..)`) share
    /// this client's credential.
    #[must_use]
    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    /// Verify every replica of `app`: each one's quote over a fresh nonce,
    /// held to this client's policy, binding its receipt key and the
    /// cluster's governance. Answers the attested receipt key per node (a
    /// receipt from any other key, or from a node not in this map, is not one
    /// to trust) and the governance every replica attests: who may move the
    /// app to new code.
    pub async fn verify_app_cluster(&self, app: &str) -> Result<AttestedCluster> {
        let nonce: [u8; 32] = calimero_identity::random::<32>();
        let answer = self.apps().attestation(app, &hex::encode(nonce)).await?;
        let mut keys = std::collections::BTreeMap::new();
        let mut governance: Option<attest::Governance> = None;
        for r in answer.replicas {
            let (node, doc) = match r {
                AppAttestationResponseReplicasItem::Attestation { node, attestation } => {
                    (node, attestation)
                }
                AppAttestationResponseReplicasItem::Error { node, error } => {
                    return Err(eyre::eyre!("replica {node} gave no attestation: {error}").into());
                }
            };
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
        self.transport.use_api_key(key).await;
    }

    /// Use a session token for every request.
    pub async fn use_session(&self, session: Session) {
        self.transport.use_session(session).await;
    }

    pub async fn session(&self) -> Option<Session> {
        self.transport.session().await
    }

    // ------------------------------------------------------------ identity

    /// Create an account with a new passkey and sign in. The node asks the
    /// new passkey to sign the account it creates (`412 CONFIRM_REQUIRED`),
    /// so a sign-up asks the passkey twice: to create it, then to sign.
    pub async fn sign_up(&self, passkey: &mut dyn Passkey, name: &str) -> Result<Session> {
        self.sign_up_with(passkey, name, None, None).await
    }

    /// [`Self::sign_up`] from a page on `origin`: for an origin a package's
    /// manager registered, the node answers its own rp-id, and the session is
    /// bound to that package.
    ///
    /// # Errors
    /// As [`Self::sign_up`]; `403 ORIGIN_NOT_REGISTERED` for an origin the
    /// cluster does not know.
    pub async fn sign_up_at(
        &self,
        passkey: &mut dyn Passkey,
        name: &str,
        origin: Option<&str>,
    ) -> Result<Session> {
        self.sign_up_with(passkey, name, origin, None).await
    }

    /// [`Self::sign_up`] with an invite: an organization invite token (the
    /// new account joins the organization) or a cluster sign-up invite
    /// token, which a cluster whose sign-up is `invite` or `closed` needs
    /// (`GET /sign-up` says which). Refused `403 SIGN_UP_REFUSED` before the
    /// passkey is asked when the cluster would not take it.
    pub async fn sign_up_invited(
        &self,
        passkey: &mut dyn Passkey,
        name: &str,
        invite: Option<&str>,
    ) -> Result<Session> {
        self.sign_up_with(passkey, name, None, invite).await
    }

    /// A sign-up from `origin` (see [`Self::sign_up_at`]) with `invite` (see
    /// [`Self::sign_up_invited`]); the invite goes on the begin, the finish
    /// and its `412` retry.
    pub async fn sign_up_with(
        &self,
        passkey: &mut dyn Passkey,
        name: &str,
        origin: Option<&str>,
        invite: Option<&str>,
    ) -> Result<Session> {
        let begin = self
            .auth()
            .register_begin(Some(name), origin, invite)
            .await?;
        let registration = passkey.create(&begin.options);
        let s = self
            .auth()
            .register_finish(&begin.challenge_id, registration, invite, Some(passkey))
            .await?;
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
        let begin = self.auth().passkey_add_begin().await?;
        let registration = new.create(&begin.options);
        let done = self
            .auth()
            .passkey_add_finish(&begin.challenge_id, registration, Some(approver))
            .await?;
        Ok(done.credential_id)
    }

    /// Sign in with an existing passkey.
    pub async fn sign_in(&self, passkey: &mut dyn Passkey) -> Result<Session> {
        self.sign_in_at(passkey, None).await
    }

    /// [`Self::sign_in`] from a page on `origin` (see [`Self::sign_up_at`]).
    ///
    /// # Errors
    /// As [`Self::sign_up_at`].
    pub async fn sign_in_at(
        &self,
        passkey: &mut dyn Passkey,
        origin: Option<&str>,
    ) -> Result<Session> {
        let begin = self.auth().login_begin(origin).await?;
        let challenge =
            calimero_identity::from_b64url(begin.options["challenge"].as_str().unwrap_or_default())
                .map_err(|e| eyre::eyre!("{e}"))?;
        let assertion = passkey.get(&challenge);
        let s = self
            .auth()
            .login_finish(&begin.challenge_id, assertion)
            .await?;
        self.use_session(s.clone()).await;
        Ok(s)
    }

    /// Swap the refresh token for a new session, on this node (any node of
    /// the cluster takes it).
    pub async fn refresh(&self) -> Result<Session> {
        self.transport.refresh().await
    }

    /// Start handing `org` to the signed-in account `account` with the
    /// recovery `seed`, which signs here and is never sent. Answers
    /// `{ startedAt, readyAt }`.
    pub async fn start_recovery(&self, org: &str, account: &str, seed: &[u8; 32]) -> Result<Value> {
        use calimero_identity::recovery::sign_recovery;
        let nonce = calimero_identity::b64url(&calimero_identity::random::<16>());
        let signature = sign_recovery(seed, org, account, &nonce);
        let started = self.orgs().recovery_start(org, &nonce, &signature).await?;
        route::value(&started)
    }

    // ------------------------------------------------------------ streams

    /// Committed events from `from`, then live. Each item is `(id, kind, data)`.
    pub async fn events(
        &self,
        app: &str,
        from: u64,
    ) -> Result<impl Stream<Item = (String, String, Value)>> {
        let req = Request::new(Method::GET, format!("apps/{}/events", path_segment(app)))
            .query("from", from.to_string());
        self.transport.events(req).await
    }

    /// Presence as a stream: `("", "snapshot", [...])` first, then
    /// `("", "presence", { account, device, state })` per change.
    pub async fn presence_stream(
        &self,
        app: &str,
    ) -> Result<impl Stream<Item = (String, String, Value)>> {
        let req = Request::new(
            Method::GET,
            format!("apps/{}/presence/stream", path_segment(app)),
        );
        self.transport.events(req).await
    }
}

/// The route methods written by hand before the generated layer
/// ([`routes`]): each points at its replacement, and goes once nothing
/// calls it (`crates/node/tests`, `crates/merod/tests`).
#[allow(deprecated, reason = "the deprecated methods call one another")]
impl Client {
    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<reqwest::Response> {
        self.send_payload(method, path, body.map_or(Body::Empty, Body::Json))
            .await
    }

    /// Send a request with this client's credential ([`Transport::send`]).
    async fn send_payload(
        &self,
        method: Method,
        path: &str,
        payload: Body,
    ) -> Result<reqwest::Response> {
        self.transport
            .send(Request::new(method, path).body(payload))
            .await
    }

    async fn json(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        answer(self.send(method, path, body).await?).await
    }

    #[deprecated(
        note = "use a route's generated method (client.<group>().<method>()), or client.transport().json(Request) for a raw route"
    )]
    pub async fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.json(Method::POST, path, Some(body)).await
    }

    #[deprecated(
        note = "use a route's generated method (client.<group>().<method>()), or client.transport().json(Request) for a raw route"
    )]
    pub async fn put(&self, path: &str, body: Value) -> Result<Value> {
        self.json(Method::PUT, path, Some(body)).await
    }

    #[deprecated(
        note = "use a route's generated method (client.<group>().<method>()), or client.transport().json(Request) for a raw route"
    )]
    pub async fn get(&self, path: &str) -> Result<Value> {
        self.json(Method::GET, path, None).await
    }

    /// `GET path` answered as text (`/ops/metrics`).
    #[deprecated(
        note = "use client.ops().metrics(), or client.transport().send(Request) for a raw route"
    )]
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

    #[deprecated(
        note = "use a route's generated method (client.<group>().<method>()), or client.transport().json(Request) for a raw route"
    )]
    pub async fn delete(&self, path: &str) -> Result<Value> {
        self.json(Method::DELETE, path, None).await
    }

    /// DELETE `path` with a JSON body: a proof the caller made itself
    /// (`{confirm}`, `{operatorProof}`).
    ///
    /// # Errors
    /// As [`Self::delete`].
    #[deprecated(
        note = "use a route's generated method (client.<group>().<method>()), or client.transport().json(Request) for a raw route"
    )]
    pub async fn delete_with_body(&self, path: &str, body: Value) -> Result<Value> {
        self.json(Method::DELETE, path, Some(body)).await
    }

    /// Remove one of the account's passkeys, approved with a passkey it has.
    #[deprecated(note = "use client.auth().passkey_remove()")]
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

    #[deprecated(note = "use client.auth().me()")]
    pub async fn me(&self) -> Result<Value> {
        self.get("auth/me").await
    }

    /// Create an API key, approved with the account's passkey; returns
    /// `(key_id, key)`. The key is shown once.
    #[deprecated(note = "use client.auth().apikey_create()")]
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

    /// Revoke one of the account's API keys, approved with its passkey
    /// (`412 CONFIRM_REQUIRED`).
    #[deprecated(note = "use client.auth().apikey_revoke()")]
    pub async fn revoke_api_key(&self, key_id: &str, passkey: &mut dyn Passkey) -> Result<Value> {
        self.delete_confirmed(&format!("auth/apikeys/{}", path_segment(key_id)), passkey)
            .await
    }

    // ------------------------------------------------------------ apps

    /// PUT raw bytes to `path`: one part of a file upload.
    ///
    /// # Errors
    /// As [`Self::delete`].
    #[deprecated(note = "use client.apps().upload_part()")]
    pub async fn put_bytes(&self, path: &str, bytes: Vec<u8>) -> Result<Value> {
        self.send_bytes(Method::PUT, path, bytes).await
    }

    /// POST raw bytes to `path`: a file small enough for one request.
    ///
    /// # Errors
    /// As [`Self::delete`].
    #[deprecated(note = "use client.apps().files_upload()")]
    pub async fn post_bytes(&self, path: &str, bytes: Vec<u8>) -> Result<Value> {
        self.send_bytes(Method::POST, path, bytes).await
    }

    async fn send_bytes(&self, method: Method, path: &str, bytes: Vec<u8>) -> Result<Value> {
        let payload = Body::Bytes {
            content_type: "application/octet-stream".into(),
            bytes: bytes.into(),
        };
        answer(self.send_payload(method, path, payload).await?).await
    }

    /// POST raw bytes (a bundle or a package) to `path`.
    async fn upload(
        &self,
        path: &str,
        content_type: impl Into<std::borrow::Cow<'static, str>>,
        bytes: Vec<u8>,
    ) -> Result<Value> {
        let payload = Body::Bytes {
            content_type: content_type.into(),
            bytes: bytes.into(),
        };
        answer(self.send_payload(Method::POST, path, payload).await?).await
    }

    /// Upload a raw WASM bundle (development nodes only); returns its hash.
    #[deprecated(note = "use client.packages().bundle_put()")]
    pub async fn upload_bundle(&self, wasm: Vec<u8>) -> Result<String> {
        let v = self.upload("bundles", "application/wasm", wasm).await?;
        Ok(v["hash"].as_str().unwrap_or_default().to_owned())
    }

    /// Install a signed `.mpk` package file.
    #[deprecated(note = "use client.packages().upload()")]
    pub async fn upload_package(&self, mpk: Vec<u8>) -> Result<Value> {
        self.upload("packages", "application/octet-stream", mpk)
            .await
    }

    /// The operator installs a signed `.mpk` (Ops API, the ops token as the key).
    #[deprecated(note = "use client.ops().package_upload()")]
    pub async fn ops_upload_package(&self, mpk: Vec<u8>) -> Result<Value> {
        self.upload("ops/packages", "application/octet-stream", mpk)
            .await
    }

    /// The operator installs `package@version` from the node's registry.
    #[deprecated(note = "use client.ops().package_install()")]
    pub async fn ops_install_package(&self, package: &str, version: &str) -> Result<Value> {
        self.post(
            "ops/packages/install",
            json!({ "package": package, "version": version }),
        )
        .await
    }

    /// The operator uninstalls one version of a package it installed, or all of it.
    #[deprecated(
        note = "use client.ops().package_uninstall() or client.ops().package_version_uninstall()"
    )]
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
    #[deprecated(note = "use client.packages().install()")]
    pub async fn install_package(&self, package: &str, version: &str) -> Result<Value> {
        self.post(
            "packages/install",
            json!({ "package": package, "version": version }),
        )
        .await
    }

    /// One installed package and its versions.
    #[deprecated(note = "use client.packages().get()")]
    pub async fn package(&self, package: &str) -> Result<Value> {
        self.get(&format!("packages/{}", path_segment(package)))
            .await
    }

    /// Uninstall one version of a package, or all of it.
    #[deprecated(
        note = "use client.packages().uninstall() or client.packages().version_uninstall()"
    )]
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
    #[deprecated(note = "use client.apps().delete()")]
    pub async fn delete_app(&self, app: &str) -> Result<Value> {
        self.delete(&format!("apps/{}", path_segment(app))).await
    }

    /// Upload a blob for app `app`; answers `{ blobId, id, size }` (`blobId`
    /// base58, as apps print it; `id` hex).
    #[deprecated(note = "use client.apps().blob_upload()")]
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
    #[deprecated(note = "use client.apps().blob_upload()")]
    pub async fn upload_blob_as(&self, app: &str, mime: &str, bytes: Vec<u8>) -> Result<Value> {
        self.upload(&format!("apps/{app}/blobs"), mime.to_owned(), bytes)
            .await
    }

    /// Download a component app's blob with a grant one of its runs handed out
    /// (`blobs.grant`); no session needed.
    #[deprecated(note = "use client.apps().blob_download()")]
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
    #[deprecated(note = "use client.apps().blob_list()")]
    pub async fn list_blobs(&self, app: &str) -> Result<Value> {
        self.get(&format!("apps/{}/blobs", path_segment(app))).await
    }

    /// One blob's id and size.
    #[deprecated(note = "use client.apps().blob_info()")]
    pub async fn blob_info(&self, app: &str, id: &str) -> Result<Value> {
        self.get(&format!(
            "apps/{}/blobs/{}/info",
            path_segment(app),
            path_segment(id)
        ))
        .await
    }

    /// Delete one of the app's blobs (its owner's or an org admin's).
    #[deprecated(note = "use client.apps().blob_delete()")]
    pub async fn delete_blob(&self, app: &str, id: &str) -> Result<Value> {
        self.delete(&format!(
            "apps/{}/blobs/{}",
            path_segment(app),
            path_segment(id)
        ))
        .await
    }

    /// Revoke the grants made so far for one of the app's published blobs (its owner's or an
    /// org admin's).
    #[deprecated(note = "use client.apps().blob_revoke()")]
    pub async fn revoke_blob(&self, app: &str, id: &str) -> Result<Value> {
        self.post(
            &format!(
                "apps/{}/blobs/{}/revoke",
                path_segment(app),
                path_segment(id)
            ),
            json!({}),
        )
        .await
    }

    /// A component app's xcalls: `{ pending, deadLetters: [{ seq, app, method, reason,
    /// atMs }] }` (its owner or an org admin).
    #[deprecated(note = "use client.apps().xcalls()")]
    pub async fn app_xcalls(&self, app: &str) -> Result<Value> {
        self.get(&format!("apps/{}/xcalls", path_segment(app)))
            .await
    }

    /// What the app's storage holds: `{ keys, bytes, blobs, blobBytes }`.
    #[deprecated(note = "use client.apps().storage()")]
    pub async fn app_storage(&self, app: &str) -> Result<Value> {
        self.get(&format!("apps/{}/storage", path_segment(app)))
            .await
    }

    /// The ABI the app's bundle embeds: `{ bundle, abi }`.
    #[deprecated(note = "use client.apps().abi()")]
    pub async fn app_abi(&self, app: &str) -> Result<Value> {
        self.get(&format!("apps/{}/abi", path_segment(app))).await
    }

    /// The app's OpenAPI 3.1 document, generated from the ABI its bundle embeds: one
    /// operation per method (`POST /apps/{app}/methods/{method}`) and its event stream.
    ///
    /// # Errors
    /// As [`Self::get`]; the access is `app_abi`'s.
    #[deprecated(note = "use client.apps().openapi()")]
    pub async fn app_openapi(&self, app: &str) -> Result<Value> {
        self.get(&format!("apps/{}/openapi.json", path_segment(app)))
            .await
    }

    /// Run `method` by its own route, `POST /apps/{app}/methods/{method}`, which the node
    /// answers as `/intents` (a call) or `/views` (a view). `body` is theirs without
    /// `method`: `{ args, idempotencyKey?, confirm? }` or `{ args, consistency?, minIndex? }`.
    ///
    /// # Errors
    /// As [`Self::post`].
    #[deprecated(note = "use client.apps().method()")]
    pub async fn call_method(&self, app: &str, method: &str, body: Value) -> Result<Value> {
        self.post(&method_path(app, method), body).await
    }

    /// What this account uses, with the cluster's quotas, and what its apps ran
    /// as a developer today (`developerFuelToday`, `developerWritesToday`).
    #[deprecated(note = "use client.auth().usage()")]
    pub async fn usage(&self) -> Result<Value> {
        self.get("auth/usage").await
    }

    /// What the app's runs used (its owner and org admins only): `{ today, days }`,
    /// each day `{ date, runs, fuel, wallMs, writes, bytesWritten, events }`.
    #[deprecated(note = "use client.apps().usage()")]
    pub async fn app_usage(&self, app: &str) -> Result<Value> {
        self.get(&format!("apps/{}/usage", path_segment(app))).await
    }

    #[deprecated(note = "use client.apps().blob_download()")]
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
    #[deprecated(note = "use client.apps().create()")]
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
    #[deprecated(note = "use client.apps().create()")]
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
    #[deprecated(
        note = "use a step-up route's generated method with its passkey, or client.transport().confirmed(Request, passkey) for a raw route"
    )]
    pub async fn post_confirmed(
        &self,
        path: &str,
        body: Value,
        passkey: &mut dyn Passkey,
    ) -> Result<Value> {
        self.confirmed(Method::POST, path, body, passkey).await
    }

    /// The web origins registered for `package`'s frontend (its manager's).
    ///
    /// # Errors
    /// As [`Self::get`].
    #[deprecated(note = "use client.packages().origins_list()")]
    pub async fn package_origins(&self, package: &str) -> Result<Value> {
        self.get(&package_origins_path(package)).await
    }

    /// Register `origin` for `package`'s frontend (its rp-id `rp_id`, else
    /// the origin's host), approved with the manager's `passkey`: a passkey
    /// ceremony there opens a session bound to the package.
    ///
    /// # Errors
    /// As [`Self::post_confirmed`].
    #[deprecated(note = "use client.packages().origins_add()")]
    pub async fn add_package_origin(
        &self,
        package: &str,
        origin: &str,
        rp_id: Option<&str>,
        passkey: &mut dyn Passkey,
    ) -> Result<Value> {
        let mut body = json!({ "origin": origin });
        if let (Some(b), Some(rp)) = (body.as_object_mut(), rp_id) {
            let _prev = b.insert("rpId".into(), json!(rp));
        }
        self.post_confirmed(&package_origins_path(package), body, passkey)
            .await
    }

    /// Remove a registered origin of `package`, approved with the manager's
    /// `passkey`; the sessions opened there end.
    ///
    /// # Errors
    /// As [`Self::post_confirmed`].
    #[deprecated(note = "use client.packages().origins_remove()")]
    pub async fn remove_package_origin(
        &self,
        package: &str,
        origin: &str,
        passkey: &mut dyn Passkey,
    ) -> Result<Value> {
        self.confirmed(
            Method::DELETE,
            &package_origins_path(package),
            json!({ "origin": origin }),
            passkey,
        )
        .await
    }

    /// End every session of this account, on every device.
    #[deprecated(note = "use client.auth().logout_all()")]
    pub async fn sign_out_everywhere(&self) -> Result<Value> {
        self.post("auth/logout-all", json!({})).await
    }

    /// This account's live sessions (`GET /auth/sessions`): `{ sessions:
    /// [{ session, passkey, app, origin, openedAt, lastActiveAt, current }] }`.
    #[deprecated(note = "use client.auth().sessions_list()")]
    pub async fn sessions(&self) -> Result<Value> {
        self.get("auth/sessions").await
    }

    /// End one of this account's sessions (`DELETE /auth/sessions/{id}`).
    #[deprecated(note = "use client.auth().session_revoke()")]
    pub async fn revoke_session(&self, session: &str) -> Result<Value> {
        self.delete(&format!("auth/sessions/{}", path_segment(session)))
            .await
    }

    /// This account's activity (`GET /auth/activity`), newest first: `{
    /// activity: [{ at, passkey, kind, app, origin, method, subject }] }`,
    /// `kind` being `signIn` (with the `app` and `origin` the session is
    /// bound to) or `approval` (with the `method` and `subject` the passkey
    /// approved). `after`: only entries after that time (Unix seconds).
    #[deprecated(note = "use client.auth().activity_list()")]
    pub async fn activity(&self, after: Option<u64>) -> Result<Value> {
        match after {
            Some(t) => self.get(&format!("auth/activity?after={t}")).await,
            None => self.get("auth/activity").await,
        }
    }

    /// DELETE `path`, answering a `412 CONFIRM_REQUIRED` as
    /// [`Self::post_confirmed`] does.
    #[deprecated(
        note = "use a step-up route's generated method with its passkey, or client.transport().confirmed(Request, passkey) for a raw route"
    )]
    pub async fn delete_confirmed(&self, path: &str, passkey: &mut dyn Passkey) -> Result<Value> {
        self.confirmed(Method::DELETE, path, json!({}), passkey)
            .await
    }

    /// DELETE `path` with a JSON `body`, answering a `412 CONFIRM_REQUIRED`
    /// as [`Self::post_confirmed`] does (`confirm` goes into `body`).
    #[deprecated(
        note = "use a step-up route's generated method with its passkey, or client.transport().confirmed(Request, passkey) for a raw route"
    )]
    pub async fn delete_confirmed_with(
        &self,
        path: &str,
        body: Value,
        passkey: &mut dyn Passkey,
    ) -> Result<Value> {
        self.confirmed(Method::DELETE, path, body, passkey).await
    }

    /// POST an operator decision (Ops API): when the node answers `428
    /// OPERATOR_SIGNATURE_REQUIRED`, sign the bytes it names (`message`,
    /// base64) with `sign` (the operator's Ed25519 key) and send it again
    /// with `operatorProof: { nonce, signature }`.
    #[deprecated(
        note = "use an operator route's generated method with its operator signer, or client.transport().approved(Request, None, Some(sign)) for a raw route"
    )]
    pub async fn post_operator_signed(
        &self,
        path: &str,
        body: Value,
        sign: &(dyn Fn(&[u8]) -> Vec<u8> + Sync),
    ) -> Result<Value> {
        self.transport
            .approved(
                Request::new(Method::POST, path).json(body),
                None,
                Some(sign),
            )
            .await
    }

    /// DELETE an operator decision, signed as [`Self::post_operator_signed`]
    /// (the proof goes in a JSON body).
    #[deprecated(
        note = "use an operator route's generated method with its operator signer, or client.transport().approved(Request, None, Some(sign)) for a raw route"
    )]
    pub async fn delete_operator_signed(
        &self,
        path: &str,
        sign: &(dyn Fn(&[u8]) -> Vec<u8> + Sync),
    ) -> Result<Value> {
        self.transport
            .approved(
                Request::new(Method::DELETE, path).json(json!({})),
                None,
                Some(sign),
            )
            .await
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
        self.transport
            .confirmed(Request::new(method, path).json(body), passkey)
            .await
    }

    /// Upgrade every app of `org` that runs `body.package` to
    /// `body.version` (optionally only `team`, with `cascade` its subtree):
    /// the org owner's, approved with their passkey. Answers the run, which
    /// [`Self::upgrade_run`] follows.
    #[deprecated(note = "use client.orgs().upgrade()")]
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
    #[deprecated(note = "use client.orgs().upgrade_status()")]
    pub async fn upgrade_run(&self, org: &str, run: &str) -> Result<Value> {
        self.get(&format!(
            "orgs/{}/upgrades/{}",
            path_segment(org),
            path_segment(run)
        ))
        .await
    }

    /// Run the failed apps of an upgrade run again, as a new run.
    #[deprecated(note = "use client.orgs().upgrade_retry()")]
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
    #[deprecated(note = "use client.auth().alias_set()")]
    pub async fn set_alias(&self, alias: &str, app: &str) -> Result<Value> {
        self.json(
            Method::PUT,
            &format!("auth/aliases/{}", path_segment(alias)),
            Some(json!({ "app": app })),
        )
        .await
    }

    /// The app an alias names: `{ alias, appId }`.
    #[deprecated(note = "use client.auth().alias_get()")]
    pub async fn alias(&self, alias: &str) -> Result<Value> {
        self.get(&format!("auth/aliases/{}", path_segment(alias)))
            .await
    }

    /// Every alias of this account: `{ aliases: { alias: appId } }`.
    #[deprecated(note = "use client.auth().alias_list()")]
    pub async fn aliases(&self) -> Result<Value> {
        self.get("auth/aliases").await
    }

    #[deprecated(note = "use client.auth().alias_delete()")]
    pub async fn delete_alias(&self, alias: &str) -> Result<Value> {
        self.delete(&format!("auth/aliases/{}", path_segment(alias)))
            .await
    }

    /// Delete an organization with its teams, invites and apps: the owner's,
    /// approved with their passkey.
    #[deprecated(note = "use client.orgs().delete()")]
    pub async fn delete_org(&self, org: &str, passkey: &mut dyn Passkey) -> Result<Value> {
        self.confirmed(
            Method::DELETE,
            &format!("orgs/{}", path_segment(org)),
            json!({}),
            passkey,
        )
        .await
    }

    /// Set or rotate (`Some`, a [`calimero_identity::recovery::recovery_public`])
    /// or remove (`None`) an organization's recovery key: the owner's,
    /// approved with their passkey. A recovery under way is cancelled.
    #[deprecated(note = "use client.orgs().recovery_set()")]
    pub async fn set_recovery_key(
        &self,
        org: &str,
        public_key: Option<&str>,
        passkey: &mut dyn Passkey,
    ) -> Result<Value> {
        self.post_confirmed(
            &format!("orgs/{}/recovery", path_segment(org)),
            json!({ "publicKey": public_key }),
            passkey,
        )
        .await
    }

    /// Cancel the recovery under way: the owner's, approved with their passkey.
    #[deprecated(note = "use client.orgs().recovery_cancel()")]
    pub async fn cancel_recovery(&self, org: &str, passkey: &mut dyn Passkey) -> Result<Value> {
        self.post_confirmed(
            &format!("orgs/{}/recovery/cancel", path_segment(org)),
            json!({}),
            passkey,
        )
        .await
    }

    /// Finish a recovery this account started, once its delay passed,
    /// approved with this account's passkey: it becomes the owner.
    #[deprecated(note = "use client.orgs().recovery_finish()")]
    pub async fn finish_recovery(&self, org: &str, passkey: &mut dyn Passkey) -> Result<Value> {
        self.post_confirmed(
            &format!("orgs/{}/recovery/finish", path_segment(org)),
            json!({}),
            passkey,
        )
        .await
    }

    /// Hand an app to another account (an org app only to one of the org's
    /// members): the owner's, approved with their passkey.
    #[deprecated(note = "use client.apps().transfer()")]
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

    /// Move an app to another organization (`to_org`), its data where it
    /// is: whoever manages it now, as an admin of the target, approved with
    /// their passkey. `bring_members` brings the people of its organization
    /// who reach it along.
    #[deprecated(note = "use client.apps().app_move()")]
    pub async fn move_app(
        &self,
        app: &str,
        to_org: &str,
        bring_members: bool,
        passkey: &mut dyn Passkey,
    ) -> Result<Value> {
        self.post_confirmed(
            &format!("apps/{}/move", path_segment(app)),
            json!({ "toOrg": to_org, "bringMembers": bring_members }),
            passkey,
        )
        .await
    }

    /// Set an organization's metadata (an admin's, or a holder of
    /// `MANAGE_METADATA`).
    #[deprecated(note = "use client.orgs().metadata()")]
    pub async fn set_org_metadata(&self, org: &str, metadata: Value) -> Result<Value> {
        self.post(
            &format!("orgs/{}/metadata", path_segment(org)),
            json!({ "metadata": metadata }),
        )
        .await
    }

    /// Each member's lockout risk, for the organization's owner and admins
    /// (`GET /orgs/{org}/members?detail=security`): `{ orgId, members: [{
    /// account, name, role, teams, passkeys, recoveryCode, lastActiveDay,
    /// sessions }] }`, `passkeys` being `1`, `2` or `"3+"`.
    #[deprecated(note = "use client.orgs().members()")]
    pub async fn org_members_security(&self, org: &str) -> Result<Value> {
        self.get(&format!(
            "orgs/{}/members?detail=security",
            path_segment(org)
        ))
        .await
    }

    /// Set a member's metadata (theirs, or an admin's); `null` clears it.
    #[deprecated(note = "use client.orgs().member_metadata()")]
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
    #[deprecated(note = "use client.apps().metadata()")]
    pub async fn set_app_metadata(&self, app: &str, metadata: Value) -> Result<Value> {
        self.post(
            &format!("apps/{}/metadata", path_segment(app)),
            json!({ "metadata": metadata }),
        )
        .await
    }

    /// Name one of this account's passkeys; `""` clears the name.
    #[deprecated(note = "use client.auth().passkey_label()")]
    pub async fn label_passkey(&self, credential: &str, label: &str) -> Result<Value> {
        self.post(
            &format!("auth/passkeys/{}", path_segment(credential)),
            json!({ "label": label }),
        )
        .await
    }

    /// Upgrade an app (`body`: `{ version }`, `{ bundle }`, optional
    /// `migrate`), approved with the owner's passkey.
    #[deprecated(note = "use client.apps().upgrade()")]
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
    #[deprecated(
        note = "use client.orgs().release_approval() or client.auth().account_release_approval()"
    )]
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
    #[deprecated(
        note = "use client.orgs().outbound_approval() or client.auth().account_outbound_approval()"
    )]
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
    #[deprecated(note = "use client.apps().call()")]
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
    #[deprecated(note = "use client.apps().call()")]
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
    #[deprecated(note = "use client.apps().view()")]
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

    /// Publish this device's presence in `app` (hex-encoded by the client;
    /// `None` retracts). Send the same `seq` and state again as a heartbeat
    /// (entries not refreshed within 7 s are gone).
    #[deprecated(note = "use client.apps().presence_publish()")]
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
    #[deprecated(note = "use client.apps().presence_get()")]
    pub async fn presence(&self, app: &str) -> Result<Value> {
        self.get(&format!("apps/{}/presence", path_segment(app)))
            .await
    }

    /// The node's attestation, unverified (for display).
    #[deprecated(note = "use client.public().attestation()")]
    pub async fn attestation(&self) -> Result<AttestationDoc> {
        let nonce = hex::encode([7_u8; 32]);
        let url = self
            .transport
            .base()
            .join(&format!("attestation?nonce={nonce}"))
            .map_err(eyre::Report::from)?;
        let doc = self
            .transport
            .http()
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

/// The route of `package`'s registered origins (`package-origins/{package}`,
/// the package percent-encoded): for a caller that sends the change without a
/// passkey and lets the node answer `412 CONFIRM_REQUIRED`.
#[must_use]
pub fn package_origins_path(package: &str) -> String {
    format!("package-origins/{}", path_segment(package))
}

/// `apps/{app}/methods/{method}`.
#[must_use]
fn method_path(app: &str, method: &str) -> String {
    format!(
        "apps/{}/methods/{}",
        path_segment(app),
        path_segment(method)
    )
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

    use super::{package_origins_path, retry_unavailable, Error, CALL_RETRIES};

    fn api(status: u16) -> Error {
        Error::Api {
            status,
            code: "X".into(),
            message: String::new(),
            details: Value::Null,
        }
    }

    #[test]
    fn a_package_names_one_path_segment() {
        assert_eq!(
            package_origins_path("com.example.kv"),
            "package-origins/com.example.kv"
        );
        assert_eq!(package_origins_path("a/b?c"), "package-origins/a%2Fb%3Fc");
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
