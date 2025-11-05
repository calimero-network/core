//! Test utilities for creating populated RocksDB fixtures.
//!
//! This module provides helper functions for setting up temporary RocksDB instances
//! with sample Calimero data for testing migration plans and dry-run reports.

use std::path::{Path, PathBuf};

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
        context_id: &[u8; 32],
        state_key: &[u8; 32],
        value: &[u8],
    ) -> Result<()> {
        let db =
            DB::open_cf_descriptors(&Self::default_opts(), &self.path, Self::cf_descriptors())?;
        let cf_state = db
            .cf_handle(Column::State.as_str())
            .ok_or_else(|| eyre::eyre!("State column family not found"))?;

        let mut full_key = [0_u8; 64];
        full_key[..32].copy_from_slice(context_id);
        full_key[32..64].copy_from_slice(state_key);

        let mut batch = WriteBatch::default();
        batch.put_cf(cf_state, full_key, value);
        db.write(batch)?;

        Ok(())
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

    /// Insert a Meta column entry (context_id + metadata_key structure).
    pub fn insert_meta_entry(
        &self,
        context_id: &[u8; 32],
        meta_key: &[u8],
        value: &[u8],
    ) -> Result<()> {
        let db =
            DB::open_cf_descriptors(&Self::default_opts(), &self.path, Self::cf_descriptors())?;
        let cf_meta = db
            .cf_handle(Column::Meta.as_str())
            .ok_or_else(|| eyre::eyre!("Meta column family not found"))?;

        let mut full_key = Vec::with_capacity(32 + meta_key.len());
        full_key.extend_from_slice(context_id);
        full_key.extend_from_slice(meta_key);

        let mut batch = WriteBatch::default();
        batch.put_cf(cf_meta, full_key, value);
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
pub fn test_context_id(byte: u8) -> [u8; 32] {
    [byte; 32]
}

/// Helper to create test state keys from simple byte patterns.
pub fn test_state_key(byte: u8) -> [u8; 32] {
    [byte; 32]
}

/// Helper to create a short (malformed) key for testing edge cases.
pub fn short_key(len: usize) -> Vec<u8> {
    vec![0xFF; len]
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

        let mut full_key = [0_u8; 64];
        full_key[..32].copy_from_slice(&ctx_id);
        full_key[32..64].copy_from_slice(&state_key);

        let value = db.get_cf(cf_state, full_key)?;
        ensure!(
            value.as_deref() == Some(&b"test-value"[..]),
            "Expected value 'test-value', got {:?}",
            value
        );

        Ok(())
    }
}
