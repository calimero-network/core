use calimero_store::Store;
use serde::{Deserialize, Serialize};

use super::did::{get_or_create_did, update_did};
use crate::admin::handlers::add_client_key::WalletType;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct ClientKey {
    pub(crate) wallet_type: WalletType,
    pub(crate) signing_key: String,
}

pub fn add_client_key(store: &Store, client_key: ClientKey) -> eyre::Result<bool> {
    let mut did_document = get_or_create_did(store)?;

    if !did_document
        .client_keys
        .iter()
        .any(|k| k.signing_key == client_key.signing_key)
    {
        did_document.client_keys.push(client_key);
        update_did(store, did_document)?;
    }
    Ok(true)
}

pub fn get_client_key(store: &Store, client_key: &ClientKey) -> eyre::Result<Option<ClientKey>> {
    let did = get_or_create_did(store)?;
    Ok(did
        .client_keys
        .into_iter()
        .find(|k| k.signing_key == client_key.signing_key))
}

pub fn exists_client_key(store: &Store, client_key: &ClientKey) -> eyre::Result<bool> {
    let did = get_or_create_did(store)?;
    Ok(did
        .client_keys
        .into_iter()
        .find(|k| k.signing_key == client_key.signing_key)
        .is_some())
}

pub fn remove_client_key(store: &Store, client_key: &ClientKey) -> eyre::Result<()> {
    let mut did_document = get_or_create_did(store)?;

    if let Some(pos) = did_document
        .client_keys
        .iter()
        .position(|x| x.signing_key == client_key.signing_key)
    {
        did_document.client_keys.remove(pos);
    }

    Ok(())
}
