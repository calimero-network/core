use calimero_primitives::application::ApplicationId;
use calimero_store::config::StoreConfig;
use serde::{Deserialize, Serialize};

use crate::storage::storage::{AdminStore, Storage};

pub const ROOT_KEY: &str = "did:cali";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Did {
    id: String,
    root_keys: Vec<RootKey>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]

pub struct RootKey {
    pub(crate) signing_key: String,
}

//Share this between calls
fn get_node_storage() -> AdminStore {
    let store_config: StoreConfig = StoreConfig {
        path: "data/node2".into(),
    };
    let store = calimero_store::Store::open(&store_config).unwrap();
    let application_id = ApplicationId("node".to_string());
    let mut storage = AdminStore::Write(calimero_store::TemporalStore::new(application_id, &store));
    storage
}

pub fn create_did(application_id: ApplicationId) -> Option<Did> {
    let mut storage = get_node_storage();

    let did_document = Did {
        id: ROOT_KEY.to_string(),
        root_keys: Vec::<RootKey>::new(),
    };
    let did_document: Vec<u8> = serde_json::to_vec(&did_document).unwrap();

    if let Some(did_document) = storage.set(ROOT_KEY.to_string().into_bytes(), did_document) {
        return serde_json::from_slice(&did_document).unwrap(); // todo: handle error
    }
    None
}

pub fn get_or_create_did(application_id: ApplicationId) -> Option<Did> {
    let mut storage = get_node_storage();

    if let Some(did_document) = storage.get(&ROOT_KEY.to_string().into_bytes()) {
        return serde_json::from_slice(&did_document).unwrap(); // todo: handle error
    }
    create_did(application_id)
}

//Root keys

pub fn add_root_key(application_id: ApplicationId, root_key: RootKey) -> bool {
    let mut storage = get_node_storage();

    let mut did_document = get_or_create_did(application_id).unwrap();
    did_document.root_keys.push(root_key);

    let did_document: Vec<u8> = serde_json::to_vec(&did_document).unwrap();

    if let Some(result) = storage.set(ROOT_KEY.to_string().into_bytes(), did_document) {
        return true;
    }
    false
}

pub fn get_root_key(application_id: ApplicationId, root_key: &RootKey) -> Option<RootKey> {
    let storage = get_node_storage();

    if let Some(did) = storage.get(&ROOT_KEY.to_string().into_bytes()) {
        let did: Did = serde_json::from_slice(&did).unwrap();

        did.root_keys
            .iter()
            .find(|k| k.signing_key == root_key.signing_key);
    }
    None
}
