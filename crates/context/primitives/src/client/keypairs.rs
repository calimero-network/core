use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::{key, types, Store};
use eyre::{bail, Result};

#[derive(Debug)]
pub struct KeypairManager {
    store: Store,
}

impl KeypairManager {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub fn generate(&mut self, alias: Option<String>) -> Result<PublicKey> {
        let mut handle = self.store.handle();
        let private_key = PrivateKey::random(&mut rand::thread_rng());
        let public_key = private_key.public_key();

        let keypair =
            types::Keypair::new(*public_key, *private_key, alias.map(|a| a.into_boxed_str()));

        handle.put(&key::Keypair::new(public_key), &keypair)?;

        Ok(public_key)
    }

    pub fn get(&self, public_key: &PublicKey) -> Result<Option<types::Keypair>> {
        let handle = self.store.handle();
        Ok(handle.get(&key::Keypair::new(*public_key))?)
    }

    pub fn list(&self) -> Result<Vec<types::Keypair>> {
        let handle = self.store.handle();
        let mut keypairs = Vec::new();
        let mut iter = handle.iter::<key::Keypair>()?;

        for entry in iter.entries() {
            let (_, value) = entry;
            keypairs.push(value?);
        }

        Ok(keypairs)
    }

    pub fn remove(&mut self, public_key: &PublicKey) -> Result<Option<types::Keypair>> {
        let mut handle = self.store.handle();
        let key = key::Keypair::new(*public_key);
        let keypair = handle.get(&key)?;

        if keypair.is_some() {
            handle.delete(&key)?;
        }

        Ok(keypair)
    }

    pub fn export(&self, public_key: &PublicKey) -> Result<Option<String>> {
        let Some(keypair) = self.get(public_key)? else {
            return Ok(None);
        };

        let export_data = serde_json::json!({
            "public_key": hex::encode(keypair.public_key),
            "private_key": hex::encode(keypair.private_key),
            "alias": keypair.alias.as_deref(),
        });

        Ok(Some(serde_json::to_string_pretty(&export_data)?))
    }

    pub fn import(&mut self, json_data: &str) -> Result<PublicKey> {
        let data: serde_json::Value = serde_json::from_str(json_data)?;

        let public_key_hex = data["public_key"]
            .as_str()
            .ok_or_else(|| eyre::eyre!("Missing public_key in import data"))?;
        let private_key_hex = data["private_key"]
            .as_str()
            .ok_or_else(|| eyre::eyre!("Missing private_key in import data"))?;
        let alias = data["alias"].as_str().map(|s| s.to_string());

        let public_key_bytes = hex::decode(public_key_hex)?;
        let private_key_bytes = hex::decode(private_key_hex)?;

        if public_key_bytes.len() != 32 || private_key_bytes.len() != 32 {
            bail!("Invalid key length");
        }

        let mut public_key_array = [0u8; 32];
        public_key_array.copy_from_slice(&public_key_bytes);
        let mut private_key_array = [0u8; 32];
        private_key_array.copy_from_slice(&private_key_bytes);

        let public_key = PublicKey::from(public_key_array);
        let private_key = PrivateKey::from(private_key_array);

        let keypair = types::Keypair::new(
            public_key_array,
            private_key_array,
            alias.map(|a| a.into_boxed_str()),
        );

        let mut handle = self.store.handle();
        handle.put(&key::Keypair::new(public_key), &keypair)?;

        Ok(public_key)
    }
}
