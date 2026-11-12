//! Client for Calimero nodes.
//!
//! Every request goes through a sealed channel to a node whose attestation was
//! verified first ([`attest`]). People sign in with a passkey ([`Passkey`]);
//! machines use an API key. Calls, views and events address one app instance.

use calimero_identity::{Assertion, Registration};
use futures_util::Stream;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod attest;
mod files;
mod generated;
mod route;
pub mod transport;

pub use attest::{AttestationDoc, HttpAttestor, Measurements, Policy};
pub use generated::{routes, types};
pub use route::OperatorSigner;
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

/// How often a retry-safe route (`client.apps().call(..)`, D70) is sent again
/// while the node is unavailable, and how long it waits first: 200 ms,
/// doubling, at most 2 s (about 9 s in all, past a leader election).
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

    use super::{path_segment, retry_unavailable, Error, CALL_RETRIES};

    fn api(status: u16) -> Error {
        Error::Api {
            status,
            code: "X".into(),
            message: String::new(),
            details: Value::Null,
        }
    }

    #[test]
    fn an_id_is_one_path_segment() {
        assert_eq!(path_segment("com.example.kv"), "com.example.kv");
        assert_eq!(path_segment("a/b?c"), "a%2Fb%3Fc");
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

    use super::Error;
    use crate::transport::api_error;

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
