//! Keeping root and the tenant groups in step once the control plane shards by
//! tenant.
//!
//! An organization's tenant group is authoritative for the organization's own
//! state (its record, its apps, their invites); root keeps a **mirror** of those
//! records for the cluster-wide machinery that reads them (Raft admission, app
//! hosting and placement, bundle collection, cross-organization reads). The
//! other way round, a tenant's `apply` reads root's cluster-wide registries (the
//! package registry and bundles, the fleet, governance, quotas, deployments),
//! so each tenant holds a **replica** of those. Both directions are written by
//! the node, never by a client: `SyncTenantRoot` brings a tenant's replica to
//! root's records, `MirrorTenant` brings root's mirror to a tenant's records.
//! Each entry names the index of the source it reflects and the index it was
//! computed against (`base`); `apply` refuses one computed against another base
//! (compare-and-set), so a node with a stale view never rolls a newer copy back
//! — it recomputes and tries again. Determinism holds: each shard applies only
//! what its own log carries.
//!
//! Design: <https://github.com/calimero-network/core/blob/master/docs/design/system-group-sharding.md>
//! ("The `Apps` flip: tenants authoritative, root mirrors").

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    BundleRecord, Deployer, DeployerGrant, Deployment, Governance, OrgQuotas, PackageRecord,
    PoolConfig, PublisherBinding, Quotas, Rejection, SystemState,
};
use crate::types::{AppId, AppInvite, AppRecord, NodeRecord, Org, OrgId};

/// The size past which a sync or mirror is split over several entries, well
/// under `calimero_log::MAX_ENTRY_BYTES`.
pub(crate) const SYNC_ENTRY_BYTES: usize = 512 * 1024;

/// One of root's records a tenant replicates: its value at root, or `None` once
/// root dropped it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RootRecord {
    Package {
        name: String,
        record: Option<PackageRecord>,
    },
    Bundle {
        hash: String,
        record: Option<BundleRecord>,
    },
    Node {
        id: u64,
        record: Option<NodeRecord>,
    },
    Deployment {
        id: String,
        record: Option<Deployment>,
    },
    Publisher {
        signer: String,
        record: Option<PublisherBinding>,
    },
    PackageOrigin {
        package: String,
        record: Option<crate::origins::PackageOrigin>,
    },
    /// Another organization's record, as root mirrors it: one a deployment is
    /// owned by or a release serves, which this tenant's rules read (whether a
    /// deployment still serves its offer, who sees a private release).
    Org {
        id: OrgId,
        record: Option<Box<Org>>,
    },
    /// Root's small cluster-wide records, whole.
    Cluster(Box<ClusterRecords>),
}

/// Root's small cluster-wide records a tenant replicates whole: the fleet's
/// bounds and pools, the governance and what it holds, the deployers.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterRecords {
    pub quotas: Quotas,
    pub org_quotas: BTreeMap<String, OrgQuotas>,
    pub pools: BTreeMap<String, PoolConfig>,
    pub price_table: Option<calimero_pipeline::PriceTable>,
    pub removed_nodes: BTreeSet<u64>,
    pub governance: Option<Governance>,
    pub operator_key: Option<crate::proof::OperatorKey>,
    pub operator_releases: BTreeSet<String>,
    pub operator_outbound: BTreeMap<String, crate::types::OutboundApproval>,
    pub release_deadlines: BTreeMap<String, u64>,
    pub unbound_publishers: BTreeMap<String, u64>,
    pub deployers: BTreeMap<String, DeployerGrant>,
    pub cluster_owner: Option<Deployer>,
}

impl ClusterRecords {
    fn of(s: &SystemState) -> Self {
        Self {
            quotas: s.quotas.clone(),
            org_quotas: s.org_quotas.clone(),
            pools: s.pools.clone(),
            price_table: s.price_table,
            removed_nodes: s.removed_nodes.clone(),
            governance: s.governance.clone(),
            operator_key: s.operator_key.clone(),
            operator_releases: s.operator_releases.clone(),
            operator_outbound: s.operator_outbound.clone(),
            release_deadlines: s.release_deadlines.clone(),
            unbound_publishers: s.unbound_publishers.clone(),
            deployers: s.deployers.clone(),
            cluster_owner: s.cluster_owner.clone(),
        }
    }

    fn put(self, s: &mut SystemState) {
        s.quotas = self.quotas;
        s.org_quotas = self.org_quotas;
        s.pools = self.pools;
        s.price_table = self.price_table;
        s.removed_nodes = self.removed_nodes;
        s.governance = self.governance;
        s.operator_key = self.operator_key;
        s.operator_releases = self.operator_releases;
        s.operator_outbound = self.operator_outbound;
        s.release_deadlines = self.release_deadlines;
        s.unbound_publishers = self.unbound_publishers;
        s.deployers = self.deployers;
        s.cluster_owner = self.cluster_owner;
    }
}

/// Whether two records are the same, by what they serialize to: what a replica
/// or a mirror holds is what the log carried.
fn same<T: Serialize>(a: &T, b: &T) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}

/// The `(key, value-or-gone)` changes that bring `held` to `source`.
fn diff_map<K: Ord + Clone, V: Clone + Serialize>(
    held: &BTreeMap<K, V>,
    source: &BTreeMap<K, V>,
) -> Vec<(K, Option<V>)> {
    let mut out: Vec<(K, Option<V>)> = source
        .iter()
        .filter(|(k, v)| held.get(*k).is_none_or(|h| !same(h, *v)))
        .map(|(k, v)| (k.clone(), Some(v.clone())))
        .collect();
    out.extend(
        held.keys()
            .filter(|k| !source.contains_key(*k))
            .map(|k| (k.clone(), None)),
    );
    out
}

/// What a tenant replica `tenant` lacks of `root`'s registries, empty when it
/// holds them all as root does.
#[must_use]
pub fn root_diff(tenant: &SystemState, root: &SystemState, own: &str) -> Vec<RootRecord> {
    let mut out = Vec::new();
    // Other organizations root's registries name: deployment owners and the
    // organizations releases serve. Never this tenant's own record, which is
    // its own.
    let named: BTreeSet<&OrgId> = root
        .deployments
        .values()
        .filter_map(|d| match &d.owner {
            Deployer::Org(o) => Some(o),
            Deployer::Account(_) => None,
        })
        .chain(
            root.packages
                .values()
                .flat_map(|p| p.versions.values())
                .flat_map(|v| v.orgs.iter()),
        )
        .filter(|o| o.as_str() != own)
        .collect();
    let held: BTreeMap<OrgId, Org> = tenant
        .orgs
        .iter()
        .filter(|(id, _)| id.as_str() != own)
        .map(|(id, o)| (id.clone(), o.clone()))
        .collect();
    let source: BTreeMap<OrgId, Org> = named
        .into_iter()
        .filter_map(|id| root.orgs.get(id).map(|o| (id.clone(), o.clone())))
        .collect();
    out.extend(
        diff_map(&held, &source)
            .into_iter()
            .map(|(id, record)| RootRecord::Org {
                id,
                record: record.map(Box::new),
            }),
    );
    out.extend(
        diff_map(&tenant.packages, &root.packages)
            .into_iter()
            .map(|(name, record)| RootRecord::Package { name, record }),
    );
    out.extend(
        diff_map(&tenant.bundles, &root.bundles)
            .into_iter()
            .map(|(hash, record)| RootRecord::Bundle { hash, record }),
    );
    out.extend(
        diff_map(&tenant.nodes, &root.nodes)
            .into_iter()
            .map(|(id, record)| RootRecord::Node { id, record }),
    );
    out.extend(
        diff_map(&tenant.deployments, &root.deployments)
            .into_iter()
            .map(|(id, record)| RootRecord::Deployment { id, record }),
    );
    out.extend(
        diff_map(&tenant.publishers, &root.publishers)
            .into_iter()
            .map(|(signer, record)| RootRecord::Publisher { signer, record }),
    );
    out.extend(
        diff_map(&tenant.package_origins, &root.package_origins)
            .into_iter()
            .map(|(package, record)| RootRecord::PackageOrigin { package, record }),
    );
    let cluster = ClusterRecords::of(root);
    if !same(&ClusterRecords::of(tenant), &cluster) {
        out.push(RootRecord::Cluster(Box::new(cluster)));
    }
    out
}

fn put<K: Ord, V>(map: &mut BTreeMap<K, V>, key: K, record: Option<V>) {
    match record {
        Some(v) => {
            let _prev = map.insert(key, v);
        }
        None => {
            let _gone = map.remove(&key);
        }
    }
}

/// Apply a `SyncTenantRoot` on a tenant state: refused unless computed against
/// the replica as it stands (`base`), then each record replaces the replica's,
/// and the replica reflects root through `through`.
///
/// # Errors
/// The sync was computed against another replica than this one.
pub fn apply_sync(
    s: &mut SystemState,
    own: &str,
    base: u64,
    through: u64,
    records: Vec<RootRecord>,
) -> Result<(), Rejection> {
    if s.root_through != base {
        return Err("a stale root sync: computed against another replica".into());
    }
    for record in records {
        match record {
            RootRecord::Package { name, record } => put(&mut s.packages, name, record),
            RootRecord::Bundle { hash, record } => put(&mut s.bundles, hash, record),
            RootRecord::Node { id, record } => put(&mut s.nodes, id, record),
            RootRecord::Deployment { id, record } => put(&mut s.deployments, id, record),
            RootRecord::Publisher { signer, record } => put(&mut s.publishers, signer, record),
            RootRecord::PackageOrigin { package, record } => {
                put(&mut s.package_origins, package, record);
            }
            RootRecord::Org { id, record } => {
                // Never the tenant's own record, which this tenant writes.
                if id != own {
                    put(&mut s.orgs, id, record.map(|o| *o));
                }
            }
            RootRecord::Cluster(c) => c.put(s),
        }
    }
    s.root_through = s.root_through.max(through);
    Ok(())
}

/// One of a tenant's records root mirrors: its value at the tenant, or `None`
/// once the tenant dropped it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TenantRecord {
    Org {
        record: Option<Box<Org>>,
    },
    App {
        id: AppId,
        record: Option<Box<AppRecord>>,
        /// Gone from this organization to another (`ReleaseApp`): root
        /// re-homes it rather than deleting it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        moved_to: Option<OrgId>,
    },
    AppInvite {
        key: String,
        record: Option<AppInvite>,
    },
}

/// What root's mirror of organization `org` lacks of its tenant's records
/// (`tenant`), empty when it mirrors them all.
#[must_use]
pub fn mirror_diff(org: &str, root: &SystemState, tenant: &SystemState) -> Vec<TenantRecord> {
    let mut out = Vec::new();
    let held = root.orgs.get(org);
    let source = tenant.orgs.get(org);
    let org_changed = match (held, source) {
        (Some(h), Some(t)) => !same(h, t),
        (None, None) => false,
        _ => true,
    };
    if org_changed {
        out.push(TenantRecord::Org {
            record: source.cloned().map(Box::new),
        });
    }
    let of_org = |s: &SystemState| -> BTreeMap<AppId, AppRecord> {
        s.apps
            .iter()
            .filter(|(_, a)| a.org == org)
            .map(|(id, a)| (id.clone(), a.clone()))
            .collect()
    };
    out.extend(
        diff_map(&of_org(root), &of_org(tenant))
            .into_iter()
            .map(|(id, record)| TenantRecord::App {
                moved_to: record
                    .is_none()
                    .then(|| tenant.moved_out.get(&id).cloned())
                    .flatten(),
                id,
                record: record.map(Box::new),
            }),
    );
    let org_apps: BTreeSet<&AppId> = tenant
        .apps
        .values()
        .chain(root.apps.values())
        .filter(|a| a.org == org)
        .map(|a| &a.app_id)
        .collect();
    let invites_of = |s: &SystemState| -> BTreeMap<String, AppInvite> {
        s.app_invites
            .iter()
            .filter(|(_, i)| org_apps.contains(&i.app))
            .map(|(k, i)| (k.clone(), i.clone()))
            .collect()
    };
    out.extend(
        diff_map(&invites_of(root), &invites_of(tenant))
            .into_iter()
            .map(|(key, record)| TenantRecord::AppInvite { key, record }),
    );
    out
}

/// Apply a `MirrorTenant` on root: refused unless computed against the mirror
/// of `org` as it stands (`base`), then each record replaces the mirror's.
///
/// # Errors
/// The mirror was computed against another mirror than this one.
pub fn apply_mirror(
    s: &mut SystemState,
    org: &OrgId,
    base: u64,
    through: u64,
    records: Vec<TenantRecord>,
) -> Result<(), Rejection> {
    if s.mirrored.get(org).copied().unwrap_or_default() != base {
        return Err("a stale tenant mirror: computed against another mirror".into());
    }
    for record in records {
        match record {
            TenantRecord::Org { record } => put(&mut s.orgs, org.clone(), record.map(|o| *o)),
            TenantRecord::App {
                id,
                record: None,
                moved_to: Some(to),
            } => {
                // Moved, not deleted: root's half of the move. The app is the
                // target's now (its own mirror brings its record); what root
                // holds for it — its file bytes and compute runs — goes to the
                // target as payer, and its origins, its managers' until now, go.
                if s.apps.get(&id).is_some_and(|a| a.org == *org) {
                    let held = s.app_file_bytes.get(&id).copied().unwrap_or(0);
                    s.set_app_file_bytes(&id, 0);
                    if let Some(a) = s.apps.get_mut(&id) {
                        a.org.clone_from(&to);
                    }
                    s.set_app_file_bytes(&id, held);
                    if let Some(slots) = s.compute.get_mut(&id) {
                        for slot in slots.values_mut() {
                            slot.payer.clone_from(&to);
                        }
                        s.grant_runs();
                    }
                    s.forget_app_origins(&id);
                }
            }
            TenantRecord::App {
                id,
                record: None,
                moved_to: None,
            } => {
                // Deleted on its tenant: root's own records of it go too.
                if s.apps.get(&id).is_some_and(|a| a.org == *org) {
                    s.remove_app(&id);
                }
            }
            TenantRecord::App { id, record, .. } => {
                // Only this organization's apps: a mirror never writes another's.
                let held = s.apps.get(&id);
                let theirs = held.is_some_and(|a| a.org != *org);
                if theirs || record.as_ref().is_some_and(|a| a.org != *org) {
                    continue;
                }
                // Root's origin index follows the instance, as the tenant's
                // delete, hand-over or upgrade would have kept it on one shard:
                // gone or handed on, its origins go; moved to another package,
                // those of the package it left go.
                let handed_on = match (held, record.as_deref()) {
                    (Some(h), Some(r)) => h.owner != r.owner,
                    (_, None) => true,
                    (None, Some(_)) => false,
                };
                put(&mut s.apps, id.clone(), record.map(|a| *a));
                if handed_on {
                    s.forget_app_origins(&id);
                } else {
                    s.forget_moved_app_origins(&id);
                }
            }
            TenantRecord::AppInvite { key, record } => put(&mut s.app_invites, key, record),
        }
    }
    let _prev = s.mirrored.insert(org.clone(), through.max(base));
    Ok(())
}

/// Split `records` into runs that each serialize under [`SYNC_ENTRY_BYTES`]
/// (a record larger than that goes alone).
pub fn chunks<T: Serialize>(records: Vec<T>) -> Vec<Vec<T>> {
    let mut out: Vec<Vec<T>> = Vec::new();
    let mut size = 0;
    for r in records {
        let n = serde_json::to_vec(&r).map_or(0, |b| b.len());
        if out.is_empty() || size + n > SYNC_ENTRY_BYTES {
            out.push(Vec::new());
            size = 0;
        }
        size += n;
        if let Some(last) = out.last_mut() {
            last.push(r);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replica_takes_roots_records_and_refuses_a_stale_sync() {
        let mut root = SystemState::default();
        root.release_deadlines.insert("1.0.0".into(), 5);
        root.unbound_publishers.insert("did:key:x".into(), 1);
        let mut tenant = SystemState::default();
        let diff = root_diff(&tenant, &root, "acme");
        assert!(!diff.is_empty());
        apply_sync(&mut tenant, "acme", 0, 9, diff).unwrap();
        assert_eq!(tenant.root_through, 9);
        assert_eq!(tenant.release_deadlines.get("1.0.0"), Some(&5));
        assert!(root_diff(&tenant, &root, "acme").is_empty());
        // Computed against the replica before that sync: refused.
        root.unbound_publishers.clear();
        let stale = root_diff(&SystemState::default(), &root, "acme");
        assert!(apply_sync(&mut tenant, "acme", 0, 12, stale).is_err());
        let fresh = root_diff(&tenant, &root, "acme");
        apply_sync(&mut tenant, "acme", 9, 12, fresh).unwrap();
        assert!(tenant.unbound_publishers.is_empty());
    }

    #[test]
    fn records_split_under_the_entry_bound() {
        let big = "x".repeat(SYNC_ENTRY_BYTES / 3);
        let runs = chunks(vec![big.clone(), big.clone(), big.clone(), big]);
        assert_eq!(runs.len(), 2);
        assert!(chunks(Vec::<String>::new()).is_empty());
    }
}
