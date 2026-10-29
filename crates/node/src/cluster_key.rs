//! The cluster key K_c: what the cluster's data is sealed under, the same on
//! every node of the cluster whatever mero-tee release it runs.
//!
//! K_r (`kms.rs`) comes from the mero-kms of the node's own release, and each
//! release has a KMS cluster with a root of its own, so K_r changes at every
//! release upgrade (mero-tee `docs/design/gcp-tdx-kms.md`, "Lifecycle"). What
//! nodes share or keep across releases is therefore keyed by K_c, never by K_r:
//! the store and its snapshots (streamed to peers as sealed), backups, the
//! app's TEE and member keys (`backup::tee_key`, `backup::member_key`), the
//! component cache's MAC and the peers' cluster secret.
//!
//! - The founding node (`--bootstrap`) takes its own K_r as K_c, and so does a
//!   node whose store was written before K_c existed: its data is sealed under
//!   K_r, which was then the cluster's one key, so nothing on disk changes.
//! - Every node keeps K_c wrapped under its own K_r ([`FILE`] in the data
//!   directory) and unwraps it at each start; a K_r that does not open it (the
//!   same disk booted on another release's KMS) stops the node.
//! - A node starting on an empty data directory without `--bootstrap` serves
//!   only `/health`, `/attestation` and the delivery route meanwhile, and asks
//!   its `--peers` for K_c (`POST /internal/cluster-key`, [`Request`]). A peer
//!   never answers with the key: it delivers it ([`deliver`]) to the address
//!   the cluster has for that node id (registered with `/ops/nodes`, or in the
//!   peer's own `--peers`), inside a sealed channel to the key the TD there
//!   attests under the peer policy. So the key reaches only a genuine TD at an
//!   address the operator chose, which is the bar mero-kms's shared disk
//!   identity sets for K_r. The delivery carries the request's random token,
//!   which travelled only inside the sealed channel to an attested peer, so
//!   the waiting node takes K_c from that peer alone.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use eyre::{bail, WrapErr};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::hmac;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tracing::{info, warn};

use crate::attest::{quote, report_data, AttestationDoc, HttpAttestor};
use crate::Config;

/// The file in the data directory that holds K_c wrapped under K_r.
pub const FILE: &str = "cluster-key";

/// How long a new node keeps asking its peers for K_c before it gives up.
pub const WAIT: Duration = Duration::from_secs(600);

/// The pause between two rounds over the peers.
const RETRY: Duration = Duration::from_secs(1);

const MAGIC: &[u8] = b"calimero.cluster-key.v1\n";

/// The key K_c is wrapped with on disk: derived from K_r.
fn wrap_key(storage_key: &[u8; 32]) -> LessSafeKey {
    let tag = hmac::sign(
        &hmac::Key::new(hmac::HMAC_SHA256, storage_key),
        b"calimero.cluster-key.wrap.v1",
    );
    // SAFETY: an HMAC-SHA256 tag is 32 bytes, an AES-256 key.
    LessSafeKey::new(UnboundKey::new(&AES_256_GCM, tag.as_ref()).expect("a 32-byte key"))
}

/// `MAGIC || nonce || AES-256-GCM(wrap_key(K_r), K_c)`.
pub fn wrap(storage_key: &[u8; 32], cluster_key: &[u8; 32]) -> eyre::Result<Vec<u8>> {
    let nonce: [u8; NONCE_LEN] = calimero_identity::random();
    let mut sealed = cluster_key.to_vec();
    wrap_key(storage_key)
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(MAGIC),
            &mut sealed,
        )
        .map_err(|_| eyre::eyre!("cannot wrap the cluster key"))?;
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// Open what [`wrap`] wrote with the same K_r.
pub fn unwrap(storage_key: &[u8; 32], bytes: &[u8]) -> eyre::Result<[u8; 32]> {
    let Some(rest) = bytes.strip_prefix(MAGIC) else {
        bail!("not a cluster key file");
    };
    if rest.len() < NONCE_LEN {
        bail!("the cluster key file is truncated");
    }
    let (nonce, sealed) = rest.split_at(NONCE_LEN);
    let mut sealed = sealed.to_vec();
    let nonce = Nonce::try_assume_unique_for_key(nonce)
        .map_err(|_| eyre::eyre!("the cluster key file is truncated"))?;
    let plain = wrap_key(storage_key)
        .open_in_place(nonce, Aad::from(MAGIC), &mut sealed)
        .map_err(|_| {
            eyre::eyre!(
                "this node's storage key does not open its cluster key: the data directory \
                 was written on another mero-tee release (whose mero-kms gave another key). \
                 A node does not move to a new release in place; start a new node on it and \
                 let it join"
            )
        })?;
    plain
        .try_into()
        .map_err(|_| eyre::eyre!("the cluster key file holds no 32-byte key"))
}

/// K_c from `dir`, if this node has one.
pub fn load(dir: &Path, storage_key: &[u8; 32]) -> eyre::Result<Option<[u8; 32]>> {
    let path = dir.join(FILE);
    match std::fs::read(&path) {
        Ok(bytes) => unwrap(storage_key, &bytes).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).wrap_err_with(|| format!("cannot read {}", path.display())),
    }
}

fn save(dir: &Path, storage_key: &[u8; 32], cluster_key: &[u8; 32]) -> eyre::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(FILE);
    let tmp = dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, wrap(storage_key, cluster_key)?)
        .wrap_err_with(|| format!("cannot write {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::File::open(&tmp)?.sync_all()?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// This node's K_c and the listener it serves on: unwrapped from its data
/// directory, minted (a founder, or a store from before K_c), or delivered by
/// a peer (a new node, which serves [`waiting_router`] on `listener` meanwhile).
pub async fn resolve(
    config: &Config,
    listener: TcpListener,
) -> eyre::Result<([u8; 32], TcpListener)> {
    if let Some(k) = load(&config.data_dir, &config.storage_key)? {
        return Ok((k, listener));
    }
    let existing = calimero_log::store::store_path(&config.data_dir).exists();
    let (key, listener) = if config.bootstrap || existing {
        (config.storage_key, listener)
    } else {
        receive(config, listener).await?
    };
    save(&config.data_dir, &config.storage_key, &key)?;
    Ok((key, listener))
}

/// `POST /internal/cluster-key`: node `node` of `cluster` asks for K_c. The
/// peer delivers it to the node's own address ([`Delivery`]), never in its
/// answer, so asking under another node's id gets the asker nothing.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub cluster: String,
    pub node: u64,
    /// Echoed in the delivery, which the asker takes only with it. Hex.
    pub token: String,
}

/// `POST /internal/cluster-key/deliver` on a node waiting for K_c, inside a
/// sealed channel to the key its quote attests.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Delivery {
    /// The token of the request this answers, hex.
    pub token: String,
    /// K_c, hex.
    pub key: String,
}

/// What the router of a node waiting for K_c holds.
struct Waiting {
    node: u64,
    release: crate::attest::Release,
    mode: crate::attest::Mode,
    cluster: String,
    transport: Arc<calimero_sealed::SealedTransport>,
    token: [u8; 32],
    key: Mutex<Option<oneshot::Sender<[u8; 32]>>>,
}

/// What a node serves while it waits for K_c: `/health` (so a manager that
/// waits for it before registering the node does not wait forever),
/// `/attestation` (so the peer delivering K_c can attest this TD's sealed
/// key), and the delivery itself.
fn waiting_router(w: &Arc<Waiting>) -> Router {
    Router::new()
        .route("/health", get(waiting_health))
        .route("/attestation", get(waiting_attestation))
        .route("/internal/cluster-key/deliver", post(take_delivery))
        .with_state(Arc::clone(w))
}

async fn waiting_health(State(w): State<Arc<Waiting>>) -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "node": w.node,
        "systemLeader": null,
        "release": w.release.version,
        "waiting": "the cluster key: register this node (/ops/nodes)",
    }))
}

#[derive(Deserialize)]
struct NonceQuery {
    nonce: String,
}

async fn waiting_attestation(
    State(w): State<Arc<Waiting>>,
    Query(q): Query<NonceQuery>,
) -> Result<Json<AttestationDoc>, StatusCode> {
    let nonce: [u8; 32] = hex::decode(&q.nonce)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let transport_key = w.transport.public_key();
    let q = quote(w.mode, report_data(&nonce, &[0; 32], &transport_key))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(AttestationDoc {
        quote: base64::engine::general_purpose::STANDARD.encode(q),
        transport_key: hex::encode(transport_key),
        app_hash: hex::encode([0; 32]),
        nonce: hex::encode(nonce),
        release: w.release.clone(),
        cluster: w.cluster.clone(),
        node: w.node,
        mock: w.mode == crate::attest::Mode::Mock,
        receipt_key: None,
        governance: None,
        kms: None,
    }))
}

async fn take_delivery(State(w): State<Arc<Waiting>>, Json(d): Json<Delivery>) -> StatusCode {
    let token = hex::decode(&d.token).unwrap_or_default();
    let same = token.len() == w.token.len()
        && token
            .iter()
            .zip(w.token)
            .fold(0_u8, |acc, (x, y)| acc | (x ^ y))
            == 0;
    if !same {
        return StatusCode::FORBIDDEN;
    }
    let Some(key) = hex::decode(&d.key)
        .ok()
        .and_then(|k| <[u8; 32]>::try_from(k).ok())
    else {
        return StatusCode::BAD_REQUEST;
    };
    let sender = w.key.lock().ok().and_then(|mut s| s.take());
    match sender.map(|s| s.send(key)) {
        Some(Ok(())) => StatusCode::OK,
        _ => StatusCode::CONFLICT,
    }
}

/// Serve [`waiting_router`] on `listener` and ask the peers, in turn, until
/// one delivers K_c or [`WAIT`] passes; then hand the address back.
async fn receive(config: &Config, listener: TcpListener) -> eyre::Result<([u8; 32], TcpListener)> {
    let peers: Vec<&String> = config
        .peers
        .iter()
        .filter(|(id, _)| **id != config.node_id)
        .map(|(_, url)| url)
        .collect();
    if peers.is_empty() {
        bail!(
            "this node is new and has no peer to take the cluster key from: pass --peers, or \
             --bootstrap to found a cluster"
        );
    }
    let addr = listener.local_addr()?;
    let (tx, mut rx) = oneshot::channel();
    let token: [u8; 32] = calimero_identity::random();
    let waiting = Arc::new(Waiting {
        node: config.node_id,
        release: config.attested_release(),
        mode: config.attestation,
        cluster: config.cluster.clone(),
        transport: Arc::new(calimero_sealed::SealedTransport::generate(
            &calimero_sealed::SealedOptions {
                required: config.sealed_required,
                unsealed_paths: vec!["/attestation".into(), "/health".into()],
            },
        )),
        token,
        key: Mutex::new(Some(tx)),
    });
    let stop = tokio_util::sync::CancellationToken::new();
    let server = {
        let stop = stop.clone();
        // The sealed envelope wraps the router from outside, as in `run`.
        let app = tower::Layer::layer(
            &axum::middleware::from_fn_with_state(
                Arc::clone(&waiting.transport),
                calimero_sealed::intercept,
            ),
            waiting_router(&waiting),
        );
        let svc = axum::ServiceExt::<axum::extract::Request>::into_make_service(app);
        tokio::spawn(async move {
            axum::serve(listener, svc)
                .with_graceful_shutdown(async move { stop.cancelled().await })
                .await
        })
    };
    info!(%addr, "waiting for a peer to deliver the cluster key");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let request = Request {
        cluster: config.cluster.clone(),
        node: config.node_id,
        token: hex::encode(token),
    };
    let deadline = tokio::time::Instant::now() + WAIT;
    let key = 'asking: loop {
        for url in &peers {
            match ask(config, &http, url, &request).await {
                Ok(()) => {}
                Err(e) => warn!(peer = %url, error = %format!("{e:#}"), "no cluster key yet"),
            }
            if let Ok(k) = rx.try_recv() {
                info!(peer = %url, "a peer delivered the cluster key");
                break 'asking Ok(k);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            break Err(eyre::eyre!(
                "no peer delivered the cluster key within {WAIT:?}; is this node registered \
                 (/ops/nodes) under its --public-url?"
            ));
        }
        tokio::time::sleep(RETRY).await;
    };
    stop.cancel();
    let _served = server.await;
    let key = key?;
    let listener = TcpListener::bind(addr)
        .await
        .wrap_err_with(|| format!("cannot listen on {addr} again"))?;
    Ok((key, listener))
}

/// One request to one peer, over a sealed channel to its attested key.
async fn ask(
    config: &Config,
    http: &reqwest::Client,
    url: &str,
    request: &Request,
) -> eyre::Result<()> {
    let base: url::Url = url.parse()?;
    let peer = calimero_sealed::client::SealedTransport::attested(
        base.clone(),
        http.clone(),
        Arc::new(HttpAttestor {
            policy: config.peer_policy(),
            seen_release: Arc::default(),
        }),
    );
    let req = http
        .post(calimero_sealed::client::resolve_path(
            &base,
            "internal/cluster-key",
        )?)
        .json(request)
        .build()?;
    let resp = peer.execute(req).await?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("{url} answered {status}: {body}");
    }
    Ok(())
}

/// On a peer: deliver K_c to `addr`, over a sealed channel to the key the TD
/// there attests under this node's peer policy. Only that TD can read it.
pub async fn deliver(
    config: &Config,
    http: &reqwest::Client,
    addr: &str,
    token: &str,
    cluster_key: &[u8; 32],
) -> eyre::Result<()> {
    let base: url::Url = addr.parse()?;
    // A channel of its own: the TD at `addr` serves its waiting key now and
    // another one once it runs, so this one is not kept.
    let peer = calimero_sealed::client::SealedTransport::attested(
        base.clone(),
        http.clone(),
        Arc::new(HttpAttestor {
            policy: config.peer_policy(),
            seen_release: Arc::default(),
        }),
    );
    let body = Delivery {
        token: token.to_owned(),
        key: hex::encode(cluster_key),
    };
    let req = http
        .post(calimero_sealed::client::resolve_path(
            &base,
            "internal/cluster-key/deliver",
        )?)
        .json(&body)
        .build()?;
    let resp = peer.execute(req).await?;
    let status = resp.status();
    if !status.is_success() {
        bail!("{addr} did not take the cluster key: {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrapped_key_opens_only_with_the_key_that_wrapped_it() {
        let k_c = [7; 32];
        let wrapped = wrap(&[1; 32], &k_c).unwrap();
        assert_eq!(unwrap(&[1; 32], &wrapped).unwrap(), k_c);
        let e = unwrap(&[2; 32], &wrapped).unwrap_err();
        assert!(e.to_string().contains("another mero-tee release"), "{e}");
        assert!(unwrap(&[1; 32], &wrapped[..MAGIC.len() + 4]).is_err());
    }
}
