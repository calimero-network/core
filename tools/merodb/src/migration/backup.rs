//!
//! Backup utilities for the migration engine.
//!
//! This module provides functionality to create backups of RocksDB databases before
//! performing mutating operations. Backups are created using RocksDB's native backup
//! engine, which supports incremental backups and efficient restoration.
//!
//! ## Features
//!
//! - **Automatic backup creation**: Creates a timestamped backup before mutations
//! - **Incremental backups**: Leverages RocksDB's backup engine for efficient storage
//! - **Configurable location**: Backups can be stored in a user-specified directory
//!
//! ## Usage
//!
//! Backups are automatically created when:
//! 1. A target database is configured in the migration plan
//! 2. The `backup_dir` field is set in the target endpoint
//! 3. The migration is executed in apply mode (not dry-run)
//!

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use eyre::{Result, WrapErr};
use rocksdb::{
    backup::{BackupEngine, BackupEngineOptions},
    DBWithThreadMode, Env, SingleThreaded,
};

/// Create a backup of the target database before performing mutations.
///
/// This function creates a timestamped backup of the target database in the specified
/// backup directory. The backup is created using RocksDB's backup engine, which supports
/// incremental backups for efficiency.
///
/// # Arguments
///
/// * `db` - The RocksDB database to back up
/// * `db_path` - Path to the database being backed up (for error messages)
/// * `backup_dir` - Directory where the backup should be stored
///
/// # Returns
///
/// The path to the created backup directory, or an error if the backup fails.
///
/// # Errors
///
/// Returns an error if:
/// - The backup directory cannot be created
/// - The backup engine cannot be opened
/// - The backup operation fails
pub fn create_backup(
    db: &DBWithThreadMode<SingleThreaded>,
    db_path: &Path,
    backup_dir: &Path,
) -> Result<PathBuf> {
    // Create backup directory if it doesn't exist
    if !backup_dir.exists() {
        fs::create_dir_all(backup_dir).wrap_err_with(|| {
            format!(
                "Failed to create backup directory: {}",
                backup_dir.display()
            )
        })?;
    }

    // Generate timestamped backup subdirectory
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let backup_path = backup_dir.join(format!("backup-{timestamp}"));

    eprintln!("Creating backup of target database...");
    eprintln!("  Source: {}", db_path.display());
    eprintln!("  Backup: {}", backup_path.display());

    // Create backup using RocksDB's backup engine
    let env = Env::new().wrap_err("Failed to create RocksDB environment")?;
    let backup_opts = BackupEngineOptions::new(&backup_path).wrap_err_with(|| {
        format!(
            "Failed to create backup options for {}",
            backup_path.display()
        )
    })?;

    let mut backup_engine = BackupEngine::open(&backup_opts, &env)
        .wrap_err_with(|| format!("Failed to open backup engine at {}", backup_path.display()))?;

    backup_engine.create_new_backup(db).wrap_err_with(|| {
        format!(
            "Failed to create backup of database at {}",
            db_path.display()
        )
    })?;

    eprintln!("  Backup created successfully");

    Ok(backup_path)
}

#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::thread;

    use super::*;
    use crate::migration::test_utils::{test_context_id, test_state_key, DbFixture};
    use eyre::ensure;
    use tempfile::TempDir;

    #[test]
    fn create_backup_succeeds_for_non_empty_database() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");
        let backup_dir = temp.path().join("backups");

        // Create a database with some data
        let fixture = DbFixture::new(&db_path)?;
        let ctx = test_context_id(0x11);
        fixture.insert_state_entry(&ctx, &test_state_key(0xAA), b"test-value")?;

        // Open the database for backup
        let db = crate::open_database(&db_path)?;

        // Create backup
        let backup_path = create_backup(&db, &db_path, &backup_dir)?;

        // Verify backup was created
        ensure!(backup_path.exists(), "Backup directory should exist");
        ensure!(
            backup_path.starts_with(&backup_dir),
            "Backup should be in backup_dir"
        );

        // Verify backup directory contains expected structure
        let backup_files: Vec<_> = fs::read_dir(&backup_path)?.filter_map(Result::ok).collect();
        ensure!(
            !backup_files.is_empty(),
            "Backup directory should contain files"
        );

        Ok(())
    }

    #[test]
    fn create_backup_creates_backup_dir_if_missing() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");
        let backup_dir = temp.path().join("nonexistent").join("backups");

        // Create a minimal database
        let _fixture = DbFixture::new(&db_path)?;
        let db = crate::open_database(&db_path)?;

        // Backup dir doesn't exist yet
        ensure!(
            !backup_dir.exists(),
            "Backup directory should not exist yet"
        );

        // Create backup should create the directory
        let backup_path = create_backup(&db, &db_path, &backup_dir)?;

        ensure!(backup_dir.exists(), "Backup directory should be created");
        ensure!(backup_path.exists(), "Backup should be created");

        Ok(())
    }

    #[test]
    fn create_backup_generates_unique_timestamps() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");
        let backup_dir = temp.path().join("backups");

        let _fixture = DbFixture::new(&db_path)?;
        let db = crate::open_database(&db_path)?;

        // Create two backups
        let backup1 = create_backup(&db, &db_path, &backup_dir)?;

        // Small delay to ensure different timestamp
        thread::sleep(Duration::from_millis(1100));

        let backup2 = create_backup(&db, &db_path, &backup_dir)?;

        // Backups should have different paths due to timestamps
        ensure!(backup1 != backup2, "Backups should have unique timestamps");
        ensure!(backup1.exists(), "First backup should exist");
        ensure!(backup2.exists(), "Second backup should exist");

        Ok(())
    }
}
