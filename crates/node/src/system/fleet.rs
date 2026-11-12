//! System commands for the fleet: nodes, quotas, pools, genesis, the cluster version.
//!
//! [`authorize`] judges the proof each carries and [`apply`] applies it;
//! `system.rs` sends a command here by its [`super::Area`].

#[allow(
    clippy::wildcard_imports,
    reason = "the arms moved here from `system.rs` read its items"
)]
use super::*;

/// Judge the proof a command of this area carries ([`super::authorize`]).
pub(super) fn authorize(
    proofs: &Proofs<'_>,
    cmd: &SystemCmd,
) -> Result<Option<String>, &'static str> {
    match cmd {
        // The fleet: the operator's signature.
        SystemCmd::RegisterNode { node, proof } => {
            let (st, used) = proofs.fresh(proofs.operator(proof)?)?;
            let mut args = json!({ "id": node.id, "addr": node.addr, "zone": node.zone });
            if node.role != NodeRole::Replica {
                args["role"] = json!(node.role);
            }
            if !node.pool.is_empty() {
                args["pool"] = json!(node.pool);
            }
            proofs.states(&st, "cluster", "__register_node", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::RemoveNode { id, proof } => {
            let (st, used) = proofs.fresh(proofs.operator(proof)?)?;
            proofs.states(&st, "cluster", "__remove_node", &|a| {
                *a == json!({ "id": id })
            })?;
            Ok(Some(used))
        }
        SystemCmd::SetQuotas { quotas, proof } => {
            let (st, used) = proofs.fresh(proofs.operator(proof)?)?;
            let args = json!({ "quotas": quotas });
            proofs.states(&st, "cluster", "__set_quotas", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::SetOrgQuotas {
            payer,
            quotas,
            proof,
        } => {
            let (st, used) = proofs.fresh(proofs.operator(proof)?)?;
            let args = json!({ "payer": payer, "quotas": quotas });
            proofs.states(&st, "cluster", "__set_org_quotas", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::SetPools { pools, proof } => {
            let (st, used) = proofs.fresh(proofs.operator(proof)?)?;
            let args = json!({ "pools": pools });
            proofs.states(&st, "cluster", "__set_pools", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::SetPriceTable { table, proof } => {
            let (st, used) = proofs.fresh(proofs.operator(proof)?)?;
            let args = json!({ "table": table });
            proofs.states(&st, "cluster", "__set_price_table", &|a| *a == args)?;
            Ok(Some(used))
        }
        _ => Ok(None),
    }
}

/// Apply a command of this area by its own rules, its proof already
/// judged ([`SystemMachine::run_authorized`]).
#[allow(clippy::too_many_lines, reason = "one arm per command")]
pub(super) fn apply(m: &mut SystemMachine, cmd: SystemCmd) -> Result<Value, &'static str> {
    let index = m.index;
    let s = &mut m.state;
    match cmd {
        SystemCmd::RegisterNode { node, .. } => {
            if s.nodes.get(&node.id).is_some_and(|n| n.role != node.role) {
                return Err("a node's role is fixed: remove it first");
            }
            let addr = normalize_addr(&node.addr);
            if s.nodes
                .values()
                .any(|n| n.id != node.id && normalize_addr(&n.addr) == addr)
            {
                return Err("another node has that address");
            }
            *s.node_epoch.entry(node.id).or_default() += 1;
            let _was = s.removed_nodes.remove(&node.id);
            let _prev = s.nodes.insert(node.id, node);
            s.grant_runs();
            Ok(Value::Null)
        }
        SystemCmd::SetPriceTable { table, .. } => {
            if table.validate().is_err() {
                return Err(BAD_PRICES);
            }
            if table.version != s.price_table.map_or(0, |t| t.version) + 1 {
                return Err(PRICE_VERSION);
            }
            s.price_table = Some(table);
            Ok(Value::Null)
        }
        SystemCmd::RemoveNode { id, .. } => {
            if s.apps.values().any(|a| a.replicas.contains(&id)) {
                return Err("an app still keeps a replica on that node; move it first");
            }
            let _gone = s.nodes.remove(&id).ok_or("no such node")?;
            *s.node_epoch.entry(id).or_default() += 1;
            let _new = s.removed_nodes.insert(id);
            s.grant_runs();
            Ok(Value::Null)
        }
        SystemCmd::SetQuotas { quotas, .. } => {
            s.quotas = quotas;
            s.grant_runs();
            Ok(Value::Null)
        }
        SystemCmd::SetOrgQuotas { payer, quotas, .. } => {
            if payer.is_empty() || payer.len() > MAX_PAYER_BYTES {
                return Err("a payer is named, in at most 128 bytes");
            }
            if !s.org_quotas.contains_key(&payer) && s.org_quotas.len() >= MAX_ORG_QUOTAS {
                return Err("quota exceeded: org quota overrides");
            }
            if quotas == OrgQuotas::default() {
                let _gone = s.org_quotas.remove(&payer);
            } else {
                let _prev = s.org_quotas.insert(payer, quotas);
            }
            s.grant_runs();
            Ok(Value::Null)
        }
        SystemCmd::SetPools { pools, .. } => {
            if pools.len() > MAX_POOLS
                || pools
                    .keys()
                    .any(|n| n.is_empty() || n.len() > MAX_POOL_NAME_BYTES)
            {
                return Err(BAD_POOLS);
            }
            let in_use = s
                .pools
                .keys()
                .filter(|name| !pools.contains_key(*name))
                .find(|name| {
                    s.job_nodes()
                        .any(|n| SystemState::node_pool(n) == name.as_str())
                        || s.compute
                            .values()
                            .flat_map(BTreeMap::values)
                            .any(|r| s.slot_pool(r) == name.as_str())
                });
            if in_use.is_some() {
                return Err(POOL_IN_USE);
            }
            s.pools = pools;
            s.grant_runs();
            Ok(Value::Null)
        }
        SystemCmd::Genesis {
            governance,
            release,
            operator_key,
            nodes,
            session_proofs,
        } => {
            if s.governance.is_some() || !s.accounts.is_empty() {
                return Err("the cluster's governance is fixed at genesis");
            }
            if let Some(k) = &operator_key {
                hex32(&k.key).ok_or("an operator key is 32 bytes, hex")?;
            }
            s.operator_key = operator_key;
            if governance.releases == Authority::Operator {
                s.operator_releases = release.into_iter().collect();
            }
            s.governance = Some(governance.clone());
            for node in nodes {
                let _prev = s.nodes.insert(node.id, node);
            }
            if session_proofs {
                s.session_proofs_from = Some(index);
            }
            Ok(json!({ "governance": governance }))
        }
        SystemCmd::RequireSessionProofs => {
            if s.session_proofs_from.is_some() {
                return Err("session proofs are required already");
            }
            s.session_proofs_from = Some(index);
            Ok(Value::Null)
        }
        SystemCmd::RequireCurrentRules => {
            if s.current_rules_from.is_some() {
                return Err("the current rules are required already");
            }
            s.current_rules_from = Some(index);
            Ok(Value::Null)
        }
        SystemCmd::SetClusterVersion { version, nodes } => {
            cluster_version::apply(s, version, &nodes)
        }
        // SAFETY: `SystemCmd::area` sends a command only to the module of
        // its area, and this match has an arm for each command of it.
        _ => unreachable!("a command of another area reached fleet::apply"),
    }
}
