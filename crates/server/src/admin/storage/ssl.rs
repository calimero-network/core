use calimero_store::entry::{Entry, Json};
use calimero_store::key::Generic as GenericKey;
use calimero_store::Store;
use eyre::Result as EyreResult;
use serde::{Deserialize, Serialize};

struct SSLEntry {
    key: GenericKey,
}

impl Entry for SSLEntry {
    type Key = GenericKey;
    type Codec = Json;
    type DataType<'a> = SSLCert;

    fn key(&self) -> &Self::Key {
        &self.key
    }
}

impl SSLEntry {
    fn new() -> Self {
        Self {
            key: GenericKey::new(*b"ssl_certs:server", [0; 32]),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SSLCert {
    cert: Vec<u8>,
    key: Vec<u8>,
}

impl SSLCert {
    #[must_use]
    pub const fn cert(&self) -> &Vec<u8> {
        &self.cert
    }

    #[must_use]
    pub const fn key(&self) -> &Vec<u8> {
        &self.key
    }
}

pub fn insert_or_update_ssl(store: &Store, cert: &[u8], key: &[u8]) -> EyreResult<SSLCert> {
    let ssl_cert = SSLCert {
        cert: cert.to_vec(),
        key: key.to_vec(),
    };

    let entry = SSLEntry::new();
    let mut handle = store.handle();
    handle.put(&entry, &ssl_cert)?;

    Ok(ssl_cert)
}

pub fn get_ssl(store: &Store) -> EyreResult<Option<SSLCert>> {
    let entry = SSLEntry::new();
    let handle = store.handle();

    match handle.get(&entry) {
        Ok(Some(ssl_document)) => Ok(Some(ssl_document)),
        Ok(None) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
