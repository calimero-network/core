//! `RootOp::GroupCreated` apply handler. Extracted from
//! `NamespaceGovernance::execute_group_created` in #2481.

use super::context::NamespaceApplyCtx;
use crate::op_events::OpEvent;
use crate::{
    ApplyError, CapabilitiesRepository, GroupCreatedRejection, MembershipRepository,
    MetaRepository, NamespaceError, NamespaceRepository,
};
use calimero_context_client::local_governance::SignedNamespaceOp;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::GroupMemberRole;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut NamespaceApplyCtx<'_>,
    op: &SignedNamespaceOp,
    group_id: [u8; 32],
    parent_id: [u8; 32],
    restricted: bool,
    declared_admin: calimero_account::AccountId,
) -> EyreResult<()> {
    let store = ctx.store();
    let namespace_id = ctx.namespace_id();
    let gid = ContextGroupId::from(group_id);
    let parent_gid = ContextGroupId::from(parent_id);

    // Namespace roots are created via a different path (local meta +
    // identity writes, no GroupCreated op); GroupCreated itself is only
    // for subgroups. Reject self-parent to make that invariant explicit
    // — a self-parent edge would cause resolve_namespace to cycle.
    if group_id == parent_id {
        eyre::bail!(NamespaceError::SelfParentEdge);
    }

    // Authorization. Namespace-root admins may create a subgroup at any
    // depth (matches `require_namespace_admin`). A non-admin namespace
    // member may create one *directly under the namespace root* if they
    // hold `CAN_CREATE_SUBGROUP` — that bit is honored only at root level
    // because every peer applying this op must be able to verify the
    // creator's authority, and only the root group's capability rows are
    // readable by all namespace members (see the capability's doc).
    //
    // Both legs resolve at the op's causal cut. Reading the live membership rows
    // here instead would let two replicas that folded different sets of concurrent
    // capability ops reach opposite verdicts on this same op — the rejecting one
    // drops it and never advances past it.
    let ns_gid = ContextGroupId::from(namespace_id.to_bytes());
    let ns_permissions = ctx.permissions_for(ns_gid);
    let authorized = ns_permissions.is_admin(&op.signer)?
        || (parent_id == namespace_id.to_bytes()
            && ns_permissions.can_create_subgroup(&op.signer)?);
    if !authorized {
        bail!(ApplyError::GroupCreatedRejected(
            GroupCreatedRejection::Unauthorized {
                signer: format!("{}", op.signer),
                namespace: hex::encode(namespace_id.as_bytes()),
            }
        ));
    }

    // Verify parent exists in this namespace (root or previously-created subgroup).
    let parent_meta = MetaRepository::new(store)
        .load(&parent_gid)?
        .ok_or_else(|| {
            eyre::eyre!("GroupCreated rejected: parent_id '{parent_gid:?}' not found in namespace")
        })?;

    // Meta rows are keyed by group id alone, so an existing `parent_meta` proves
    // only that the parent exists SOMEWHERE — not that it belongs to THIS
    // namespace. Without this check an admin of namespace A could graft a
    // subgroup under a group of namespace B, splicing A's crypto/access boundary
    // into B. Require the parent to resolve to this namespace's root.
    let parent_ns = NamespaceRepository::new(store)
        .resolve(&parent_gid)
        .map_err(|e| eyre::eyre!("GroupCreated rejected: cannot resolve parent namespace: {e}"))?;
    if parent_ns.to_bytes() != namespace_id.to_bytes() {
        bail!(ApplyError::GroupCreatedRejected(
            GroupCreatedRejection::ParentCrossNamespace {
                parent: format!("{parent_gid:?}"),
                parent_namespace: hex::encode(parent_ns.to_bytes()),
                namespace: hex::encode(namespace_id.as_bytes()),
            }
        ));
    }

    // The originating node's `create_group` handler pre-populates
    // `GroupMeta` (and related state) BEFORE publishing this op, so a
    // naive "if meta exists, return early" idempotency check would
    // short-circuit on the originator's local apply, leaving the group
    // without `GroupParentRef` / `GroupChildIndex` edges. Remote peers
    // applying a fresh op would write edges correctly, causing silent
    // divergence between originator and peers (resolve_namespace,
    // list_child_groups, and reparent would all fail on the originator).
    //
    // Fix: only skip the meta write if it already exists, but ALWAYS
    // ensure parent edge + child index + admin membership are present.
    // These are idempotent puts — a second apply is a no-op with
    // identical effect, so true replay is still safe.
    // The creator becomes the subgroup's founding admin and owner, and both are
    // recorded as accounts. Resolving rather than storing a key-derived stand-in
    // is what makes the pins comparable to the membership rows every later gate
    // reads; a creator this namespace has no binding for cannot found a group,
    // because nothing it later signs would match the admin it was pinned as.
    //
    // Resolved once for both the meta pin and the Admin row below. They must
    // agree — same signer, same parent — and each resolution is a binding-column
    // scan, so doing it twice bought nothing but the chance of drifting apart.
    //
    // Resolved through the permission checker, not off live binding rows: those
    // rows are folded state like any other, so a bare live read would let a
    // replica that has not yet folded the creator's device-link op answer
    // `NotMember` for an op its peer admitted — a permanent, fold-order-dependent
    // divergence in projected state. The checker parks instead, and the op is
    // retried when the ancestry arrives. It resolves against the parent, which is
    // where the authority that admitted this op lives.
    let Some(creator) = ctx
        .permissions_for(parent_gid)
        .account_for_signer(&op.signer)?
    else {
        bail!(crate::MembershipError::NotMember {
            group_id: hex::encode(parent_gid.to_bytes()),
            identity: format!("{}", op.signer),
        });
    };

    let meta_existed = MetaRepository::new(store).load(&gid)?.is_some();
    if !meta_existed {
        // Inherit application ID AND bytecode_id from the immediate parent.
        // target_application_id is inherited (matches mero-drive folder
        // mental model: a subfolder runs the same app as its parent), so
        // bytecode_id (which on the originator is derived from that
        // application's bytecode blob_id by `create_group::handle`) must
        // be inherited too — otherwise the cascade predicate
        // (from_bytecode_id == descendant.bytecode_id) would silently skip every
        // remote-created subgroup the originator added. Zero-init here
        // was the source of #2358-class cascade-skip bugs.
        // The op CARRIES the creator's account so a receiver can fold it without
        // resolving anything — but authority still comes from the resolution
        // above, never from the field. They must agree: a signer that names an
        // account it does not speak for would otherwise pin a subgroup admin its
        // own later signatures could never match, and the fold would record a
        // principal the rows disagree with.
        if declared_admin != creator {
            bail!(ApplyError::GroupCreatedRejected(
                GroupCreatedRejection::Unauthorized {
                    signer: format!("{}", op.signer),
                    namespace: hex::encode(namespace_id.as_bytes()),
                }
            ));
        }
        let meta = calimero_store::key::GroupMetaValue {
            admin_identity: creator,
            owner_identity: creator,
            // One field, so the id cannot be inherited without the coordinates
            // that address it - the subgroup has no ladder of its own to
            // recover them from.
            target: parent_meta.target.clone(),
            migration: None,
            created_at: 0,
            auto_join: false,
        };
        MetaRepository::new(store).save(&gid, &meta)?;
    } else {
        tracing::debug!(
            group_id = %hex::encode(group_id),
            "GroupCreated: meta already present (pre-populated by handler or replay); \
             skipping meta write but still ensuring parent edge + admin membership"
        );
    }

    // Ordered writes — NOT a single RocksDB atomic batch. Each call
    // below opens its own store handle. A crash between any two steps
    // leaves partial state. Recovery path: re-applying the same
    // GroupCreated op is idempotent (meta-exists check skips the meta
    // write; edge writes are idempotent puts; add_member is an upsert)
    // — so retries complete whatever was missing.
    {
        use calimero_store::key::{GroupChildIndex, GroupParentRef};
        let mut handle = store.handle();
        handle.put(&GroupParentRef::new(group_id), &parent_id)?;
        handle.put(&GroupChildIndex::new(parent_id, group_id), &())?;
    }
    MembershipRepository::new(store).add_member(&gid, &creator, GroupMemberRole::Admin)?;

    // Born-Open atomic create (#2771): write the subgroup's visibility key
    // from `restricted` using the SAME mechanism `SubgroupVisibilitySet`
    // apply uses (`CapabilitiesRepository::set_subgroup_visibility`). This
    // write happens DURING apply, BEFORE `OpEvent::SubgroupCreated` is
    // queued/drained (emit-after-persist, #2770) — so when
    // `tee_subgroup_admit` reacts and walks `is_open_chain_to_namespace`,
    // it reads the real visibility from the store. A born-Open subgroup is
    // therefore already Open at admit time, so the TEE is skipped (it reads
    // via inheritance) and no transient direct `ReadOnlyTee` row is left
    // behind. `restricted: true` (the default) preserves legacy behavior,
    // and the absent-key ⇒ Restricted default in `capabilities.rs` stays as
    // a safety net for old state.
    //
    // ONLY write birth visibility on the genuine FIRST create. Birth
    // visibility is an initial condition, not idempotent state: a duplicate
    // `GroupCreated` (different nonce, same `group_id`) is a replay, and a
    // later `SubgroupVisibilitySet` may have flipped the group's visibility
    // in the meantime — re-asserting the birth value on replay would silently
    // clobber that flip.
    //
    // The gate is the ABSENCE of an explicit visibility key, NOT `!meta_existed`:
    // the originator's `create_group` handler pre-populates `GroupMeta` before
    // publishing this op (so `meta_existed` is true on the originator's own
    // first apply) but does NOT write the visibility key — that key is born
    // here. So `!has_subgroup_visibility` is true exactly on the first apply
    // (originator and remote alike) and false on every replay. This mirrors
    // the idempotent-seed discipline used for the meta write above.
    let caps = CapabilitiesRepository::new(store);
    if !caps.has_subgroup_visibility(&gid)? {
        let visibility = if restricted {
            calimero_context_config::VisibilityMode::Restricted
        } else {
            calimero_context_config::VisibilityMode::Open
        };
        caps.set_subgroup_visibility(&gid, visibility)?;
    }

    ctx.queue_event(OpEvent::SubgroupCreated {
        namespace_id,
        parent_group_id: parent_id,
        child_group_id: group_id,
    });
    Ok(())
}
