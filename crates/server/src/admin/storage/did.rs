use calimero_primitives::identity::Did;
use calimero_store::entry::{Entry, Json};
use calimero_store::key::Generic as GenericKey;
use calimero_store::Store;
use eyre::Result as EyreResult;

struct DidEntry {
    key: GenericKey,
}

impl Entry for DidEntry {
    type Key = GenericKey;
    type Codec = Json;
    type DataType<'a> = Did;

    fn key(&self) -> &Self::Key {
        &self.key
    }
}

impl DidEntry {
    fn new() -> Self {
        Self {
            key: GenericKey::new(*b"id:calimero:node", [0; 32]),
        }
    }
}

pub fn create_did(store: &Store) -> EyreResult<Did> {
    let did_document = Did::new("did:cali".to_owned(), vec![], vec![]);

    let entry = DidEntry::new();

    let mut handle = store.handle();

    handle.put(&entry, &did_document)?;

    Ok(did_document)
}

pub fn get_or_create_did(store: &Store) -> EyreResult<Did> {
    let entry = DidEntry::new();

    let handle = store.handle();

    let Some(did_document) = handle.get(&entry)? else {
        return create_did(store);
    };

    Ok(did_document)
}

pub fn update_did(store: &Store, did: &Did) -> EyreResult<()> {
    let entry = DidEntry::new();

    let mut handle = store.handle();

    handle.put(&entry, did)?;

    Ok(())
}

pub fn delete_did(store: &Store) -> EyreResult<()> {
    let entry = DidEntry::new();
    let mut handle = store.handle();
    handle.delete(&entry)?;

    Ok(())
}
