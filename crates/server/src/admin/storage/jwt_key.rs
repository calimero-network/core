use calimero_store::entry::{Entry, Json};
use calimero_store::key::Generic;
use calimero_store::Store;
use rand::Rng;
use serde::{Deserialize, Serialize};
struct JwtTokenKeyEntry {
    key: Generic,
}

impl Entry for JwtTokenKeyEntry {
    type Key = Generic;
    type Codec = Json;
    type DataType<'a> = JwtTokenKey;

    fn key(&self) -> &Self::Key {
        &self.key
    }
}

impl JwtTokenKeyEntry {
    fn new() -> Self {
        Self {
            key: Generic::new(*b"jwt_salt::server", [0; 32]),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct JwtTokenKey {
    key: Vec<u8>,
}

impl JwtTokenKey {
    pub fn key(&self) -> &Vec<u8> {
        &self.key
    }
}

// Method to generate a new JWT key
fn generate_jwt_key() -> Vec<u8> {
    let mut rng = rand::thread_rng();
    (0..32).map(|_| rng.gen()).collect()
}

// Method to insert the JWT key if it doesn't exist
pub fn insert_jwt_key_if_not_exists(store: Store) -> eyre::Result<JwtTokenKey> {
    // Check if the key already exists
    if let Some(existing_key) = get_jwt_key(store.clone())? {
        return Ok(existing_key);
    }

    // Generate a new key if it doesn't exist
    let new_key = generate_jwt_key();
    let jwt_key = JwtTokenKey {
        key: new_key.to_vec(),
    };

    let entry = JwtTokenKeyEntry::new();
    let mut handle = store.handle();
    match handle.put(&entry, &jwt_key) {
        Ok(_) => Ok(jwt_key),
        Err(e) => Err(e.into()),
    }
}

// Method to get the JWT key from the store
pub fn get_jwt_key(store: Store) -> eyre::Result<Option<JwtTokenKey>> {
    let entry = JwtTokenKeyEntry::new();
    let handle = store.handle();

    match handle.get(&entry) {
        Ok(Some(jwt_key)) => Ok(Some(jwt_key)),
        Ok(None) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
