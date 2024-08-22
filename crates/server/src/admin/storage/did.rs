use calimero_primitives::identity::Did;
use calimero_store::entry::{Entry, Json};
use calimero_store::key::Generic;
use calimero_store::Store;

struct DidEntry {
    key: Generic,
}

impl Entry for DidEntry {
    type Key = Generic;
    type Codec = Json;
    type DataType<'a> = Did;

    fn key(&self) -> &Self::Key {
        &self.key
    }
}

impl DidEntry {
    fn new() -> Self {
        Self {
            key: Generic::new(*b"id:calimero:node", [0; 32]),
        }
    }
}

pub fn create_did(store: &Store) -> eyre::Result<Did> {
    let did_document = Did {
        id: "did:cali".to_string(),
        root_keys: vec![],
        client_keys: vec![],
    };

    let entry = DidEntry::new();

    let mut handle = store.handle();

    handle.put(&entry, &did_document)?;

    Ok(did_document)
}

pub fn get_or_create_did(store: &Store) -> eyre::Result<Did> {
    let entry = DidEntry::new();

    let handle = store.handle();

    let Some(did_document) = handle.get(&entry)? else {
        return create_did(store);
    };

    Ok(did_document)
}

pub fn update_did(store: &Store, did: &Did) -> eyre::Result<()> {
    let entry = DidEntry::new();

    let mut handle = store.handle();

    handle.put(&entry, did)?;

    Ok(())
}
