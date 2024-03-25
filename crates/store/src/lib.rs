use std::collections::btree_map::{self, BTreeMap};
use std::sync::Arc;

pub mod config;
pub mod db;

#[derive(Clone)]
pub struct Store {
    db: Arc<dyn db::Database>,
}

impl Store {
    pub fn open(config: &config::StoreConfig) -> eyre::Result<Self> {
        let db = db::rocksdb::RocksDB::open(&config)?;

        Ok(Self { db: Arc::new(db) })
    }

    pub fn get(&self, key: &db::Key) -> eyre::Result<Option<db::Value>> {
        self.db.get(key)
    }

    pub fn apply(&self, tx: db::Transaction) -> eyre::Result<()> {
        self.db.apply(tx)
    }
}

pub struct TemporalStore {
    application_id: String,
    inner: Store,
    shadow: BTreeMap<db::Key, db::Value>,
}

impl TemporalStore {
    pub fn new(application_id: String, store: &Store) -> Self {
        Self {
            application_id: application_id.clone(),
            inner: store.clone(),
            shadow: BTreeMap::new(),
        }
    }

    pub fn get(&self, key: &db::Key) -> eyre::Result<Option<db::Value>> {
        let application_key = get_application_key(self.application_id.clone(), key);

        if let Some(value) = self.shadow.get(&application_key) {
            return Ok(Some(value.clone()));
        }

        self.inner.get(&application_key)
    }

    pub fn put(&mut self, key: db::Key, value: db::Value) -> Option<db::Value> {
        let application_key = get_application_key(self.application_id.clone(), &key);

        match self.shadow.entry(application_key) {
            btree_map::Entry::Occupied(mut entry) => Some(entry.insert(value)),
            btree_map::Entry::Vacant(entry) => {
                let evicted = self.inner.get(entry.key()).unwrap_or(None);
                entry.insert(value);
                evicted
            }
        }
    }

    // pub fn delete(&mut self, key: &db::Key) -> Option<db::Value> {
    //     // todo! translate to Delete operation in db
    // }

    pub fn commit(self) -> eyre::Result<()> {
        let mut tx = db::Transaction::default();

        for (key, value) in self.shadow {
            tx.put(key, value);
        }

        self.inner.apply(tx)
    }

    pub fn has_changes(&self) -> bool {
        !self.shadow.is_empty()
    }
}

pub struct ReadOnlyStore {
    application_id: String,
    inner: Store,
}

impl ReadOnlyStore {
    pub fn new(application_id: String, store: &Store) -> Self {
        Self {
            application_id: application_id.clone(),
            inner: store.clone(),
        }
    }

    pub fn get(&self, key: &db::Key) -> eyre::Result<Option<db::Value>> {
        self.inner
            .get(&get_application_key(self.application_id.clone(), key))
    }
}

pub fn get_application_key(application_id: String, key: &db::Key) -> Vec<u8> {
    let mut application_key = Vec::from((application_id.clone() + ":").as_bytes());
    application_key.append(&mut key.clone());
    application_key
}
