//! The API's routes for the peer routes (`/internal/*`) between the cluster's nodes.

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers moved here from `api.rs` read its items"
)]
use super::*;

#[derive(Deserialize)]
struct TakeChallengeBody {
    id: String,
}

/// A peer finishes a ceremony this node began: hand it the challenge, once.
async fn internal_challenge_take(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Json(body): Json<TakeChallengeBody>,
) -> ApiResult {
    let _caller = peer(&node, &headers, key).await?;
    let p = take_local(&node, &body.id)?;
    Ok(Json(
        serde_json::to_value(p).map_err(|e| ApiError::internal(&e.to_string()))?,
    ))
}

/// A presence update another node took (and authenticated).
async fn internal_presence(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Json(u): Json<crate::presence::Update>,
) -> ApiResult {
    let caller = peer(&node, &headers, key).await?;
    caller_may_hold_app(&node, &caller, &u.app).await?;
    let _applied = node.presence.apply(&u, crate::types::now_ms());
    Ok(Json(json!({})))
}

pub(crate) fn peer_only(node: &Node, headers: &HeaderMap) -> Result<(), ApiError> {
    let ok = headers
        .get("x-cluster-auth")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| ct_eq(v.as_bytes(), node.cluster_secret().as_bytes()));
    if ok {
        Ok(())
    } else {
        Err(ApiError::forbidden("peers only"))
    }
}

/// Who called an internal route, as this node verified it.
pub type PeerCaller = crate::node::PeerIdentity;

/// A peer request's caller: the cluster secret, and an identified session
/// whose proven key the claimed node's address attests (`x-calimero-node`
/// names it); an anonymous session is refused. The cluster secret alone
/// authorizes no data access: each route judges the caller.
async fn peer(
    node: &Node,
    headers: &HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
) -> Result<PeerCaller, ApiError> {
    peer_only(node, headers)?;
    let Some(Extension(calimero_sealed::PeerKey(key))) = key else {
        return Err(ApiError::forbidden(
            "peers identify themselves (an identified sealed session)",
        ));
    };
    let claimed = headers
        .get(crate::node::PEER_NODE_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or_else(|| ApiError::forbidden("name the calling node (x-calimero-node)"))?;
    node.identify_peer(key, claimed).await.ok_or_else(|| {
        ApiError::forbidden("the calling node's address does not attest the key it proved")
    })
}

/// The inbound twin of `Node::may_send`: refuse a caller whose verified
/// release `app`'s owner has not approved to hold its data.
fn caller_may_hold(
    node: &Node,
    caller: &PeerCaller,
    app: &crate::types::AppRecord,
) -> Result<(), ApiError> {
    if node.sys(|s| s.release_may_hold(app, caller.release.as_deref())) {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "the calling node's release is not approved for this app's data",
        ))
    }
}

/// [`caller_may_hold`] for an app named by id.
async fn caller_may_hold_app(
    node: &Node,
    caller: &PeerCaller,
    app_id: &str,
) -> Result<(), ApiError> {
    let app = node
        .sys_until(|s| s.apps.get(app_id).cloned())
        .await
        .ok_or_else(|| ApiError::forbidden("no such app"))?;
    caller_may_hold(node, caller, &app)
}

/// The apps hosted here whose state marks blob `hash`.
fn hosted_holders(node: &Node, hash: &str) -> Vec<crate::types::AppRecord> {
    let Some(raw) = hex::decode(hash)
        .ok()
        .and_then(|v| <[u8; 32]>::try_from(v).ok())
    else {
        return Vec::new();
    };
    let apps: Vec<crate::types::AppRecord> = node.sys(|s| s.apps.values().cloned().collect());
    apps.into_iter()
        .filter(|a| {
            node.hosted(&a.app_id)
                .is_some_and(|g| g.read_local(|m, _| m.holds_blob(&raw)))
        })
        .collect()
}

/// The apps hosted here whose sealed files list segment `hash`.
fn hosted_segment_holders(node: &Node, hash: &str) -> Vec<crate::types::AppRecord> {
    let apps: Vec<crate::types::AppRecord> = node.sys(|s| s.apps.values().cloned().collect());
    apps.into_iter()
        .filter(|a| {
            node.hosted(&a.app_id)
                .is_some_and(|g| g.read_local(|m, _| m.holds_segment(hash)))
        })
        .collect()
}

/// `POST /internal/cluster-key`: a new node (often of another mero-tee
/// release, so without the cluster secret) asks for K_c. It is delivered, not
/// answered: to the address the cluster has for that node id
/// (`Node::key_recipient_addr`: registered or a system member; this node's
/// `--peers` only before genesis), inside a sealed channel to the key the TD
/// there attests (`cluster_key::deliver`). Refused for another cluster, an unknown
/// or removed node id; at most `MAX_CONCURRENT_KEY_RELEASES` at once.
async fn internal_cluster_key(
    State(node): State<Arc<Node>>,
    Json(body): Json<crate::cluster_key::Request>,
) -> ApiResult {
    let Ok(_permit) = node.key_releases.try_acquire() else {
        return Err(ApiError::new(
            429,
            "BUSY",
            "too many cluster-key requests; retry",
        ));
    };
    if body.cluster != node.config.cluster {
        return Err(ApiError::forbidden("another cluster"));
    }
    let id = body.node;
    if node.sys(|s| s.job_nodes().any(|n| n.id == id)) {
        return Err(ApiError::forbidden(
            "a job node never holds the cluster key",
        ));
    }
    let Some(addr) = node.key_recipient_addr(id) else {
        return Err(ApiError::forbidden(
            "not a node of this cluster: register it (/ops/nodes) first",
        ));
    };
    crate::cluster_key::deliver(
        &node.config,
        &node.http,
        &addr,
        &body.token,
        node.cluster_key(),
    )
    .await
    .map_err(|e| {
        warn!(node = id, error = %format!("{e:#}"), "could not deliver the cluster key");
        ApiError::forbidden("the cluster key could not be delivered to that node's address")
    })?;
    info!(node = id, "delivered the cluster key");
    Ok(Json(json!({ "delivered": true })))
}

#[derive(Deserialize)]
struct RaftBody {
    group: u64,
    /// The node the sender meant (`PeerTransport::send`). Absent from a peer
    /// on a release before it; accepted then, until every peer sends it.
    #[serde(default)]
    target: Option<calimero_log::NodeId>,
    rpc: calimero_log::RpcRequest,
}

async fn internal_raft(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Json(body): Json<RaftBody>,
) -> ApiResult {
    let caller = peer(&node, &headers, key).await?;
    // The addressee, as the sender checks below check the other end: a message
    // meant for another node (an address now held by another node of this
    // cluster: a reused port, a node moved or replaced) is refused, never
    // handled as this node's. A genuine member's vote or append there would
    // move this node's term or log for a sender that meant someone else.
    match body.target {
        Some(target) if target != node.config.node_id => {
            let _seen =
                crate::metrics::raft_misaddressed(crate::metrics::RaftAddressee::NotAddressee);
            return Err(ApiError::new(
                421,
                "NOT_ADDRESSEE",
                &format!(
                    "this Raft message is for node {target}; this is node {}",
                    node.config.node_id
                ),
            ));
        }
        Some(_) => {}
        // No addressee: a peer on a release before it. Accepted this release,
        // counted, and warned about at most once per `NO_TARGET_WARN_EVERY`;
        // the release after refuses it (crates/node/AGENTS.md, "Raft addressee").
        None => {
            let seen = crate::metrics::raft_misaddressed(crate::metrics::RaftAddressee::NoTarget);
            if no_target_warning_due() {
                warn!(
                    group = body.group,
                    seen,
                    "accepted a Raft message without an addressee (a peer on an earlier release); the release after this one refuses them"
                );
            }
        }
    }
    // openraft takes the sender an RPC names on trust, and a follower takes
    // an append from any node with a higher term: the sender must be the
    // caller, and a member of the group (F4).
    if body.rpc.sender() != Some(caller.node) {
        return Err(ApiError::forbidden(
            "a Raft message names another node than the one that sent it",
        ));
    }
    if body.group != crate::node::SYSTEM_GROUP {
        let app = node
            .sys(|s| s.apps.values().find(|a| a.group == body.group).cloned())
            .ok_or_else(|| ApiError::forbidden("no such group"))?;
        caller_may_hold(&node, &caller, &app)?;
        let member = app.replicas.contains(&caller.node)
            || node
                .group_members(body.group)
                .is_some_and(|m| m.contains(&caller.node));
        if !member {
            return Err(ApiError::forbidden(
                "the calling node is not a member of that group",
            ));
        }
    }
    let r = node.engine.handle_rpc(body.group, body.rpc).await;
    Ok(Json(serde_json::to_value(r).unwrap_or_default()))
}

async fn internal_system(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Json(cmd): Json<SystemCmd>,
) -> ApiResult {
    // Any node of the cluster may propose; what it proposes is judged where
    // it applies (`system::authorize` and each command's own checks).
    let _caller = peer(&node, &headers, key).await?;
    // Only the leader answers; a follower refuses rather than forwarding again.
    let bytes = serde_json::to_vec(&cmd).map_err(|e| ApiError::internal(&e.to_string()))?;
    match node.system.write(bytes).await {
        Ok((_i, out)) => crate::system::output(&out)
            .map(Json)
            .map_err(|e| crate::node::rejected(&e)),
        Err(e) => Err(ApiError::unavailable(&e.to_string())),
    }
}

#[derive(Deserialize)]
struct BlobBody {
    hash: String,
    #[serde(default)]
    wasm: Option<calimero_log::Bytes>,
}

async fn internal_blob_put(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Json(body): Json<BlobBody>,
) -> ApiResult {
    // Content-addressed: a node can store nothing but the bytes of the hash.
    let _caller = peer(&node, &headers, key).await?;
    let wasm = body
        .wasm
        .ok_or_else(|| ApiError::bad_request("BAD_REQUEST", "wasm missing"))?;
    if hex::encode(sha256(&wasm.0)) != body.hash {
        return Err(ApiError::bad_request("BAD_REQUEST", "hash does not match"));
    }
    #[cfg(feature = "testing")]
    node.note_blob_put();
    #[cfg(feature = "testing")]
    let mode = node.peer_blobs_mode();
    #[cfg(feature = "testing")]
    if mode == crate::node::PeerBlobs::Refuse {
        return Err(ApiError::unavailable("blob puts are refused"));
    }
    node.blob_guards
        .put(&body.hash, || node.engine.put_blob(&body.hash, &wasm.0))
        .map_err(|e| ApiError::internal(&e.to_string()))?;
    #[cfg(feature = "testing")]
    if mode == crate::node::PeerBlobs::StoreThenFail {
        return Err(ApiError::unavailable("blob put answered late"));
    }
    Ok(Json(json!({})))
}

async fn internal_blob_get(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Json(body): Json<BlobBody>,
) -> ApiResult {
    let caller = peer(&node, &headers, key).await?;
    if !crate::job_wire::is_hash(&body.hash) {
        return Err(ApiError::bad_request("BAD_REQUEST", "not a blob hash"));
    }
    // Code goes to any node of the cluster; an app's blob only to a node
    // approved for an app hosted here that marks it.
    if !node.is_code_blob(&body.hash) {
        let allowed = hosted_holders(&node, &body.hash)
            .iter()
            .any(|app| caller_may_hold(&node, &caller, app).is_ok());
        if !allowed {
            return Err(ApiError::forbidden(
                "the calling node's release is not approved for an app that holds this blob here",
            ));
        }
    }
    let wasm = node
        .engine
        .get_blob(&body.hash)
        .map_err(|e| ApiError::internal(&e.to_string()))?
        .ok_or_else(|| ApiError::not_found("no such blob here"))?;
    Ok(Json(json!({ "wasm": calimero_log::Bytes(wasm) })))
}

/// `PUT /internal/segment/{hash}`: a file segment, raw, stored with its index row. Content-addressed
/// like a blob: a node can store nothing but the bytes of the hash.
async fn internal_segment_put(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Path(hash): Path<String>,
    body: Body,
) -> ApiResult {
    let _caller = peer(&node, &headers, key).await?;
    if body.len() > crate::files::SEGMENT_BYTES
        || crate::node::off_workers(|| hex::encode(sha256(&body))) != hash
    {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            "not the segment of that hash",
        ));
    }
    #[cfg(feature = "testing")]
    node.note_blob_put();
    #[cfg(feature = "testing")]
    let mode = node.peer_blobs_mode();
    #[cfg(feature = "testing")]
    if mode == crate::node::PeerBlobs::Refuse {
        return Err(ApiError::unavailable("blob puts are refused"));
    }
    #[cfg(feature = "testing")]
    if let crate::node::PeerBlobs::Delay(ms) = mode {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    }
    let _kept = node.keep_segment(&hash, body).await?;
    #[cfg(feature = "testing")]
    if mode == crate::node::PeerBlobs::StoreThenFail {
        return Err(ApiError::unavailable("blob put answered late"));
    }
    Ok(Json(json!({})))
}

/// `GET /internal/segment/{hash}`: a segment, raw, only to a node approved for an app hosted here
/// whose files list it.
async fn internal_segment_get(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Path(hash): Path<String>,
) -> Result<Response, ApiError> {
    let caller = peer(&node, &headers, key).await?;
    if !crate::job_wire::is_hash(&hash) {
        return Err(ApiError::bad_request("BAD_REQUEST", "not a segment hash"));
    }
    let holders = hosted_segment_holders(&node, &hash);
    if holders.is_empty() {
        return Err(ApiError::not_found("no such segment here"));
    }
    let allowed = holders
        .iter()
        .any(|app| caller_may_hold(&node, &caller, app).is_ok());
    if !allowed {
        return Err(ApiError::forbidden(
            "the calling node's release is not approved for an app that holds this segment here",
        ));
    }
    let bytes = node
        .local_segment(&hash)
        .ok_or_else(|| ApiError::not_found("no such segment here"))?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        bytes,
    )
        .into_response())
}

/// An upload request another node received, for the app's leader.
async fn internal_upload(
    node: &Node,
    headers: &HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    (app_id, account): (String, String),
    op: crate::node::UploadOp,
) -> ApiResult {
    let caller = peer(node, headers, key).await?;
    caller_may_hold_app(node, &caller, &app_id).await?;
    if !crate::node::valid_uploader(&account) {
        return Err(ApiError::bad_request("BAD_REQUEST", "unusable uploader"));
    }
    Ok(Json(node.upload_op_here(&app_id, &account, op).await?))
}

async fn internal_upload_begin(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Path((app_id, account, size)): Path<(String, String, u64)>,
) -> ApiResult {
    let op = crate::node::UploadOp::Begin { size };
    internal_upload(&node, &headers, key, (app_id, account), op).await
}

async fn internal_upload_part(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Path((app_id, account, upload, k)): Path<(String, String, String, usize)>,
    bytes: Body,
) -> ApiResult {
    let op = crate::node::UploadOp::Part { upload, k, bytes };
    internal_upload(&node, &headers, key, (app_id, account), op).await
}

async fn internal_upload_status(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Path((app_id, account, upload)): Path<(String, String, String)>,
) -> ApiResult {
    let op = crate::node::UploadOp::Status { upload };
    internal_upload(&node, &headers, key, (app_id, account), op).await
}

async fn internal_upload_complete(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Path((app_id, account, upload)): Path<(String, String, String)>,
) -> ApiResult {
    let op = crate::node::UploadOp::Complete { upload };
    internal_upload(&node, &headers, key, (app_id, account), op).await
}

async fn internal_upload_abort(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Path((app_id, account, upload)): Path<(String, String, String)>,
) -> ApiResult {
    let op = crate::node::UploadOp::Abort { upload };
    internal_upload(&node, &headers, key, (app_id, account), op).await
}

/// An app's usage book on this replica, for a peer ([`Node::app_usage`]): only
/// from a node that is one of the app's replicas, never from a group it still has
/// after the app moved away.
async fn internal_usage(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Json(body): Json<Value>,
) -> ApiResult {
    let _caller = peer(&node, &headers, key).await?;
    let app_id = body["app"].as_str().unwrap_or_default();
    let g = node
        .sys(|s| s.apps.get(app_id).cloned())
        .and_then(|app| node.replica_group(&app))
        .ok_or_else(|| ApiError::unavailable("not a replica of the app"))?;
    let book = g.read_local(|m, _| m.state.component.head.usage.clone());
    Ok(Json(json!({ "usage": book })))
}

/// What every component app this node replicates used on `{ day }`, by app id, for
/// a peer's fuel-cap refresh ([`Node::refresh_usage`]).
async fn internal_usage_today(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Json(body): Json<Value>,
) -> ApiResult {
    let _caller = peer(&node, &headers, key).await?;
    let day = body["day"]
        .as_u64()
        .ok_or_else(|| ApiError::bad_request("BAD_REQUEST", "day is a number"))?;
    Ok(Json(json!(node.replicas_usage_on(day))))
}

async fn internal_storage(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Json(body): Json<Value>,
) -> ApiResult {
    let caller = peer(&node, &headers, key).await?;
    let app = body["app"].as_str().unwrap_or_default();
    caller_may_hold_app(&node, &caller, app).await?;
    let g = node
        .hosted(app)
        .ok_or_else(|| ApiError::unavailable("app not hosted here"))?;
    // `strong`: answered only by the app's leader, after it confirmed it leads.
    let stats = if body["strong"].as_bool().unwrap_or(false) {
        g.read_strong(|m, _| m.storage_stats())
            .await
            .map_err(|e| ApiError::unavailable(&e.to_string()))?
    } else {
        g.read_local(|m, _| m.storage_stats())
    };
    Ok(Json(json!({
        "keys": stats.keys,
        "bytes": stats.bytes,
        "blobBytes": stats.blob_bytes,
        "blobs": stats.blobs.iter().map(|(id, s)| (hex::encode(id), *s)).collect::<Vec<_>>(),
        "files": stats.files.iter().map(|(id, s, h)| (hex::encode(id), *s, hex::encode(h))).collect::<Vec<_>>(),
    })))
}

/// When a hosted app last committed a write, for a peer's report
/// ([`Node::last_write_ms`]). App data, so only for a release approved to
/// hold the app, as `internal/storage`.
async fn internal_last_write(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Json(body): Json<LastWriteBody>,
) -> ApiResult {
    let caller = peer(&node, &headers, key).await?;
    caller_may_hold_app(&node, &caller, &body.app).await?;
    let g = node
        .hosted(&body.app)
        .ok_or_else(|| ApiError::unavailable("app not hosted here"))?;
    Ok(Json(
        json!({ "lastWriteMs": g.read_local(|m, _| m.last_write_ms()) }),
    ))
}

/// A hosted app's xcalls, for a peer's `GET /apps/{app}/xcalls`
/// ([`Node::app_xcalls`]), which judged the caller. App data, so only for a
/// release approved to hold the app, as `internal/storage`.
async fn internal_xcalls(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Json(body): Json<LastWriteBody>,
) -> ApiResult {
    let caller = peer(&node, &headers, key).await?;
    caller_may_hold_app(&node, &caller, &body.app).await?;
    let g = node
        .hosted(&body.app)
        .ok_or_else(|| ApiError::unavailable("app not hosted here"))?;
    Ok(Json(g.read_local(|m, _| m.xcalls_report())))
}

/// `internal/last-write`'s and `internal/xcalls`' body: the app.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LastWriteBody {
    app: String,
}

/// Delete a blob's bytes here, at the request of the node that collected it.
/// Code (a bundle, a package release) is never deleted this way.
async fn internal_blob_delete(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Json(body): Json<Value>,
) -> ApiResult {
    let _caller = peer(&node, &headers, key).await?;
    let hash = body["hash"].as_str().unwrap_or_default().to_owned();
    if !crate::job_wire::is_hash(&hash) {
        return Err(ApiError::bad_request("BAD_REQUEST", "not a blob hash"));
    }
    let deletion = node.blob_guards.begin_deletion(&hash);
    // The collecting node checked that no app holds it; this node asks the leaders again.
    let raw = crate::job_wire::hash_bytes(&hash).map_err(|e| ApiError::internal(&e.to_string()))?;
    if node.blob_in_use(&raw).await {
        return Err(ApiError::forbidden("an app still holds that blob"));
    }
    #[cfg(feature = "testing")]
    node.deletion_checkpoint().await;
    if !node.is_code_blob(&hash) {
        let deleted = deletion.delete(|| node.engine.delete_blob(&hash));
        deleted
            .ok_or_else(|| ApiError::conflict("the blob was stored again"))?
            .map_err(|e| ApiError::internal(&e.to_string()))?;
    }
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
struct HeldBody {
    app: String,
    id: String,
    #[serde(default)]
    strong: bool,
    /// Ask whether it is held as a sealed file, not whether it is held at all.
    #[serde(default)]
    file: bool,
    /// Count bytes an entry stored here and has not committed yet as held (strong reads only).
    #[serde(default)]
    pinned: bool,
    /// Answer a sealed file's size and seal too (`record`).
    #[serde(default)]
    record: bool,
}

async fn internal_blob_held(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Json(body): Json<HeldBody>,
) -> ApiResult {
    let caller = peer(&node, &headers, key).await?;
    let app = node.app(&body.app).await?;
    caller_may_hold(&node, &caller, &app)?;
    let raw = parse_blob_id(&body.id)?;
    let g = node
        .hosted(&app.app_id)
        .ok_or_else(|| ApiError::unavailable("app not hosted here"))?;
    // `strong`: answered only by the app's leader, after it confirmed it leads.
    let asks = |m: &AppMachine| {
        if body.file {
            m.is_sealed_file(&raw)
        } else {
            m.holds_blob(&raw) || (body.pinned && node.blob_guards.is_pinned(&body.id))
        }
    };
    let read = |m: &AppMachine| {
        let record = if body.record {
            m.file_record(&raw)
        } else {
            None
        };
        (asks(m), m.blob_standing(&raw), record)
    };
    let (held, standing, record) = if body.strong {
        g.read_strong(|m, _| read(m))
            .await
            .map_err(|e| ApiError::unavailable(&e.to_string()))?
    } else {
        g.read_local(|m, _| read(m))
    };
    let (published, epoch) = standing.unwrap_or_default();
    let mut answer = json!({ "held": held, "published": published, "epoch": epoch });
    if let Some((size, seal)) = record {
        answer["record"] = json!({ "size": size, "seal": seal });
    }
    Ok(Json(answer))
}

/// Pins ids on this node, the app's leader, for a peer that is about to store their bytes
/// (`ids`), or releases such a pin (`release`).
#[derive(Deserialize)]
struct PinBody {
    #[serde(default)]
    app: String,
    #[serde(default)]
    ids: Vec<String>,
    release: Option<u64>,
}

async fn internal_blob_pin(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Json(body): Json<PinBody>,
) -> ApiResult {
    let caller = peer(&node, &headers, key).await?;
    let app = node.app(&body.app).await?;
    caller_may_hold(&node, &caller, &app)?;
    if let Some(token) = body.release {
        node.blob_guards.release_lease(&app.app_id, token);
        return Ok(Json(json!({})));
    }
    if body.ids.is_empty() || body.ids.len() > MAX_LEASE_IDS {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            "a pin names 1 to 16 blobs",
        ));
    }
    for id in &body.ids {
        parse_blob_id(id)?;
    }
    let g = node
        .hosted(&app.app_id)
        .ok_or_else(|| ApiError::unavailable("app not hosted here"))?;
    g.read_strong(|_, _| ())
        .await
        .map_err(|e| ApiError::unavailable(&e.to_string()))?;
    let token = node
        .blob_guards
        .lease(&app.app_id, body.ids)
        .ok_or_else(|| ApiError::new(429, "TOO_MANY_PINS", "too many blob pins are open"))?;
    Ok(Json(json!({ "token": token })))
}

/// The system leader's applied index, once it confirmed it leads.
async fn internal_system_index(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
) -> ApiResult {
    let _caller = peer(&node, &headers, key).await?;
    let index = node
        .system
        .read_strong(|_, applied| applied)
        .await
        .map_err(|e| ApiError::unavailable(&e.to_string()))?;
    Ok(Json(json!({ "index": index })))
}

/// This node's runtime (`Config::release`), for a peer judging whether this
/// node may run a release (`Node::runtime_of_node`).
async fn internal_runtime(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
) -> ApiResult {
    let _caller = peer(&node, &headers, key).await?;
    Ok(Json(json!({ "runtime": node.config.release })))
}

async fn internal_call(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Json(req): Json<CallRequest>,
) -> ApiResult {
    let caller = peer(&node, &headers, key).await?;
    caller_may_hold_app(&node, &caller, &req.app_id).await?;
    Ok(Json(node.call_here_any(&req).await?))
}

async fn internal_view(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Json(req): Json<ViewRequest>,
) -> ApiResult {
    let caller = peer(&node, &headers, key).await?;
    caller_may_hold_app(&node, &caller, &req.app_id).await?;
    let a = node.view(&req).await?;
    Ok(Json(serde_json::to_value(a).unwrap_or_default()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadyBody {
    app_id: String,
}

async fn internal_app_ready(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Json(b): Json<ReadyBody>,
) -> ApiResult {
    let caller = peer(&node, &headers, key).await?;
    caller_may_hold_app(&node, &caller, &b.app_id).await?;
    let state = node
        .hosted(&b.app_id)
        .and_then(|g| g.read_local(|m, _| init_state(&m.state)));
    Ok(Json(match state {
        Some(Ok(())) => json!({ "ready": true }),
        Some(Err(e)) => json!({ "ready": true, "initError": e }),
        None => json!({ "ready": false }),
    }))
}

#[derive(Deserialize)]
struct InternalEvents {
    app: String,
    #[serde(default)]
    from: u64,
    /// The account a component's events are filtered for; empty: anonymous.
    #[serde(default)]
    subscriber: String,
}

async fn internal_events(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Query(q): Query<InternalEvents>,
) -> Result<Response, ApiError> {
    let caller = peer(&node, &headers, key).await?;
    caller_may_hold_app(&node, &caller, &q.app).await?;
    let subscriber = Some(q.subscriber).filter(|s| !s.is_empty());
    let stream = local_events(&node, &q.app, q.from, subscriber)
        .ok_or_else(|| ApiError::unavailable("app not hosted here"))?;
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

/// This area's routes, merged into [`super::router`].
pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route("/internal/cluster-key", post(internal_cluster_key))
        .route("/internal/raft", post(internal_raft))
        .route("/internal/system", post(internal_system))
        .route("/internal/system/index", post(internal_system_index))
        .route("/internal/runtime", post(internal_runtime))
        .route("/internal/call", post(internal_call))
        .route(
            "/internal/upload-begin/{app}/{account}/{size}",
            post(internal_upload_begin),
        )
        .route(
            "/internal/upload/{app}/{account}/{upload}",
            get(internal_upload_status).delete(internal_upload_abort),
        )
        .route(
            "/internal/upload/{app}/{account}/{upload}/{k}",
            put(internal_upload_part),
        )
        .route(
            "/internal/upload/{app}/{account}/{upload}/complete",
            post(internal_upload_complete),
        )
        .route("/internal/blob", post(internal_blob_put))
        .route("/internal/challenge/take", post(internal_challenge_take))
        .route(
            "/internal/segment/{hash}",
            put(internal_segment_put).get(internal_segment_get),
        )
        .route("/internal/blob/get", post(internal_blob_get))
        .route("/internal/blob/held", post(internal_blob_held))
        .route("/internal/blob/delete", post(internal_blob_delete))
        .route("/internal/blob/pin", post(internal_blob_pin))
        .route("/internal/storage", post(internal_storage))
        .route("/internal/usage", post(internal_usage))
        .route("/internal/usage/today", post(internal_usage_today))
        .route("/internal/last-write", post(internal_last_write))
        .route("/internal/xcalls", post(internal_xcalls))
        .route("/internal/presence", post(internal_presence))
        .route("/internal/view", post(internal_view))
        .route("/internal/app-ready", post(internal_app_ready))
        .route("/internal/events", get(internal_events))
}
