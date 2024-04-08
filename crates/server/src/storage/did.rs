use calimero_primitives::application::ApplicationId;
use calimero_store::{config::StoreConfig, Store};
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

pub fn create_did(store: &Store) -> Did {
    let application_id = ApplicationId(
        "/calimero/experimental/app/9SFTEoc6RBHtCn9b6cm4PPmhYzrogaMCd5CRiYAQichP".to_string(),
    );
    let mut storage = AdminStore::Write(calimero_store::TemporalStore::new(
        application_id.clone(),
        &store,
    ));

    let did_document = Did {
        id: ROOT_KEY.to_string(),
        root_keys: Vec::<RootKey>::new(),
    };
    let did_document_vec: Vec<u8> = serde_json::to_vec(&did_document).unwrap();

    println!("Creating DID: {:?}", did_document_vec);

    storage.set(ROOT_KEY.to_string().into_bytes(), did_document_vec);
    calimero_store::TemporalStore::new(application_id, &store).commit();

    // if let Some(did_document) = storage.set(ROOT_KEY.to_string().into_bytes(), did_document_vec) {
    //     println!("DID created: {:?}", did_document);
    //     return serde_json::from_slice(&did_document).unwrap(); // todo: handle error
    // }
    did_document
}

pub fn get_or_create_did(store: &Store) -> Did {
    let application_id = ApplicationId(
        "/calimero/experimental/app/9SFTEoc6RBHtCn9b6cm4PPmhYzrogaMCd5CRiYAQichP".to_string(),
    );
    let mut storage = AdminStore::Read(calimero_store::ReadOnlyStore::new(application_id, &store));

    if let Some(did_document) = storage.get(&ROOT_KEY.to_string().into_bytes()) {
        return serde_json::from_slice(&did_document).unwrap(); // todo: handle error
    }
    create_did(store)
}

//Root keys

pub fn add_root_key(store: &Store, root_key: RootKey) -> bool {
    let application_id = ApplicationId(
        "/calimero/experimental/app/9SFTEoc6RBHtCn9b6cm4PPmhYzrogaMCd5CRiYAQichP".to_string(),
    );
    let mut storage = AdminStore::Write(calimero_store::TemporalStore::new(
        application_id.clone(),
        &store,
    ));

    let mut did_document = get_or_create_did(store);
    did_document.root_keys.push(root_key);

    println!("Created: {:?}", did_document);

    let did_document: Vec<u8> = serde_json::to_vec(&did_document).unwrap();

    storage.set(ROOT_KEY.to_string().into_bytes(), did_document);
    calimero_store::TemporalStore::new(application_id.clone(), &store).commit();

    // println!("Stored");

    // let mut storage = AdminStore::Read(calimero_store::ReadOnlyStore::new(application_id, &store));

    // if let Some(result) = storage.get(&ROOT_KEY.to_string().into_bytes()) {
    //     return true;
    // }
    // println!("Not fetched");

    // false

    true
}

pub fn get_root_key(store: &Store, root_key: &RootKey) -> Option<RootKey> {
    let application_id = ApplicationId(
        "/calimero/experimental/app/9SFTEoc6RBHtCn9b6cm4PPmhYzrogaMCd5CRiYAQichP".to_string(),
    );
    let mut storage = AdminStore::Read(calimero_store::ReadOnlyStore::new(application_id, &store));

    if let Some(did) = storage.get(&ROOT_KEY.to_string().into_bytes()) {
        let did: Did = serde_json::from_slice(&did).unwrap();

        did.root_keys
            .iter()
            .find(|k| k.signing_key == root_key.signing_key);
    }
    None
}
