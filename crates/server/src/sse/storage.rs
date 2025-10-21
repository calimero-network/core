use calimero_server_primitives::sse::ConnectionId;
use calimero_store::key::Generic as GenericKey;
use calimero_store::layer::{ReadLayer, WriteLayer};
use calimero_store::slice::Slice;
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::config::SSE_SESSION_SCOPE;
use super::session::PersistedSessionData;

/// Generate storage key for a session
#[must_use]
pub fn session_key(session_id: ConnectionId) -> GenericKey {
    let mut fragment = [0u8; 32];
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
