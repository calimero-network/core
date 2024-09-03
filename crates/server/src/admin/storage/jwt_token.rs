use calimero_store::entry::{Entry, Json};
use calimero_store::key::Generic;
use calimero_store::Store;
use serde::{Deserialize, Serialize};

struct JwtRefreshTokenEntry {
    key: Generic,
}

impl Entry for JwtRefreshTokenEntry {
    type Key = Generic;
    type Codec = Json;
    type DataType<'a> = JwtRefreshToken;

    fn key(&self) -> &Self::Key {
        &self.key
    }
}

impl JwtRefreshTokenEntry {
    fn new(db_key: [u8; 32]) -> Self {
        Self {
            key: Generic::new(*b"jwt_token:server", db_key),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct JwtRefreshToken {
    refresh_token: Vec<u8>,
}

impl JwtRefreshToken {
    pub fn refresh_token(&self) -> &Vec<u8> {
        &self.refresh_token
    }
}

pub fn create_refresh_token(
    store: Store,
    refresh_token: Vec<u8>,
    db_key: &[u8; 32],
) -> eyre::Result<()> {
    let entry = JwtRefreshTokenEntry::new(*db_key);
    let jwt_refresh_token = JwtRefreshToken { refresh_token };

    let mut handle = store.handle();

    match handle.put(&entry, &jwt_refresh_token) {
        Ok(_) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

pub fn get_refresh_token(store: Store, db_key: &[u8; 32]) -> eyre::Result<Option<JwtRefreshToken>> {
    let entry = JwtRefreshTokenEntry::new(*db_key);
    let handle = store.handle();

    match handle.get(&entry) {
        Ok(Some(jwt_refresh_token)) => Ok(Some(jwt_refresh_token)),
        Ok(None) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn delete_refresh_token(store: Store, db_key: &[u8; 32]) -> eyre::Result<()> {
    let entry = JwtRefreshTokenEntry::new(*db_key);
    let mut handle = store.handle();

    match handle.delete(&entry) {
        Ok(_) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
