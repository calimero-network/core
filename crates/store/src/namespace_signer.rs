//! Resolving the node's own namespace signing identity for a context.
//!
//! In a namespace-backed context (every context created under the group model)
//! a member's [`key::ContextIdentity`] row is a **keyless marker**: the row's
//! presence is the membership signal, and the signing key is *not* copied per
//! context. The node signs with the single identity it holds for the context's
//! **namespace root**, resolved live.
//!
//! This lives in `calimero-store` because more than one layer needs it and there
//! is no dependency edge between them: `calimero-context-client` (which owns the
//! membership view) and `calimero-node-primitives` (which signs blob requests)
//! both depend on this crate, but `calimero-context-client` depends on
//! `calimero-node-primitives`, so the latter cannot reach the former. Keeping one
//! implementation here avoids two copies of a security-relevant lookup drifting
//! apart — which is exactly how blob authorization silently broke for every
//! namespace-backed context.

use calimero_primitives::context::ContextId;

use crate::key;
use crate::Store;

/// The node's own namespace identity for `context_id`'s namespace, if it holds one.
///
/// Returns `(public_key, private_key)` as raw 32-byte arrays; callers wrap them in
/// whatever key newtype they use.
///
/// `max_depth` bounds the walk from the context's group up to the namespace root
/// and should be `calimero_context_config::MAX_NAMESPACE_DEPTH` — the same bound
/// the canonical `NamespaceRepository::resolve` uses. It is a parameter rather
/// than a constant here so this crate needs no dependency on the config crate.
///
/// # Errors
///
/// Fails loud if the parent chain exceeds `max_depth` (too deep, or cyclic
/// `GroupParentRef` data) rather than silently resolving against a non-root
/// ancestor, matching the canonical resolver's `DepthExceeded` behaviour. Also
/// propagates datastore read errors.
pub fn resolve_owned_namespace_signer(
    store: &Store,
    context_id: &ContextId,
    max_depth: usize,
) -> eyre::Result<Option<([u8; 32], [u8; 32])>> {
    let handle = store.handle();

    let Some(group_id) = handle.get(&key::ContextGroupRef::new(*context_id))? else {
        return Ok(None);
    };

    // The namespace identity is keyed at the namespace root; walk up to it.
    // Inclusive loop bound mirrors `NamespaceRepository::resolve`.
    let mut ns_root = group_id;
    let mut reached_root = false;
    for _ in 0..=max_depth {
        match handle.get(&key::GroupParentRef::new(ns_root))? {
            Some(parent) => ns_root = parent,
            None => {
                reached_root = true;
                break;
            }
        }
    }
    if !reached_root {
        eyre::bail!(
            "namespace parent chain for context {context_id} exceeds \
             MAX_NAMESPACE_DEPTH (too deep or cyclic GroupParentRef data)"
        );
    }

    let Some(identity) = handle.get(&key::NamespaceIdentity::new(ns_root))? else {
        return Ok(None); // this node holds no identity for the namespace
    };
    let identity: key::NamespaceIdentityValue = identity;

    Ok(Some((identity.public_key, identity.private_key)))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_primitives::context::ContextId;

    use super::resolve_owned_namespace_signer;
    use crate::db::InMemoryDB;
    use crate::key;
    use crate::Store;

    const MAX_DEPTH: usize = 16;

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    /// Wire up `context -> group -> ... -> ns_root` and, optionally, the node's
    /// identity at the root. `chain` is ordered leaf-first; the last element is
    /// the namespace root (it gets no `GroupParentRef`).
    fn seed(
        store: &Store,
        context_id: ContextId,
        chain: &[[u8; 32]],
        identity_at_root: Option<([u8; 32], [u8; 32])>,
    ) {
        let mut handle = store.handle();
        handle
            .put(&key::ContextGroupRef::new(context_id), &chain[0])
            .unwrap();
        for pair in chain.windows(2) {
            handle
                .put(&key::GroupParentRef::new(pair[0]), &pair[1])
                .unwrap();
        }
        if let Some((public_key, private_key)) = identity_at_root {
            handle
                .put(
                    &key::NamespaceIdentity::new(*chain.last().unwrap()),
                    &key::NamespaceIdentityValue {
                        public_key,
                        private_key,
                        sender_key: [0u8; 32],
                    },
                )
                .unwrap();
        }
    }

    #[test]
    fn resolves_identity_held_at_the_namespace_root() {
        let store = store();
        let context_id = ContextId::from([1u8; 32]);
        // context -> group(2) -> group(3) -> root(4)
        seed(
            &store,
            context_id,
            &[[2u8; 32], [3u8; 32], [4u8; 32]],
            Some(([9u8; 32], [8u8; 32])),
        );

        let resolved = resolve_owned_namespace_signer(&store, &context_id, MAX_DEPTH).unwrap();
        assert_eq!(resolved, Some(([9u8; 32], [8u8; 32])));
    }

    #[test]
    fn resolves_when_the_context_group_is_itself_the_root() {
        let store = store();
        let context_id = ContextId::from([1u8; 32]);
        seed(
            &store,
            context_id,
            &[[2u8; 32]],
            Some(([7u8; 32], [6u8; 32])),
        );

        let resolved = resolve_owned_namespace_signer(&store, &context_id, MAX_DEPTH).unwrap();
        assert_eq!(resolved, Some(([7u8; 32], [6u8; 32])));
    }

    #[test]
    fn none_when_the_node_holds_no_identity_for_the_namespace() {
        let store = store();
        let context_id = ContextId::from([1u8; 32]);
        seed(&store, context_id, &[[2u8; 32], [3u8; 32]], None);

        assert_eq!(
            resolve_owned_namespace_signer(&store, &context_id, MAX_DEPTH).unwrap(),
            None
        );
    }

    #[test]
    fn none_when_the_context_is_not_group_backed() {
        let store = store();
        // No ContextGroupRef at all — a standalone context.
        assert_eq!(
            resolve_owned_namespace_signer(&store, &ContextId::from([1u8; 32]), MAX_DEPTH).unwrap(),
            None
        );
    }

    #[test]
    fn errors_on_a_cyclic_parent_chain_rather_than_resolving_wrongly() {
        let store = store();
        let context_id = ContextId::from([1u8; 32]);
        let mut handle = store.handle();
        handle
            .put(&key::ContextGroupRef::new(context_id), &[2u8; 32])
            .unwrap();
        // 2 -> 3 -> 2 -> ... never reaches a root.
        handle
            .put(&key::GroupParentRef::new([2u8; 32]), &[3u8; 32])
            .unwrap();
        handle
            .put(&key::GroupParentRef::new([3u8; 32]), &[2u8; 32])
            .unwrap();

        let err = resolve_owned_namespace_signer(&store, &context_id, MAX_DEPTH)
            .expect_err("a cyclic chain must fail loud, not resolve to an ancestor");
        assert!(
            err.to_string().contains("MAX_NAMESPACE_DEPTH"),
            "unexpected error: {err}"
        );
    }
}
