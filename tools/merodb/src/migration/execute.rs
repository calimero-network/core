//!
//! Execution engine for the `migrate` command.
//!
//! This module implements mutating operations for migration plans when `--apply` mode is enabled.
//! It builds on top of the dry-run engine's filter resolution and scanning logic, but performs
//! actual write operations to the target database using RocksDB `WriteBatch` for atomicity.
//!
//! ## Key Features
//!
//! - **WriteBatch Operations**: All writes within a step are batched and committed atomically
//! - **Idempotency**: Steps can be safely re-run if interrupted (future enhancement)
//! - **Detailed Logging**: Progress and key operations are logged for observability
//! - **Filter Reuse**: Leverages the same filter resolution logic from dry-run mode
//!
//! ## Step Execution
//!
//! Each migration step type is executed as follows:
//!
//! - **Copy**: Reads matching keys from source, writes to target using WriteBatch
//! - **Delete**: Identifies matching keys in target, deletes them using WriteBatch
//! - **Upsert**: Writes literal key-value entries to target using WriteBatch
//! - **Verify**: Evaluates assertions against the target database (read-only)
//!
//! ## Safety Mechanisms
//!
//! - All operations require an explicit target database with write access
//! - WriteBatch ensures atomic commits per step
//! - Verification steps can abort the migration if assertions fail
//!

use std::collections::HashSet;

use eyre::{bail, ensure, Result, WrapErr};
use rocksdb::{DBWithThreadMode, IteratorMode, SingleThreaded, WriteBatch};
use serde::Serialize;

use core::convert::TryFrom;

use crate::types::Column;

use super::context::MigrationContext;
use super::plan::{
    CopyStep, DeleteStep, PlanDefaults, PlanFilters, PlanStep, UpsertStep, VerificationAssertion,
    VerifyStep,
};

/// Maximum number of keys to process per WriteBatch for memory efficiency
const BATCH_SIZE_LIMIT: usize = 1000;

/// Aggregated execution report containing results for all steps in the migration.
#[derive(Debug, Serialize)]
pub struct ExecutionReport {
    pub steps: Vec<StepExecutionReport>,
}

/// Per-step execution report containing operation counts and warnings.
#[derive(Debug, Serialize)]
pub struct StepExecutionReport {
    pub index: usize,
    pub keys_processed: usize,
    pub filters_summary: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub detail: StepExecutionDetail,
}

/// Additional execution information specific to each step type.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepExecutionDetail {
    Copy {
        keys_copied: usize,
        bytes_copied: usize,
    },
    Delete {
        keys_deleted: usize,
    },
    Upsert {
        entries_written: usize,
    },
    Verify {
        summary: String,
        passed: bool,
    },
}

/// Execute all steps in the migration plan, writing changes to the target database.
///
/// This function performs the actual migration by:
/// 1. Iterating through each step in the plan
/// 2. Resolving filters from defaults and step-specific overrides
/// 3. Executing the appropriate operation (copy, delete, upsert, verify)
/// 4. Collecting execution statistics and warnings
///
/// # Arguments
///
/// * `context` - Migration context containing plan, source, and target databases
///
/// # Returns
///
/// An `ExecutionReport` containing per-step results, or an error if any step fails.
///
/// # Errors
///
/// - If no target database is configured
/// - If target database is opened in read-only mode
/// - If any verification step fails its assertion
/// - If any database operation fails during execution
pub fn execute_migration(context: &MigrationContext) -> Result<ExecutionReport> {
    ensure!(
        !context.is_dry_run(),
        "Cannot execute migration in dry-run mode; context must be created with dry_run=false"
    );

    let target = context.target().ok_or_else(|| {
        eyre::eyre!("Migration execution requires a target database, but none was configured")
    })?;

    ensure!(
        !target.is_read_only(),
        "Target database is opened in read-only mode; cannot execute mutating operations"
    );

    let plan = context.plan();
    let source_db = context.source().db();
    let target_db = target.db();

    let mut steps = Vec::with_capacity(plan.steps.len());

    for (index, step) in plan.steps.iter().enumerate() {
        eprintln!(
            "Executing step {}/{}: {}",
            index + 1,
            plan.steps.len(),
            step_label(index, step)
        );

        let report = match step {
            PlanStep::Copy(copy) => {
                execute_copy_step(index, copy, &plan.defaults, source_db, target_db)?
            }
            PlanStep::Delete(delete) => {
                execute_delete_step(index, delete, &plan.defaults, target_db)?
            }
            PlanStep::Upsert(upsert) => {
                execute_upsert_step(index, upsert, &plan.defaults, target_db)?
            }
            PlanStep::Verify(verify) => {
                execute_verify_step(index, verify, &plan.defaults, target_db)?
            }
        };

        eprintln!("  Completed: {} keys processed", report.keys_processed);

        steps.push(report);
    }

    Ok(ExecutionReport { steps })
}

/// Execute a `copy` step: read matching keys from source, write to target.
///
/// This function:
/// 1. Resolves filters from defaults and step overrides
/// 2. Scans the source database column for matching keys
/// 3. Writes matched key-value pairs to the target database in batches
/// 4. Returns statistics about the operation
///
/// # Arguments
///
/// * `index` - Step index for reporting
/// * `step` - Copy step configuration
/// * `defaults` - Plan-level defaults for filters and options
/// * `source_db` - Source database (read-only)
/// * `target_db` - Target database (writable)
///
/// # Returns
///
/// A `StepExecutionReport` containing copy statistics and any warnings.
fn execute_copy_step(
    index: usize,
    step: &CopyStep,
    defaults: &PlanDefaults,
    source_db: &DBWithThreadMode<SingleThreaded>,
    target_db: &DBWithThreadMode<SingleThreaded>,
) -> Result<StepExecutionReport> {
    let filters = defaults.merge_filters(&step.filters);
    let resolved = ResolvedFilters::resolve(step.column, &filters);

    let source_cf = source_db
        .cf_handle(step.column.as_str())
        .ok_or_else(|| eyre::eyre!("Source column family '{}' not found", step.column.as_str()))?;

    let target_cf = target_db
        .cf_handle(step.column.as_str())
        .ok_or_else(|| eyre::eyre!("Target column family '{}' not found", step.column.as_str()))?;

    let mut keys_copied = 0;
    let mut bytes_copied = 0;
    let mut batch = WriteBatch::default();

    let iter = source_db.iterator_cf(source_cf, IteratorMode::Start);
    for item in iter {
        let (key, value) = item.wrap_err_with(|| {
            format!(
                "Failed to iterate source column family '{}' during copy",
                step.column.as_str()
            )
        })?;

        if resolved.matches(step.column, &key) {
            // Add to current batch
            batch.put_cf(target_cf, &key, &value);
            keys_copied += 1;
            bytes_copied += key.len() + value.len();

            // Commit batch if size limit reached
            if keys_copied % BATCH_SIZE_LIMIT == 0 {
                target_db.write(batch).wrap_err_with(|| {
                    format!(
                        "Failed to write batch to target column family '{}' after {} keys",
                        step.column.as_str(),
                        keys_copied
                    )
                })?;
                batch = WriteBatch::default();
                eprintln!("    Progress: {} keys copied...", keys_copied);
            }
        }
    }

    // Commit any remaining keys in the final batch
    if !batch.is_empty() {
        target_db.write(batch).wrap_err_with(|| {
            format!(
                "Failed to write final batch to target column family '{}' with {} keys",
                step.column.as_str(),
                keys_copied
            )
        })?;
    }

    Ok(StepExecutionReport {
        index,
        keys_processed: keys_copied,
        filters_summary: filters.summary(),
        warnings: resolved.warnings,
        detail: StepExecutionDetail::Copy {
            keys_copied,
            bytes_copied,
        },
    })
}

/// Execute a `delete` step: identify matching keys in target and delete them.
///
/// This function:
/// 1. Resolves filters from defaults and step overrides
/// 2. Scans the target database column for matching keys
/// 3. Deletes matched keys from the target database in batches
/// 4. Returns statistics about the operation
///
/// # Arguments
///
/// * `index` - Step index for reporting
/// * `step` - Delete step configuration
/// * `defaults` - Plan-level defaults for filters and options
/// * `target_db` - Target database (writable)
///
/// # Returns
///
/// A `StepExecutionReport` containing deletion statistics and any warnings.
fn execute_delete_step(
    index: usize,
    step: &DeleteStep,
    defaults: &PlanDefaults,
    target_db: &DBWithThreadMode<SingleThreaded>,
) -> Result<StepExecutionReport> {
    let filters = defaults.merge_filters(&step.filters);
    let resolved = ResolvedFilters::resolve(step.column, &filters);

    let target_cf = target_db
        .cf_handle(step.column.as_str())
        .ok_or_else(|| eyre::eyre!("Target column family '{}' not found", step.column.as_str()))?;

    let mut keys_deleted = 0;
    let mut batch = WriteBatch::default();

    // First pass: collect keys to delete (we can't modify while iterating)
    let mut keys_to_delete = Vec::new();
    let iter = target_db.iterator_cf(target_cf, IteratorMode::Start);
    for item in iter {
        let (key, _value) = item.wrap_err_with(|| {
            format!(
                "Failed to iterate target column family '{}' during delete",
                step.column.as_str()
            )
        })?;

        if resolved.matches(step.column, &key) {
            keys_to_delete.push(key.to_vec());
        }
    }

    // Second pass: delete collected keys in batches
    for key in keys_to_delete {
        batch.delete_cf(target_cf, &key);
        keys_deleted += 1;

        // Commit batch if size limit reached
        if keys_deleted % BATCH_SIZE_LIMIT == 0 {
            target_db.write(batch).wrap_err_with(|| {
                format!(
                    "Failed to write delete batch to target column family '{}' after {} keys",
                    step.column.as_str(),
                    keys_deleted
                )
            })?;
            batch = WriteBatch::default();
            eprintln!("    Progress: {} keys deleted...", keys_deleted);
        }
    }

    // Commit any remaining deletes in the final batch
    if !batch.is_empty() {
        target_db.write(batch).wrap_err_with(|| {
            format!(
                "Failed to write final delete batch to target column family '{}' with {} keys",
                step.column.as_str(),
                keys_deleted
            )
        })?;
    }

    Ok(StepExecutionReport {
        index,
        keys_processed: keys_deleted,
        filters_summary: filters.summary(),
        warnings: resolved.warnings,
        detail: StepExecutionDetail::Delete { keys_deleted },
    })
}

/// Execute an `upsert` step: write literal entries to the target database.
///
/// This function:
/// 1. Decodes key-value entries from the plan
/// 2. Writes all entries to the target database in a single batch
/// 3. Returns statistics about the operation
///
/// # Arguments
///
/// * `index` - Step index for reporting
/// * `step` - Upsert step configuration containing literal entries
/// * `defaults` - Plan-level defaults (unused for upsert)
/// * `target_db` - Target database (writable)
///
/// # Returns
///
/// A `StepExecutionReport` containing upsert statistics and any warnings.
fn execute_upsert_step(
    index: usize,
    step: &UpsertStep,
    _defaults: &PlanDefaults,
    target_db: &DBWithThreadMode<SingleThreaded>,
) -> Result<StepExecutionReport> {
    let target_cf = target_db
        .cf_handle(step.column.as_str())
        .ok_or_else(|| eyre::eyre!("Target column family '{}' not found", step.column.as_str()))?;

    let warnings = Vec::new();
    let mut batch = WriteBatch::default();
    let mut entries_written = 0;

    for entry in &step.entries {
        let key = entry
            .key
            .to_bytes()
            .wrap_err_with(|| format!("Failed to decode upsert key: {:?}", entry.key))?;

        let value = entry
            .value
            .to_bytes()
            .wrap_err_with(|| format!("Failed to decode upsert value: {:?}", entry.value))?;

        batch.put_cf(target_cf, &key, &value);
        entries_written += 1;
    }

    // Commit all upsert entries in a single batch
    if !batch.is_empty() {
        target_db.write(batch).wrap_err_with(|| {
            format!(
                "Failed to write upsert batch to target column family '{}' with {} entries",
                step.column.as_str(),
                entries_written
            )
        })?;
    }

    Ok(StepExecutionReport {
        index,
        keys_processed: entries_written,
        filters_summary: None,
        warnings,
        detail: StepExecutionDetail::Upsert { entries_written },
    })
}

/// Execute a `verify` step: evaluate assertions against the target database.
///
/// This function:
/// 1. Resolves filters from defaults and step overrides
/// 2. Scans the target database to count matching keys
/// 3. Evaluates the verification assertion
/// 4. Returns pass/fail status
///
/// Verification steps are read-only and do not modify the target database.
/// If a verification fails, the entire migration is aborted with an error.
///
/// # Arguments
///
/// * `index` - Step index for reporting
/// * `step` - Verify step configuration with assertion
/// * `defaults` - Plan-level defaults for filters and options
/// * `target_db` - Target database (read-only for verification)
///
/// # Returns
///
/// A `StepExecutionReport` containing verification results, or an error if the assertion fails.
fn execute_verify_step(
    index: usize,
    step: &VerifyStep,
    defaults: &PlanDefaults,
    target_db: &DBWithThreadMode<SingleThreaded>,
) -> Result<StepExecutionReport> {
    let filters = defaults.merge_filters(&step.filters);
    let resolved = ResolvedFilters::resolve(step.column, &filters);

    // Count matching keys in the target database
    let matched_count = count_matching_keys(target_db, step.column, &resolved)?;

    // Evaluate the assertion
    let outcome = evaluate_assertion(target_db, step.column, &step.assertion, matched_count)?;

    // Fail the migration if the verification did not pass
    if let Some(false) = outcome.passed {
        bail!(
            "Verification step {} failed: {}",
            index + 1,
            outcome.summary
        );
    }

    Ok(StepExecutionReport {
        index,
        keys_processed: matched_count,
        filters_summary: filters.summary(),
        warnings: outcome.warnings,
        detail: StepExecutionDetail::Verify {
            summary: outcome.summary,
            passed: outcome.passed.unwrap_or(false),
        },
    })
}

/// Count the number of keys matching the resolved filters in a column.
fn count_matching_keys(
    db: &DBWithThreadMode<SingleThreaded>,
    column: Column,
    filters: &ResolvedFilters,
) -> Result<usize> {
    let cf = db
        .cf_handle(column.as_str())
        .ok_or_else(|| eyre::eyre!("Column family '{}' not found", column.as_str()))?;

    let mut matched = 0;
    let iter = db.iterator_cf(cf, IteratorMode::Start);
    for item in iter {
        let (key, _value) = item.wrap_err_with(|| {
            format!(
                "Failed to iterate column family '{}' during verification",
                column.as_str()
            )
        })?;

        if filters.matches(column, &key) {
            matched += 1;
        }
    }

    Ok(matched)
}

/// Generate a human-readable label for a migration step.
fn step_label(_index: usize, step: &PlanStep) -> String {
    match step {
        PlanStep::Copy(s) => format!(
            "{} (copy from {} column{})",
            s.name.as_deref().unwrap_or("copy"),
            s.column.as_str(),
            if s.filters.is_empty() {
                ""
            } else {
                " with filters"
            }
        ),
        PlanStep::Delete(s) => format!(
            "{} (delete from {} column{})",
            s.name.as_deref().unwrap_or("delete"),
            s.column.as_str(),
            if s.filters.is_empty() {
                ""
            } else {
                " with filters"
            }
        ),
        PlanStep::Upsert(s) => format!(
            "{} (upsert {} entries to {} column)",
            s.name.as_deref().unwrap_or("upsert"),
            s.entries.len(),
            s.column.as_str()
        ),
        PlanStep::Verify(s) => format!(
            "{} (verify {} column)",
            s.name.as_deref().unwrap_or("verify"),
            s.column.as_str()
        ),
    }
}

/// Verification outcome containing summary, pass/fail status, and warnings.
struct VerificationOutcome {
    summary: String,
    passed: Option<bool>,
    warnings: Vec<String>,
}

/// Evaluate a verification assertion against the target database.
///
/// This function checks one of the following assertion types:
/// - `ExpectedCount`: Exact count match
/// - `MinCount`: Count is at least the specified minimum
/// - `MaxCount`: Count is at most the specified maximum
/// - `ContainsKey`: Specific key exists in the database
/// - `MissingKey`: Specific key does not exist in the database
fn evaluate_assertion(
    db: &DBWithThreadMode<SingleThreaded>,
    column: Column,
    assertion: &VerificationAssertion,
    matched_count: usize,
) -> Result<VerificationOutcome> {
    let cf = db
        .cf_handle(column.as_str())
        .ok_or_else(|| eyre::eyre!("Column family '{}' not found", column.as_str()))?;

    let matched_u64 = u64::try_from(matched_count).unwrap_or(u64::MAX);

    match assertion {
        VerificationAssertion::ExpectedCount { expected_count } => {
            let passed = matched_u64 == *expected_count;
            Ok(VerificationOutcome {
                summary: format!(
                    "expected count == {expected_count}, actual {matched_count} ({})",
                    pass_label(passed)
                ),
                passed: Some(passed),
                warnings: Vec::new(),
            })
        }
        VerificationAssertion::MinCount { min_count } => {
            let passed = matched_u64 >= *min_count;
            Ok(VerificationOutcome {
                summary: format!(
                    "expected count >= {min_count}, actual {matched_count} ({})",
                    pass_label(passed)
                ),
                passed: Some(passed),
                warnings: Vec::new(),
            })
        }
        VerificationAssertion::MaxCount { max_count } => {
            let passed = matched_u64 <= *max_count;
            Ok(VerificationOutcome {
                summary: format!(
                    "expected count <= {max_count}, actual {matched_count} ({})",
                    pass_label(passed)
                ),
                passed: Some(passed),
                warnings: Vec::new(),
            })
        }
        VerificationAssertion::ContainsKey { contains_key } => {
            let mut warnings = Vec::new();
            match contains_key.to_bytes() {
                Ok(bytes) => {
                    let present = db.get_cf(cf, &bytes)?.is_some();
                    Ok(VerificationOutcome {
                        summary: format!(
                            "expect key present ({}), actual {} ({})",
                            contains_key.preview(16),
                            if present { "present" } else { "missing" },
                            pass_label(present)
                        ),
                        passed: Some(present),
                        warnings,
                    })
                }
                Err(err) => {
                    warnings.push(format!("unable to decode contains_key value: {err}"));
                    Ok(VerificationOutcome {
                        summary: "expect key present, but decoding failed".into(),
                        passed: Some(false),
                        warnings,
                    })
                }
            }
        }
        VerificationAssertion::MissingKey { missing_key } => {
            let mut warnings = Vec::new();
            match missing_key.to_bytes() {
                Ok(bytes) => {
                    let present = db.get_cf(cf, &bytes)?.is_some();
                    let passed = !present;
                    Ok(VerificationOutcome {
                        summary: format!(
                            "expect key missing ({}), actual {} ({})",
                            missing_key.preview(16),
                            if present { "present" } else { "missing" },
                            pass_label(passed)
                        ),
                        passed: Some(passed),
                        warnings,
                    })
                }
                Err(err) => {
                    warnings.push(format!("unable to decode missing_key value: {err}"));
                    Ok(VerificationOutcome {
                        summary: "expect key missing, but decoding failed".into(),
                        passed: Some(false),
                        warnings,
                    })
                }
            }
        }
    }
}

/// Helper to map boolean results to human-friendly labels.
const fn pass_label(passed: bool) -> &'static str {
    if passed {
        "PASS"
    } else {
        "FAIL"
    }
}

/// Concrete filter values (decoded/parsed) used during column scans.
///
/// This structure mirrors the filter resolution logic from the dry-run engine
/// to ensure consistent behavior between preview and execution modes.
struct ResolvedFilters {
    context_ids: Option<HashSet<Vec<u8>>>,
    state_key_prefix: Option<Vec<u8>>,
    raw_key_prefix: Option<Vec<u8>>,
    key_range_start: Option<Vec<u8>>,
    key_range_end: Option<Vec<u8>>,
    alias_name: Option<String>,
    warnings: Vec<String>,
}

impl ResolvedFilters {
    /// Decode plan filters into byte-oriented structures, accumulating warnings as needed.
    ///
    /// This method performs the same filter resolution as the dry-run engine to ensure
    /// that the execution mode behaves identically to the preview mode.
    fn resolve(column: Column, filters: &PlanFilters) -> Self {
        let mut warnings = Vec::new();

        let context_ids = if filters.context_ids.is_empty() {
            None
        } else {
            let mut set = HashSet::new();
            for id in &filters.context_ids {
                match decode_hex_string(id) {
                    Ok(bytes) => {
                        let _ = set.insert(bytes);
                    }
                    Err(err) => warnings.push(format!("unable to parse context_id '{id}': {err}")),
                }
            }
            Some(set)
        };

        // Context aliases are not yet supported in execution mode
        if !filters.context_aliases.is_empty() {
            warnings.push(
                "context_aliases filter is not yet supported during execution; step may process more keys than expected"
                    .into(),
            );
        }

        let state_key_prefix =
            filters
                .state_key_prefix
                .as_ref()
                .map(|prefix| match decode_hex_string(prefix) {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        warnings.push(format!(
                            "unable to interpret state_key_prefix '{prefix}': {err}"
                        ));
                        prefix.as_bytes().to_vec()
                    }
                });

        let raw_key_prefix =
            filters
                .raw_key_prefix
                .as_ref()
                .and_then(|prefix| match decode_hex_string(prefix) {
                    Ok(bytes) => Some(bytes),
                    Err(err) => {
                        warnings.push(format!(
                            "unable to interpret raw_key_prefix '{prefix}': {err}"
                        ));
                        None
                    }
                });

        let key_range_start = filters.key_range.as_ref().and_then(|range| {
            range
                .start
                .as_ref()
                .and_then(|start| match decode_hex_string(start) {
                    Ok(bytes) => Some(bytes),
                    Err(err) => {
                        warnings.push(format!(
                            "unable to interpret key_range start '{start}': {err}"
                        ));
                        None
                    }
                })
        });

        let key_range_end = filters.key_range.as_ref().and_then(|range| {
            range
                .end
                .as_ref()
                .and_then(|end| match decode_hex_string(end) {
                    Ok(bytes) => Some(bytes),
                    Err(err) => {
                        warnings.push(format!("unable to interpret key_range end '{end}': {err}"));
                        None
                    }
                })
        });

        let alias_name = filters.alias_name.clone();
        if alias_name.is_some() && column != Column::Alias {
            warnings.push(
                "alias_name filter only applies to the Alias column; no rows will match in other columns"
                    .into(),
            );
        }

        Self {
            context_ids,
            state_key_prefix,
            raw_key_prefix,
            key_range_start,
            key_range_end,
            alias_name,
            warnings,
        }
    }

    /// Check if a raw key satisfies every resolved predicate.
    ///
    /// This method applies all active filters to determine whether a key should be
    /// included in the current operation. It returns true only if the key passes
    /// all filter checks (AND logic).
    fn matches(&self, column: Column, key: &[u8]) -> bool {
        // Context ID filter: extract and check the first 32 bytes
        if let Some(set) = &self.context_ids {
            let Some(context_slice) = extract_context_id(column, key) else {
                return false;
            };

            if !set.contains(&context_slice.to_vec()) {
                return false;
            }
        }

        // State key prefix filter: check bytes [32..] in State column
        if let Some(prefix) = &self.state_key_prefix {
            if column != Column::State {
                return false;
            }
            let Some(end) = 32_usize.checked_add(prefix.len()) else {
                return false;
            };
            if key.len() < end || !key[32..end].starts_with(prefix) {
                return false;
            }
        }

        // Raw key prefix filter: check the entire key
        if let Some(prefix) = &self.raw_key_prefix {
            if !key.starts_with(prefix) {
                return false;
            }
        }

        // Key range start filter: lexicographic comparison
        if let Some(start) = &self.key_range_start {
            if key < start.as_slice() {
                return false;
            }
        }

        // Key range end filter: lexicographic comparison (exclusive)
        if let Some(end) = &self.key_range_end {
            if key >= end.as_slice() {
                return false;
            }
        }

        // Alias name filter: only applies to Alias column
        if let Some(alias) = &self.alias_name {
            if column != Column::Alias {
                return false;
            }

            match extract_alias_name(key) {
                Some(name) => {
                    if &name != alias {
                        return false;
                    }
                }
                None => return false,
            }
        }

        true
    }
}

/// Best-effort hex decoder used by filter resolution.
///
/// Accepts hex strings with or without "0x" prefix and ensures even length.
fn decode_hex_string(value: &str) -> Result<Vec<u8>> {
    let trimmed = value.trim().trim_start_matches("0x");
    if trimmed.len() % 2 != 0 {
        bail!("hex string has odd length");
    }
    Ok(hex::decode(trimmed)?)
}

/// Extract the context ID portion from a key when the column layout supports it.
///
/// For columns with context ID as the first 32 bytes (Meta, Config, Identity, State, Delta),
/// this function returns a slice of those bytes. For other columns, returns None.
fn extract_context_id(column: Column, key: &[u8]) -> Option<&[u8]> {
    if key.len() < 32 {
        return None;
    }

    match column {
        Column::Meta | Column::Config | Column::Identity | Column::State | Column::Delta => {
            Some(&key[0..32])
        }
        _ => None,
    }
}

/// Pull the alias name out of the canonical 83-byte alias key.
///
/// Alias keys have the structure: [kind: 1 byte][scope: 32 bytes][name: 50 bytes].
/// This function extracts the name portion and strips trailing null bytes.
fn extract_alias_name(key: &[u8]) -> Option<String> {
    if key.len() != 83 {
        return None;
    }
    let name_bytes = &key[33..83];
    Some(
        String::from_utf8_lossy(name_bytes)
            .trim_end_matches('\0')
            .to_owned(),
    )
}
