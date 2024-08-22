use calimero_primitives::context::ContextId;
use calimero_primitives::identity::ClientKey;
use calimero_store::Store;

use super::did::{get_or_create_did, update_did};

pub fn add_client_key(store: &Store, client_key: ClientKey) -> eyre::Result<bool> {
    let mut did_document = get_or_create_did(store)?;

    if !did_document
        .client_keys
        .iter()
        .any(|k| k.signing_key == client_key.signing_key)
    {
        did_document.client_keys.push(client_key);
        update_did(store, &did_document)?;
    }
    Ok(true)
}

pub fn get_client_key(store: &Store, signing_key: &str) -> eyre::Result<Option<ClientKey>> {
    let did = get_or_create_did(store)?;
    Ok(did
        .client_keys
        .into_iter()
        .find(|k| k.signing_key == signing_key))
}

pub fn get_context_client_key(
    store: &Store,
    context_id: &ContextId,
) -> eyre::Result<Vec<ClientKey>> {
    // todo! use independent records for client keys

    let did = get_or_create_did(store)?;
    Ok(did
        .client_keys
        .into_iter()
        .filter(|k| k.context_id.as_ref() == Some(context_id))
        .collect())
}

pub fn exists_client_key(store: &Store, client_key: &ClientKey) -> eyre::Result<bool> {
    let did = get_or_create_did(store)?;
    Ok(did
        .client_keys
        .into_iter()
        .any(|k| k.signing_key == client_key.signing_key))
}

pub fn remove_client_key(store: &Store, client_key: &ClientKey) -> eyre::Result<()> {
    let mut did_document = get_or_create_did(store)?;

    if let Some(pos) = did_document
        .client_keys
        .iter()
        .position(|x| x.signing_key == client_key.signing_key)
    {
        drop(did_document.client_keys.remove(pos));
        update_did(store, &did_document)?;
    }

    Ok(())
}
