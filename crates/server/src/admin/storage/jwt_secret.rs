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

#[derive(Serialize, Deserialize, Clone, Debug, Copy)]
pub struct JwtTokenSecret {
    jwt_secret: [u8; 32],
}

impl JwtTokenSecret {
    #[must_use]
    pub const fn jwt_secret(&self) -> &[u8; 32] {
        &self.jwt_secret
    }
}

// Method to generate a new JWT key
fn generate_jwt_secret() -> [u8; 32] {
    rand::thread_rng().gen()
}

// Method to insert the JWT key if it doesn't exist
pub fn get_or_create_jwt_secret(store: &Store) -> eyre::Result<JwtTokenSecret> {
    // Check if the key already exists
    if let Some(existing_secret) = get_jwt_secret(store)? {
        return Ok(existing_secret);
    }

    // Generate a new key if it doesn't exist
    let new_secret = generate_jwt_secret();
    let jwt_secret = JwtTokenSecret {
        jwt_secret: new_secret,
    };

    let entry = JwtTokenSecretEntry::new();
    let mut handle = store.handle();
    match handle.put(&entry, &jwt_secret) {
        Ok(()) => Ok(jwt_secret),
        Err(e) => Err(e.into()),
    }
}

// Method to get the JWT key from the store
pub fn get_jwt_secret(store: &Store) -> eyre::Result<Option<JwtTokenSecret>> {
    let entry = JwtTokenSecretEntry::new();
    let handle = store.handle();

    handle.get(&entry).map_err(Into::into)
}
