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
//! # Apply order (and WHY)
//!
//! The handler branches FIRST on whether the namespace is already established —
//! i.e. its root meta exists with `admin_identity != placeholder`
//! (`crate::PLACEHOLDER_ADMIN_IDENTITY`). The established check is the load-bearing
//! pivot, NOT the structural (parents / signer) checks:
//!
//!   1. Load the root meta; `established = meta.admin_identity != placeholder`.
//!
//!   2. **If established ⇒ ALWAYS `Ok(())`, NEVER `Err`.** Any `NamespaceCreated`
//!      arriving on an established namespace is harmless and MUST be a no-op.
//!      Repair/ensure work runs ONLY for a genesis-shaped (parentless) op; a
//!      parented `NamespaceCreated` on an established namespace is structurally
//!      not a genesis and is a harmless pure no-op (it touches NOTHING — no
//!      member row, caps, or meta):
//!        * `admin_identity == founder` AND `op.parent_op_hashes.is_empty()` —
//!          idempotent genesis-shaped re-arrival (e.g. a duplicate/late genesis
//!          via sync backfill). Ensure the founder's Admin member row (written
//!          FIRST, the authority-bearing row) + default caps (absence-gated), and
//!          (#602) repair a diverged `owner_identity` back to the founder. Other
//!          meta fields are preserved.
//!        * `admin_identity == founder` but the op HAS parents — pure no-op (not
//!          genesis-shaped; do not act on it).
//!        * `admin_identity != founder` — established by someone else: pure no-op,
//!          touch nothing (anti-hijack).
//!          WHY no `Err` here: a parented or late `NamespaceCreated` (e.g. a duplicate
//!          replayed via DAG sync) used to return `Err(NotGenesis)`, which the
//!          `apply_signed_op` caller can treat as fatal and STALL DAG processing
//!          (#591). On an established namespace nothing can be hijacked, so there is
//!          no reason to error — we no-op and let the DAG advance.
//!
//!   3. **If NOT established (admin == placeholder) ⇒ this op is trying to FOUND
//!      the namespace, so enforce the genesis invariants, THEN establish:**
//!        * (#596) no-parents FIRST — `if !op.parent_op_hashes.is_empty()` →
//!          `Err(NotGenesis)`. Structural: a parented op was minted against an
//!          existing DAG head, so it is NOT the genesis and is REJECTED
//!          regardless of signer (checked before the signer check so a parented
//!          founding op never leaks/pins the declared founder). It MUST be `Err`,
//!          not a no-op `Ok`: an `Err` propagates BEFORE `advance_dag_head` runs
//!          in `apply_signed_op`, so the head stays empty and the namespace
//!          stays establishable; a no-op `Ok` would advance the head on a bare
//!          namespace and BRICK establishment. (Backfill is retry-tolerant, so
//!          this never permanently stalls the DAG.)
//!        * then `if op.signer != founder` → `SignerNotFounder`.
//!        * then establish: write meta `admin == owner == founder`, the founder's
//!          Admin member row, and the default caps.
//!
//! NET INVARIANT: a `NamespaceCreated` only ESTABLISHES the founder when
//! (NOT established AND parentless AND signer == founder).
//!   * NOT-established + parented ⇒ `Err(NotGenesis)` — prevents a head-advance
//!     that would brick establishment; backfill retry-tolerance means no
//!     permanent stall.
//!   * NOT-established + parentless + signer != founder ⇒ `Err(SignerNotFounder)`.
//!   * ESTABLISHED (parented OR parentless) ⇒ `Ok(())` no-op — never `Err` (the
//!     #591 fix: the namespace is already founded, so a head-advance is harmless
//!     and erroring would risk a DAG stall).
//!     The two parented cases differ on purpose: the ESTABLISHED one is a no-op `Ok`
//!     (already founded, nothing to brick), the NOT-established one is `Err`
//!     (a no-op `Ok` there would advance the head and brick the later genesis).

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
    let ns_gid = ContextGroupId::from(namespace_id.to_bytes());

    // ---- Load the root meta and decide established-ness FIRST. ----
    // The established check (NOT the structural parents/signer checks) is the
    // load-bearing pivot of this handler. It keys SOLELY on `admin_identity`,
    // the authority field:
    //
    //   * `admin_identity == placeholder` ⇒ no real admin yet ⇒ this op is
    //     trying to FOUND the namespace ⇒ enforce genesis invariants, then
    //     establish (step 3 below);
    //   * `admin_identity != placeholder` ⇒ a real admin already exists ⇒ this
    //     op can hijack nothing ⇒ it is ALWAYS a no-op `Ok(())`, NEVER `Err`
    //     (step 2 below).
    //
    // `admin_identity` is THE authority field. This crate's own code paths
    // always write it together with `owner_identity`: the bootstrap KeyDelivery
    // seed (`seed_bootstrap_admin_if_absent`) writes BOTH as the placeholder,
    // and the establish branch below writes BOTH as the founder. They are NOT
    // guaranteed to be equal in general, though — a partial write (a crash
    // between two non-atomic `put`s) or an external/legacy writer can leave the
    // two diverged, and the test
    // `namespace_created_genesis_proceeds_when_only_admin_is_placeholder`
    // deliberately constructs exactly such a state. Keying the established check
    // on `admin_identity` ONLY answers "is this namespace established?"
    // correctly regardless of what `owner_identity` holds, and fixes a
    // correctness bug in an earlier OR-of-both form (an OR gate could declare a
    // namespace "established" while `admin_identity` was still the placeholder,
    // wedging it with no real admin forever and blocking the repairing genesis).
    // The sentinel is shared with the seed via `crate::PLACEHOLDER_ADMIN_IDENTITY`
    // so the two cannot drift.
    let placeholder = placeholder_admin_identity();
    let existing = MetaRepository::new(store).load(&ns_gid)?;

    if let Some(meta) = &existing {
        let established = meta.admin_identity != placeholder;
        if established {
            // ---- (2) ESTABLISHED ⇒ ALWAYS Ok(()), NEVER Err. ----
            // Any `NamespaceCreated` arriving here (parented or not, late via
            // sync, duplicate, or forged) is harmless: a real admin already
            // exists and genesis may never OVERWRITE one. Returning `Err` (as the
            // old order did on parented ops via `NotGenesis`) can make the
            // `apply_signed_op` caller treat the op as fatal and STALL DAG
            // processing (#591). So we no-op and let the DAG advance. The
            // structural parents/signer checks are deliberately NOT consulted on
            // this path — they only gate FOUNDING (step 3).
            if meta.admin_identity == founder {
                // (2a) SAME founder re-arriving on an established namespace.
                //
                // We only perform repair/ensure work for a GENESIS-SHAPED op —
                // i.e. one with NO parents (`op.parent_op_hashes.is_empty()`). A
                // genesis is structurally the DAG root; a `NamespaceCreated`
                // carrying parents is NOT a genesis even when it names the
                // established founder (it was minted against an existing DAG
                // head). Acting on such a parented op would let a structurally
                // non-genesis op mutate authority-bearing state (the Admin member
                // row / caps / owner meta) on an already-established namespace, so
                // we treat it as a pure no-op. This stays consistent with the
                // #591 fix: established namespaces NEVER return `Err` (no DAG
                // stall), we simply do not act on a non-genesis-shaped op.
                if !op.parent_op_hashes.is_empty() {
                    tracing::debug!(
                        namespace_id = %hex::encode(namespace_id.as_bytes()),
                        %founder,
                        parent_count = op.parent_op_hashes.len(),
                        "NamespaceCreated: same-founder PARENTED op on an established \
                         namespace is not genesis-shaped; no-op (no repair)"
                    );
                    return Ok(());
                }

                // Genesis-shaped same-founder re-arrival (e.g. the founder's own
                // genesis coming back via sync backfill, or a path that wrote a
                // non-placeholder root `admin_identity` for the founder before
                // genesis applied). The meta's `admin_identity` is already
                // correct, but several pieces of state may be missing or diverged
                // and must be repaired idempotently:
                //
                //   * the founder's explicit Admin member row may never have been
                //     written (the path that set `admin_identity` need not have
                //     created it) — ensure it with an idempotent upsert so the
                //     founder is always enumerable as Admin;
                //   * the default caps row may be absent — seed it absence-gated;
                //   * (#602) `owner_identity` may have DIVERGED from the founder
                //     (a partial write or legacy writer set admin but not owner).
                //     Repair it by writing back updated meta with
                //     `owner_identity = founder` while PRESERVING every other
                //     field. Only write when it actually differs, to avoid a
                //     pointless `put` on the common already-correct path.
                //
                // Ordering: write the Admin member row BEFORE the owner_identity
                // meta-save. The member row is the authority-bearing row; with a
                // non-atomic store, if a write fails midway, losing the (#602)
                // owner repair is less harmful than losing the Admin row.
                //
                // ONLY-UPGRADE, NEVER-DOWNGRADE: `add_member` is a guaranteed
                // upsert (overwrites the role), and Admin is the top role today,
                // so an unconditional write would be harmless now. But to stay
                // correct against a hypothetical future role richer than Admin,
                // we read the current role and only force Admin when the founder
                // is absent or a plain Member. This preserves the
                // seed-before-genesis upgrade path (Member → Admin) while never
                // clobbering an already-Admin-or-richer row on this re-arrival
                // branch (where the row already exists).
                let membership = MembershipRepository::new(store);
                match membership.role_of(&ns_gid, &founder)? {
                    None | Some(GroupMemberRole::Member) => {
                        membership.add_member(&ns_gid, &founder, GroupMemberRole::Admin)?;
                    }
                    Some(_) => {}
                }
                if meta.owner_identity != founder {
                    let mut repaired = meta.clone();
                    repaired.owner_identity = founder;
                    MetaRepository::new(store).save(&ns_gid, &repaired)?;
                    tracing::debug!(
                        namespace_id = %hex::encode(namespace_id.as_bytes()),
                        %founder,
                        prior_owner = %meta.owner_identity,
                        "NamespaceCreated: same-founder re-arrival repaired diverged \
                         owner_identity to the founder (#602)"
                    );
                }
                // Seed the Open-join default caps, mirroring the establish branch
                // below. Absence-gated for the same no-clobber reason documented
                // there: a later `DefaultCapabilitiesSet` must never be
                // overwritten.
                let caps = CapabilitiesRepository::new(store);
                if caps.default_capabilities(&ns_gid)?.is_none() {
                    caps.set_default_capabilities(
                        &ns_gid,
                        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits(),
                    )?;
                }
                tracing::debug!(
                    namespace_id = %hex::encode(namespace_id.as_bytes()),
                    %founder,
                    "NamespaceCreated: founder already established; ensured Admin member row \
                     (genesis-shaped idempotent re-arrival)"
                );
                return Ok(());
            }
            // (2b) established by SOMEONE ELSE — pure no-op, touch nothing. We
            // must NOT write membership/caps/meta, or a forged second genesis
            // would grant its declared founder state on a namespace they do not
            // own. That is the anti-hijack guarantee.
            tracing::debug!(
                namespace_id = %hex::encode(namespace_id.as_bytes()),
                established_admin = %meta.admin_identity,
                established_owner = %meta.owner_identity,
                %founder,
                "NamespaceCreated: namespace already has an established founder; \
                 ignoring genesis (anti-hijack no-op)"
            );
            return Ok(());
        }
    }

    // ---- (3) NOT established ⇒ this op is trying to FOUND the namespace. ----
    // Enforce the genesis invariants, THEN establish. These structural checks
    // are load-bearing ONLY here, while no real admin exists: they stop a
    // parented or signer-mismatched op from forging the genesis admin.

    // ---- (3a) TRUE-genesis gate: a genesis MUST be the DAG root (no parents). ----
    // Checked FIRST, BEFORE the signer check (#596): a brand-new namespace has
    // no persisted head, so `read_head_record` (namespace/dag.rs) returns an
    // EMPTY `parent_hashes`, and the signer (`sign_apply_and_publish`) signs the
    // genesis with `parent_op_hashes == []`. A `NamespaceCreated` carrying
    // parents on a not-yet-established namespace was therefore minted against an
    // EXISTING DAG head — it is structurally NOT the genesis, so it is REJECTED
    // regardless of signer (checking parents first means we never even
    // consult/leak the declared founder for a structurally-invalid founding op).
    //
    // WHY `Err` (and NOT a no-op `Ok`): `apply_signed_op` (governance.rs) calls
    // `apply_root_op(op)?` and, ONLY on `Ok`, then runs `advance_dag_head` +
    // `store_operation`. Returning `Err` here propagates BEFORE `advance_dag_head`
    // runs, so the DAG head is NOT advanced and the namespace stays establishable
    // by a subsequent parentless genesis. A no-op `Ok(())` here would advance the
    // head on a BARE namespace, after which the legitimate parentless genesis can
    // no longer apply cleanly (the head is non-empty) — BRICKING establishment.
    //
    // The "DAG stall" worry that might motivate a no-op is unfounded: a parented
    // `NamespaceCreated` on a bare namespace is essentially unreachable in a
    // VALID DAG (a parented op only applies after its parents, by which point the
    // parentless genesis has already established the namespace, so the ESTABLISHED
    // branch (step 2) handles it as a harmless `Ok` no-op). And the backfill path
    // is retry-tolerant — it logs "failed to apply ... from backfill" and
    // continues, never a permanent stall.
    //
    // CONTRAST: the ESTABLISHED + parented case (step 2a above) MUST STAY an `Ok`
    // no-op (the #591 fix). On an already-founded namespace, advancing the head
    // is harmless and erroring could stall the DAG. Only this NOT-established +
    // parented case returns `Err`.
    //
    // DAG sequencing comes from `read_head_record().next_nonce`, not the
    // informational `op.nonce`.
    if !op.parent_op_hashes.is_empty() {
        bail!(ApplyError::NamespaceCreatedRejected(
            NamespaceCreatedRejection::NotGenesis {
                parent_count: op.parent_op_hashes.len(),
            }
        ));
    }

    // ---- (3b) Self-authorization binding: signer MUST equal the founder. ----
    // Genesis is the one authority-bearing root op that SKIPS
    // `require_namespace_admin` (there is no prior admin to check against), so
    // the only thing tying the established admin to a real signing key is this
    // check. The invariant holds because the genesis is signed with the
    // namespace key == the founder's key at creation. Without it a non-founder
    // could sign `NamespaceCreated { founder: <someone-else> }` with their own
    // key and, on a namespace with no prior genesis, pin a forged/wrong admin.
    //
    // SECURITY: RESIDUAL (#2474 reviewer batch 3): this check only blocks MISMATCHED
    // forgeries (signer signs a genesis naming a DIFFERENT founder). It does
    // NOT block a SELF-CONSISTENT forged genesis — an attacker who signs
    // `NamespaceCreated { founder: <self> }` on a BARE namespace passes this
    // check (signer == founder == attacker) and becomes that namespace's admin.
    // Nothing here binds `namespace_id` to the legitimate founder, because today
    // `namespace_id` is RANDOM and unrelated to any key. The established gate
    // above only protects an ALREADY-established namespace; it cannot tell a
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

    // ---- Establish the founder as admin == owner on the root meta. ----
    // Only the founding identity is authoritative here; we carry forward every
    // OTHER meta field from `existing` (target_application_id / app_key /
    // upgrade_policy / migration / created_at / auto_join) rather than reset it.
    //
    // What this preserve actually covers:
    //   * ORIGINATOR case — the creating node may have written real
    //     `app_key` / `target_application_id` (and friends) into the root meta
    //     BEFORE this genesis op applied (e.g. the create_group handler's local
    //     meta write). Carrying them forward keeps those real bindings intact.
    //   * REPLICA case — a placeholder bootstrap seed
    //     (`seed_bootstrap_admin_if_absent`) ALWAYS writes ZEROS for these app
    //     fields, so on a replica this preserve is a no-op: the zeros are simply
    //     carried forward and SELF-HEAL on the first `ContextRegistered` op,
    //     exactly per the seed contract. (It is NOT preserving meaningful app
    //     bindings a seed set — the seed never sets any.)
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
    //
    // INVARIANT: add_member must be an unconditional upsert (overwrite, not
    // skip-if-exists) so the seed-before-genesis ordering upgrades a seeded
    // Member row to Admin. If add_member ever becomes skip-if-exists, this
    // handler must switch to an explicit role-upgrade. Covered by the regression
    // test `namespace_created_genesis_upgrades_seeded_member_founder_to_admin`
    // (Member → Admin upgrade on this establish path).
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
        caps.set_default_capabilities(&ns_gid, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits())?;
    }

    tracing::info!(
        namespace_id = %hex::encode(namespace_id.as_bytes()),
        %founder,
        "NamespaceCreated genesis applied: founder established as namespace admin/owner"
    );
    Ok(())
}
