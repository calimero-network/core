//! Test utilities for creating populated RocksDB fixtures.
//!
//! This module provides helper functions for setting up temporary RocksDB instances
//! with sample Calimero data for testing migration plans and dry-run reports.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "Test utility calculations for buffer sizes are safe"
)]

use std::path::{Path, PathBuf};

use calimero_primitives::context::ContextId;
use calimero_storage::address::Id;
use calimero_storage::row::{encode as encode_row, Row};
use calimero_storage::store::Key as StorageKey;
use calimero_store::key::{
    AsKeyParts, ContextMeta as ContextMetaKey, ContextState as ContextStateKey, STATE_KEY_LEN,
};
use calimero_store::types::ContextMeta;
use eyre::{ensure, Result};
use rocksdb::{ColumnFamilyDescriptor, Options, WriteBatch, DB};

use crate::types::Column;

/// Configuration for database fixture setup.
pub struct DbFixture {
    /// Path where the database will be created.
    pub path: PathBuf,
}

impl DbFixture {
    /// Create a new RocksDB fixture with all Calimero column families.
    pub fn new(path: &Path) -> Result<Self> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);

        let descriptors: Vec<_> = Column::all()
            .iter()
            .map(|column| ColumnFamilyDescriptor::new(column.as_str(), Options::default()))
            .collect();

        let db = DB::open_cf_descriptors(&opts, path, descriptors)?;
        drop(db);

        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    /// Insert a single state entry for a given context ID.
    pub fn insert_state_entry(
        &self,
        context_id: &ContextId,
        state_key: &[u8; STATE_KEY_LEN],
        value: &[u8],
    ) -> Result<()> {
        let db =
            DB::open_cf_descriptors(&Self::default_opts(), &self.path, Self::cf_descriptors())?;
        let cf_state = db
            .cf_handle(Column::State.as_str())
            .ok_or_else(|| eyre::eyre!("State column family not found"))?;

        // Use actual Calimero types to construct the key
        let key = ContextStateKey::new(*context_id, *state_key);
        let key_bytes = key.as_key().as_bytes();

        let mut batch = WriteBatch::default();
        batch.put_cf(cf_state, key_bytes, value);
        db.write(batch)?;

        Ok(())
    }

    /// Insert the entity row of `id` the way the node writes it: encoded for
    /// that id (the row does not store it) under its `Key::Index`.
    pub fn insert_entity_row(&self, context_id: &ContextId, id: Id, row: &Row) -> Result<()> {
        self.insert_state_entry(
            context_id,
            &StorageKey::Index(id).to_bytes(),
            &encode_row(id, row),
        )
    }

    /// Insert a generic column entry (simple key-value pair).
    pub fn insert_generic_entry(&self, key: &[u8], value: &[u8]) -> Result<()> {
        let db =
            DB::open_cf_descriptors(&Self::default_opts(), &self.path, Self::cf_descriptors())?;
        let cf_generic = db
            .cf_handle(Column::Generic.as_str())
            .ok_or_else(|| eyre::eyre!("Generic column family not found"))?;

        let mut batch = WriteBatch::default();
        batch.put_cf(cf_generic, key, value);
        db.write(batch)?;

        Ok(())
    }

    /// Insert a Meta column entry with serialized ContextMeta value.
    pub fn insert_meta_entry(
        &self,
        context_id: &ContextId,
        meta_value: &ContextMeta,
    ) -> Result<()> {
        let db =
            DB::open_cf_descriptors(&Self::default_opts(), &self.path, Self::cf_descriptors())?;
        let cf_meta = db
            .cf_handle(Column::Meta.as_str())
            .ok_or_else(|| eyre::eyre!("Meta column family not found"))?;

        // Use actual Calimero types to construct the key
        let key = ContextMetaKey::new(*context_id);
        let key_bytes = key.as_key().as_bytes();

        // Serialize the value using Borsh
        let value_bytes = borsh::to_vec(meta_value)?;

        let mut batch = WriteBatch::default();
        batch.put_cf(cf_meta, key_bytes, value_bytes);
        db.write(batch)?;

        Ok(())
    }

    fn default_opts() -> Options {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        opts
    }

    fn cf_descriptors() -> Vec<ColumnFamilyDescriptor> {
        Column::all()
            .iter()
            .map(|column| ColumnFamilyDescriptor::new(column.as_str(), Options::default()))
            .collect()
    }
}

/// Helper to create test context IDs from simple byte patterns.
/// Returns an actual ContextId instead of raw bytes.
pub fn test_context_id(byte: u8) -> ContextId {
    ContextId::from([byte; 32])
}

/// Helper to create test state keys from simple byte patterns: the key of
/// the entity row whose id is `[byte; 32]`.
pub fn test_state_key(byte: u8) -> [u8; STATE_KEY_LEN] {
    StorageKey::Index(Id::new([byte; 32])).to_bytes()
}

/// Helper to create a short (malformed) key for testing edge cases.
pub fn short_key(len: usize) -> Vec<u8> {
    vec![0xFF; len]
}

/// Helper to create test ContextMeta with placeholder values.
pub fn test_context_meta(app_id_byte: u8) -> ContextMeta {
    use calimero_primitives::application::ApplicationId;
    use calimero_store::key::ApplicationMeta as ApplicationMetaKey;

    let app_id = ApplicationId::from([app_id_byte; 32]);
    let app_meta_key = ApplicationMetaKey::new(app_id);

    ContextMeta::new(
        app_meta_key,
        [0_u8; 32], // root_hash
        Vec::new(), // dag_heads
        None,       // service_name
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn fixture_creates_db_with_all_cfs() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        let _fixture = DbFixture::new(&db_path)?;

        // Verify we can open the database and all column families exist
        let db = DB::open_cf_descriptors(
            &DbFixture::default_opts(),
            &db_path,
            DbFixture::cf_descriptors(),
        )?;

        for column in Column::all() {
            ensure!(
                db.cf_handle(column.as_str()).is_some(),
                "Column family {} should exist",
                column.as_str()
            );
        }

        Ok(())
    }

    #[test]
    fn fixture_inserts_state_entry() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        let fixture = DbFixture::new(&db_path)?;
        let ctx_id = test_context_id(0x11);
        let state_key = test_state_key(0x22);

        fixture.insert_state_entry(&ctx_id, &state_key, b"test-value")?;

        // Verify the entry was written
        let db = DB::open_cf_descriptors(
            &DbFixture::default_opts(),
            &db_path,
            DbFixture::cf_descriptors(),
        )?;
        let cf_state = db.cf_handle(Column::State.as_str()).unwrap();

        // Use actual Calimero types to construct the key
        let key = ContextStateKey::new(ctx_id, state_key);
        let key_bytes = key.as_key().as_bytes();

        let value = db.get_cf(cf_state, key_bytes)?;
        ensure!(
            value.as_deref() == Some(&b"test-value"[..]),
            "Expected value 'test-value', got {:?}",
            value
        );

        Ok(())
    }

    #[test]
    fn fixture_inserts_entity_row() -> Result<()> {
        use calimero_storage::index::EntityIndex as StoredIndex;

        use crate::export::EntityIndex;

        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        let fixture = DbFixture::new(&db_path)?;
        let ctx_id = test_context_id(0x11);
        let id = Id::new([0x22; 32]);
        let entry = [&b"item"[..], id.as_bytes()].concat();
        fixture.insert_entity_row(
            &ctx_id,
            id,
            &Row {
                index: Some(borsh::to_vec(&StoredIndex::minimal_for_test(id))?),
                data: Some(entry.clone()),
            },
        )?;

        let db = DB::open_cf_descriptors(
            &DbFixture::default_opts(),
            &db_path,
            DbFixture::cf_descriptors(),
        )?;
        let cf_state = db.cf_handle(Column::State.as_str()).unwrap();
        let key = ContextStateKey::new(ctx_id, test_state_key(0x22));
        let key_bytes = key.as_key().as_bytes();
        let value = db
            .get_cf(cf_state, key_bytes)?
            .ok_or_else(|| eyre::eyre!("entity row not written"))?;

        let index = EntityIndex::decode_at(key_bytes, &value)
            .ok_or_else(|| eyre::eyre!("entity row does not decode"))?;
        ensure!(index.id == id, "decoded id {:?}, expected {id:?}", index.id);
        ensure!(
            index.data.as_deref() == Some(&entry[..]),
            "entry data differs"
        );

        Ok(())
    }
}
