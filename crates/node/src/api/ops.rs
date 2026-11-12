//! The API's routes for the operator's routes (`/ops/*`): status, fleet, placement, backups, quotas.

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers moved here from `api.rs` read its items"
)]
use super::*;

/// How long a replica move retries the second hand-over of files, and how long it pauses.
const HAND_OVER_AGAIN_WITHIN: Duration = Duration::from_secs(10);

const HAND_OVER_AGAIN_PAUSE: Duration = Duration::from_millis(500);

/// Prometheus metrics, behind the operator token: counts,
/// this node's groups and its streams. Never names, state or credentials.
async fn ops_metrics(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    operator(&node, &headers)?;
    let disk = crate::metrics::disk_sample(&node.config.data_dir).await;
    Ok(metrics_response(&node, disk))
}

/// `GET /ops/cluster`: the cluster's name and identity key, which a job node is started with.
async fn ops_cluster(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    operator(&node, &headers)?;
    Ok(Json(json!({
        "cluster": node.config.cluster,
        "identity": hex::encode(node.cluster_identity()),
    })))
}

async fn ops_status(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    operator(&node, &headers)?;
    let groups: Vec<Value> = node
        .sys(|s| {
            s.apps
                .values()
                .map(|a| (a.app_id.clone(), a.group))
                .collect::<Vec<_>>()
        })
        .into_iter()
        .filter_map(|(app, _g)| {
            node.hosted(&app)
                .map(|g| json!({ "app": app, "status": g.status() }))
        })
        .collect();
    let (nodes, apps, accounts) = node.sys(|s| {
        (
            s.nodes.values().cloned().collect::<Vec<_>>(),
            s.apps.len(),
            s.accounts.len(),
        )
    });
    // Counts and ids only: the Ops API never returns app state, names or
    // credentials (`/ops/placement` is the same: ids, zones, releases, counts).
    Ok(Json(json!({
        "node": node.config.node_id,
        "system": node.system.status(),
        "governance": node.sys(crate::system::SystemState::governance),
        "groups": groups,
        "nodes": nodes,
        "apps": apps,
        "accounts": accounts,
        // This node's rollback counter (`rollback::Status`); null: unprotected.
        "rollback": node.checkpoints.as_ref().and_then(|c| c.status()),
    })))
}

/// Where every app's replicas are, for a rebalancer (mdma): each app's id,
/// group and replica node ids, and each node's id, zone, attested release and
/// app count. Ids and counts only: no names, state or credentials. Read from
/// the system state after a read index (`fresh` says whether this replica
/// caught up with the system leader).
async fn ops_placement(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    operator(&node, &headers)?;
    let fresh = node.sys_fresh().await;
    let (apps, nodes) = node.sys(|s| {
        let apps: Vec<Value> = s
            .apps
            .values()
            .map(|a| json!({ "app": a.app_id, "group": a.group, "replicas": a.replicas }))
            .collect();
        let nodes: Vec<(u64, String, NodeRole, usize)> = s
            .nodes
            .values()
            .map(|n| {
                let hosted = s
                    .apps
                    .values()
                    .filter(|a| a.replicas.contains(&n.id))
                    .count();
                (n.id, n.zone.clone(), n.role, hosted)
            })
            .collect();
        (apps, nodes)
    });
    let releases =
        futures_util::future::join_all(nodes.iter().map(|(id, ..)| {
            tokio::time::timeout(PLACEMENT_RELEASE_WAIT, node.release_of_node(*id))
        }))
        .await;
    let nodes: Vec<Value> = nodes
        .into_iter()
        .zip(releases)
        .map(|((id, zone, role, hosted), release)| {
            json!({ "id": id, "zone": zone, "role": role, "release": release.ok().flatten(), "apps": hosted })
        })
        .collect();
    // The logged cluster version (`null` until the system leader logs one):
    // a rollout is finished for the system log once it is the new release's.
    let cluster_version = node.sys(|s| s.cluster_version.clone());
    Ok(Json(
        json!({ "fresh": fresh, "apps": apps, "nodes": nodes, "clusterVersion": cluster_version }),
    ))
}

/// What every app runs, for the operator (mdma): per app its id, group,
/// scope (organization id or `account`, never the account), bundle digest,
/// package release only where the operator may see it
/// (`SystemState::sees_release`: the releases it installed), who holds app
/// upgrades, its last upgrade by actor kind, the releases approved to hold
/// it, each replica's release and whether it is approved, and when it last
/// committed a write ([`with_last_writes`]). Paged.
async fn ops_apps(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Query(q): Query<PageQuery>,
) -> ApiResult {
    operator(&node, &headers)?;
    let limit = q.limit()?;
    let fresh = node.sys_fresh().await;
    let releases = attested_releases(&node).await;
    let (mut apps, next, governance) = node.sys(|s| {
        let (page, next) = report::page(s, q.after.as_deref(), limit, |_| true);
        let apps: Vec<Value> = page
            .into_iter()
            .map(|a| report::app_json(s, a, &releases, &BTreeMap::new(), Reader::Operator))
            .collect();
        (apps, next, s.governance())
    });
    with_last_writes(&node, &mut apps).await;
    Ok(Json(json!({
        "fresh": fresh,
        "governance": governance,
        "apps": apps,
        "next": next,
    })))
}

/// Each organization's id and the releases approved to hold its apps, for
/// the operator. Paged like [`ops_apps`].
async fn ops_orgs(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Query(q): Query<PageQuery>,
) -> ApiResult {
    operator(&node, &headers)?;
    let limit = q.limit()?;
    let fresh = node.sys_fresh().await;
    let (orgs, next, governance) = node.sys(|s| {
        let start = q.after.clone().map_or(Bound::Unbounded, Bound::Excluded);
        let mut ids: Vec<&String> = s
            .orgs
            .range((start, Bound::Unbounded))
            .map(|(id, _)| id)
            .take(limit.saturating_add(1))
            .collect();
        let next = (ids.len() > limit).then(|| {
            ids.truncate(limit);
            ids.last().map(|id| (*id).clone())
        });
        let orgs: Vec<Value> = ids
            .into_iter()
            .map(|id| {
                json!({
                    "org": id,
                    "approvedReleases": report::approved_releases(s.approved_for(id)),
                })
            })
            .collect();
        (orgs, next.flatten(), s.governance())
    });
    Ok(Json(json!({
        "fresh": fresh,
        "governance": governance,
        "orgs": orgs,
        "next": next,
    })))
}

#[derive(Deserialize)]
pub(super) struct NodeBody {
    id: u64,
    addr: String,
    #[serde(default)]
    zone: String,
    #[serde(default, rename = "operatorProof")]
    operator_proof: Option<OperatorProofBody>,
    #[serde(default)]
    role: NodeRole,
    #[serde(default)]
    pool: String,
}

/// Register a node: the operator's decision, signed
/// (`cluster / __register_node / { id, addr, zone }`, plus `role` for a job node; 428 first).
async fn ops_register_node(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(b): Json<NodeBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    let st = node.system.status();
    let member = st.voters.iter().chain(&st.learners).any(|m| *m == b.id)
        || node.config.peers.contains_key(&b.id);
    if b.role == NodeRole::Job && member {
        return Err(ApiError::bad_request(
            "NODE_HOLDS_CLUSTER",
            "that node is a member of this cluster: it cannot be a job node",
        ));
    }
    if !matches!(
        url::Url::parse(&b.addr)
            .map(|u| u.scheme().to_owned())
            .as_deref(),
        Ok("http" | "https")
    ) {
        return Err(ApiError::bad_request(
            "BAD_ADDRESS",
            "a node's address is an http or https URL",
        ));
    }
    let wanted = crate::system::normalize_addr(&b.addr);
    let taken = node
        .config
        .peers
        .iter()
        .any(|(id, addr)| *id != b.id && crate::system::normalize_addr(addr) == wanted)
        || node.sys(|s| {
            s.nodes
                .values()
                .any(|n| n.id != b.id && crate::system::normalize_addr(&n.addr) == wanted)
        });
    if taken {
        return Err(ApiError::new(
            409,
            "ADDRESS_TAKEN",
            "another node has that address",
        ));
    }
    // Only a job node has a pool, and registering one already waits for every node to upgrade.
    if b.pool.len() > crate::system::MAX_POOL_NAME_BYTES {
        return Err(ApiError::bad_request(
            "BAD_POOL",
            "a pool's name is at most 64 bytes",
        ));
    }
    if b.role != NodeRole::Job && !b.pool.is_empty() {
        return Err(ApiError::bad_request(
            "POOL_FOR_JOB_NODES",
            "only a job node is registered in a pool",
        ));
    }
    if b.role == NodeRole::Job {
        node.require_upgraded_nodes().await?;
    }
    let mut args = json!({ "id": b.id, "addr": b.addr, "zone": b.zone });
    if b.role != NodeRole::Replica {
        args["role"] = json!(b.role);
    }
    if !b.pool.is_empty() {
        args["pool"] = json!(b.pool);
    }
    let proof = operator_proof(&node, "cluster", "__register_node", &args, b.operator_proof)?;
    let _ok = node
        .sys_write(&SystemCmd::RegisterNode {
            node: crate::types::NodeRecord {
                id: b.id,
                addr: b.addr,
                zone: b.zone,
                role: b.role,
                pool: b.pool,
            },
            proof,
        })
        .await?;
    Ok(Json(json!({})))
}

/// Remove a node: out of the directory and the system group. Refused while an
/// app keeps a replica on it (move them with `/ops/apps/{app}/replicas`). Runs
/// on the system leader; any other node answers `421 NOT_LEADER`. A registered
/// learner takes the removed voter's place while fewer than 3 voters are left.
async fn ops_remove_node(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    body: axum::body::Bytes,
) -> ApiResult {
    operator(&node, &headers)?;
    // Signed (`cluster / __remove_node / { id }`, 428 first) before the
    // leader redirect, so the signed retry can go to the leader as it is.
    let given = ops_proof_body(&body)?.operator_proof;
    let proof = operator_proof(
        &node,
        "cluster",
        "__remove_node",
        &json!({ "id": id }),
        given,
    )?;
    let st = node.system.status();
    if !st.is_leader {
        let addr = st
            .leader
            .and_then(|l| node.sys(|s| s.nodes.get(&l).map(|n| (l, n.addr.clone()))));
        return Err(membership_err(&calimero_log::WriteError::NotLeader {
            leader: addr,
        }));
    }
    let mut voters: std::collections::BTreeSet<u64> =
        st.voters.iter().copied().filter(|v| *v != id).collect();
    if voters.is_empty() {
        return Err(ApiError::bad_request(
            "LAST_NODE",
            "the cluster's last voter cannot be removed",
        ));
    }
    let in_membership = st.voters.iter().chain(&st.learners).any(|m| *m == id);
    let _ok = node.sys_write(&SystemCmd::RemoveNode { id, proof }).await?;
    if !in_membership {
        return Ok(Json(json!({ "status": node.system.status() })));
    }
    let learners: Vec<u64> = node.sys(|s| {
        st.learners
            .iter()
            .copied()
            .filter(|l| *l != id && s.replica_node(l).is_some())
            .collect()
    });
    for l in learners {
        if voters.len() >= 3 {
            break;
        }
        let _new = voters.insert(l);
    }
    node.system
        .change_voters(voters)
        .await
        .map_err(|e| membership_err(&e))?;
    Ok(Json(json!({ "status": node.system.status() })))
}

#[derive(Deserialize)]
pub(super) struct ReplicaBody {
    replicas: Vec<u64>,
    #[serde(default, rename = "operatorProof")]
    operator_proof: Option<OperatorProofBody>,
}

/// A membership change failed; a follower names the leader to retry at.
fn membership_err(e: &calimero_log::WriteError) -> ApiError {
    match e {
        calimero_log::WriteError::NotLeader {
            leader: Some((id, addr)),
        } => not_leader("send this to the app group's leader", Some(*id), addr),
        _ => ApiError::unavailable(&e.to_string()),
    }
}

/// Move an app to a new replica set: add learners, wait, then switch voters.
/// Runs on the app group's leader; any other replica answers `421 NOT_LEADER`.
/// Safe for a rebalancer to repeat or race: a move to the set the app already
/// has answers `unchanged: true` and writes nothing, and a second move of the
/// same app while one runs here is refused `409 MOVE_IN_PROGRESS`.
async fn ops_set_replicas(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(b): Json<ReplicaBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    operator(&node, &headers)?;
    let target: std::collections::BTreeSet<u64> = b.replicas.iter().copied().collect();
    if target.len() != b.replicas.len() {
        return Err(ApiError::bad_request(
            "DUPLICATE_REPLICA",
            "a node is named twice",
        ));
    }
    if target.len() < 3.min(node.sys(|s| s.replica_nodes().count())) {
        return Err(ApiError::bad_request(
            "TOO_FEW",
            "an app keeps at least 3 replicas",
        ));
    }
    if let Some(id) = node.sys(|s| target.iter().copied().find(|id| !s.nodes.contains_key(id))) {
        return Err(ApiError::bad_request(
            "UNKNOWN_NODE",
            &format!("node {id} is not registered"),
        ));
    }
    if let Some(id) = node.sys(|s| {
        target
            .iter()
            .copied()
            .find(|id| s.replica_node(id).is_none())
    }) {
        return Err(ApiError::bad_request(
            "JOB_NODE",
            &format!("node {id} is a job node: it holds no app"),
        ));
    }
    // One signature over the target covers both of the move's writes
    // (`system::authorize`); asked for before the leader redirect, so the
    // signed retry goes to the leader as it is.
    let proof = operator_proof(
        &node,
        &app_id,
        "__set_replicas",
        &json!({ "replicas": target }),
        b.operator_proof,
    )?;
    let app = node.app(&app_id).await?;
    // The operator moves an app only onto nodes on a release its owner
    // approved: a move is how data would reach a new image.
    for id in &target {
        let release = node.release_of_node(*id).await;
        if !node.sys(|s| s.release_may_hold(&app, release.as_deref())) {
            return Err(ApiError::forbidden(&format!(
                "node {id} runs release {}, which this app's owner has not approved",
                release.as_deref().unwrap_or("(not attested yet)")
            )));
        }
    }
    // Nor onto a node whose runtime is older than the app's release needs:
    // it could not run the app. Nodes it is on already are not judged, so an
    // app on such a node can always be moved off it.
    let joining: Vec<u64> = target
        .iter()
        .copied()
        .filter(|id| !app.replicas.contains(id))
        .collect();
    node.require_runtime(
        &joining,
        app.release.as_ref(),
        &app.bundle,
        &release_label(app.release.as_ref(), &app.bundle),
    )
    .await?;
    // Only the leader changes membership; others redirect before writing
    // anything. A node holding no replica names a current replica, which
    // redirects again if it does not lead.
    let to = match node.hosted(&app_id).map(|g| g.status()) {
        Some(st) if st.is_leader => None,
        Some(st) => Some(st.leader),
        None => Some(app.replicas.first().copied()),
    };
    if let Some(id) = to {
        let addr = id.and_then(|id| node.sys(|s| s.nodes.get(&id).map(|n| (id, n.addr.clone()))));
        return Err(membership_err(&calimero_log::WriteError::NotLeader {
            leader: addr,
        }));
    }
    // One move per app at a time: two would each record their own union and
    // final set, and the directory could end naming neither group's voters.
    let Some(moving) = node.begin_move(&app_id) else {
        return Err(ApiError::new(
            409,
            "MOVE_IN_PROGRESS",
            "this app's replicas are being moved; retry once that move answers",
        ));
    };
    // The move runs to the end in a task of its own, which holds the guard:
    // a caller that disconnects does not leave it half done and unguarded.
    tokio::spawn(move_replicas(node, app_id, target, proof, moving))
        .await
        .map_err(|e| ApiError::internal(&e.to_string()))?
}

/// The body of [`ops_set_replicas`], on the app group's leader, under `_moving`.
async fn move_replicas(
    node: Arc<Node>,
    app_id: String,
    target: std::collections::BTreeSet<u64>,
    proof: Value,
    _moving: crate::node::MoveGuard,
) -> ApiResult {
    // The replica set as of now, not as of before the guard.
    let _caught_up = node.sys_fresh().await;
    let app = node.app(&app_id).await?;
    let g = node
        .hosted(&app_id)
        .ok_or_else(|| ApiError::bad_request("NOT_HOSTED", "send this to a current replica"))?;
    let current: std::collections::BTreeSet<u64> = app.replicas.iter().copied().collect();
    let st = g.status();
    let voters: std::collections::BTreeSet<u64> = st.voters.iter().copied().collect();
    if current == target && voters == target {
        return Ok(Json(json!({ "status": st, "unchanged": true })));
    }
    let union: Vec<u64> = current.union(&target).copied().collect();
    // Already the target (the move only adds nodes): this write spends the
    // signature, and the final one is not needed.
    let grows = union.len() == target.len();
    // Every node of the union starts the group before membership changes.
    let _ok = node
        .sys_write(&SystemCmd::SetReplicas {
            app_id: app_id.clone(),
            replicas: union,
            proof: proof.clone(),
        })
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let addrs: Vec<(u64, String)> = node.sys(|s| {
        target
            .iter()
            .filter_map(|r| s.nodes.get(r).map(|n| (*r, n.addr.clone())))
            .collect()
    });
    // The new replicas get the bundle and blobs before they can vote.
    let joining: Vec<(u64, String)> = addrs
        .iter()
        .filter(|(id, _)| !current.contains(id))
        .cloned()
        .collect();
    let handed = node
        .hand_over_bytes(&app, &g, &joining, &std::collections::HashSet::new())
        .await?;
    for (id, addr) in addrs {
        if !g.status().voters.contains(&id) {
            g.add_learner(id, addr)
                .await
                .map_err(|e| membership_err(&e))?;
        }
    }
    g.change_voters(target.clone())
        .await
        .map_err(|e| membership_err(&e))?;
    // Files made while the move ran may have reached a majority that left the joining nodes
    // out: hand those over before the old nodes go, but never abandon the move for it.
    let again = crate::node::retry_within(HAND_OVER_AGAIN_WITHIN, HAND_OVER_AGAIN_PAUSE, || {
        node.hand_over_bytes(&app, &g, &joining, &handed)
    })
    .await;
    if let Err(e) = again {
        warn!(app = %app_id, error = %e, "files made during the move were not handed over");
    }
    if !grows {
        let _ok = node
            .sys_write(&SystemCmd::SetReplicas {
                app_id,
                replicas: target.into_iter().collect(),
                proof,
            })
            .await?;
    }
    Ok(Json(json!({ "status": g.status(), "unchanged": false })))
}

#[derive(Deserialize)]
pub(super) struct PoolsBody {
    pools: std::collections::BTreeMap<String, crate::system::PoolConfig>,
    #[serde(default, rename = "operatorProof")]
    operator_proof: Option<OperatorProofBody>,
}

/// Set the compute pools: the operator's decision, signed (`cluster / __set_pools / { pools }`).
async fn ops_pools_set(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(b): Json<PoolsBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    node.require_current_release("node pools").await?;
    let proof = operator_proof(
        &node,
        "cluster",
        "__set_pools",
        &json!({ "pools": b.pools }),
        b.operator_proof,
    )?;
    let _ok = node
        .sys_write(&SystemCmd::SetPools {
            pools: b.pools,
            proof,
        })
        .await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
pub(super) struct PricesBody {
    table: calimero_pipeline::PriceTable,
    #[serde(default, rename = "operatorProof")]
    operator_proof: Option<OperatorProofBody>,
}

/// Set the price table: the operator's decision, signed (`cluster / __set_price_table / { table }`).
async fn ops_prices_set(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(b): Json<PricesBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    node.require_current_release("a price table").await?;
    let proof = operator_proof(
        &node,
        "cluster",
        "__set_price_table",
        &json!({ "table": b.table }),
        b.operator_proof,
    )?;
    let _ok = node
        .sys_write(&SystemCmd::SetPriceTable {
            table: b.table,
            proof,
        })
        .await?;
    Ok(Json(json!({})))
}

/// The price table, or `{}` until one is set.
async fn ops_prices_get(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    operator(&node, &headers)?;
    Ok(Json(
        node.sys(|s| s.price_table)
            .map_or_else(|| json!({}), |t| json!(t)),
    ))
}

/// The pools and the runs each takes at once.
async fn ops_pools_get(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    operator(&node, &headers)?;
    // An operator reads back what it just set, through any node.
    let _caught_up = node.sys_fresh().await;
    Ok(Json(node.sys(|s| {
        let capacity: std::collections::BTreeMap<&String, usize> =
            s.pools.keys().map(|p| (p, s.pool_capacity(p))).collect();
        json!({ "pools": s.pools, "capacity": capacity })
    })))
}

async fn ops_quotas_get(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    operator(&node, &headers)?;
    // An operator reads back what it just set, through any node.
    let _caught_up = node.sys_fresh().await;
    let (quotas, orgs) = node.sys(|s| (s.quotas.clone(), s.org_quotas.clone()));
    let mut shown = serde_json::to_value(quotas).unwrap_or_default();
    shown["orgs"] = serde_json::to_value(orgs).unwrap_or_default();
    Ok(Json(shown))
}

/// `POST /ops/quotas` with `org`: change the limits named for one payer; a field set to `null`
/// goes back to the cluster's. Signed (`cluster / __set_org_quotas / { payer, quotas }`).
async fn ops_org_quotas_set(
    node: &Node,
    payer: &str,
    changes: serde_json::Map<String, Value>,
    given: Option<OperatorProofBody>,
) -> ApiResult {
    node.require_current_release("org quotas").await?;
    node.sys_fresh().await;
    let mut merged = serde_json::to_value(
        node.sys(|s| s.org_quotas.get(payer).cloned())
            .unwrap_or_default(),
    )
    .map_err(|e| ApiError::internal(&e.to_string()))?;
    let limits = merged
        .as_object_mut()
        .ok_or_else(|| ApiError::internal("quotas are not an object"))?;
    for (k, v) in changes {
        if !["maxConcurrentRuns", "computeFuel", "maxFileBytes"].contains(&k.as_str()) {
            return Err(ApiError::bad_request(
                "UNKNOWN_QUOTA",
                &format!("no org quota named {k}"),
            ));
        }
        if v.is_null() {
            let _gone = limits.remove(&k);
        } else {
            let _prev = limits.insert(k, v);
        }
    }
    let quotas: crate::system::OrgQuotas = serde_json::from_value(merged)
        .map_err(|e| ApiError::bad_request("BAD_REQUEST", &e.to_string()))?;
    let proof = operator_proof(
        node,
        "cluster",
        "__set_org_quotas",
        &json!({ "payer": payer, "quotas": quotas }),
        given,
    )?;
    let _ok = node
        .sys_write(&SystemCmd::SetOrgQuotas {
            payer: payer.to_owned(),
            quotas,
            proof,
        })
        .await?;
    Ok(Json(json!({})))
}

/// Change the limits the body names; the others stay as they are. A key that
/// names no limit is refused (a typo would otherwise change nothing silently).
async fn ops_quotas_set(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(mut changes): Json<serde_json::Map<String, Value>>,
) -> ApiResult {
    operator(&node, &headers)?;
    let given: Option<OperatorProofBody> = changes
        .remove("operatorProof")
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| ApiError::bad_request("BAD_REQUEST", &e.to_string()))?;
    // `orgs` is what GET answers; a read-modify-write of that answer leaves the overrides alone.
    let _read_only = changes.remove("orgs");
    if let Some(org) = changes.remove("org") {
        let payer = org
            .as_str()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| ApiError::bad_request("BAD_REQUEST", "org is a payer id"))?
            .to_owned();
        return ops_org_quotas_set(&node, &payer, changes, given).await;
    }
    node.sys_fresh().await;
    let mut merged = serde_json::to_value(node.sys(|s| s.quotas.clone()))
        .map_err(|e| ApiError::internal(&e.to_string()))?;
    let limits = merged
        .as_object_mut()
        .ok_or_else(|| ApiError::internal("quotas are not an object"))?;
    for (k, v) in changes {
        if !limits.contains_key(&k) {
            return Err(ApiError::bad_request(
                "UNKNOWN_QUOTA",
                &format!("no quota named {k}"),
            ));
        }
        let _prev = limits.insert(k, v);
    }
    let quotas: crate::system::Quotas = serde_json::from_value(merged)
        .map_err(|e| ApiError::bad_request("BAD_REQUEST", &e.to_string()))?;
    // A fuel cap is written only once every node runs a release that knows it: an
    // older node drops the field, and its snapshots would bring the cap back to 0.
    // Judged before the operator is asked to sign, so a refused write is never signed.
    if quotas.max_fuel_per_developer_per_day != 0 {
        node.require_current_release("a fuel cap").await?;
    }
    // Signed over the whole merged limits (`cluster / __set_quotas /
    // { quotas }`, 428 first), so a change made in between is not signed
    // over by mistake: the retry merges again and must match.
    let proof = operator_proof(
        &node,
        "cluster",
        "__set_quotas",
        &json!({ "quotas": quotas }),
        given,
    )?;
    let _ok = node
        .sys_write(&SystemCmd::SetQuotas { quotas, proof })
        .await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BackupQuery {
    /// Every group this node holds, leader or not (`Node::backup_everything`).
    #[serde(default)]
    all: bool,
}

async fn ops_backup(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Query(q): Query<BackupQuery>,
) -> ApiResult {
    operator(&node, &headers)?;
    let n = if q.all {
        node.backup_everything().await
    } else {
        node.backup().await
    }
    .map_err(|e| ApiError::internal(&e.to_string()))?;
    Ok(Json(json!({ "written": n })))
}

/// This area's routes, merged into [`super::router`].
pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route("/ops/status", get(ops_status))
        .route("/ops/cluster", get(ops_cluster))
        .route("/ops/metrics", get(ops_metrics))
        .route("/ops/placement", get(ops_placement))
        .route("/ops/apps", get(ops_apps))
        .route("/ops/orgs", get(ops_orgs))
        .route("/ops/nodes", post(ops_register_node))
        .route("/ops/nodes/{id}", delete(ops_remove_node))
        .route("/ops/apps/{app}/replicas", post(ops_set_replicas))
        .route("/ops/backup", post(ops_backup))
        .route("/ops/quotas", get(ops_quotas_get).post(ops_quotas_set))
        .route("/ops/pools", get(ops_pools_get).post(ops_pools_set))
        .route("/ops/prices", get(ops_prices_get).post(ops_prices_set))
}
