//! `RootOp::GroupCreated` apply handler. Extracted from
//! `NamespaceGovernance::execute_group_created` in #2481.

use super::context::NamespaceApplyCtx;
use crate::op_events::OpEvent;
use crate::{
    ApplyError, CapabilitiesRepository, GroupCreatedRejection, MembershipRepository,
    MetaRepository, NamespaceError,
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
    let ns_gid = ContextGroupId::from(namespace_id.to_bytes());
    let authorized = MembershipRepository::new(store).is_admin(&ns_gid, &op.signer)?
        || (parent_id == namespace_id.to_bytes()
            && MembershipRepository::new(store).is_admin_or_has_capability(
                &ns_gid,
                &op.signer,
                calimero_context_config::MemberCapabilities::CAN_CREATE_SUBGROUP.bits(),
            )?);
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
    let meta_existed = MetaRepository::new(store).load(&gid)?.is_some();
    if !meta_existed {
        // Inherit application ID AND app_key from the immediate parent.
        // target_application_id is inherited (matches mero-drive folder
        // mental model: a subfolder runs the same app as its parent), so
        // app_key (which on the originator is derived from that
        // application's bytecode blob_id by `create_group::handle`) must
        // be inherited too — otherwise the cascade predicate
        // (from_app_key == descendant.app_key) would silently skip every
        // remote-created subgroup the originator added. Zero-init here
        // was the source of #2358-class cascade-skip bugs.
        let meta = calimero_store::key::GroupMetaValue {
            admin_identity: op.signer,
            owner_identity: op.signer,
            target_application_id: parent_meta.target_application_id,
            app_key: parent_meta.app_key,
            upgrade_policy: calimero_primitives::context::UpgradePolicy::default(),
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
    MembershipRepository::new(store).add_member(&gid, &op.signer, GroupMemberRole::Admin)?;

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
