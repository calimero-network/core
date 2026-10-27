//! Client for the Calimero cloud (release-v2).
//!
//! Every request goes through a sealed channel to a node whose attestation was
//! verified first ([`attest`]). People sign in with a passkey ([`Passkey`]);
//! machines use an API key. Calls, views and events address one app instance.

use std::sync::Arc;

use calimero_identity::{Assertion, Registration};
use calimero_sealed::client::SealedTransport;
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
    /// The node could not be reached, attested or understood.
    #[error(transparent)]
    Transport(#[from] eyre::Report),
}

impl Error {
    #[must_use]
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Api { code, .. } => Some(code),
            Self::Transport(_) => None,
        }
    }

    #[must_use]
    pub fn details(&self) -> Option<&Value> {
        match self {
            Self::Api { details, .. } => Some(details),
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

/// A connection to one node of a cluster.
pub struct Client {
    base: Url,
    http: reqwest::Client,
    policy: Policy,
    sealed: Arc<SealedTransport>,
    auth: RwLock<Option<String>>,
    session: RwLock<Option<Session>>,
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
        })
    }

    /// Verify every replica of `app`: each one's quote over a fresh nonce,
    /// held to this client's policy, binding its receipt key. Answers the
    /// attested receipt key per node; a receipt from any other key, or from a
    /// node not in this map, is not one to trust.
    pub async fn verify_app_cluster(
        &self,
        app: &str,
    ) -> Result<std::collections::BTreeMap<u64, [u8; 32]>> {
        let nonce: [u8; 32] = calimero_identity::random::<32>();
        let v = self
            .get(&format!(
                "apps/{}/attestation?nonce={}",
                path_segment(app),
                hex::encode(nonce)
            ))
            .await?;
        let mut keys = std::collections::BTreeMap::new();
        for r in v["replicas"].as_array().cloned().unwrap_or_default() {
            let node = r["node"].as_u64().unwrap_or_default();
            let doc: attest::AttestationDoc = serde_json::from_value(r["attestation"].clone())
                .map_err(|e| eyre::eyre!("replica {node} gave no attestation: {e}"))?;
            if doc.node != node {
                return Err(eyre::eyre!("replica {node} answered as node {}", doc.node).into());
            }
            let key = attest::verify_receipt_key(&doc, &nonce, &self.policy)
                .await
                .map_err(|e| eyre::eyre!("replica {node}: {e:#}"))?;
            let _prev = keys.insert(node, key);
        }
        Ok(keys)
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
        let url = self.base.join(path).map_err(eyre::Report::from)?;
        let mut req = self.http.request(method, url);
        if let Some(token) = self.auth.read().await.as_deref() {
            req = req.bearer_auth(token);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let req = req.build().map_err(eyre::Report::from)?;
        Ok(self.sealed.execute(req).await?)
    }

    async fn json(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        let resp = self.send(method, path, body).await?;
        let status = resp.status().as_u16();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        if status >= 400 {
            return Err(Error::Api {
                status,
                code: v["error"].as_str().unwrap_or("ERROR").to_owned(),
                message: v["message"].as_str().unwrap_or_default().to_owned(),
                details: v.get("details").cloned().unwrap_or(Value::Null),
            });
        }
        Ok(v)
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.json(Method::POST, path, Some(body)).await
    }

    pub async fn get(&self, path: &str) -> Result<Value> {
        self.json(Method::GET, path, None).await
    }

    pub async fn delete(&self, path: &str) -> Result<Value> {
        self.json(Method::DELETE, path, None).await
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

    /// Add another passkey (another device) to the signed-in account.
    /// Returns its credential id.
    pub async fn add_passkey(&self, passkey: &mut dyn Passkey) -> Result<String> {
        let begin = self.post("auth/passkeys/add/begin", json!({})).await?;
        let registration = passkey.create(&begin["options"]);
        let done = self
            .post(
                "auth/passkeys/add/finish",
                json!({ "challengeId": begin["challengeId"], "registration": registration }),
            )
            .await?;
        Ok(done["credentialId"].as_str().unwrap_or_default().to_owned())
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

    /// Swap the refresh token for a new session.
    pub async fn refresh(&self) -> Result<Session> {
        let current = self
            .session()
            .await
            .ok_or_else(|| eyre::eyre!("no session"))?;
        let s: Session = serde_json::from_value(
            self.post(
                "auth/refresh",
                json!({ "refreshToken": current.refresh_token }),
            )
            .await?,
        )
        .map_err(eyre::Report::from)?;
        self.use_session(s.clone()).await;
        Ok(s)
    }

    pub async fn me(&self) -> Result<Value> {
        self.get("auth/me").await
    }

    /// Create an API key; returns `(key_id, key)`. The key is shown once.
    pub async fn create_api_key(&self, options: Value) -> Result<(String, String)> {
        let v = self.post("auth/apikeys", options).await?;
        Ok((
            v["keyId"].as_str().unwrap_or_default().to_owned(),
            v["key"].as_str().unwrap_or_default().to_owned(),
        ))
    }

    // ------------------------------------------------------------ apps

    /// POST raw bytes (a bundle or a package) to `path`.
    async fn upload(&self, path: &str, content_type: &str, bytes: Vec<u8>) -> Result<Value> {
        let url = self.base.join(path).map_err(eyre::Report::from)?;
        let mut req = self
            .http
            .post(url)
            .header("content-type", content_type)
            .body(bytes);
        if let Some(token) = self.auth.read().await.as_deref() {
            req = req.bearer_auth(token);
        }
        let resp = self
            .sealed
            .execute(req.build().map_err(eyre::Report::from)?)
            .await?;
        let status = resp.status().as_u16();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        if status >= 400 {
            return Err(Error::Api {
                status,
                code: v["error"].as_str().unwrap_or("ERROR").to_owned(),
                message: v["message"].as_str().unwrap_or_default().to_owned(),
                details: v.get("details").cloned().unwrap_or(Value::Null),
            });
        }
        Ok(v)
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
        let url = self
            .base
            .join(&format!("apps/{app}/blobs"))
            .map_err(eyre::Report::from)?;
        let mut req = self
            .http
            .post(url)
            .header("content-type", "application/octet-stream")
            .body(bytes);
        if let Some(token) = self.auth.read().await.as_deref() {
            req = req.bearer_auth(token);
        }
        let resp = self
            .sealed
            .execute(req.build().map_err(eyre::Report::from)?)
            .await?;
        let status = resp.status().as_u16();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        if status >= 400 {
            return Err(Error::Api {
                status,
                code: v["error"].as_str().unwrap_or("ERROR").to_owned(),
                message: v["message"].as_str().unwrap_or_default().to_owned(),
                details: v.get("details").cloned().unwrap_or(Value::Null),
            });
        }
        Ok(v)
    }

    /// One of an app's blobs, by id (base58 or hex).
    pub async fn download_blob(&self, app: &str, id: &str) -> Result<Vec<u8>> {
        let resp = self
            .send(Method::GET, &format!("apps/{app}/blobs/{id}"), None)
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let v: Value = resp.json().await.unwrap_or(Value::Null);
            return Err(Error::Api {
                status,
                code: v["error"].as_str().unwrap_or("ERROR").to_owned(),
                message: v["message"].as_str().unwrap_or_default().to_owned(),
                details: v.get("details").cloned().unwrap_or(Value::Null),
            });
        }
        Ok(resp.bytes().await.map_err(eyre::Report::from)?.to_vec())
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
        let details = match self.post(path, body.clone()).await {
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
        self.post(path, body).await
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

    /// Call a method. Retries with the same idempotency key are answered once.
    pub async fn call(
        &self,
        app: &str,
        method: &str,
        args: Value,
        idempotency_key: Option<&str>,
    ) -> Result<CallResult> {
        let v = self
            .post(
                &format!("apps/{app}/intents"),
                json!({ "method": method, "args": args, "idempotencyKey": idempotency_key }),
            )
            .await?;
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
        let resp = self
            .send(Method::GET, &format!("apps/{app}/events?from={from}"), None)
            .await?;
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
