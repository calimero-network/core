use std::path::{Path, PathBuf};

use calimero_wasm_abi::schema::Manifest;
use eyre::{ensure, Result, WrapErr};
use once_cell::sync::OnceCell;
use rocksdb::{DBWithThreadMode, SingleThreaded};

use crate::{abi, open_database};

use super::plan::MigrationPlan;

/// Command-line overrides that can replace plan-provided paths.
#[derive(Debug, Default)]
pub struct MigrationOverrides {
    pub source_db: Option<PathBuf>,
    pub state_schema_file: Option<PathBuf>,
    pub target_db: Option<PathBuf>,
    pub backup_dir: Option<PathBuf>,
    pub no_backup: bool,
}

/// Aggregates plan metadata with opened database handles for a run.
pub struct MigrationContext {
    /// Parsed migration plan that drives the run.
    plan: MigrationPlan,
    /// Active source database plus its optional state schema.
    source: SourceContext,
    /// Optional target database if the plan or CLI provided one.
    target: Option<TargetContext>,
    /// True when execution must avoid writes and stay in preview mode.
    dry_run: bool,
}

impl MigrationContext {
    /// Build a migration context using plan defaults plus optional overrides.
    ///
    /// This method constructs the migration context by:
    /// 1. Resolving source and target database paths from the plan and CLI overrides
    /// 2. Opening the source database in read-only mode
    /// 3. Opening the target database in either read-only (dry-run) or writable (apply) mode
    /// 4. Setting up optional state schema loading for the source database
    ///
    /// # Arguments
    ///
    /// * `plan` - Parsed migration plan containing step definitions and endpoints
    /// * `overrides` - CLI-provided paths that take precedence over plan values
    /// * `dry_run` - If true, opens target database read-only; if false, opens with write access
    ///
    /// # Returns
    ///
    /// A fully initialized `MigrationContext` ready for dry-run or mutating execution.
    pub fn new(plan: MigrationPlan, overrides: MigrationOverrides, dry_run: bool) -> Result<Self> {
        let MigrationOverrides {
            source_db,
            state_schema_file,
            target_db,
            backup_dir,
            no_backup,
        } = overrides;

        let source_db_path = source_db.unwrap_or_else(|| plan.source.db_path.clone());
        let source_state_schema_path =
            state_schema_file.or_else(|| plan.source.state_schema_file.clone());

        let source = SourceContext::new(source_db_path, source_state_schema_path)?;

        let target_path = target_db.or_else(|| plan.target.as_ref().map(|t| t.db_path.clone()));
        let target_backup = if no_backup {
            None
        } else {
            backup_dir.or_else(|| plan.target.as_ref().and_then(|t| t.backup_dir.clone()))
        };

        // Open target database in appropriate mode based on dry_run flag
        let target = match target_path {
            Some(path) => {
                if dry_run {
                    Some(TargetContext::new_read_only(path, target_backup)?)
                } else {
                    Some(TargetContext::new_writable(path, target_backup)?)
                }
            }
            None => None,
        };

        Ok(Self {
            plan,
            source,
            target,
            dry_run,
        })
    }

    /// Return the loaded migration plan.
    pub const fn plan(&self) -> &MigrationPlan {
        &self.plan
    }

    /// Whether the context is configured for dry-run mode.
    pub const fn is_dry_run(&self) -> bool {
        self.dry_run
    }

    /// Read-only access to the source endpoint.
    pub const fn source(&self) -> &SourceContext {
        &self.source
    }

    /// Read-only access to the target endpoint, if configured.
    pub const fn target(&self) -> Option<&TargetContext> {
        self.target.as_ref()
    }
}

/// Indicates how far schema loading progressed.
#[derive(Debug, Clone, Copy)]
pub enum SchemaStatus<'a> {
    NotConfigured,
    PendingStateSchema { schema_path: &'a Path },
    Loaded,
}

/// Holds source RocksDB handle and optional state schema.
pub struct SourceContext {
    path: PathBuf,
    state_schema_path: Option<PathBuf>,
    db: DBWithThreadMode<SingleThreaded>,
    // Thread-safe, write-once cache of the decoded schema shared across readers.
    schema: OnceCell<Manifest>,
}

impl SourceContext {
    /// Open the source database and remember the optional state schema path.
    fn new(path: PathBuf, state_schema_path: Option<PathBuf>) -> Result<Self> {
        ensure!(
            path.exists(),
            "Source database path does not exist: {}",
            path.display()
        );

        if let Some(ref schema) = state_schema_path {
            ensure!(
                schema.exists(),
                "State schema file does not exist: {}",
                schema.display()
            );
        }

        let db = open_database(&path)?;

        Ok(Self {
            path,
            state_schema_path,
            db,
            schema: OnceCell::new(),
        })
    }

    /// Filesystem location of the source database.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Optional path to the state schema JSON file.
    pub fn state_schema_path(&self) -> Option<&Path> {
        self.state_schema_path.as_deref()
    }

    /// Whether the schema is absent, pending load, or cached.
    pub fn schema_status(&self) -> SchemaStatus<'_> {
        if self.schema.get().is_some() {
            return SchemaStatus::Loaded;
        }

        if let Some(path) = self.state_schema_path() {
            return SchemaStatus::PendingStateSchema { schema_path: path };
        }

        SchemaStatus::NotConfigured
    }

    /// Lazily load the state schema, caching the result.
    /// The schema is required whenever a plan requests schema-aware decoding (e.g. `decode_with_abi`).
    ///
    /// Loads from state schema file (sufficient for state deserialization).
    pub fn schema(&self) -> Result<Option<&Manifest>> {
        let schema = if let Some(schema_path) = &self.state_schema_path {
            self.schema.get_or_try_init(|| {
                abi::load_state_schema_from_json(schema_path).wrap_err_with(|| {
                    format!("Failed to load state schema from {}", schema_path.display())
                })
            })?
        } else {
            return Ok(None);
        };

        Ok(Some(schema))
    }

    /// Access the underlying RocksDB handle.
    pub const fn db(&self) -> &DBWithThreadMode<SingleThreaded> {
        &self.db
    }
}

/// Holds target RocksDB handle and associated metadata.
pub struct TargetContext {
    path: PathBuf,
    backup_dir: Option<PathBuf>,
    db: DBWithThreadMode<SingleThreaded>,
    read_only: bool,
}

impl TargetContext {
    /// Open the target database in read-only mode for dry runs.
    fn new_read_only(path: PathBuf, backup_dir: Option<PathBuf>) -> Result<Self> {
        ensure!(
            path.exists(),
            "Target database path does not exist: {}",
            path.display()
        );

        let db = open_database(&path)?;

        Ok(Self {
            path,
            backup_dir,
            db,
            read_only: true,
        })
    }

    /// Open the target database with write access for mutating operations.
    ///
    /// This method is used when `--apply` mode is enabled, allowing the migration
    /// engine to write data to the target database. The database is opened with
    /// all column families to ensure proper schema alignment with the source.
    ///
    /// If the target directory does not exist, it will be created automatically.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the target RocksDB database (will be created if missing)
    /// * `backup_dir` - Optional directory for storing backups before mutations
    ///
    /// # Returns
    ///
    /// A `TargetContext` with write access enabled, or an error if the database
    /// cannot be opened or the directory cannot be created.
    fn new_writable(path: PathBuf, backup_dir: Option<PathBuf>) -> Result<Self> {
        use crate::types::Column;
        use rocksdb::Options;

        // Create the directory if it doesn't exist
        if !path.exists() {
            std::fs::create_dir_all(&path).wrap_err_with(|| {
                format!(
                    "Failed to create target database directory at {}",
                    path.display()
                )
            })?;
        }

        // Prepare column family names for opening the database with write access
        let cf_names: Vec<String> = Column::all()
            .iter()
            .map(|c| c.as_str().to_owned())
            .collect();

        let mut options = Options::default();
        options.create_if_missing(true);
        options.create_missing_column_families(true);

        // Open database in read-write mode for mutating operations
        let db = DBWithThreadMode::<SingleThreaded>::open_cf(&options, &path, &cf_names)
            .wrap_err_with(|| {
                format!(
                    "Failed to open target database in write mode at {}",
                    path.display()
                )
            })?;

        Ok(Self {
            path,
            backup_dir,
            db,
            read_only: false,
        })
    }

    /// Filesystem location of the target database.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Optional directory where backups should be stored.
    pub fn backup_dir(&self) -> Option<&Path> {
        self.backup_dir.as_deref()
    }

    /// Whether the target database was opened read-only.
    pub const fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Access the underlying RocksDB handle.
    pub const fn db(&self) -> &DBWithThreadMode<SingleThreaded> {
        &self.db
    }
}
