//! `RootOp::GroupDeleted` apply handler. Extracted from
//! `NamespaceGovernance::execute_group_deleted` in #2481.

use super::context::NamespaceApplyCtx;
use crate::group_store::{
    delete_group_local_rows, enumerate_group_contexts, unregister_context_from_group, ApplyError,
    CapabilitiesError, GroupDeletedRejection, MetaRepository, NamespaceError, NamespaceRepository,
    PermissionChecker,
};
use calimero_context_client::local_governance::SignedNamespaceOp;
use calimero_context_config::types::ContextGroupId;
use eyre::Result as EyreResult;

pub(crate) fn apply(
    ctx: &NamespaceApplyCtx<'_>,
    op: &SignedNamespaceOp,
    root_group_id: [u8; 32],
    cascade_group_ids: &[[u8; 32]],
    cascade_context_ids: &[[u8; 32]],
) -> EyreResult<()> {
    let store = ctx.store();
    let namespace_id = ctx.namespace_id();
    let root_gid = ContextGroupId::from(root_group_id);
    if root_group_id == namespace_id {
        eyre::bail!(NamespaceError::CannotDeleteRoot(format!("{root_gid:?}")));
    }

    // Authorization. Cascade-delete is allowed for: the owner of the
    // subgroup being deleted; an admin of the namespace root (moderation);
    // or a namespace member holding `CAN_DELETE_SUBGROUP` (an explicit
    // delegation). All three are deterministically verifiable on every
    // peer applying this op — `GroupDeleted` is cleartext, and the
    // deleting peer holds the root group's meta on the *first* apply. The
    // non-owner case routes through `PermissionChecker` to match the local
    // `delete_group` handler.
    //
    // The owner branch checks only `owner_identity == op.signer`, not
    // current namespace membership — `owner_identity` is a persistent
    // record from group creation, and matching it *is* being the owner
    // (32-byte keys don't "happen to collide"). In practice the owner is
    // always a current namespace member anyway: `leave_namespace` /
    // `leave_group` reject an owner with `MustTransferOwnership`, so you
    // can't leave while owning a subgroup in the subtree.
    //
    // Crash-recovery: the cascade below tears the root group's meta down
    // *last* (after every descendant), and only then does `apply_signed_op`
    // advance the DAG head. If the process dies in between, the re-apply
    // finds the root meta already gone — the op was authorized on the
    // first pass, so we skip the auth check here and let the (idempotent)
    // cascade finish any remaining cleanup.
    let ns_gid = ContextGroupId::from(namespace_id);
    if let Some(root_meta) = MetaRepository::new(store).load(&root_gid)? {
        if root_meta.owner_identity != op.signer {
            if let Err(e) =
                PermissionChecker::new(store, ns_gid).require_can_delete_subgroup(&op.signer)
            {
                // Preserve the typed `CapabilitiesError` inside
                // `GroupDeletedRejection::Unauthorized` (see PR #2495 review).
                return Err(match e.downcast::<CapabilitiesError>() {
                    Ok(cap_err) => {
                        ApplyError::GroupDeletedRejected(GroupDeletedRejection::Unauthorized {
                            cause: cap_err,
                            subgroup: hex::encode(root_group_id),
                        })
                        .into()
                    }
                    Err(other) => other,
                });
            }
        }
    }

    // Determinism check: every surviving element of the local subtree MUST
    // be in the op's payload. We use subset rather than exact equality
    // because a previous apply attempt may have crashed mid-cascade,
    // leaving the local subtree as a partial-delete state.
    let local_payload = NamespaceRepository::new(store).collect_subtree_for_cascade(&root_gid)?;
    let local_groups: std::collections::BTreeSet<[u8; 32]> = local_payload
        .descendant_groups
        .iter()
        .map(|g| g.to_bytes())
        .collect();
    let local_contexts: std::collections::BTreeSet<[u8; 32]> =
        local_payload.contexts.iter().map(|c| **c).collect();
    let payload_groups: std::collections::BTreeSet<[u8; 32]> =
        cascade_group_ids.iter().copied().collect();
    let payload_contexts: std::collections::BTreeSet<[u8; 32]> =
        cascade_context_ids.iter().copied().collect();
    if !local_groups.is_subset(&payload_groups) {
        let extra: Vec<String> = local_groups
            .difference(&payload_groups)
            .map(hex::encode)
            .collect();
        eyre::bail!(ApplyError::GroupDeletedRejected(
            GroupDeletedRejection::CascadeDivergenceGroups { extra }
        ));
    }
    if !local_contexts.is_subset(&payload_contexts) {
        let extra: Vec<String> = local_contexts
            .difference(&payload_contexts)
            .map(hex::encode)
            .collect();
        eyre::bail!(ApplyError::GroupDeletedRejected(
            GroupDeletedRejection::CascadeDivergenceContexts { extra }
        ));
    }
    // Inverse direction is *not* an error — it's the expected shape on a
    // crash-recovery re-apply (the local subtree shrank since the op was
    // built) — but log it so a genuinely anomalous payload is visible.
    let payload_only_groups: Vec<[u8; 32]> =
        payload_groups.difference(&local_groups).copied().collect();
    if !payload_only_groups.is_empty() {
        tracing::warn!(
            root_group_id = %hex::encode(root_group_id),
            groups = ?payload_only_groups.iter().map(hex::encode).collect::<Vec<_>>(),
            "GroupDeleted payload lists groups not present locally (expected on a \
             crash-recovery re-apply; otherwise investigate for divergence)"
        );
    }
    let payload_only_contexts: Vec<[u8; 32]> = payload_contexts
        .difference(&local_contexts)
        .copied()
        .collect();
    if !payload_only_contexts.is_empty() {
        tracing::warn!(
            root_group_id = %hex::encode(root_group_id),
            contexts = ?payload_only_contexts.iter().map(hex::encode).collect::<Vec<_>>(),
            "GroupDeleted payload lists contexts not present locally (expected on a \
             crash-recovery re-apply; otherwise investigate for divergence)"
        );
    }

    // Children-first deletion: descendants then root.
    let all_groups_iter = cascade_group_ids
        .iter()
        .copied()
        .chain(std::iter::once(root_group_id));
    for gid_bytes in all_groups_iter {
        let gid = ContextGroupId::from(gid_bytes);
        for context_id in enumerate_group_contexts(store, &gid, 0, usize::MAX)? {
            unregister_context_from_group(store, &gid, &context_id)?;
        }
        // Capture parent before delete_group_local_rows runs.
        let parent_for_cleanup = NamespaceRepository::new(store).parent(&gid)?;
        delete_group_local_rows(store, &gid)?;
        if let Some(parent) = parent_for_cleanup {
            let mut handle = store.handle();
            handle.delete(&calimero_store::key::GroupParentRef::new(gid_bytes))?;
            handle.delete(&calimero_store::key::GroupChildIndex::new(
                parent.to_bytes(),
                gid_bytes,
            ))?;
        }
    }

    tracing::info!(
        ?root_gid,
        deleted_groups = cascade_group_ids.len() + 1,
        deleted_contexts = cascade_context_ids.len(),
        "cascade-deleted group subtree"
    );
    Ok(())
}
