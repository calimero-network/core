use calimero_primitives::application::ApplicationId;
use calimero_store::Store;
use serde::{Deserialize, Serialize};

use super::did::{get_or_create_did, update_did};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct RootKey {
    pub(crate) signing_key: String,
}

pub fn add_root_key(
    application_id: ApplicationId,
    store: &Store,
    root_key: RootKey,
) -> eyre::Result<bool> {
    let mut did_document = get_or_create_did(application_id.clone(), store)?;

    if !did_document
        .root_keys
        .iter()
        .any(|k| k.signing_key == root_key.signing_key)
    {
        did_document.root_keys.push(root_key);
        update_did(application_id, store, did_document)?;
    }
    Ok(true)
}

pub fn get_root_key(
    application_id: ApplicationId,
    store: &Store,
    root_key: &RootKey,
) -> eyre::Result<Option<RootKey>> {
    let mut storage = calimero_store::ReadOnlyStore::new(application_id.clone(), &store);

    let did = get_or_create_did(application_id.clone(), store)?;
    Ok(did
        .root_keys
        .into_iter()
        .find(|k| k.signing_key == root_key.signing_key))
}
