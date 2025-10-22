use calimero_primitives::common::DIGEST_SIZE;
use calimero_server_primitives::sse::ConnectionId;
use calimero_store::key::Generic as GenericKey;
use calimero_store::layer::{ReadLayer, WriteLayer};
use calimero_store::slice::Slice;
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::config::SSE_SESSION_SCOPE;
use super::session::PersistedSessionData;

/// Generates a generic storage key for a given session ID.
///
/// This function creates a unique key by taking the `session_id`, converting it
/// to its big-endian byte representation, and using it as the first 8 bytes
/// of the key's fragment (which is [`DIGEST_SIZE`] bytes). The remaining bytes
/// of the fragment are zero-padded. The entire key is scoped under
/// `SSE_SESSION_SCOPE` to prevent collisions.
///
/// # Arguments
///
/// * `session_id` - The `ConnectionId` for which to generate a storage key.
///
/// # Returns
///
/// A `GenericKey` that can be used to uniquely identify session data in a storage backend.
#[must_use]
pub fn session_key(session_id: ConnectionId) -> GenericKey {
    let mut fragment = [0u8; DIGEST_SIZE];
    fragment[..8].copy_from_slice(&session_id.to_be_bytes());
    GenericKey::new(SSE_SESSION_SCOPE, fragment)
}

/// Load session data from persistent storage
///
/// # Errors
/// Returns error if storage operation fails or deserialization fails
pub fn load_session(
    store: &Store,
    session_id: ConnectionId,
) -> EyreResult<Option<PersistedSessionData>> {
    let key = session_key(session_id);
    let Some(data) = store.get(&key)? else {
        return Ok(None);
    };
    let session_data: PersistedSessionData = serde_json::from_slice(&data)?;
    Ok(Some(session_data))
}

/// Save session data to persistent storage
///
/// # Errors
/// Returns error if serialization or storage operation fails
pub fn save_session(
    store: &mut Store,
    session_id: ConnectionId,
    data: &PersistedSessionData,
) -> EyreResult<()> {
    let key = session_key(session_id);
    let json = serde_json::to_vec(data)?;
    store.put(&key, Slice::from(json))?;
    Ok(())
}

/// Delete session from persistent storage
///
/// # Errors
/// Returns error if storage operation fails
pub fn delete_session(store: &mut Store, session_id: ConnectionId) -> EyreResult<()> {
    let key = session_key(session_id);
    store.delete(&key)?;
    Ok(())
}
