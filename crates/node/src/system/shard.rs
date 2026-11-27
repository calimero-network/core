//! Where a system command's state lives once the system group shards by tenant.
//!
//! **This is the routing layer of the migration, still gated off.** Today the
//! control plane is a single Raft group of three voters (`SYSTEM_GROUP`,
//! `membership::SYSTEM_VOTERS`) holding every [`Area`]. At fleet scale that one
//! group is the ceiling for control-plane writes. The scalable split is by
//! **tenant** (organization): an organization's own control-plane state moves to
//! a per-tenant shard, while cluster-wide state stays in a single **root** group.
//!
//! This module decides *which shard a command's scope belongs to* and resolves a
//! shard to a Raft group. The decision is gated: [`shard_for`] routes everything
//! to the single [`SYSTEM_GROUP`] until tenant sharding is active **and** the
//! command's tenant is known, so wiring it into the routers changes no behaviour
//! until a later slice turns the gate on and creates per-tenant groups. Keeping
//! the decision in one typed, tested place lets the rest of the node be written
//! against shards before any state is split.
//!
//! The migration plan and the cross-shard reads this implies are in the design
//! note: <https://github.com/calimero-network/core/blob/master/docs/design/system-group-sharding.md>.

#![allow(
    dead_code,
    reason = "the tenant-shard routing layer: later migration slices wire it into \
              the system routers and turn the gate on; until then only its own tests \
              exercise it"
)]

use calimero_log::GroupId;

use crate::node::SYSTEM_GROUP;
use crate::system::Area;
use crate::types::{AccountId, AppId, OrgId};

/// The first Raft group id a tenant group may take. App groups climb from 2
/// ([`SYSTEM_GROUP`] is 1) and never reach this, so tenant groups live in their
/// own high id range: an org's tenant group gets a stable id here without
/// shifting any app group id, and the two id spaces never collide. A monotonic
/// counter (`SystemState::next_tenant_group`) hands them out and never reuses
/// one, so a deleted org's id is never aliased to a new group.
pub(crate) const TENANT_GROUP_BASE: GroupId = 1 << 32;

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

/// The scope a system command acts on, read from the command itself. Some
/// commands name their organization directly ([`Scope::Org`]); others name an
/// app or an account whose **owning organization is the tenant**, read from
/// system state at routing time ([`Scope::OfApp`], [`Scope::OfAccount`]) — the
/// cross-shard read the migration must perform, and the reason scope is not a
/// pure function of the command (see the design note). A command that names no
/// organization at all is cluster-wide ([`Scope::Cluster`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    /// Cluster-wide: no single organization owns the change.
    Cluster,
    /// Scoped to the named organization.
    Org(OrgId),
    /// Scoped to the organization that owns the named app.
    OfApp(AppId),
    /// Scoped to the organization context of the named account.
    OfAccount(AccountId),
}

/// How an [`Area`]'s commands place: the structural half of the decision, before
/// a specific command's [`Scope`] is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    /// Always the root group: the state is inherently cluster-wide.
    Root,
    /// Always a tenant shard: the state belongs to one organization.
    Tenant,
    /// Governance — follows the command's scope. An org-scoped approval, update
    /// policy or deployment lives in that tenant's shard; a cluster-wide one in
    /// the root group.
    Scoped,
}

/// Where an [`Area`]'s state belongs. The split follows the data, not the code
/// layout: the fleet, packages, compute accounting and the sign-up policy are
/// root; an organization's structure, its apps and its accounts' sessions and
/// recovery are tenant; the three governance areas depend on each command's
/// scope. `Accounts` places as `Tenant` because the write-heavy part (sessions,
/// refreshes, API keys) belongs to the account's organization context, with the
/// account-identity records read across shards — the same coupling `CreateApp`
/// already has.
pub(crate) const fn area_placement(area: Area) -> Placement {
    match area {
        Area::Fleet | Area::Packages | Area::Compute | Area::SignUp => Placement::Root,
        Area::Apps | Area::Accounts | Area::Recovery | Area::Orgs => Placement::Tenant,
        Area::Approvals | Area::Deployments | Area::Updates => Placement::Scoped,
    }
}

/// The organization a command's scope belongs to, or `None` for cluster-wide.
/// For [`Scope::OfApp`] and [`Scope::OfAccount`] the owning organization is read
/// from system state through `resolve_org` (the cross-shard read); a resolver
/// that cannot answer (or the `|_| None` used before the state is wired in)
/// leaves the tenant unresolved, and [`shard_for`] then routes to the root.
pub(crate) fn tenant_of(
    scope: &Scope,
    resolve_org: impl Fn(&Scope) -> Option<OrgId>,
) -> Option<OrgId> {
    match scope {
        Scope::Cluster => None,
        Scope::Org(org) => Some(org.clone()),
        Scope::OfApp(_) | Scope::OfAccount(_) => resolve_org(scope),
    }
}

/// The shard a command routes to, gated. Until tenant sharding is `active`, or
/// while a tenant-scoped command's organization is unresolved, everything routes
/// to the single root group — so wiring this into the routers is behaviour-
/// preserving until a later slice turns the gate on and creates per-tenant
/// groups.
pub(crate) fn shard_for(active: bool, tenant: Option<OrgId>) -> ShardId {
    match (active, tenant) {
        (true, Some(org)) => ShardId::Tenant(org),
        _ => ShardId::Root,
    }
}

/// The Raft group a shard is served by. Until the system log is split every
/// shard — root and tenant alike — is the single [`SYSTEM_GROUP`], so routing is
/// unchanged; a later slice gives each tenant shard its own group id here.
pub(crate) const fn resolve(_shard: &ShardId) -> GroupId {
    SYSTEM_GROUP
}

/// Whether a system machine of role `role` owns a command whose scope resolves
/// to `tenant` (via [`tenant_of`]) under the sharding gate `active`: the
/// apply-side mirror of [`shard_for`].
///
/// The invariant that makes the split deterministic: **a command is applied by
/// exactly the shard its write routed to.** Routing a write picks
/// `shard_for(active, tenant)`; the machine that owns — and so applies — the
/// entry is the one whose role *is* that shard. So ownership is a single
/// equality, with no second decision that could disagree with the router.
///
/// Gated like the router. With sharding off every command's shard is
/// [`ShardId::Root`], so the root machine owns all and no tenant machine owns
/// any — behaviour-preserving until the gate turns on. Org-directory lifecycle
/// (`CreateOrg`, `DeleteOrg`) carries [`Scope::Cluster`], so it owns on the root
/// even with the gate on: it allocates and frees the tenant group and writes the
/// root directory, and the new tenant's own detail is initialised by a
/// root-initiated follow-on entry on the tenant shard (resolve-at-propose).
pub(crate) fn owns(role: &ShardId, tenant: Option<OrgId>, active: bool) -> bool {
    shard_for(active, tenant) == *role
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_wide_areas_place_in_the_root() {
        for area in [Area::Fleet, Area::Packages, Area::Compute, Area::SignUp] {
            assert_eq!(area_placement(area), Placement::Root, "{area:?}");
        }
    }

    #[test]
    fn an_organizations_own_areas_place_in_its_tenant_shard() {
        for area in [Area::Apps, Area::Accounts, Area::Recovery, Area::Orgs] {
            assert_eq!(area_placement(area), Placement::Tenant, "{area:?}");
        }
    }

    #[test]
    fn governance_areas_follow_the_commands_scope() {
        for area in [Area::Approvals, Area::Deployments, Area::Updates] {
            assert_eq!(area_placement(area), Placement::Scoped, "{area:?}");
        }
    }

    #[test]
    fn a_named_org_is_its_own_tenant_a_cluster_scope_has_none() {
        let none = |_: &Scope| None;
        assert_eq!(tenant_of(&Scope::Cluster, none), None);
        assert_eq!(
            tenant_of(&Scope::Org("acme".into()), none),
            Some("acme".into())
        );
    }

    #[test]
    fn an_app_or_account_scope_resolves_its_org_from_state() {
        // The resolver stands in for the cross-shard read of the owning org.
        let owner = |s: &Scope| match s {
            Scope::OfApp(_) | Scope::OfAccount(_) => Some("acme".to_owned()),
            _ => None,
        };
        assert_eq!(
            tenant_of(&Scope::OfApp("app1".into()), owner),
            Some("acme".into())
        );
        assert_eq!(
            tenant_of(&Scope::OfAccount("alice".into()), owner),
            Some("acme".into())
        );
        // Before the state is wired in, the tenant is unresolved.
        assert_eq!(tenant_of(&Scope::OfApp("app1".into()), |_| None), None);
    }

    #[test]
    fn the_gate_keeps_everything_on_the_root_until_sharding_is_active() {
        // Off: even a known tenant routes to the root group.
        assert_eq!(shard_for(false, Some("acme".into())), ShardId::Root);
        assert_eq!(shard_for(false, None), ShardId::Root);
        // On: a known tenant routes to its shard; an unresolved one still roots.
        assert_eq!(
            shard_for(true, Some("acme".into())),
            ShardId::Tenant("acme".into())
        );
        assert_eq!(shard_for(true, None), ShardId::Root);
    }

    #[test]
    fn every_shard_resolves_to_the_single_group_today() {
        assert_eq!(resolve(&ShardId::Root), SYSTEM_GROUP);
        assert_eq!(resolve(&ShardId::Tenant("acme".into())), SYSTEM_GROUP);
    }

    #[test]
    fn with_the_gate_off_the_root_owns_everything() {
        // No tenant machine exists yet, and the root applies every command.
        assert!(owns(&ShardId::Root, None, false));
        assert!(owns(&ShardId::Root, Some("acme".into()), false));
        // A tenant role (there is none yet) would own nothing.
        assert!(!owns(
            &ShardId::Tenant("acme".into()),
            Some("acme".into()),
            false
        ));
    }

    #[test]
    fn with_the_gate_on_each_command_is_owned_by_exactly_its_routed_shard() {
        let active = true;
        // An org-scoped command is owned by that org's tenant, no one else.
        let acme = tenant_of(&Scope::Org("acme".into()), |_| None);
        assert!(owns(&ShardId::Tenant("acme".into()), acme.clone(), active));
        assert!(!owns(
            &ShardId::Tenant("other".into()),
            acme.clone(),
            active
        ));
        assert!(!owns(&ShardId::Root, acme, active));
        // A cluster-scoped command (fleet, packages, and org-directory
        // lifecycle like CreateOrg/DeleteOrg) is owned by the root even on.
        let cluster = tenant_of(&Scope::Cluster, |_| None);
        assert!(owns(&ShardId::Root, cluster.clone(), active));
        assert!(!owns(&ShardId::Tenant("acme".into()), cluster, active));
    }

    #[test]
    fn ownership_is_exactly_the_routing_decision() {
        // owns() must never disagree with shard_for(): applied-by == routed-to.
        for active in [false, true] {
            for tenant in [None, Some("acme".to_owned()), Some("other".to_owned())] {
                let routed = shard_for(active, tenant.clone());
                assert!(
                    owns(&routed, tenant.clone(), active),
                    "the routed shard must own it ({active}, {tenant:?})"
                );
                let not_routed = match &routed {
                    ShardId::Root => ShardId::Tenant("elsewhere".into()),
                    ShardId::Tenant(_) => ShardId::Root,
                };
                assert!(
                    !owns(&not_routed, tenant.clone(), active),
                    "no other shard may own it ({active}, {tenant:?})"
                );
            }
        }
    }
}
