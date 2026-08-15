//! Shared scaffolding for the JSON-RPC handler tests.
//!
//! Builds a `ServiceState` over an in-memory store so a handler can be driven
//! directly (`Request::handle`) without standing up the HTTP stack, and seeds
//! the context-membership rows the auth gate reads.

use std::sync::Arc;

use calimero_context_client::client::ContextClient;
use calimero_node_primitives::messages::NodeMessage;
use calimero_primitives::events::NodeEvent;
use calimero_store::db::InMemoryDB;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use tempfile::TempDir;
use tokio::sync::broadcast;

use super::ServiceState;

// Re-exported so the JSON-RPC handler tests reach the membership seeder through
// one import alongside `state_with`.
pub(crate) use crate::test_support::seed_context_member;

/// A `ServiceState` wired to an in-memory store, plus the store itself (so a
/// test can seed membership rows) and the `TempDir` backing the blob store
/// (dropping it would delete the directory out from under the state).
pub(crate) struct TestState {
    pub(crate) state: Arc<ServiceState>,
    pub(crate) store: Store,
    _blob_dir: TempDir,
}

/// Build a `ServiceState` over a fresh in-memory store.
///
/// `auth_enabled` mirrors the real flag: `false` is the intentional no-auth
/// deployment, `true` means the auth guard is expected to inject an identity
/// extension on every request.
///
/// `node_manager` is the recipient the `NodeClient` routes actor messages to.
/// Pass an uninitialized `LazyRecipient::new()` for tests that never reach the
/// node actor; pass one initialized by a stub actor when they do.
pub(crate) async fn state_with(
    auth_enabled: bool,
    node_manager: LazyRecipient<NodeMessage>,
) -> TestState {
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (event_sender, _) = broadcast::channel::<NodeEvent>(16);
    let (node_client, blob_dir) =
        crate::test_support::test_node_client(&store, node_manager, event_sender).await;

    let ctx_client = ContextClient::new(store.clone(), node_client.clone(), LazyRecipient::new());

    TestState {
        state: Arc::new(ServiceState {
            ctx_client,
            node_client,
            auth_enabled,
        }),
        store,
        _blob_dir: blob_dir,
    }
}
