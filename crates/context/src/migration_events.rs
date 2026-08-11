//! Emits [`NodeEvent::GroupMigration`] from the migration write and read paths.

use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::NamespaceRepository;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::events::{GroupMigrationEvent, GroupMigrationPayload, NodeEvent};
use calimero_primitives::hash::Hash;
use tracing::debug;

/// Announce a migration phase change for `group_id`'s namespace.
///
/// Routing is always the namespace root: a cascade descendant is not an id any
/// client subscribed with, so an event keyed on one would reach nobody.
pub fn emit(
    node_client: &NodeClient,
    datastore: &calimero_store::Store,
    group_id: &ContextGroupId,
    payload: GroupMigrationPayload,
) {
    let root = NamespaceRepository::new(datastore)
        .resolve(group_id)
        .unwrap_or(*group_id);
    let event = NodeEvent::GroupMigration(GroupMigrationEvent {
        group_id: Hash::from(root.to_bytes()),
        payload,
    });
    if let Err(err) = node_client.send_event(event) {
        debug!(?err, "migration event send_event failed (no receivers?)");
    }
}
