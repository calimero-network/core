use calimero_store::entry::{Entry, Json};
use calimero_store::key::Generic;
use calimero_store::Store;
use rand::Rng;
use serde::{Deserialize, Serialize};
struct JwtTokenSecretEntry {
    key: Generic,
}

impl Entry for JwtTokenSecretEntry {
    type Key = Generic;
    type Codec = Json;
    type DataType<'a> = JwtTokenSecret;

    fn key(&self) -> &Self::Key {
        &self.key
    }
}

impl JwtTokenSecretEntry {
    fn new() -> Self {
        Self {
            key: Generic::new(*b"jwt_salt::server", [0; 32]),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct JwtTokenSecret {
    jwt_secret: Vec<u8>,
}

impl JwtTokenSecret {
    pub fn jwt_secret(&self) -> &Vec<u8> {
        &self.jwt_secret
    }
}

// Method to generate a new JWT key
fn generate_jwt_secret() -> Vec<u8> {
    let mut rng = rand::thread_rng();
    (0..32).map(|_| rng.gen()).collect()
}

// Method to insert the JWT key if it doesn't exist
pub fn get_or_create_jwt_secret(store: Store) -> eyre::Result<JwtTokenSecret> {
    // Check if the key already exists
    if let Some(existing_secret) = get_jwt_secret(store.clone())? {
        return Ok(existing_secret);
    }

    // Generate a new key if it doesn't exist
    let new_secret = generate_jwt_secret();
    let jwt_secret = JwtTokenSecret {
        jwt_secret: new_secret.to_vec(),
    };

    let entry = JwtTokenSecretEntry::new();
    let mut handle = store.handle();
    match handle.put(&entry, &jwt_secret) {
        Ok(_) => Ok(jwt_secret),
        Err(e) => Err(e.into()),
    }
}

// Method to get the JWT key from the store
pub fn get_jwt_secret(store: Store) -> eyre::Result<Option<JwtTokenSecret>> {
    let entry = JwtTokenSecretEntry::new();
    let handle = store.handle();

    match handle.get(&entry) {
        Ok(Some(jwt_secret)) => Ok(Some(jwt_secret)),
        Ok(None) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
