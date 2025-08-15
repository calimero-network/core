use std::borrow::Cow;

use eyre::bail;
use libp2p::{
    kad::{
        store::{Error as KadError, RecordStore, Result},
        KBucketKey, ProviderRecord, Record, RecordKey, K_VALUE,
    },
    PeerId,
};
use rocksdb::ColumnFamily;
use rocksdb::DB;

use calimero_store::{
    iter::DBIter,
    key::{self, AsKeyParts},
    Store,
};

/// DB implementation of a `RecordStore`.
pub struct KadStore {
    /// The identity of the peer owning the store.
    local_key: KBucketKey<PeerId>,
    // Rocks DB store.
    db: Store,
    /// The configuration of the store.
    config: KadStoreConfig,
}

/// Configuration for a `RocksDB` store.
#[derive(Debug, Clone)]
pub struct KadStoreConfig {
    /// The maximum number of records.
    pub max_records: usize,
    /// The maximum size of record values, in bytes.
    pub max_value_bytes: usize,
    /// The maximum number of providers stored for a key.
    ///
    /// This should match up with the chosen replication factor.
    pub max_providers_per_key: usize,
    /// The maximum number of provider records for which the
    /// local node is the provider.
    pub max_provided_keys: usize,
}

impl Default for KadStoreConfig {
    fn default() -> Self {
        Self {
            max_records: 1024,
            max_value_bytes: 65 * 1024,
            max_provided_keys: 1024,
            max_providers_per_key: K_VALUE.get(),
        }
    }
}

impl KadStore {
    pub fn new() -> KadStore {
        // TODO: Query store for all records, and then store the len
        // and increment, this will ensure we don't recurrently query store
        // when gate-keeping.
        todo!()
    }

    pub fn with_config(config: KadStoreConfig) -> KadStore {
        todo!()
    }
}

impl RecordStore for KadStore {
    // type ProvidedIter<'a> =
    //     where
    //         Self: 'a;

    //         type RecordsIter<'a> =
    //             where
    //                 Self: 'a;
    fn get(&self, key: &RecordKey) -> Option<Cow<'_, Record>> {
        let record = self.db.handle().get(&key::RecordMeta::new(key));

        match record {
            Ok(e) => e
                .map(|a| match a.record() {
                    Ok(e) => Some(e),
                    Err(e) => None,
                })
                .flatten()
                .map(|a| Cow::Owned(a)),

            Err(e) => None,
        }
    }

    fn put(&mut self, r: Record) -> Result<()> {
        if r.value.len() >= self.config.max_value_bytes {
            return Err(KadError::ValueTooLarge);
        }

        // TODO: How can I query column???
        for a in self.db.handle().iter().unwrap() {}

        todo!()
    }

    fn remove(&mut self, k: &RecordKey) {
        todo!()
    }

    fn add_provider(&mut self, record: ProviderRecord) -> Result<()> {
        todo!()
    }

    fn provided(&self) -> Self::ProvidedIter<'_> {
        todo!()
    }

    fn providers(&self, key: &RecordKey) -> Vec<ProviderRecord> {
        todo!()
    }

    fn records(&self) -> Self::RecordsIter<'_> {
        todo!()
    }

    fn remove_provider(&mut self, k: &RecordKey, p: &libp2p::PeerId) {
        todo!()
    }
}
