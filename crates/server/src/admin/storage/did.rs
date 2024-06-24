use calimero_primitives::identity::Did;
use calimero_store::key::Generic;
use calimero_store::layer::{read_only, temporal, ReadLayer, WriteLayer};
use calimero_store::Store;

const DID_KEY: &str = "did:cali";
const DID_SCOPE: &[u8; 16] = b"id:calimero:node";

pub const NODE_STORE_KEY: &str = "node";

fn did_key() -> Generic {
    Generic::new(*DID_SCOPE, [0; 32])
}

pub fn create_did(store: &mut Store) -> eyre::Result<Did> {
    let mut storage = temporal::Temporal::new(store);

    let did_document = Did {
        id: DID_KEY.to_string(),
        root_keys: Vec::new(),
        client_keys: Vec::new(),
        contexts: Vec::new(),
    };

    let did_document_vec = serde_json::to_vec(&did_document)
        .map_err(|e| eyre::Report::new(e).wrap_err("Serialization error"))?;

    let key = did_key();

    storage.put(&key, did_document_vec.into());

    storage.commit()?;

    Ok(did_document)
}

pub fn get_or_create_did(store: &mut Store) -> eyre::Result<Did> {
    let storage = read_only::ReadOnly::new(store);

    let key = did_key();

    let Some(bytes) = storage.get(&key)? else {
        return create_did(store);
    };

    serde_json::from_slice(&bytes)
        .map_err(|e| eyre::Report::new(e).wrap_err("Deserialization error"))
}

pub fn update_did(store: &mut Store, did: Did) -> eyre::Result<()> {
    let did_document_vec = serde_json::to_vec(&did)
        .map_err(|e| eyre::Report::new(e).wrap_err("Serialization error"))?;

    let mut storage = temporal::Temporal::new(store);

    let key = did_key();

    storage.put(&key, did_document_vec.into());

    storage.commit()?;

    Ok(())
}
