use calimero_primitives::identity::RootKey;
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::did::{get_or_create_did, update_did};

pub fn add_root_key(store: &Store, root_key: RootKey) -> EyreResult<bool> {
    let mut did_document = get_or_create_did(store)?;

    if !did_document
        .root_keys
        .iter()
        .any(|k| k.signing_key == root_key.signing_key)
    {
        did_document.root_keys.push(root_key);
        update_did(store, &did_document)?;
    }

    Ok(true)
}

pub fn get_root_key(store: &Store, signing_key: &str) -> EyreResult<Option<RootKey>> {
    let did = get_or_create_did(store)?;
    Ok(did
        .root_keys
        .into_iter()
        .find(|k| k.signing_key == signing_key))
}

pub fn get_root_keys(store: &Store) -> EyreResult<Vec<RootKey>> {
    let did = get_or_create_did(store)?;
    Ok(did.root_keys)
}

pub fn exists_root_keys(store: &Store) -> EyreResult<bool> {
    let did = get_or_create_did(store)?;
    Ok(!did.root_keys.is_empty())
}

pub fn clean_auth_keys(store: &Store) -> EyreResult<()> {
    let mut did = get_or_create_did(store)?;

    did.client_keys.clear();
    did.root_keys.clear();

    update_did(store, &did)?;

    Ok(())
}
