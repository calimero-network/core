use std::{
    borrow::Cow,
    collections::{hash_map, hash_set, HashSet},
    iter,
    vec::IntoIter,
};

use eyre::WrapErr;
use libp2p::{
    kad::{
        self,
        store::{self, Error as KadError, RecordStore, Result},
        KBucketKey, ProviderRecord, Record, RecordKey, K_VALUE,
    },
    PeerId,
};
use tracing::*;

use calimero_store::{key, types, Store};

/// DB implementation of a `RecordStore`.
pub struct KadStore {
    /// Number of stored records.
    records_len: usize,
    /// The identity of the peer owning the store.
    local_key: KBucketKey<PeerId>,
    /// The configuration of the store.
    config: KadStoreConfig,
    /// The set of all provider records for the node identified by `local_key`.
    ///
    /// Must be kept in sync with `providers`.
    /// We store local provider record both in DB and memory.
    provided: HashSet<ProviderRecord>,
    // Rocks DB store.
    db: Store,
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
    pub fn new(local_id: PeerId, db: Store) -> KadStore {
        Self::with_config(local_id, db, KadStoreConfig::default())
    }

    pub fn with_config(local_id: PeerId, db: Store, config: KadStoreConfig) -> KadStore {
        let valid_kad_record_count = db
            .handle()
            .iter::<key::RecordMeta>()
            .unwrap()
            .read()
            .into_iter()
            .flat_map(|a| {
                // TODO: Discard expired records.

                Some(())
            })
            .count();

        let provided_record = db
            .handle()
            .iter::<key::ProviderRecordMeta>()
            .unwrap()
            .read()
            .into_iter()
            .flat_map(|a| {
                // TODO: Discard expired records and delete from DB.

                // TODO: Skip if record doesn't belong to local.
                a.provider_record().ok()
            })
            .collect::<HashSet<ProviderRecord>>();

        KadStore {
            db,
            config,
            provided: provided_record,
            local_key: KBucketKey::from(local_id),
            records_len: valid_kad_record_count,
        }
    }
}

impl RecordStore for KadStore {
    type RecordsIter<'a> = IntoIter<Cow<'a, Record>>;
    type ProvidedIter<'a> = iter::Map<
        hash_set::Iter<'a, ProviderRecord>,
        fn(&'a ProviderRecord) -> Cow<'a, ProviderRecord>,
    >;

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

        match self.db.handle().get(&key::RecordMeta::new(&r.key)) {
            Ok(Some(_)) => {
                info!("Kad record updated for key {:?}", r.key);
            }
            Ok(None) => {
                if self.records_len >= self.config.max_records {
                    return Err(store::Error::MaxRecords);
                }

                info!("new kad record found with key {:?}", r.key);
                self.records_len += 1;
            }

            Err(err) => {
                error!(%err, "Failed to retrieve kad record from DB");
                // TODO: Should we panic on DB error?
            }
        }

        match self
            .db
            .handle()
            .put(&key::RecordMeta::new(&r.key), &types::RecordMeta::new(r))
        {
            Ok(_) => Ok(()),
            Err(err) => {
                error!(%err, "Failed to update kad store");
                Err(store::Error::ValueTooLarge)
            }
        }
    }

    fn remove(&mut self, k: &RecordKey) {
        if let Err(err) = self.db.handle().delete(&key::RecordMeta::new(&k)) {
            error!(%err, "Failed to delete key for kad store");
        }
    }

    fn add_provider(&mut self, record: ProviderRecord) -> Result<()> {
        todo!()
    }

    fn records(&self) -> Self::RecordsIter<'_> {
        self.db
            .handle()
            .iter::<key::RecordMeta>()
            .unwrap()
            .read()
            .into_iter()
            .flat_map(|a| a.record())
            .map(|r| Cow::Owned(r))
            .collect::<Vec<_>>()
            .into_iter()
    }

    fn remove_provider(&mut self, k: &RecordKey, p: &libp2p::PeerId) {
        todo!()
    }

    fn providers(&self, key: &RecordKey) -> Vec<ProviderRecord> {
        todo!()
    }

    fn provided(&self) -> Self::ProvidedIter<'_> {
        self.provided.iter().map(Cow::Borrowed)
    }
}
