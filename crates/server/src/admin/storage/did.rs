use calimero_primitives::application::ApplicationId;
use calimero_store::Store;
use serde::{Deserialize, Serialize};

use super::{client_keys::ClientKey, root_key::RootKey};

const DID_KEY: &str = "did:cali";
pub const NODE_STORE_KEY: &str = "node";

//TODO extract this to identity where suitable
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Did {
    pub(crate) id: String,
    pub(crate) root_keys: Vec<RootKey>,
    pub(crate) client_keys: Vec<ClientKey>,
}

pub fn create_did(store: &Store) -> eyre::Result<Did> {
    let mut storage =
        calimero_store::TemporalStore::new(ApplicationId(NODE_STORE_KEY.to_string()), &store);

    let did_document = Did {
        id: DID_KEY.to_string(),
        root_keys: Vec::<RootKey>::new(),
        client_keys: Vec::<ClientKey>::new(),
    };

    let did_document_vec = serde_json::to_vec(&did_document)
        .map_err(|e| eyre::Report::new(e).wrap_err("Serialization error"))?;

    storage.put(DID_KEY.as_bytes().to_owned(), did_document_vec);
    storage.commit()?;

    Ok(did_document)
}

pub fn get_or_create_did(store: &Store) -> eyre::Result<Did> {
    let mut storage =
        calimero_store::ReadOnlyStore::new(ApplicationId(NODE_STORE_KEY.to_string()), &store);

    let did_vec = storage.get(&DID_KEY.as_bytes().to_vec())?;
    match did_vec {
        Some(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| eyre::Report::new(e).wrap_err("Deserialization error")),
        None => create_did(store),
    }
}

pub fn update_did(store: &Store, did: Did) -> eyre::Result<()> {
    let did_document_vec = serde_json::to_vec(&did)
        .map_err(|e| eyre::Report::new(e).wrap_err("Serialization error"))?;

    let mut storage =
        calimero_store::TemporalStore::new(ApplicationId(NODE_STORE_KEY.to_string()), store);
    storage.put(DID_KEY.as_bytes().to_owned(), did_document_vec);
    storage.commit()?;
    Ok(())
}
