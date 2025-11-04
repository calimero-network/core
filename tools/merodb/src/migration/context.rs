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
    pub wasm_file: Option<PathBuf>,
    pub target_db: Option<PathBuf>,
}

/// Aggregates plan metadata with opened database handles for a run.
pub struct MigrationContext {
    /// Parsed migration plan that drives the run.
    plan: MigrationPlan,
    /// Active source database plus its optional ABI manifest.
    source: SourceContext,
    /// Optional target database if the plan or CLI provided one.
    target: Option<TargetContext>,
    /// True when execution must avoid writes and stay in preview mode.
    dry_run: bool,
}

impl MigrationContext {
    /// Build a migration context using plan defaults plus optional overrides.
    pub fn new(plan: MigrationPlan, overrides: MigrationOverrides, dry_run: bool) -> Result<Self> {
        let MigrationOverrides {
            source_db,
            wasm_file,
            target_db,
        } = overrides;

        let source_db_path = source_db.unwrap_or_else(|| plan.source.db_path.clone());
        let source_wasm_path = wasm_file.or_else(|| plan.source.wasm_file.clone());

        let source = SourceContext::new(source_db_path, source_wasm_path)?;

        let target_path = target_db.or_else(|| plan.target.as_ref().map(|t| t.db_path.clone()));
        let target_backup = plan.target.as_ref().and_then(|t| t.backup_dir.clone());

        let target = match target_path {
            Some(path) => Some(TargetContext::new_read_only(path, target_backup)?),
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

/// Indicates how far ABI manifest loading progressed.
#[derive(Debug, Clone, Copy)]
pub enum AbiManifestStatus<'a> {
    NotConfigured,
    Pending { wasm_path: &'a Path },
    Loaded,
}

/// Holds source RocksDB handle and optional ABI manifest.
pub struct SourceContext {
    path: PathBuf,
    wasm_path: Option<PathBuf>,
    db: DBWithThreadMode<SingleThreaded>,
    // Thread-safe, write-once cache of the decoded ABI manifest shared across readers.
    manifest: OnceCell<Manifest>,
}

impl SourceContext {
    /// Open the source database and remember the optional WASM path.
    fn new(path: PathBuf, wasm_path: Option<PathBuf>) -> Result<Self> {
        ensure!(
            path.exists(),
            "Source database path does not exist: {}",
            path.display()
        );

        if let Some(ref wasm) = wasm_path {
            ensure!(
                wasm.exists(),
                "WASM file does not exist: {}",
                wasm.display()
            );
        }

        let db = open_database(&path)?;

        Ok(Self {
            path,
            wasm_path,
            db,
            manifest: OnceCell::new(),
        })
    }

    /// Filesystem location of the source database.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Optional path to the WASM file carrying the ABI manifest.
    pub fn wasm_path(&self) -> Option<&Path> {
        self.wasm_path.as_deref()
    }

    /// Whether the ABI manifest is absent, pending load, or cached.
    pub fn abi_status(&self) -> AbiManifestStatus<'_> {
        self.wasm_path()
            .map_or(AbiManifestStatus::NotConfigured, |path| {
                if self.manifest.get().is_some() {
                    AbiManifestStatus::Loaded
                } else {
                    AbiManifestStatus::Pending { wasm_path: path }
                }
            })
    }

    /// Lazily decode the ABI manifest, caching the result.
    /// The manifest is required whenever a plan requests ABI-aware decoding (e.g. `decode_with_abi`).
    pub fn abi_manifest(&self) -> Result<Option<&Manifest>> {
        let Some(path) = &self.wasm_path else {
            return Ok(None);
        };

        let manifest = self.manifest.get_or_try_init(|| {
            abi::extract_abi_from_wasm(path)
                .wrap_err_with(|| format!("Failed to load ABI manifest from {}", path.display()))
        })?;

        Ok(Some(manifest))
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
