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
//! established founder yet — i.e. its root meta is absent, or its authority
//! field `admin_identity` is still the placeholder sentinel
//! (`crate::PLACEHOLDER_ADMIN_IDENTITY`). A second `NamespaceCreated` on an
//! already-established namespace (real `admin_identity`) is a NO-OP, so a forged
//! second genesis cannot overwrite an existing admin and apply stays idempotent.

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
    //
    // SECURITY: RESIDUAL (#2474 reviewer batch 3): this check only blocks MISMATCHED
    // forgeries (signer signs a genesis naming a DIFFERENT founder). It does
    // NOT block a SELF-CONSISTENT forged genesis — an attacker who signs
    // `NamespaceCreated { founder: <self> }` on a BARE namespace passes this
    // check (signer == founder == attacker) and becomes that namespace's admin.
    // Nothing here binds `namespace_id` to the legitimate founder, because today
    // `namespace_id` is RANDOM and unrelated to any key. The anti-hijack gate
    // below only protects an ALREADY-established namespace; it cannot tell a
    // legitimate first genesis from a forged first genesis on a bare one.
    // The tracked long-term fix is to make the namespace id a root-of-trust by
    // deriving it as `namespace_id = H(founder ‖ …)`, so a self-consistent
    // forged genesis would target a different (attacker-derived) namespace id
    // and could never collide with the legitimate one. See the #2474
    // root-of-trust follow-up.
    if op.signer != founder {
        bail!(ApplyError::NamespaceCreatedRejected(
            NamespaceCreatedRejection::SignerNotFounder {
                signer: format!("{}", op.signer),
                founder: format!("{founder}"),
            }
        ));
    }

    // ---- TRUE-genesis gate: the op MUST be the DAG root (no parents). ----
    // `NamespaceCreated` is by definition the FIRST op in the namespace DAG.
    // A brand-new namespace has no persisted head, so `read_head_record`
    // (namespace/dag.rs) returns an EMPTY `parent_hashes`, and the signer
    // (`sign_apply_and_publish`) signs the genesis with
    // `parent_op_hashes == []`. Any `NamespaceCreated` carrying parents was
    // therefore minted against an EXISTING DAG head — i.e. injected late onto a
    // namespace that already has history — and must never be allowed to
    // establish/re-found the founder. Rejecting on non-empty parents enforces
    // the reviewer's intent ("only the true first op can establish the
    // founder") without relying on `op.nonce`, which is informational here: DAG
    // sequencing comes from `read_head_record().next_nonce`, not `op.nonce`.
    //
    // CAVEAT for the tracked startup-repair re-emit follow-up: if a future
    // repair path re-emits a genesis on an ALREADY-rooted namespace, it must
    // either emit at the genesis position (with NO parents — i.e. against an
    // empty head) so it passes this gate, or be routed through a distinct,
    // explicitly-authorized repair op. Do NOT relax this gate to admit a
    // parented `NamespaceCreated`, or the anti-hijack guarantee collapses.
    if !op.parent_op_hashes.is_empty() {
        bail!(ApplyError::NamespaceCreatedRejected(
            NamespaceCreatedRejection::NotGenesis {
                parent_count: op.parent_op_hashes.len(),
            }
        ));
    }

    // ---- Anti-hijack / idempotency gate. ----
    // Genesis may only ESTABLISH a founder; it may never overwrite one. The
    // gate keys SOLELY on `admin_identity`, the authority field:
    //
    //   * `admin_identity == placeholder` ⇒ no real admin yet ⇒ genesis may
    //     proceed (establish/repair);
    //   * `admin_identity != placeholder` ⇒ a real admin already exists ⇒
    //     genesis is a NO-OP (anti-hijack).
    //
    // `admin_identity` is THE authority field. This crate's own code paths
    // always write it together with `owner_identity`: the bootstrap KeyDelivery
    // seed (`seed_bootstrap_admin_if_absent`) writes BOTH as the placeholder,
    // and the establish branch below writes BOTH as the founder. They are NOT
    // guaranteed to be equal in general, though — a partial write (a crash
    // between two non-atomic `put`s) or an external/legacy writer can leave the
    // two diverged, and the test
    // `namespace_created_genesis_proceeds_when_only_admin_is_placeholder`
    // deliberately constructs exactly such a state. That divergence is harmless
    // here precisely because the gate keys on `admin_identity` as the SOLE
    // authority field: "is this namespace established?" is answered correctly
    // regardless of what `owner_identity` holds. Keying on `admin_identity` ONLY
    // also
    // fixes a correctness bug in the earlier OR-of-both form: an OR gate could
    // declare a namespace "established" while `admin_identity` was still the
    // placeholder (e.g. a partial write that set only `owner_identity`),
    // wedging the namespace with no real admin forever and blocking the
    // repairing genesis. Gating on the authority field means genesis proceeds
    // exactly when there is no real admin, writing the real one. The sentinel
    // itself is shared with the seed via `crate::PLACEHOLDER_ADMIN_IDENTITY` so
    // the two cannot drift.
    let placeholder = placeholder_admin_identity();
    let existing = MetaRepository::new(store).load(&ns_gid)?;
    if let Some(meta) = &existing {
        let established = meta.admin_identity != placeholder;
        if established {
            // The namespace already has an established founder. Genesis may
            // never OVERWRITE one, so we return early without re-writing the
            // root meta. There are two sub-cases:
            //
            //  (1) `meta.admin_identity == founder` — the SAME founder is
            //      re-arriving (idempotent genesis, or some path wrote a
            //      non-placeholder root `admin_identity` for the founder before
            //      genesis applied). The meta is already correct, but the
            //      founder's explicit Admin member row may NOT have been written
            //      (the path that set `admin_identity` need not have created the
            //      member row). Ensure it here with an idempotent upsert so the
            //      founder is always enumerable as Admin, regardless of which
            //      write landed first. This mirrors the establish branch below.
            //
            //  (2) `meta.admin_identity != founder` — the namespace was
            //      established by SOMEONE ELSE. This MUST stay a pure no-op: we
            //      must NOT touch membership, or a forged second genesis would
            //      grant its declared founder an Admin member row on a namespace
            //      they do not own. That is the anti-hijack guarantee, so the
            //      member-row upsert is gated strictly on admin == founder.
            if meta.admin_identity == founder {
                MembershipRepository::new(store).add_member(
                    &ns_gid,
                    &founder,
                    GroupMemberRole::Admin,
                )?;
                // Also seed the Open-join default caps on this idempotent
                // same-founder path, mirroring the establish branch below. The
                // path that set `admin_identity` need not have written the caps
                // row, so a same-founder re-arrival must guarantee BOTH the
                // Admin member row AND the default caps. Absence-gated for the
                // same no-clobber reason documented at the establish branch: a
                // later `DefaultCapabilitiesSet` must never be overwritten.
                let caps = CapabilitiesRepository::new(store);
                if caps.default_capabilities(&ns_gid)?.is_none() {
                    caps.set_default_capabilities(
                        &ns_gid,
                        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
                    )?;
                }
                tracing::debug!(
                    namespace_id = %hex::encode(namespace_id),
                    %founder,
                    "NamespaceCreated: founder already established; ensured Admin member row \
                     (idempotent re-arrival)"
                );
                return Ok(());
            }
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
