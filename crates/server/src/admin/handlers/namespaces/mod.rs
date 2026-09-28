pub mod admit_join;
pub mod create_group_in_namespace;
pub mod create_namespace;
pub mod delete_namespace;
pub mod get_namespace;
pub mod invite_namespace;
pub mod join_namespace;
pub mod leave_namespace;
pub mod list;
pub mod list_for_application;
pub mod list_namespace_groups;
pub mod revoke_device;

/// Per-namespace `appVersion`: the bundle-manifest version of the
/// namespace's `bytecode_id` blob. `None` when unresolvable (zero/legacy key,
/// raw-wasm app, blob not retained locally) — display-only, never an error.
pub(crate) async fn namespace_app_version(
    node_client: &calimero_node_primitives::client::NodeClient,
    bytecode_id: [u8; 32],
) -> Option<String> {
    if bytecode_id == [0u8; 32] {
        return None;
    }
    node_client
        .blob_app_version(&calimero_primitives::blobs::BlobId::from(bytecode_id))
        .await
}

/// The founder and salt `namespace_id` was derived from, as recorded when this
/// node applied the namespace's genesis. Display-only like `appVersion`: a
/// failed read is logged and reported as absent rather than failing the request.
pub(crate) fn namespace_founding(
    store: &calimero_store::Store,
    namespace_id: &calimero_context_config::types::ContextGroupId,
) -> Option<calimero_server_primitives::admin::NamespaceFoundingApi> {
    match calimero_governance_store::NamespaceFoundingRepository::new(store).get(namespace_id) {
        Ok(found) => found.map(|(founder, salt)| founding_api(&founder, &salt)),
        Err(err) => {
            tracing::warn!(
                ?err,
                ?namespace_id,
                "could not read the namespace founding record"
            );
            None
        }
    }
}

pub(crate) fn founding_api(
    founder: &calimero_account::AccountId,
    salt: &[u8; 32],
) -> calimero_server_primitives::admin::NamespaceFoundingApi {
    calimero_server_primitives::admin::NamespaceFoundingApi {
        founder_account_id: hex::encode(founder.as_bytes()),
        salt: hex::encode(salt),
    }
}

/// The founder and genesis op this replica's copy of a namespace founded
/// before ids were derived was founded by — `legacyFounding`, kept apart from
/// `founding` so it can never pass for a derived-id proof. Backfilled from the
/// stored genesis op on first read by a node that applied it before the row
/// existed. Display-only like `founding`: a failed read is logged and reported
/// as absent.
pub(crate) fn namespace_legacy_founding(
    store: &calimero_store::Store,
    namespace_id: &calimero_context_config::types::ContextGroupId,
) -> Option<calimero_server_primitives::admin::NamespaceLegacyFoundingApi> {
    match calimero_governance_store::NamespaceLegacyFoundingRepository::new(store)
        .get_or_backfill(namespace_id)
    {
        Ok(found) => found.map(|(founder, genesis_op_hash)| {
            calimero_server_primitives::admin::NamespaceLegacyFoundingApi {
                founder_account_id: hex::encode(founder.as_bytes()),
                genesis_op_hash: hex::encode(genesis_op_hash),
            }
        }),
        Err(err) => {
            tracing::warn!(
                ?err,
                ?namespace_id,
                "could not read the namespace legacy founding record"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use calimero_context_client::local_governance::SignedNamespaceOp;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::test_fixtures::{namespace_genesis_for, test_store};
    use calimero_governance_store::NamespaceGovernance;
    use calimero_primitives::identity::PrivateKey;

    use super::*;

    #[test]
    fn a_legacy_namespace_shows_legacy_founding_and_no_founding() {
        let store = test_store();
        let founder_sk = PrivateKey::from([0x41u8; 32]);
        let namespace_id = [0x42u8; 32];
        let (genesis, founder) = namespace_genesis_for(&founder_sk);
        let signed =
            SignedNamespaceOp::sign(&founder_sk, namespace_id.into(), vec![], 0, genesis).unwrap();
        NamespaceGovernance::new(&store, namespace_id.into())
            .apply_signed_op(&signed)
            .unwrap();
        let ns = ContextGroupId::from(namespace_id);

        assert!(namespace_founding(&store, &ns).is_none());
        let legacy = namespace_legacy_founding(&store, &ns).expect("a plain genesis was applied");
        assert_eq!(legacy.founder_account_id, hex::encode(founder.as_bytes()));
        assert_eq!(
            legacy.genesis_op_hash,
            hex::encode(signed.content_hash().unwrap())
        );
        assert_eq!(legacy.genesis_op_hash.len(), 64);
        assert_eq!(
            legacy.genesis_op_hash,
            legacy.genesis_op_hash.to_lowercase()
        );
    }

    #[test]
    fn an_unknown_namespace_shows_neither() {
        let store = test_store();
        let ns = ContextGroupId::from([0x43u8; 32]);
        assert!(namespace_founding(&store, &ns).is_none());
        assert!(namespace_legacy_founding(&store, &ns).is_none());
    }
}
