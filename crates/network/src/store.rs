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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone, Copy)]
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
    pub fn new(local_id: PeerId, db: Store) -> eyre::Result<KadStore> {
        Self::with_config(local_id, db, KadStoreConfig::default())
    }

    pub fn with_config(
        local_id: PeerId,
        db: Store,
        config: KadStoreConfig,
    ) -> eyre::Result<KadStore> {
        let valid_regular_record_count = Self::get_all_records(&db)?.len();

        // TODO: We still need to re-announce the records to the DHT.
        let providers = Self::get_all_providers(&db)?;
        let providers_len = providers.len();

        let provided_records = providers
            .into_iter()
            .flatten()
            .filter(|a| a.provider == local_id && a.is_expired(Instant::now()))
            .collect::<HashSet<_>>();

        Ok(KadStore {
            db,
            providers_len,
            config,
            provided: provided_records,
            local_key: KBucketKey::from(local_id),
            records_len: valid_regular_record_count,
        })
    }

    fn get_all_records(db: &Store) -> eyre::Result<Vec<Record>> {
        Ok(db
            .handle()
            .iter::<key::RecordMeta>()
            .map_err(|a| eyre::eyre!("{a:?}"))?
            .read()
            .into_iter()
            .flat_map(|a| {
                let Ok(record) = a.record() else { return None };
                Some(record)
            })
            .collect())
    }

    fn get_all_providers(db: &Store) -> eyre::Result<Vec<Vec<ProviderRecord>>> {
        let a = db
            .handle()
            .iter::<key::ProviderRecordMeta>()
            .map_err(|a| eyre::eyre!("{a:?}"))?
            .read()
            .into_iter()
            .map(|a| a.provider_record())
            .collect::<Vec<_>>();
        Ok(a)
    }

    fn get_provider(db: &Store, key: &RecordKey) -> Option<Vec<ProviderRecord>> {
        match db.handle().get(&key::ProviderRecordMeta::new(key)) {
            Ok(opt) => opt.map(|a| a.provider_record()),
            Err(err) => {
                error!(%err, ?key, "Failed to read provider records from DB");
                None
            }
        }
    }

    fn update_provider_record(db: &Store, record: Vec<ProviderRecord>) -> eyre::Result<()> {
        if record.is_empty() {
            warn!("an empty provider record was provided for DB update");
            return Ok(());
        }

        // TODO: We should impl From/Into eyre::Result for easy error conversion.
        db.handle()
            .put(
                &key::ProviderRecordMeta::new(&record[0].key),
                &types::ProviderRecordsMeta::new(record.into_iter()),
            )
            .map_err(|a| eyre::eyre!("{a:?}"))
    }
}

impl RecordStore for KadStore {
    type RecordsIter<'a> = IntoIter<Cow<'a, Record>>;
    type ProvidedIter<'a> = iter::Map<
        hash_set::Iter<'a, ProviderRecord>,
        fn(&'a ProviderRecord) -> Cow<'a, ProviderRecord>,
    >;

    fn get(&self, key: &RecordKey) -> Option<Cow<'_, Record>> {
        match self.db.handle().get(&key::RecordMeta::new(key)) {
            Ok(e) => e
                .map(|a| match a.record() {
                    Ok(e) => Some(e),
                    Err(err) => {
                        error!(%err, ?key, "failed to deconstruct kad record data retrieved from store");
                        None
                    }
                })
                .flatten()
                .map(|a| Cow::Owned(a)),

            Err(e) => {
                error!("Failed to read from DB when retrieve kad record with key {key:?}: {e}");
                None
            },
        }
    }

    fn put(&mut self, r: Record) -> Result<()> {
        if r.value.len() >= self.config.max_value_bytes {
            return Err(KadError::ValueTooLarge);
        }

        let key = r.key.clone();
        info!(?key, "Updating kad record");

        match self.db.handle().get(&key::RecordMeta::new(&key)) {
            Ok(None) => {
                if self.records_len >= self.config.max_records {
                    return Err(store::Error::MaxRecords);
                }

                info!(?key, "new kad record found");
                self.records_len += 1;
            }

            Err(err) => {
                error!(%err, ?key, "Failed to retrieve kad record from DB");
            }

            _ => {}
        }

        match self
            .db
            .handle()
            .put(&key::RecordMeta::new(&key), &types::RecordMeta::new(r))
        {
            Ok(_) => {
                info!(?key, "Kad record added to DB store");
            }
            Err(err) => {
                error!(%err, ?key, "Failed to update kad store");
            }
        }

        Ok(())
    }

    fn remove(&mut self, k: &RecordKey) {
        match self.db.handle().delete(&key::RecordMeta::new(&k)) {
            Ok(_) => {
                self.records_len -= 1;
            }
            Err(err) => error!(%err, "Failed to delete key for kad store"),
        }
    }

    fn records(&self) -> Self::RecordsIter<'_> {
        let a = match Self::get_all_records(&self.db) {
            Ok(e) => e.into_iter().map(Cow::Owned).collect::<Vec<_>>(),
            Err(err) => {
                error!("error getting all kad records: {err}");
                vec![]
            }
        };

        a.into_iter()
    }

    fn add_provider(&mut self, record: ProviderRecord) -> Result<()> {
        info!("Received kad provider record with key {:?}", record.key);
        let mut providers = Self::get_provider(&self.db, &record.key).unwrap_or_default();

        if providers.is_empty() && self.config.max_provided_keys == self.providers_len {
            return Err(KadError::MaxProvidedKeys);
        }

        let key = record.key.clone();
        let update_record_len = providers.is_empty();

        for p in providers.iter_mut() {
            if p.provider == record.provider {
                // In-place update of an existing provider record.
                if self.local_key.preimage() == &record.provider {
                    let _ = self.provided.remove(p);
                    let _ = self.provided.insert(record.clone());
                }

                *p = record;
                match Self::update_provider_record(&self.db, providers) {
                    Ok(_) => info!("Updated provider record for key {key:?}"),
                    Err(err) => {
                        error!("Failed to update provider record to store for key {key:?}: {err}")
                    }
                }
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
            let _ = self.provided.insert(record.clone());
        }

        providers.push(record);
        match Self::update_provider_record(&self.db, providers) {
            Ok(_) => info!("Saved key provider record with key {:?} to store", key),
            Err(err) => {
                error!("Failed to update kad provider record with key: {key:?}: {err}");
                return Ok(());
            }
        }

        if update_record_len {
            self.providers_len += 1;
        }

        Ok(())
    }

    fn remove_provider(&mut self, k: &RecordKey, peer_id: &PeerId) {
        info!("Removed kad provider with key {k:?}");

        let Some(mut providers) = Self::get_provider(&self.db, k) else {
            return;
        };

        if let Some(i) = providers.iter().position(|p| &p.provider == peer_id) {
            let p = providers.remove(i);
            if &p.provider == self.local_key.preimage() {
                let _ = self.provided.remove(&p);
            }
        }

        if providers.is_empty() {
            if let Err(err) = self.db.handle().delete(&key::ProviderRecordMeta::new(&k)) {
                error!(
                    "Failed to delete kad provider record with key {:?} from store: {}",
                    k, err
                );
                return;
            }

            self.providers_len -= 1;
            return;
        }

        if let Err(err) = Self::update_provider_record(&self.db, providers) {
            error!(
                "Failed to update kad record provider on update of key {:?}: {}",
                k, err
            )
        }
    }

    fn providers(&self, key: &RecordKey) -> Vec<ProviderRecord> {
        Self::get_provider(&self.db, key).unwrap_or_default()
    }

    fn provided(&self) -> Self::ProvidedIter<'_> {
        self.provided.iter().map(Cow::Borrowed)
    }
}

#[cfg(test)]
mod tests {
    // TODO: Do we have an in-memory db store for tests?
    #[test]
    fn put_get_remove_record() {}

    #[test]
    fn add_get_remove_provider() {}

    #[test]
    fn provided() {}

    #[test]
    fn update_provider() {}

    #[test]
    fn update_provided() {}

    #[test]
    fn max_providers_per_key() {}

    #[test]
    fn max_provided_keys() {}
}
