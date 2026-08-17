//! Emits [`NodeEvent::GroupMigration`] from the migration write and read paths.

use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::NamespaceRepository;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::events::{GroupMigrationEvent, GroupMigrationPayload, NodeEvent};
use calimero_primitives::hash::Hash;
use tracing::{debug, warn};

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
    // Falling back to `group_id` is right when it IS the root and wrong when it
    // is a cascade descendant, and this cannot tell which - so the fault has to
    // be visible. Dropping the event instead would lose the root-keyed ones too.
    let root = NamespaceRepository::new(datastore)
        .resolve(group_id)
        .unwrap_or_else(|err| {
            warn!(
                ?err,
                ?group_id,
                "migration event: namespace resolve failed; keying on the subscribed id, \
                 which reaches nobody if it is a cascade descendant"
            );
            *group_id
        });
    let event = NodeEvent::GroupMigration(GroupMigrationEvent {
        group_id: Hash::from(root.to_bytes()),
        payload,
    });
    if let Err(err) = node_client.send_event(event) {
        debug!(?err, "migration event send_event failed (no receivers?)");
    }
}
