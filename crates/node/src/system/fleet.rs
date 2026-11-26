//! System commands for the fleet: nodes, quotas, pools, genesis, the cluster version.
//!
//! [`authorize`] judges the proof each carries and [`apply`] applies it;
//! `system.rs` sends a command here by its [`super::Area`]. Each arm of either
//! is one call to a function of its own, in the order of the arms, and each
//! refusal of this area's own rules is a named [`Rejection`] below.

#[allow(
    clippy::wildcard_imports,
    reason = "the arms moved here from `system.rs` read its items"
)]
use super::*;

// This area's refusals. Each is `400 REJECTED`, the kind the bare message it
// names was answered with; the message is what a client reads.

/// A node registered again with another role.
const ROLE_FIXED: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a node's role is fixed: remove it first",
);
/// A node registered at an address another node holds.
const ADDRESS_TAKEN: Rejection =
    Rejection::new(RejectionKind::Rejected, "another node has that address");
/// A node removed while an app keeps a replica on it.
const NODE_KEEPS_A_REPLICA: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "an app still keeps a replica on that node; move it first",
);
/// A node removed that the fleet does not hold.
const NO_SUCH_NODE: Rejection = Rejection::new(RejectionKind::Rejected, "no such node");
/// A payer's quotas named with no payer or more than [`MAX_PAYER_BYTES`].
const BAD_PAYER: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a payer is named, in at most 128 bytes",
);
/// A second genesis, or one after an account exists.
const GOVERNANCE_FIXED: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "the cluster's governance is fixed at genesis",
);
/// An operator key that is not 32 bytes of hex.
const BAD_OPERATOR_KEY: Rejection =
    Rejection::new(RejectionKind::Rejected, "an operator key is 32 bytes, hex");
/// `RequireSessionProofs` after its mark.
const SESSION_PROOFS_REQUIRED: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "session proofs are required already",
);
/// `AttributeByRelease` after its mark.
const ATTRIBUTED_BY_RELEASE: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "apps are judged by their release already",
);
/// `RequireCurrentRules` after its mark.
const CURRENT_RULES_REQUIRED: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "the current rules are required already",
);
/// `ExpireProofs` after its mark.
const PROOFS_EXPIRE: Rejection =
    Rejection::new(RejectionKind::Rejected, "sign-in proofs expire already");

/// Judge the proof a command of this area carries ([`super::authorize`]):
/// every change to the fleet carries the operator's signature.
pub(super) fn authorize(proofs: &Proofs<'_>, cmd: &SystemCmd) -> Result<Option<String>, Rejection> {
    match cmd {
        SystemCmd::RegisterNode { node, proof } => {
            operator_signed(proofs, proof, "__register_node", &register_node_args(node))
        }
        SystemCmd::RemoveNode { id, proof } => {
            operator_signed(proofs, proof, "__remove_node", &json!({ "id": id }))
        }
        SystemCmd::SetQuotas { quotas, proof } => {
            operator_signed(proofs, proof, "__set_quotas", &json!({ "quotas": quotas }))
        }
        SystemCmd::SetOrgQuotas {
            payer,
            quotas,
            proof,
        } => operator_signed(
            proofs,
            proof,
            "__set_org_quotas",
            &json!({ "payer": payer, "quotas": quotas }),
        ),
        SystemCmd::SetPools { pools, proof } => {
            operator_signed(proofs, proof, "__set_pools", &json!({ "pools": pools }))
        }
        SystemCmd::SetPriceTable { table, proof } => operator_signed(
            proofs,
            proof,
            "__set_price_table",
            &json!({ "table": table }),
        ),
        _ => Ok(None),
    }
}

/// The operator's signature, not used before, stating `method` on the
/// cluster with exactly the arguments `stated`.
fn operator_signed(
    proofs: &Proofs<'_>,
    proof: &Value,
    method: &str,
    stated: &Value,
) -> Result<Option<String>, Rejection> {
    let (statement, used) = proofs.fresh(proofs.operator(proof)?)?;
    proofs.states(&statement, "cluster", method, &|args| args == stated)?;
    Ok(Some(used))
}

/// What the operator signs to register a node: its role only when not a
/// replica, its pool only when set.
fn register_node_args(node: &NodeRecord) -> Value {
    let mut args = json!({ "id": node.id, "addr": node.addr, "zone": node.zone });
    if node.role != NodeRole::Replica {
        args["role"] = json!(node.role);
    }
    if !node.pool.is_empty() {
        args["pool"] = json!(node.pool);
    }
    args
}

/// Apply a command of this area by its own rules, its proof already
/// judged ([`SystemMachine::run_authorized`]).
pub(super) fn apply(m: &mut SystemMachine, cmd: SystemCmd) -> Result<Value, Rejection> {
    let index = m.index;
    let state = &mut m.state;
    match cmd {
        SystemCmd::RegisterNode { node, .. } => register_node(state, node),
        SystemCmd::SetPriceTable { table, .. } => set_price_table(state, table),
        SystemCmd::RemoveNode { id, .. } => remove_node(state, id),
        SystemCmd::SetQuotas { quotas, .. } => {
            state.quotas = quotas;
            state.grant_runs();
            Ok(Value::Null)
        }
        SystemCmd::SetOrgQuotas { payer, quotas, .. } => set_org_quotas(state, payer, quotas),
        SystemCmd::SetPools { pools, .. } => set_pools(state, pools),
        SystemCmd::Genesis {
            governance,
            release,
            operator_key,
            nodes,
            session_proofs,
        } => genesis(
            state,
            index,
            governance,
            release,
            operator_key,
            nodes,
            session_proofs,
        ),
        SystemCmd::RequireSessionProofs => {
            if state.session_proofs_from.is_some() {
                return Err(SESSION_PROOFS_REQUIRED);
            }
            state.session_proofs_from = Some(index);
            Ok(Value::Null)
        }
        SystemCmd::AttributeByRelease => {
            if state.attributes_by_release {
                return Err(ATTRIBUTED_BY_RELEASE);
            }
            state.attributes_by_release = true;
            Ok(Value::Null)
        }
        SystemCmd::RequireCurrentRules => {
            if state.current_rules_from.is_some() {
                return Err(CURRENT_RULES_REQUIRED);
            }
            state.current_rules_from = Some(index);
            Ok(Value::Null)
        }
        SystemCmd::ExpireProofs => {
            if state.expires_proofs {
                return Err(PROOFS_EXPIRE);
            }
            state.expires_proofs = true;
            Ok(Value::Null)
        }
        SystemCmd::SetClusterVersion { version, nodes } => {
            cluster_version::apply(state, version, &nodes)
        }
        // SAFETY: `SystemCmd::area` sends a command only to the module of
        // its area, and this match has an arm for each command of it.
        _ => unreachable!("a command of another area reached fleet::apply"),
    }
}

/// Register a node (again): its role fixed, its address its own; a new epoch
/// for its id, and no longer removed.
fn register_node(state: &mut SystemState, node: NodeRecord) -> Result<Value, Rejection> {
    if state
        .nodes
        .get(&node.id)
        .is_some_and(|known| known.role != node.role)
    {
        return Err(ROLE_FIXED);
    }
    let addr = normalize_addr(&node.addr);
    if state
        .nodes
        .values()
        .any(|other| other.id != node.id && normalize_addr(&other.addr) == addr)
    {
        return Err(ADDRESS_TAKEN);
    }
    *state.node_epoch.entry(node.id).or_default() += 1;
    let _was = state.removed_nodes.remove(&node.id);
    let _prev = state.nodes.insert(node.id, node);
    state.grant_runs();
    Ok(Value::Null)
}

/// Set the price table: a valid one, the next version.
fn set_price_table(state: &mut SystemState, table: PriceTable) -> Result<Value, Rejection> {
    if table.validate().is_err() {
        return Err(BAD_PRICES.into());
    }
    if table.version != state.price_table.map_or(0, |current| current.version) + 1 {
        return Err(PRICE_VERSION.into());
    }
    state.price_table = Some(table);
    Ok(Value::Null)
}

/// Remove a node no app keeps a replica on; a new epoch for its id.
fn remove_node(state: &mut SystemState, id: u64) -> Result<Value, Rejection> {
    if state.apps.values().any(|app| app.replicas.contains(&id)) {
        return Err(NODE_KEEPS_A_REPLICA);
    }
    let _gone = state.nodes.remove(&id).ok_or(NO_SUCH_NODE)?;
    *state.node_epoch.entry(id).or_default() += 1;
    let _new = state.removed_nodes.insert(id);
    state.grant_runs();
    Ok(Value::Null)
}

/// Set (or, with every field cleared, drop) a payer's quota override, within
/// [`MAX_ORG_QUOTAS`] overrides.
fn set_org_quotas(
    state: &mut SystemState,
    payer: String,
    quotas: OrgQuotas,
) -> Result<Value, Rejection> {
    if payer.is_empty() || payer.len() > MAX_PAYER_BYTES {
        return Err(BAD_PAYER);
    }
    if !state.org_quotas.contains_key(&payer) && state.org_quotas.len() >= MAX_ORG_QUOTAS {
        return Err(Rejection::quota("org quota overrides"));
    }
    if quotas == OrgQuotas::default() {
        let _gone = state.org_quotas.remove(&payer);
    } else {
        let _prev = state.org_quotas.insert(payer, quotas);
    }
    state.grant_runs();
    Ok(Value::Null)
}

/// Set the pools: within the bounds, and never dropping a pool that still has
/// job nodes or runs.
fn set_pools(
    state: &mut SystemState,
    pools: BTreeMap<String, PoolConfig>,
) -> Result<Value, Rejection> {
    if pools.len() > MAX_POOLS
        || pools
            .keys()
            .any(|name| name.is_empty() || name.len() > MAX_POOL_NAME_BYTES)
    {
        return Err(BAD_POOLS.into());
    }
    let in_use = state
        .pools
        .keys()
        .filter(|name| !pools.contains_key(*name))
        .find(|name| pool_in_use(state, name));
    if in_use.is_some() {
        return Err(POOL_IN_USE.into());
    }
    state.pools = pools;
    state.grant_runs();
    Ok(Value::Null)
}

/// Whether a job node is in the pool or a run slot is placed in it.
fn pool_in_use(state: &SystemState, name: &str) -> bool {
    state
        .job_nodes()
        .any(|node| SystemState::node_pool(node) == name)
        || state
            .compute
            .values()
            .flat_map(BTreeMap::values)
            .any(|slot| state.slot_pool(slot) == name)
}

/// The cluster's genesis, once and before any account: its governance, the
/// operator's key, the nodes it starts with, and whether session proofs are
/// required from here on.
fn genesis(
    state: &mut SystemState,
    index: u64,
    governance: Governance,
    release: Option<String>,
    operator_key: Option<crate::proof::OperatorKey>,
    nodes: Vec<NodeRecord>,
    session_proofs: bool,
) -> Result<Value, Rejection> {
    if state.governance.is_some() || !state.accounts.is_empty() {
        return Err(GOVERNANCE_FIXED);
    }
    if let Some(key) = &operator_key {
        hex32(&key.key).ok_or(BAD_OPERATOR_KEY)?;
    }
    state.operator_key = operator_key;
    if governance.releases == Authority::Operator {
        state.operator_releases = release.into_iter().collect();
    }
    state.governance = Some(governance.clone());
    for node in nodes {
        let _prev = state.nodes.insert(node.id, node);
    }
    if session_proofs {
        state.session_proofs_from = Some(index);
    }
    Ok(json!({ "governance": governance }))
}
