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
//! established founder yet — i.e. its root meta is absent, or BOTH
//! `admin_identity` AND `owner_identity` are still the placeholder sentinel
//! (`crate::PLACEHOLDER_ADMIN_IDENTITY`). A second `NamespaceCreated` on an
//! already-established namespace is a NO-OP, so a forged second genesis cannot
//! overwrite an existing admin and apply stays idempotent.

use super::context::NamespaceApplyCtx;
use crate::{
    placeholder_admin_identity, ApplyError, CapabilitiesRepository, MembershipRepository,
    MetaRepository, NamespaceCreatedRejection,
};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut NamespaceApplyCtx<'_>,
    op: &calimero_context_client::local_governance::SignedNamespaceOp,
    founder: PublicKey,
) -> EyreResult<()> {
    let store = ctx.store();
    let namespace_id = ctx.namespace_id();
    let ns_gid = ContextGroupId::from(namespace_id);

    // ---- Self-authorization binding: signer MUST equal the declared founder. ----
    // Genesis is the one authority-bearing root op that SKIPS
    // `require_namespace_admin` (there is no prior admin to check against), so
    // the only thing tying the established admin to a real signing key is this
    // check. The invariant holds because the genesis is signed with the
    // namespace key == the founder's key at creation. Without it a non-founder
    // could sign `NamespaceCreated { founder: <someone-else> }` with their own
    // key and, on a namespace with no prior genesis, pin a forged/wrong admin.
    // Enforced BEFORE the anti-hijack/established gate so a mismatched op is
    // rejected outright (logged as rejected via `ApplyError`, never applied),
    // never silently treated as a no-op.
    if op.signer != founder {
        bail!(ApplyError::NamespaceCreatedRejected(
            NamespaceCreatedRejection::SignerNotFounder {
                signer: format!("{}", op.signer),
                founder: format!("{founder}"),
            }
        ));
    }

    // ---- Anti-hijack / idempotency gate. ----
    // Genesis may only ESTABLISH a founder; it may never overwrite one. A
    // namespace is treated as "not yet established" (genesis may overwrite it)
    // ONLY when BOTH `admin_identity` AND `owner_identity` are the placeholder
    // sentinel. The bootstrap KeyDelivery seed
    // (`seed_bootstrap_admin_if_absent`) writes BOTH as the placeholder, so
    // this is consistent with the seed; requiring both (rather than just
    // `admin_identity`) hardens the gate against a hypothetical future partial
    // write that sets one identity but not the other — if EITHER is already a
    // real identity, the namespace is considered established and genesis is a
    // no-op. This is what makes the seed/genesis ordering converge regardless
    // of which lands first. The sentinel itself is shared with the seed via
    // `crate::PLACEHOLDER_ADMIN_IDENTITY` so the two cannot drift.
    let placeholder = placeholder_admin_identity();
    let existing = MetaRepository::new(store).load(&ns_gid)?;
    if let Some(meta) = &existing {
        let established = meta.admin_identity != placeholder || meta.owner_identity != placeholder;
        if established {
            tracing::debug!(
                namespace_id = %hex::encode(namespace_id),
                established_admin = %meta.admin_identity,
                established_owner = %meta.owner_identity,
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
    //
    // `add_member` is a guaranteed UPSERT: it unconditionally `put`s the row
    // with the supplied `role`, overwriting any existing role (it preserves
    // only `auto_follow`). So if the bootstrap seed previously wrote the
    // founder as a non-authoritative `Member` placeholder, this call UPGRADES
    // them to `Admin` rather than no-op'ing on the existing row. That makes the
    // genesis handler self-contained and correct regardless of seed-vs-genesis
    // ordering. (If `add_member` ever changes to skip existing rows, this must
    // become an explicit `role_of`-checked force-to-Admin.)
    MembershipRepository::new(store).add_member(&ns_gid, &founder, GroupMemberRole::Admin)?;

    // ---- Default caps: CAN_JOIN_OPEN_SUBGROUPS. ----
    // Mirrors the bootstrap seed and the owner-side `store_group_meta`
    // precedent so members admitted before a later `DefaultCapabilitiesSet`
    // gossip still inherit the bit that gates Open-subgroup inheritance.
    //
    // The `is_none()` gate is load-bearing, NOT just an optimization: genesis
    // can arrive LATE on a backfilling replica — after an admin-authored
    // `DefaultCapabilitiesSet` has already run and, say, REMOVED
    // CAN_JOIN_OPEN_SUBGROUPS. Writing the default unconditionally here would
    // clobber that later admin decision and silently re-grant the bit. Gating
    // on absence means genesis only ever SEEDS the default when no explicit
    // caps row exists yet; once any `DefaultCapabilitiesSet` has written one,
    // genesis leaves it untouched regardless of arrival order. If this gate
    // condition is ever changed, this no-clobber invariant MUST be preserved.
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
