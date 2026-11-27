//! Where a system command's state lives once the system group shards by tenant.
//!
//! **This is a seam, not yet a behaviour change.** Today the control plane is a
//! single Raft group of three voters (`SYSTEM_GROUP`, `membership::SYSTEM_VOTERS`)
//! holding every [`Area`]. At fleet scale that one group is the ceiling for
//! control-plane writes. The scalable split is by
//! **tenant** (organization): an organization's own control-plane state —​ its
//! members and teams, its apps, its sessions —​ moves to a per-tenant shard,
//! while cluster-wide state (the fleet, the package registry, the sign-up and
//! deployment policy) stays in a single **root** group. Leaders then spread
//! across nodes per tenant instead of funnelling through one.
//!
//! This module encodes only the first, safe step: *which shard each command's
//! scope belongs to*. [`resolve`] maps every shard to the single existing system
//! group, so nothing moves yet; the migration fills this seam behind a feature
//! gate and a declared cutover (see the design note linked below). Keeping the
//! placement decision in one typed place is what lets the rest of the node be
//! written against shards before any state is actually split.
//!
//! Design: <https://github.com/calimero-network/core/blob/master/docs/design/system-group-sharding.md>.
//! The coupling that makes **tenant** the right seam (and `Area` the wrong one)
//! —​ a single `CreateApp` reads the fleet quota, the caller's org membership and
//! the release —​ is argued there and in [`Placement::Scoped`].

#![allow(
    dead_code,
    reason = "the tenant-shard placement seam: the migration wires it into routing; \
              until then only its own tests exercise it"
)]

use calimero_log::GroupId;

use crate::node::SYSTEM_GROUP;
use crate::system::Area;
use crate::types::OrgId;

/// Which control-plane shard a piece of state lives in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ShardId {
    /// Cluster-wide state shared by every tenant: the fleet, the package
    /// registry, the sign-up and deployment policy, the cluster version.
    Root,
    /// One organization's own state: its members and teams, its apps, the
    /// sessions and recovery of its accounts.
    Tenant(OrgId),
}

/// The scope a system command acts on, taken from the command itself (its
/// `actor`, its target org, or neither). A command that names no organization is
/// cluster-wide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    /// Cluster-wide: no single organization owns the change.
    Cluster,
    /// Scoped to one organization.
    Org(OrgId),
}

/// How an [`Area`]'s commands place: the structural half of the decision, before
/// a specific command's [`Scope`] is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    /// Always the root group: the state is inherently cluster-wide.
    Root,
    /// Always a tenant shard: the state belongs to one organization.
    Tenant,
    /// Governance —​ follows the command's scope. An org-scoped approval, update
    /// policy or deployment lives in that tenant's shard; a cluster-wide one in
    /// the root group ([`shard_for`]).
    Scoped,
}

/// Where an [`Area`]'s state belongs. The split follows the data, not the code
/// layout: the fleet, packages, compute accounting, the sign-up policy and the
/// cluster-wide deployment/update policy are root; an organization's structure,
/// its apps and its accounts' sessions and recovery are tenant; the three
/// governance areas depend on each command's scope.
///
/// `Accounts` is the subtle one. An account is a cluster-wide identity, but the
/// write-heavy part —​ sessions, refreshes and API keys —​ is what the control
/// plane's write rate is mostly made of, and each belongs to the account's
/// organization context, so the area places as `Tenant`; the account-identity
/// records a tenant needs are read across shards (or cached), the same
/// cross-shard read `CreateApp` already needs for the fleet quota and the
/// release. That coupling, not the area boundary, is why the design shards by
/// tenant rather than by area.
pub(crate) const fn area_placement(area: Area) -> Placement {
    match area {
        Area::Fleet | Area::Packages | Area::Compute | Area::SignUp => Placement::Root,
        Area::Apps | Area::Accounts | Area::Recovery | Area::Orgs => Placement::Tenant,
        Area::Approvals | Area::Deployments | Area::Updates => Placement::Scoped,
    }
}

/// The shard a command places in, from its area and the scope it acts on. A
/// `Tenant` or scoped-to-an-org command needs a `Scope::Org`; a root or
/// cluster-scoped one ignores the scope. A `Tenant`/`Scoped` area with a
/// `Cluster` scope has no tenant to place in and falls back to the root group,
/// where the single-group deployment keeps every such command today.
pub(crate) fn shard_for(area: Area, scope: &Scope) -> ShardId {
    match (area_placement(area), scope) {
        (Placement::Root, _) | (_, Scope::Cluster) => ShardId::Root,
        (Placement::Tenant | Placement::Scoped, Scope::Org(org)) => ShardId::Tenant(org.clone()),
    }
}

/// The Raft group a shard is served by. Until the system log is split every
/// shard —​ root and tenant alike —​ is the single [`SYSTEM_GROUP`], so routing
/// is unchanged; the migration gives each tenant shard its own group id here.
pub(crate) const fn resolve(_shard: &ShardId) -> GroupId {
    SYSTEM_GROUP
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_wide_areas_place_in_the_root() {
        for area in [Area::Fleet, Area::Packages, Area::Compute, Area::SignUp] {
            assert_eq!(area_placement(area), Placement::Root, "{area:?}");
            assert_eq!(shard_for(area, &Scope::Cluster), ShardId::Root, "{area:?}");
            // A root area ignores an org scope: it has no tenant home.
            assert_eq!(
                shard_for(area, &Scope::Org("acme".into())),
                ShardId::Root,
                "{area:?}"
            );
        }
    }

    #[test]
    fn an_organizations_own_areas_place_in_its_tenant_shard() {
        for area in [Area::Apps, Area::Accounts, Area::Recovery, Area::Orgs] {
            assert_eq!(area_placement(area), Placement::Tenant, "{area:?}");
            assert_eq!(
                shard_for(area, &Scope::Org("acme".into())),
                ShardId::Tenant("acme".into()),
                "{area:?}"
            );
        }
    }

    #[test]
    fn governance_follows_the_commands_scope() {
        for area in [Area::Approvals, Area::Deployments, Area::Updates] {
            assert_eq!(area_placement(area), Placement::Scoped, "{area:?}");
            // An org-scoped governance command lives in that tenant's shard...
            assert_eq!(
                shard_for(area, &Scope::Org("acme".into())),
                ShardId::Tenant("acme".into()),
                "{area:?}"
            );
            // ...a cluster-wide one in the root.
            assert_eq!(shard_for(area, &Scope::Cluster), ShardId::Root, "{area:?}");
        }
    }

    #[test]
    fn every_shard_resolves_to_the_single_group_today() {
        // The seam changes no routing until the system log is split.
        assert_eq!(resolve(&ShardId::Root), SYSTEM_GROUP);
        assert_eq!(resolve(&ShardId::Tenant("acme".into())), SYSTEM_GROUP);
    }
}
