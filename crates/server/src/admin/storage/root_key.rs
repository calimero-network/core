use calimero_primitives::identity::RootKey;
use calimero_store::Store;

use super::did::{get_or_create_did, update_did};

pub fn add_root_key(store: &Store, root_key: RootKey) -> eyre::Result<bool> {
    let mut did_document = get_or_create_did(store)?;

    if !did_document
        .root_keys
        .iter()
        .any(|k| k.signing_key == root_key.signing_key)
    {
        did_document.root_keys.push(root_key);
        update_did(store, did_document)?;
    }
    Ok(true)
}

pub fn get_root_key(store: &Store, signing_key: String) -> eyre::Result<Option<RootKey>> {
    let did = get_or_create_did(store)?;
    Ok(did
        .root_keys
        .into_iter()
        .find(|k| k.signing_key == signing_key))
}
