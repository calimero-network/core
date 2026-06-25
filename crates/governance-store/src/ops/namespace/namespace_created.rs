//! `RootOp::NamespaceCreated` apply handler — the namespace GENESIS op (#2474).
//!
//! This is the FIRST op in every namespace DAG. It authoritatively records the
//! namespace's founding administrator/owner so a bootstrapping replica derives
//! the founder from the synced governance DAG instead of trust-on-first-use
//! seeding from the KeyDelivery signer (`seed_bootstrap_admin_if_absent`), which
//! pinned the WRONG admin whenever the key-deliverer was a non-owner member and
//! permanently wedged backfill (#2474, production-confirmed).
//!
//! It is **self-authorizing**: unlike every other authority-bearing root op it
//! does NOT call `require_namespace_admin`, because genesis is precisely what
//! establishes that authority — there is no prior admin to check against.
//!
//! Anti-hijack: a `NamespaceCreated` is applied only when the namespace has no
//! established founder yet (no root meta with a non-zero `admin_identity`). A
//! second `NamespaceCreated` on an already-established namespace is a NO-OP, so a
//! forged second genesis cannot overwrite an existing admin and apply stays
//! idempotent.

use super::context::NamespaceApplyCtx;
use crate::{CapabilitiesRepository, MembershipRepository, MetaRepository};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use eyre::Result as EyreResult;

/// The zero public key — the placeholder `admin_identity` that the
/// bootstrap KeyDelivery seed writes before genesis arrives. It grants
/// authority to nobody (it can never equal a real identity), and it is the
/// sentinel the anti-hijack gate uses to tell a not-yet-established namespace
/// (placeholder admin) from an established one (real founder).
fn placeholder_admin() -> PublicKey {
    PublicKey::from([0u8; 32])
}

pub(crate) fn apply(
    ctx: &mut NamespaceApplyCtx<'_>,
    _op: &calimero_context_client::local_governance::SignedNamespaceOp,
    founder: PublicKey,
) -> EyreResult<()> {
    let store = ctx.store();
    let namespace_id = ctx.namespace_id();
    let ns_gid = ContextGroupId::from(namespace_id);

    // ---- Anti-hijack / idempotency gate. ----
    // Genesis may only ESTABLISH a founder; it may never overwrite one. An
    // established namespace is one whose root meta carries a non-zero
    // `admin_identity`. A placeholder-admin meta (written by the bootstrap
    // KeyDelivery seed before genesis arrived) is NOT established, so genesis
    // is still allowed to fill in the real founder over it — this is what makes
    // the seed/genesis ordering converge regardless of which lands first.
    let existing = MetaRepository::new(store).load(&ns_gid)?;
    if let Some(meta) = &existing {
        if meta.admin_identity != placeholder_admin() {
            tracing::debug!(
                namespace_id = %hex::encode(namespace_id),
                established_admin = %meta.admin_identity,
                %founder,
                "NamespaceCreated: namespace already has an established founder; \
                 ignoring genesis (anti-hijack no-op)"
            );
            return Ok(());
        }
    }

    // ---- Establish the founder as admin == owner on the root meta. ----
    // Preserve any application bindings a placeholder seed may have set; only
    // the founding identity is authoritative here.
    let meta = calimero_store::key::GroupMetaValue {
        admin_identity: founder,
        owner_identity: founder,
        target_application_id: existing
            .as_ref()
            .map(|m| m.target_application_id)
            .unwrap_or_else(|| calimero_primitives::application::ApplicationId::from([0u8; 32])),
        app_key: existing.as_ref().map(|m| m.app_key).unwrap_or([0u8; 32]),
        upgrade_policy: existing
            .as_ref()
            .map(|m| m.upgrade_policy.clone())
            .unwrap_or_default(),
        migration: existing.as_ref().and_then(|m| m.migration.clone()),
        created_at: existing.as_ref().map(|m| m.created_at).unwrap_or(0),
        auto_join: existing.as_ref().map(|m| m.auto_join).unwrap_or(true),
    };
    MetaRepository::new(store).save(&ns_gid, &meta)?;

    // ---- Founder gets the explicit Admin member row. ----
    // `is_admin` also matches `meta.admin_identity`, but the explicit row keeps
    // the founder visible in member enumerations and mirrors the subgroup
    // `GroupCreated` path.
    MembershipRepository::new(store).add_member(&ns_gid, &founder, GroupMemberRole::Admin)?;

    // ---- Default caps: CAN_JOIN_OPEN_SUBGROUPS. ----
    // Mirrors the bootstrap seed and the owner-side `store_group_meta`
    // precedent so members admitted before a later `DefaultCapabilitiesSet`
    // gossip still inherit the bit that gates Open-subgroup inheritance.
    // Gated on absence so an admin-authored override is never clobbered.
    let caps = CapabilitiesRepository::new(store);
    if caps.default_capabilities(&ns_gid)?.is_none() {
        caps.set_default_capabilities(&ns_gid, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS)?;
    }

    tracing::info!(
        namespace_id = %hex::encode(namespace_id),
        %founder,
        "NamespaceCreated genesis applied: founder established as namespace admin/owner"
    );
    Ok(())
}
