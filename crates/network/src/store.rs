use std::{
    borrow::Cow,
    collections::{hash_set, HashSet},
    iter,
    time::Instant,
    vec::IntoIter,
};

use libp2p::{
    kad::{
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
    /// Number of providers.
    providers_len: usize,
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
        let local_id = KBucketKey::from(local_id);

        let valid_regular_record_count = Self::get_all_records(&db).len();

        // TODO: We still need to re-announce the records to the DHT.
        let providers = Self::get_all_providers(&db);
        let providers_len = providers.len();

        let provided_records = providers
            .into_iter()
            .filter(|a| &a[0].provider == local_id.preimage())
            .flatten()
            .flat_map(|a| {
                if a.is_expired(Instant::now()) {
                    return None;
                } else {
                    return Some(a);
                }
            })
            .collect::<HashSet<_>>();

        KadStore {
            db,
            providers_len,
            config,
            provided: provided_records,
            local_key: KBucketKey::from(local_id),
            records_len: valid_regular_record_count,
        }
    }

    fn get_all_records(db: &Store) -> Vec<Record> {
        db.handle()
            .iter::<key::RecordMeta>()
            .unwrap()
            .read()
            .into_iter()
            .flat_map(|a| {
                let Ok(record) = a.record() else { return None };

                Some(record)
            })
            .collect()
    }

    fn get_all_providers(db: &Store) -> Vec<Vec<ProviderRecord>> {
        let a = db
            .handle()
            .iter::<key::ProviderRecordMeta>()
            .unwrap()
            .read()
            .into_iter()
            .map(|a| a.provider_record())
            .collect::<Vec<_>>();
        a
    }

    fn get_provider(db: &Store, key: &RecordKey) -> Option<Vec<ProviderRecord>> {
        db.handle()
            .get(&key::ProviderRecordMeta::new(key))
            .unwrap()
            .map(|a| a.provider_record())
    }

    fn delete_provider_record(db: &Store, key: &RecordKey) {
        db.handle().delete(&key::ProviderRecordMeta::new(&key));
    }

    fn update_provider_record(db: &Store, record: Vec<ProviderRecord>) {
        if record.is_empty() {
            warn!("an empty provider record was provided for DB update");
            return;
        }

        db.handle().put(
            &key::ProviderRecordMeta::new(&record[0].key),
            &types::ProviderRecordsMeta::new(record.into_iter()),
        );
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

    fn records(&self) -> Self::RecordsIter<'_> {
        let a = Self::get_all_records(&self.db)
            .into_iter()
            .map(Cow::Owned)
            .collect::<Vec<_>>();

        a.into_iter()
    }

    fn add_provider(&mut self, record: ProviderRecord) -> Result<()> {
        let mut providers = Self::get_provider(&self.db, &record.key).unwrap_or_default();

        if providers.is_empty() && self.config.max_provided_keys == self.providers_len {
            return Err(KadError::MaxProvidedKeys);
        }

        for p in providers.iter_mut() {
            if p.provider == record.provider {
                // In-place update of an existing provider record.
                if self.local_key.preimage() == &record.provider {
                    self.provided.remove(p);
                    self.provided.insert(record.clone());
                }

                *p = record;
                Self::update_provider_record(&self.db, providers);
                return Ok(());
            }
        }

        // If the providers list is full, we ignore the new provider.
        // This strategy can mitigate Sybil attacks, in which an attacker
        // floods the network with fake provider records.
        if providers.len() == self.config.max_providers_per_key {
            return Ok(());
        }

        // Otherwise, insert the new provider record.
        if self.local_key.preimage() == &record.provider {
            self.provided.insert(record.clone());
        }

        providers.push(record);
        Self::update_provider_record(&self.db, providers);

        Ok(())
    }

    fn remove_provider(&mut self, k: &RecordKey, peer_id: &PeerId) {
        let Some(mut providers) = Self::get_provider(&self.db, k) else {
            return;
        };

        if let Some(i) = providers.iter().position(|p| &p.provider == peer_id) {
            let p = providers.remove(i);
            if &p.provider == self.local_key.preimage() {
                self.provided.remove(&p);
            }
        }
        if providers.is_empty() {
            Self::delete_provider_record(&self.db, k);
            return;
        }

        Self::update_provider_record(&self.db, providers);
    }

    fn providers(&self, key: &RecordKey) -> Vec<ProviderRecord> {
        Self::get_provider(&self.db, key).unwrap_or_default()
    }

    fn provided(&self) -> Self::ProvidedIter<'_> {
        self.provided.iter().map(Cow::Borrowed)
    }
}
