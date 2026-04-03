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
