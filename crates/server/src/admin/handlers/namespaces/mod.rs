pub mod create_group_in_namespace;
pub mod create_namespace;
pub mod delete_namespace;
pub mod get_identity;
pub mod get_namespace;
pub mod invite_namespace;
pub mod join_namespace;
pub mod leave_namespace;
pub mod list;
pub mod list_for_application;
pub mod list_namespace_groups;

/// Per-namespace `appVersion`: the bundle-manifest version of the
/// namespace's `app_key` blob. `None` when unresolvable (zero/legacy key,
/// raw-wasm app, blob not retained locally) — display-only, never an error.
pub(crate) async fn namespace_app_version(
    node_client: &calimero_node_primitives::client::NodeClient,
    app_key: [u8; 32],
) -> Option<String> {
    if app_key == [0u8; 32] {
        return None;
    }
    node_client
        .blob_app_version(&calimero_primitives::blobs::BlobId::from(app_key))
        .await
}
