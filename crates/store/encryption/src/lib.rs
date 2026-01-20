//! Application-level encryption for Calimero store.
//!
//! This crate provides transparent encryption for database values while keeping
//! keys in plaintext for searchability and iteration.
//!
//! # Architecture
//!
//! - **Keys are NOT encrypted** - required for iteration and range queries
//! - **Values ARE encrypted** - using AES-256-GCM with versioned keys
//! - **Key rotation** - supported via version bytes in ciphertext format
//!
//! # Usage
//!
//! ```ignore
//! use calimero_store_encryption::EncryptedDatabase;
//! use calimero_store::config::StoreConfig;
//!
//! let master_key = get_key_from_kms();
//! let inner_db = RocksDB::open(&config)?;
//! let encrypted_db = EncryptedDatabase::wrap(inner_db, master_key)?;
//! ```

mod iter;
pub mod key_manager;

use std::fmt::{self, Debug};
use std::sync::{Arc, RwLock};

use calimero_store::config::StoreConfig;
use calimero_store::db::{Column, Database};
use calimero_store::iter::Iter;
use calimero_store::slice::Slice;
use calimero_store::tx::{Operation, Transaction};
use eyre::Result;

use crate::iter::DecryptingIter;
pub use crate::key_manager::KeyManager;

/// A database wrapper that transparently encrypts values.
///
/// This wraps any `Database` implementation and encrypts all values on write
/// and decrypts on read. Keys remain unencrypted to support iteration and
/// range queries.
pub struct EncryptedDatabase<D> {
    inner: D,
    key_manager: Arc<RwLock<KeyManager>>,
}

impl<D: Debug> Debug for EncryptedDatabase<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let version = self
            .key_manager
            .read()
            .map(|km| km.current_version())
            .unwrap_or(0);
        f.debug_struct("EncryptedDatabase")
            .field("inner", &self.inner)
            .field("key_version", &version)
            .finish()
    }
}

impl<D> EncryptedDatabase<D> {
    /// Wrap an existing database with encryption.
    ///
    /// # Arguments
    ///
    /// * `inner` - The underlying database to wrap
    /// * `master_key` - The Key Encryption Key (KEK) from KMS
    ///
    /// # Errors
    ///
    /// Returns an error if the master key is invalid.
    pub fn wrap(inner: D, master_key: Vec<u8>) -> Result<Self> {
        let key_manager = KeyManager::new(master_key)?;
        Ok(Self {
            inner,
            key_manager: Arc::new(RwLock::new(key_manager)),
        })
    }

    /// Get a reference to the underlying database.
    #[must_use]
    pub const fn inner(&self) -> &D {
        &self.inner
    }

    /// Rotate to a new encryption key version.
    ///
    /// After rotation, all new writes use the new key version.
    /// Old data can still be read using cached keys.
    ///
    /// # Returns
    ///
    /// The new key version number.
    pub fn rotate_key(&self) -> Result<u8> {
        self.key_manager
            .write()
            .map_err(|e| eyre::eyre!("Lock poisoned: {e}"))?
            .rotate_key()
    }

    /// Get the current encryption key version.
    pub fn current_key_version(&self) -> Result<u8> {
        Ok(self
            .key_manager
            .read()
            .map_err(|e| eyre::eyre!("Lock poisoned: {e}"))?
            .current_version())
    }
}

impl<'a, D: Database<'a>> Database<'a> for EncryptedDatabase<D> {
    fn open(_config: &StoreConfig) -> Result<Self>
    where
        Self: Sized,
    {
        // EncryptedDatabase cannot be opened directly via the Database trait
        // because it requires a master key that must be obtained from KMS.
        // Use EncryptedDatabase::wrap(inner_db, master_key) instead.
        eyre::bail!(
            "EncryptedDatabase::open() is not supported. \
             Use EncryptedDatabase::wrap(inner_db, master_key) to create an encrypted database."
        )
    }

    fn has(&self, col: Column, key: Slice<'_>) -> Result<bool> {
        // Keys are not encrypted, so pass through
        self.inner.has(col, key)
    }

    fn get(&self, col: Column, key: Slice<'_>) -> Result<Option<Slice<'_>>> {
        match self.inner.get(col, key)? {
            Some(encrypted) => {
                let decrypted = self
                    .key_manager
                    .write()
                    .map_err(|e| eyre::eyre!("Lock poisoned: {e}"))?
                    .decrypt(encrypted.as_ref())?;
                Ok(Some(Slice::from(decrypted.into_boxed_slice())))
            }
            None => Ok(None),
        }
    }

    fn put(&self, col: Column, key: Slice<'a>, value: Slice<'a>) -> Result<()> {
        let encrypted = self
            .key_manager
            .write()
            .map_err(|e| eyre::eyre!("Lock poisoned: {e}"))?
            .encrypt(value.as_ref())?;
        self.inner
            .put(col, key, Slice::from(encrypted.into_boxed_slice()))
    }

    fn delete(&self, col: Column, key: Slice<'_>) -> Result<()> {
        // Delete doesn't involve values, pass through
        self.inner.delete(col, key)
    }

    fn iter(&self, col: Column) -> Result<Iter<'_>> {
        let inner_iter = self.inner.iter(col)?;
        // Extract the inner DBIter and wrap it with decryption
        Ok(Iter::new(DecryptingIter::new(
            Box::new(inner_iter),
            self.key_manager.clone(),
        )))
    }

    fn apply(&self, tx: &Transaction<'a>) -> Result<()> {
        // Build a new transaction with encrypted values
        let mut encrypted_tx = Transaction::default();

        // Acquire lock once for all encryptions
        let mut key_manager = self
            .key_manager
            .write()
            .map_err(|e| eyre::eyre!("Lock poisoned: {e}"))?;

        for (entry, op) in tx.iter() {
            let key = Slice::from(entry.key().to_vec().into_boxed_slice());

            match op {
                Operation::Put { value } => {
                    let encrypted = key_manager.encrypt(value.as_ref())?;
                    encrypted_tx.raw_put(
                        entry.column(),
                        key,
                        Slice::from(encrypted.into_boxed_slice()),
                    );
                }
                Operation::Delete => {
                    encrypted_tx.raw_delete(entry.column(), key);
                }
            }
        }

        // Release lock before calling inner apply
        drop(key_manager);

        // Delegate to inner DB - this is atomic (e.g., RocksDB uses WriteBatch)
        self.inner.apply(&encrypted_tx)
    }
}

// Safety: EncryptedDatabase is Send + Sync if the inner DB is
unsafe impl<D: Send> Send for EncryptedDatabase<D> {}
unsafe impl<D: Sync> Sync for EncryptedDatabase<D> {}

#[cfg(test)]
mod tests {
    use calimero_store::db::InMemoryDB;

    use super::*;

    fn test_master_key() -> Vec<u8> {
        vec![0x42; 48]
    }

    #[test]
    fn test_put_get_roundtrip() {
        let inner = InMemoryDB::owned();
        let db = EncryptedDatabase::wrap(inner, test_master_key()).unwrap();

        let key = Slice::from(b"test-key".to_vec());
        let value = Slice::from(b"secret value".to_vec());

        db.put(Column::Generic, key.clone(), value.clone()).unwrap();

        let retrieved = db.get(Column::Generic, key).unwrap().unwrap();
        assert_eq!(retrieved.as_ref(), b"secret value");
    }

    #[test]
    fn test_data_is_encrypted_in_inner_db() {
        let inner = InMemoryDB::owned();
        let db = EncryptedDatabase::wrap(inner, test_master_key()).unwrap();

        let key = Slice::from(b"test-key".to_vec());
        let value = Slice::from(b"secret value".to_vec());

        db.put(Column::Generic, key.clone(), value.clone()).unwrap();

        // Access inner DB directly - data should be encrypted
        let raw = db.inner().get(Column::Generic, key).unwrap().unwrap();
        assert_ne!(raw.as_ref(), b"secret value");
        assert!(raw.len() > value.len()); // Encrypted data is larger (version + nonce + tag)
    }

    #[test]
    fn test_has_works() {
        let inner = InMemoryDB::owned();
        let db = EncryptedDatabase::wrap(inner, test_master_key()).unwrap();

        let key = Slice::from(b"test-key".to_vec());
        let value = Slice::from(b"value".to_vec());

        assert!(!db.has(Column::Generic, key.clone()).unwrap());

        db.put(Column::Generic, key.clone(), value).unwrap();

        assert!(db.has(Column::Generic, key).unwrap());
    }

    #[test]
    fn test_delete_works() {
        let inner = InMemoryDB::owned();
        let db = EncryptedDatabase::wrap(inner, test_master_key()).unwrap();

        let key = Slice::from(b"test-key".to_vec());
        let value = Slice::from(b"value".to_vec());

        db.put(Column::Generic, key.clone(), value).unwrap();
        assert!(db.has(Column::Generic, key.clone()).unwrap());

        db.delete(Column::Generic, key.clone()).unwrap();
        assert!(!db.has(Column::Generic, key).unwrap());
    }

    #[test]
    fn test_key_rotation() {
        let inner = InMemoryDB::owned();
        let db = EncryptedDatabase::wrap(inner, test_master_key()).unwrap();

        // Write with v1
        let key1 = Slice::from(b"key1".to_vec());
        let value1 = Slice::from(b"value1".to_vec());
        db.put(Column::Generic, key1.clone(), value1.clone())
            .unwrap();
        assert_eq!(db.current_key_version().unwrap(), 1);

        // Rotate to v2
        db.rotate_key().unwrap();
        assert_eq!(db.current_key_version().unwrap(), 2);

        // Write with v2
        let key2 = Slice::from(b"key2".to_vec());
        let value2 = Slice::from(b"value2".to_vec());
        db.put(Column::Generic, key2.clone(), value2.clone())
            .unwrap();

        // Both can still be read
        assert_eq!(
            db.get(Column::Generic, key1).unwrap().unwrap().as_ref(),
            b"value1"
        );
        assert_eq!(
            db.get(Column::Generic, key2).unwrap().unwrap().as_ref(),
            b"value2"
        );
    }

    #[test]
    fn test_iter_with_encryption() {
        let inner = InMemoryDB::owned();
        let db = EncryptedDatabase::wrap(inner, test_master_key()).unwrap();

        // Insert several values
        let data = [
            (b"aaa".to_vec(), b"value_a".to_vec()),
            (b"bbb".to_vec(), b"value_b".to_vec()),
            (b"ccc".to_vec(), b"value_c".to_vec()),
        ];

        for (key, value) in &data {
            db.put(
                Column::Generic,
                Slice::from(key.clone()),
                Slice::from(value.clone()),
            )
            .unwrap();
        }

        // Iterate using entries() and verify decryption works
        let mut iter = db.iter(Column::Generic).unwrap();
        let mut found = Vec::new();

        for (key_result, value_result) in iter.entries() {
            let _key = key_result.unwrap();
            let value = value_result.unwrap();
            found.push(value.as_ref().to_vec());
        }

        // All values should be decrypted correctly
        assert_eq!(found.len(), 3);
        assert!(found.contains(&b"value_a".to_vec()));
        assert!(found.contains(&b"value_b".to_vec()));
        assert!(found.contains(&b"value_c".to_vec()));
    }
}
