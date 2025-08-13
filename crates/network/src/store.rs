use eyre::bail;
use libp2p::{
    kad::{
        store::{RecordStore, Result},
        KBucketKey, ProviderRecord, Record, RecordKey, K_VALUE,
    },
    PeerId,
};
use rocksdb::ColumnFamily;
use rocksdb::DB;

use calimero_store::db::Column;

/// DB implementation of a `RecordStore`.
pub struct RocksDB {
    /// The identity of the peer owning the store.
    local_key: KBucketKey<PeerId>,
    // Rocks DB store.
    db: DB,
    /// The configuration of the store.
    config: RocksDBConfig,
}

/// Configuration for a `RocksDB` store.
#[derive(Debug, Clone)]
pub struct RocksDBConfig {
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

impl Default for RocksDBConfig {
    fn default() -> Self {
        Self {
            max_records: 1024,
            max_value_bytes: 65 * 1024,
            max_provided_keys: 1024,
            max_providers_per_key: K_VALUE.get(),
        }
    }
}

macro_rules! create_handle {
    ($column:expr,$self:expr) => {
        if let Some(cf_handle) = $self.get_handle($column) {
            Ok(cf_handle)
        } else {
            bail!("unknown column family: {:?}", $column);
        }
    };
}

impl RocksDB {
    pub fn new() -> RocksDB {
        // TODO: How do I initialize the DB
        todo!()
    }

    pub fn with_config(config: RocksDBConfig) -> RocksDB {
        todo!()
    }

    fn get_handle(&self, column: Column) -> Option<&ColumnFamily> {
        self.db.cf_handle(column.as_ref())
    }

    fn get_provider_handle(&self) -> eyre::Result<&ColumnFamily> {
        create_handle!(Column::KadProviders, self)
    }

    fn get_provided_handle(&self) -> eyre::Result<&ColumnFamily> {
        create_handle!(Column::KadProvided, self)
    }

    fn get_records_handle(&self) -> eyre::Result<&ColumnFamily> {
        create_handle!(Column::KadRecords, self)
    }
}

impl RecordStore for RocksDB {
    // type ProvidedIter<'a> =
    //     where
    //         Self: 'a;

    //         type RecordsIter<'a> =
    //             where
    //                 Self: 'a;
    fn get(&self, key: &RecordKey) -> Option<std::borrow::Cow<'_, Record>> {
        let cf_handle = self.get_records_handle().ok()?;

        let result = self.db.get_pinned_cf(cf_handle, key).ok()??;
        let record = serde_json::from_slice::<Record>(&result);
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

    fn put(&mut self, r: Record) -> Result<()> {
        todo!()
    }

    fn records(&self) -> Self::RecordsIter<'_> {
        todo!()
    }

    fn remove(&mut self, k: &RecordKey) {
        todo!()
    }

    fn remove_provider(&mut self, k: &RecordKey, p: &libp2p::PeerId) {
        todo!()
    }
}
