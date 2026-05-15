use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::context_tree::ContextTreeService;

pub fn register_context_in_group(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<()> {
    ContextTreeService::new(store, *group_id).register_context(context_id)
}

pub fn unregister_context_from_group(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<()> {
    ContextTreeService::new(store, *group_id).unregister_context(context_id)
}

pub fn get_group_for_context(
    store: &Store,
    context_id: &ContextId,
) -> EyreResult<Option<ContextGroupId>> {
    ContextTreeService::new(store, ContextGroupId::from([0u8; 32])).group_for_context(context_id)
}

pub fn enumerate_group_contexts(
    store: &Store,
    group_id: &ContextGroupId,
    offset: usize,
    limit: usize,
) -> EyreResult<Vec<ContextId>> {
    ContextTreeService::new(store, *group_id).enumerate_contexts(offset, limit)
}

/// Internal helper intended to be used only from authorization-checked paths.
/// Callers must enforce the relevant governance permissions.
pub fn cascade_remove_member_from_group_tree(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<()> {
    ContextTreeService::new(store, *group_id).cascade_remove_member(member)
}

/// Inverse of [`cascade_remove_member_from_group_tree`]: re-create
/// `ContextIdentity` rows for the rejoiner under every context registered
/// directly beneath `group_id`.
///
/// Idempotent on rows that already exist (e.g., first-time join via the
/// `join_context` handler beats the apply-path call to here). A rejoiner
/// who never had a `ContextIdentity` row for a given context (because the
/// context was registered after they were removed) gets a freshly-written
/// row with `private_key: Some(_)` and `sender_key: None` — the same
/// shape `join_context` writes — so KeyDelivery can then populate
/// `sender_key` exactly as it would on first-join.
///
/// This is the *local* half of rejoin recovery. Other peers do not hold
/// `ContextIdentity` rows for this rejoiner (only the rejoiner's own
/// store does — that's the row the cascade deleted), so the apply path
/// only needs to do anything on the rejoiner's own node. Callers must
/// gate on `private_key.is_some()` (i.e., they must have access to the
/// rejoiner's namespace identity bytes) before invoking — peers without
/// the key cannot and should not write a row claiming to own a private
/// key they don't have.
pub fn restore_member_context_identities(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
    private_key: [u8; 32],
) -> EyreResult<()> {
    let contexts = enumerate_group_contexts(store, group_id, 0, usize::MAX)?;
    let mut handle = store.handle();
    for context_id in &contexts {
        let identity_key = calimero_store::key::ContextIdentity::new(*context_id, (*member).into());
        if !handle.has(&identity_key)? {
            handle.put(
                &identity_key,
                &calimero_store::types::ContextIdentity {
                    private_key: Some(private_key),
                    sender_key: None,
                },
            )?;
            tracing::info!(
                group_id = %hex::encode(group_id.to_bytes()),
                context_id = %hex::encode(context_id.as_ref()),
                member = %member,
                "rejoin: restored ContextIdentity row for local rejoiner"
            );
        }
    }
    Ok(())
}

/// Scans the ContextIdentity column for the given context and returns the first
/// `PublicKey` for which the node holds a local private key. Used to find a
/// valid signer when performing group upgrades on behalf of a context that the
/// group admin may not be a member of.
pub fn find_local_signing_identity(
    store: &Store,
    context_id: &ContextId,
) -> EyreResult<Option<PublicKey>> {
    ContextTreeService::new(store, ContextGroupId::from([0u8; 32]))
        .find_local_signing_identity(context_id)
}
