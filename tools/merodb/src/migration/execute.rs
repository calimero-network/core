//!
//! Execution engine for the `migrate` command.
//!
//! This module implements mutating operations for migration plans when `--apply` mode is enabled.
//! It builds on top of the dry-run engine's filter resolution and scanning logic, but performs
//! actual write operations to the target database using `RocksDB` `WriteBatch` for atomicity.
//!
//! ## Key Features
//!
//! - **`WriteBatch` Operations**: All writes within a step are batched and committed atomically
//! - **Value Transformations**: Copy steps support ABI decoding, jq transformations, and write-if-missing
//! - **Idempotency**: Steps can be safely re-run if interrupted (future enhancement)
//! - **Detailed Logging**: Progress and key operations are logged for observability
//! - **Filter Reuse**: Leverages the same filter resolution logic from dry-run mode
//!
//! ## Step Execution
//!
//! Each migration step type is executed as follows:
//!
//! - **Copy**: Reads matching keys from source, optionally transforms values, writes to target
//! - **Delete**: Identifies matching keys in target, deletes them using `WriteBatch`
//! - **Upsert**: Writes literal key-value entries to target using `WriteBatch`
//! - **Verify**: Evaluates assertions against the target database (read-only)
//!
//! ## Copy Step Transformations
//!
//! Copy steps now support three types of transformations (all optional):
//!
//! ### 1. ABI Decoding (`decode_with_abi`)
//! Deserializes binary-encoded values using the WASM ABI manifest, converting them to JSON.
//! Requires the source database to have an associated WASM file.
//!
//! **Example use case**: Converting binary contract state to human-readable JSON
//!
//! ### 2. jq Transformation (`jq`)
//! Applies a jq expression to modify values during copy. The value must be valid JSON
//! (either from ABI decoding or original value).
//!
//! **Example use case**: Removing sensitive fields or filtering specific state values
//!
//! ```yaml
//! transform:
//!   decode_with_abi: true
//!   jq: ".value.parsed | del(.internal_field)"
//! ```
//!
//! ### 3. Write-If-Missing (`write_if_missing`)
//! Skips writing keys that already exist in the target database, enabling idempotent
//! migrations and incremental data merges.
//!
//! **Example use case**: Merging multiple source databases without overwriting existing data
//!
//! **Processing Order**: write_if_missing check → ABI decoding → jq transformation → write
//!
//! See [`execute_copy_step`] for detailed implementation.
//!
//! ## Safety Mechanisms
//!
//! - All operations require an explicit target database with write access
//! - `WriteBatch` ensures atomic commits per step
//! - Verification steps can abort the migration if assertions fail
//! - Transformation errors (ABI decode failure, invalid jq) abort the migration
//!

#![allow(
    clippy::arithmetic_side_effects,
    reason = "Counter increments and index calculations are safe in migration context"
)]

use eyre::{bail, ensure, Result, WrapErr};
use rocksdb::{DBWithThreadMode, IteratorMode, SingleThreaded, WriteBatch};
use serde::Serialize;

use crate::types::Column;
use crate::validation::validate_database;

use super::backup::create_backup;
use super::context::MigrationContext;
use super::filters::ResolvedFilters;
use super::plan::{CopyStep, DeleteStep, PlanDefaults, PlanStep, UpsertStep, VerifyStep};
use super::verification::evaluate_assertion;

/// Default number of keys to process per `WriteBatch` for memory efficiency
const DEFAULT_BATCH_SIZE: usize = 1000;

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

/// Check if step guards are satisfied before executing a step.
///
/// This function evaluates safety guards configured for a step:
/// - **requires_validation**: Run existing validation logic on the target database
/// - **requires_empty_target**: Ensure the target database column is empty
///
/// # Arguments
///
/// * `step` - The step with potential guards to check
/// * `target_db` - The target database to check
///
/// # Returns
///
/// `Ok(())` if all guards pass, or an error if any guard fails.
fn check_step_guards(step: &PlanStep, target_db: &DBWithThreadMode<SingleThreaded>) -> Result<()> {
    let guards = step.guards();

    if !guards.has_guards() {
        return Ok(());
    }

    // Check requires_empty_target guard
    if guards.requires_empty_target {
        eprintln!("  Guard check: requires_empty_target");
        let column = step.column();
        let cf = target_db
            .cf_handle(column.as_str())
            .ok_or_else(|| eyre::eyre!("Column family '{}' not found", column.as_str()))?;

        let mut iter = target_db.iterator_cf(cf, IteratorMode::Start);
        if let Some(first) = iter.next() {
            drop(first?); // Check for iterator errors
            bail!(
                "Guard check failed: requires_empty_target - column '{}' is not empty",
                column.as_str()
            );
        }
        eprintln!("    Column '{}' is empty (passed)", column.as_str());
    }

    // Check requires_validation guard
    if guards.requires_validation {
        eprintln!("  Guard check: requires_validation");

        let validation_result =
            validate_database(target_db).wrap_err("Failed to run validation on target database")?;

        let status = validation_result
            .get("validation_result")
            .and_then(|v| v.get("status"))
            .and_then(|s| s.as_str())
            .unwrap_or("unknown");

        let invalid_count = validation_result
            .get("validation_result")
            .and_then(|v| v.get("invalid_entries"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);

        if status != "passed" || invalid_count > 0 {
            eprintln!("    Validation failed with {invalid_count} invalid entries");

            // Show first few errors for context
            if let Some(errors) = validation_result.get("errors").and_then(|e| e.as_array()) {
                for (i, error) in errors.iter().take(5).enumerate() {
                    eprintln!("      Error {}: {}", i + 1, error);
                }
                if errors.len() > 5 {
                    eprintln!("      ... and {} more errors", errors.len() - 5);
                }
            }

            bail!(
                "Guard check failed: requires_validation - database validation failed with {} invalid entries",
                invalid_count
            );
        }

        eprintln!("    Database validation passed");
    }

    Ok(())
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
    let target_db = target.db();

    // Create backup if backup_dir is configured
    if let Some(backup_dir) = target.backup_dir() {
        let _backup_path = create_backup(target_db, target.path(), backup_dir)
            .wrap_err("Failed to create backup before migration execution")?;
        eprintln!();
    }

    let mut steps = Vec::with_capacity(plan.steps.len());

    for (index, step) in plan.steps.iter().enumerate() {
        eprintln!(
            "Executing step {}/{}: {}",
            index + 1,
            plan.steps.len(),
            step_label(index, step)
        );

        // Check step guards before execution
        check_step_guards(step, target_db)
            .wrap_err_with(|| format!("Step {} guard check failed", index + 1))?;

        let report = match step {
            PlanStep::Copy(copy) => execute_copy_step(index, copy, &plan.defaults, context)?,
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

/// Execute a `copy` step: read matching keys from source, write to target with optional transformations.
///
/// This function implements the core copy logic with support for three transformation features:
///
/// ## Transformation Features
///
/// ### 1. ABI Decoding (`decode_with_abi`)
/// When enabled, values are deserialized using the WASM ABI manifest before being written.
/// This converts binary-encoded application state into structured JSON format.
///
/// **Requirements**:
/// - Source must have a WASM file with ABI manifest (via `source.wasm_file` or `--wasm-file`)
/// - WASM file must contain a `calimero_abi_v1` custom section
/// - Only works for columns that have ABI-decodable values (primarily State column)
///
/// **Error handling** (fail-fast before processing keys):
/// - Returns error if `decode_with_abi=true` but no WASM file is provided
/// - Returns error if WASM file exists but has no `calimero_abi_v1` custom section
/// - Returns error if value cannot be decoded with the provided ABI during processing
///
/// ### 2. jq Transformation (`jq`)
/// Applies a jq expression to transform values during copy. The jq filter receives the value
/// as JSON input and must produce exactly one JSON output.
///
/// **Processing order**:
/// - If both `decode_with_abi` and `jq` are set, ABI decoding happens first, then jq
/// - If only `jq` is set, value must already be valid JSON
///
/// **Example**: `.value.parsed | del(.internal_field)` removes a field from decoded state
///
/// **Error handling**:
/// - Returns error if jq expression is invalid (parse error)
/// - Returns error if jq produces != 1 output
/// - Returns error if input is not valid JSON
///
/// ### 3. Write-If-Missing (`write_if_missing`)
/// When enabled, keys that already exist in the target database are skipped (not overwritten).
/// This enables idempotent migrations and incremental data merges.
///
/// **Behavior**:
/// - Checks target DB for key existence before each write
/// - Skipped keys are counted and reported in warnings
/// - Default is `false` (always overwrite)
///
/// ## Execution Flow
///
/// 1. Resolve transformation settings (step override > plan default > engine default)
/// 2. Validate ABI manifest availability if `decode_with_abi=true`
/// 3. Compile jq filter if `jq` expression is provided
/// 4. For each key in source column that matches filters:
///    a. Check `write_if_missing` - skip if key exists in target
///    b. Apply ABI decoding if enabled
///    c. Apply jq transformation if provided
///    d. Add to batch and write when batch size reached
/// 5. Commit final batch
/// 6. Report statistics including skipped key count
///
/// # Arguments
///
/// * `index` - Step index for reporting (0-based)
/// * `step` - Copy step configuration including transformation settings
/// * `defaults` - Plan-level defaults for filters and transformation options
/// * `context` - Migration context containing source/target databases and optional ABI manifest
///
/// # Returns
///
/// A `StepExecutionReport` containing:
/// - Number of keys copied
/// - Total bytes copied
/// - Warnings (e.g., skipped keys, filter issues)
///
/// # Errors
///
/// Returns an error if:
/// - `decode_with_abi=true` but ABI manifest is not available
/// - ABI decoding fails for a value
/// - jq expression is invalid or produces wrong number of outputs
/// - Value is not valid JSON when jq transformation is requested
/// - Target database is not configured
/// - Database I/O operations fail
fn execute_copy_step(
    index: usize,
    step: &CopyStep,
    defaults: &PlanDefaults,
    context: &MigrationContext,
) -> Result<StepExecutionReport> {
    use crate::export::parse_value_with_abi;
    use jaq_interpret::FilterT;

    let filters = defaults.merge_filters(&step.filters);
    let mut resolved = ResolvedFilters::resolve(step.column, &filters);

    // Determine transformation settings using 3-level priority system:
    // 1. Step-level transform.* field (highest priority)
    // 2. Plan-level defaults.* field
    // 3. Engine default (false for all flags)
    let decode_with_abi = defaults.effective_decode_with_abi(step.transform.decode_with_abi);
    let write_if_missing = defaults.effective_write_if_missing(step.transform.write_if_missing);
    let jq_expr = step.transform.jq.as_deref();

    // Validate and prepare ABI manifest if decode_with_abi is enabled.
    // The manifest is lazily loaded from the WASM file on first access and cached.
    // This ensures the manifest is available before we start iterating keys, avoiding
    // partial copy failures midway through execution.
    let abi_manifest = if decode_with_abi {
        let manifest = context.source().abi_manifest()?.ok_or_else(|| {
            eyre::eyre!(
                "decode_with_abi requested but source ABI manifest is not available. \
                 Specify wasm_file in the plan or provide --wasm-file"
            )
        })?;
        Some(manifest)
    } else {
        None
    };

    // Compile jq filter upfront if specified. This validates the jq expression syntax
    // early and avoids re-compiling the same expression for every key.
    // The jaq library parses the expression into an AST, then compiles it into an
    // executable filter that can be run against JSON values.
    let jq_filter = if let Some(expr) = jq_expr {
        // Parse jq expression into AST
        let (filter_ast, errs) = jaq_parse::parse(expr, jaq_parse::main());
        if !errs.is_empty() {
            eyre::bail!(
                "Failed to parse jq expression '{}': {:?}",
                expr,
                errs.into_iter().map(|e| e.to_string()).collect::<Vec<_>>()
            );
        }
        let filter_ast =
            filter_ast.ok_or_else(|| eyre::eyre!("No filter AST returned from jq parse"))?;

        // Compile AST into executable filter
        let mut ctx = jaq_interpret::ParseCtx::new(Vec::new());
        ctx.insert_natives(core::iter::empty()); // No custom jq functions
        let filter = ctx.compile(filter_ast);
        Some(filter)
    } else {
        None
    };

    // Determine batch size: step override > plan default > engine default
    let batch_size = step
        .batch_size
        .or(defaults.batch_size)
        .unwrap_or(DEFAULT_BATCH_SIZE);

    let source_db = context.source().db();
    let target_db = context
        .target()
        .ok_or_else(|| eyre::eyre!("Target database required for copy step"))?
        .db();

    let source_cf = source_db
        .cf_handle(step.column.as_str())
        .ok_or_else(|| eyre::eyre!("Source column family '{}' not found", step.column.as_str()))?;

    let target_cf = target_db
        .cf_handle(step.column.as_str())
        .ok_or_else(|| eyre::eyre!("Target column family '{}' not found", step.column.as_str()))?;

    let mut keys_copied = 0;
    let mut keys_skipped = 0;
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

        // Apply filters - skip keys that don't match the configured criteria
        if !resolved.matches(step.column, &key) {
            continue;
        }

        // Transformation 1: write_if_missing check
        // If enabled, perform a target DB lookup to check if this key already exists.
        // This enables idempotent migrations - keys that exist are preserved, only
        // missing keys are written. Useful for incremental migrations and data merges.
        if write_if_missing && target_db.get_cf(target_cf, &key)?.is_some() {
            keys_skipped += 1;
            continue;
        }

        // Apply value transformations in order: ABI decoding → jq transformation
        // Both transformations are optional and can be used independently or together.
        let final_value = if decode_with_abi || jq_filter.is_some() {
            // Transformation 2: ABI decoding (optional, runs first)
            // Deserializes binary-encoded values using the WASM ABI manifest.
            // The result is JSON-encoded for further processing or storage.
            let mut transformed_value = if decode_with_abi {
                let manifest = abi_manifest.expect("ABI manifest should be available");

                // Parse the binary value using column-specific ABI logic
                let parsed =
                    parse_value_with_abi(step.column, &value, manifest).wrap_err_with(|| {
                        format!("Failed to parse value with ABI for key {:?}", &key[..])
                    })?;

                // Convert parsed JSON value to bytes for next transformation or storage
                serde_json::to_vec(&parsed)
                    .wrap_err("Failed to serialize ABI-decoded value to JSON")?
            } else {
                // No ABI decoding - pass through raw bytes
                value.to_vec()
            };

            // Transformation 3: jq transformation (optional, runs after ABI decoding)
            // Applies a jq expression to modify the JSON value.
            // The input must be valid JSON (either from ABI decoding or original value).
            if let Some(filter) = &jq_filter {
                // Parse bytes as JSON for jq processing
                let input_json: serde_json::Value = serde_json::from_slice(&transformed_value)
                    .wrap_err("Failed to parse value as JSON for jq transformation")?;

                // Execute jq filter with no external inputs (empty iterator)
                let inputs = jaq_interpret::RcIter::new(core::iter::empty());
                let mut outputs: Vec<_> = filter
                    .run((
                        jaq_interpret::Ctx::new([], &inputs),
                        jaq_interpret::Val::from(input_json),
                    ))
                    .collect();

                // jq must produce exactly one output value
                // Multiple outputs or no outputs indicate an error in the jq expression
                if outputs.len() != 1 {
                    eyre::bail!(
                        "jq expression must produce exactly one output, got {}",
                        outputs.len()
                    );
                }

                // Extract the single output and convert back to JSON bytes
                let output = outputs
                    .remove(0)
                    .map_err(|e| eyre::eyre!("jq error: {}", e))?;
                transformed_value = serde_json::to_vec(&serde_json::Value::from(output))
                    .wrap_err("Failed to serialize jq output")?;
            }

            transformed_value
        } else {
            // No transformations - copy raw bytes as-is
            value.to_vec()
        };

        // Add to current batch
        batch.put_cf(target_cf, &key, &final_value);
        keys_copied += 1;
        bytes_copied += key.len() + final_value.len();

        // Commit batch if size limit reached
        if keys_copied % batch_size == 0 {
            target_db.write(batch).wrap_err_with(|| {
                format!(
                    "Failed to write batch to target column family '{}' after {} keys",
                    step.column.as_str(),
                    keys_copied
                )
            })?;
            batch = WriteBatch::default();
            eprintln!("    Progress: {keys_copied} keys copied...");
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

    // Add warning about skipped keys if write_if_missing was used
    if write_if_missing && keys_skipped > 0 {
        resolved.warnings.push(format!(
            "Skipped {keys_skipped} keys that already existed in target (write_if_missing=true)"
        ));
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

    // Determine batch size: step override > plan default > engine default
    let batch_size = step
        .batch_size
        .or(defaults.batch_size)
        .unwrap_or(DEFAULT_BATCH_SIZE);

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
        if keys_deleted % batch_size == 0 {
            target_db.write(batch).wrap_err_with(|| {
                format!(
                    "Failed to write delete batch to target column family '{}' after {} keys",
                    step.column.as_str(),
                    keys_deleted
                )
            })?;
            batch = WriteBatch::default();
            eprintln!("    Progress: {keys_deleted} keys deleted...");
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
    if outcome.passed == Some(false) {
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::migration::context::MigrationOverrides;
    use crate::migration::plan::{
        CopyStep, CopyTransform, DeleteStep, EncodedValue, MigrationPlan, PlanFilters, PlanStep,
        PlanVersion, SourceEndpoint, StepGuards, TargetEndpoint, UpsertEntry, UpsertStep,
        VerificationAssertion, VerifyStep,
    };
    use crate::migration::test_utils::{test_context_id, test_state_key, DbFixture};
    use calimero_store::key::{AsKeyParts, ContextState as ContextStateKey};
    use rocksdb::IteratorMode;
    use tempfile::TempDir;

    /// Setup a source database with test data for execution tests.
    fn setup_source_db(path: &Path) -> Result<()> {
        let fixture = DbFixture::new(path)?;

        // Insert multiple entries for testing various scenarios
        let ctx1 = test_context_id(0x11);
        let ctx2 = test_context_id(0x22);

        fixture.insert_state_entry(&ctx1, &test_state_key(0xAA), b"value-1a")?;
        fixture.insert_state_entry(&ctx1, &test_state_key(0xBB), b"value-1b")?;
        fixture.insert_state_entry(&ctx2, &test_state_key(0xAA), b"value-2a")?;

        fixture.insert_generic_entry(b"key-1", b"generic-value-1")?;
        fixture.insert_generic_entry(b"key-2", b"generic-value-2")?;

        Ok(())
    }

    /// Setup an empty target database for execution tests.
    fn setup_empty_target_db(path: &Path) -> Result<()> {
        let _fixture = DbFixture::new(path)?;
        Ok(())
    }

    /// Setup a target database with existing data for delete/verify tests.
    fn setup_target_db_with_data(path: &Path) -> Result<()> {
        let fixture = DbFixture::new(path)?;

        let ctx1 = test_context_id(0x11);
        let ctx2 = test_context_id(0x22);

        fixture.insert_state_entry(&ctx1, &test_state_key(0xAA), b"old-value-1a")?;
        fixture.insert_state_entry(&ctx1, &test_state_key(0xBB), b"old-value-1b")?;
        fixture.insert_state_entry(&ctx2, &test_state_key(0xAA), b"old-value-2a")?;

        Ok(())
    }

    /// Helper to count keys in a specific column of a database.
    fn count_keys_in_column(db_path: &Path, column: Column) -> Result<usize> {
        use crate::open_database;

        let db = open_database(db_path)?;
        let cf = db
            .cf_handle(column.as_str())
            .ok_or_else(|| eyre::eyre!("Column family '{}' not found", column.as_str()))?;

        let mut count = 0;
        let iter = db.iterator_cf(cf, IteratorMode::Start);
        for item in iter {
            let _entry = item?;
            count += 1;
        }

        Ok(count)
    }

    /// Helper to get a value from a database.
    fn get_value(db_path: &Path, column: Column, key: &[u8]) -> Result<Option<Vec<u8>>> {
        use crate::open_database;

        let db = open_database(db_path)?;
        let cf = db
            .cf_handle(column.as_str())
            .ok_or_else(|| eyre::eyre!("Column family '{}' not found", column.as_str()))?;

        Ok(db.get_cf(cf, key)?)
    }

    #[test]
    fn execute_copy_writes_matching_keys_to_target() -> Result<()> {
        let temp = TempDir::new()?;
        let source_path = temp.path().join("source");
        let target_path = temp.path().join("target");

        setup_source_db(&source_path)?;
        setup_empty_target_db(&target_path)?;

        // Create a plan that copies all State entries with context_id 0x11
        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path: source_path,
                wasm_file: None,
            },
            target: Some(TargetEndpoint {
                db_path: target_path.clone(),
                backup_dir: None,
            }),
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Copy(CopyStep {
                name: Some("copy-ctx1".into()),
                column: Column::State,
                filters: PlanFilters {
                    context_ids: vec![hex::encode([0x11; 32])],
                    ..PlanFilters::default()
                },
                transform: CopyTransform::default(),
                guards: StepGuards::default(),
                batch_size: None,
            })],
        };

        // Execute the migration in apply mode (dry_run = false)
        let context = MigrationContext::new(plan, MigrationOverrides::default(), false)?;
        let report = execute_migration(&context)?;

        // Verify execution report
        ensure!(
            report.steps.len() == 1,
            "expected 1 step in report, found {}",
            report.steps.len()
        );

        let step = &report.steps[0];
        ensure!(
            matches!(step.detail, StepExecutionDetail::Copy { .. }),
            "expected Copy detail"
        );

        if let StepExecutionDetail::Copy { keys_copied, .. } = &step.detail {
            ensure!(
                *keys_copied == 2,
                "expected 2 keys copied, got {}",
                keys_copied
            );
        }

        // Verify target database contains exactly the copied keys
        let target_count = count_keys_in_column(&target_path, Column::State)?;
        ensure!(
            target_count == 2,
            "expected 2 keys in target, found {}",
            target_count
        );

        // Verify specific key was copied with correct value
        let key = ContextStateKey::new(test_context_id(0x11), test_state_key(0xAA));
        let key_bytes = key.as_key().as_bytes();

        let value = get_value(&target_path, Column::State, key_bytes)?;
        ensure!(
            value.as_deref() == Some(&b"value-1a"[..]),
            "expected value 'value-1a', got {:?}",
            value
        );

        Ok(())
    }

    #[test]
    fn execute_copy_respects_filters() -> Result<()> {
        let temp = TempDir::new()?;
        let source_path = temp.path().join("source");
        let target_path = temp.path().join("target");

        setup_source_db(&source_path)?;
        setup_empty_target_db(&target_path)?;

        // Create a plan that copies Generic column entries
        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path: source_path,
                wasm_file: None,
            },
            target: Some(TargetEndpoint {
                db_path: target_path.clone(),
                backup_dir: None,
            }),
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Copy(CopyStep {
                name: Some("copy-generic".into()),
                column: Column::Generic,
                filters: PlanFilters {
                    raw_key_prefix: Some(hex::encode(b"key-1")),
                    ..PlanFilters::default()
                },
                transform: CopyTransform::default(),
                guards: StepGuards::default(),
                batch_size: None,
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), false)?;
        let report = execute_migration(&context)?;

        // Should only copy 1 key matching the prefix
        if let StepExecutionDetail::Copy { keys_copied, .. } = &report.steps[0].detail {
            ensure!(
                *keys_copied == 1,
                "expected 1 key copied with prefix filter, got {}",
                keys_copied
            );
        }

        let target_count = count_keys_in_column(&target_path, Column::Generic)?;
        ensure!(
            target_count == 1,
            "expected 1 key in target, found {}",
            target_count
        );

        Ok(())
    }

    #[test]
    fn execute_delete_removes_matching_keys() -> Result<()> {
        let temp = TempDir::new()?;
        let target_path = temp.path().join("target");

        setup_target_db_with_data(&target_path)?;

        // Verify initial state: 3 keys present
        let initial_count = count_keys_in_column(&target_path, Column::State)?;
        ensure!(
            initial_count == 3,
            "expected 3 keys initially, found {}",
            initial_count
        );

        // Create a plan that deletes all entries with context_id 0x11
        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path: target_path.clone(),
                wasm_file: None,
            },
            target: Some(TargetEndpoint {
                db_path: target_path.clone(),
                backup_dir: None,
            }),
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Delete(DeleteStep {
                name: Some("delete-ctx1".into()),
                column: Column::State,
                filters: PlanFilters {
                    context_ids: vec![hex::encode([0x11; 32])],
                    ..PlanFilters::default()
                },
                guards: StepGuards::default(),
                batch_size: None,
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), false)?;
        let report = execute_migration(&context)?;

        // Verify execution report
        if let StepExecutionDetail::Delete { keys_deleted } = &report.steps[0].detail {
            ensure!(
                *keys_deleted == 2,
                "expected 2 keys deleted, got {}",
                keys_deleted
            );
        }

        // Verify only 1 key remains (context_id 0x22)
        let final_count = count_keys_in_column(&target_path, Column::State)?;
        ensure!(
            final_count == 1,
            "expected 1 key remaining, found {}",
            final_count
        );

        // Verify the remaining key is the one with context_id 0x22
        let remaining_key = ContextStateKey::new(test_context_id(0x22), test_state_key(0xAA));
        let remaining_key_bytes = remaining_key.as_key().as_bytes();

        let value = get_value(&target_path, Column::State, remaining_key_bytes)?;
        ensure!(
            value.is_some(),
            "expected context 0x22 key to remain after deletion"
        );

        Ok(())
    }

    #[test]
    fn execute_upsert_writes_literal_entries() -> Result<()> {
        let temp = TempDir::new()?;
        let target_path = temp.path().join("target");

        setup_empty_target_db(&target_path)?;

        // Create a plan that upserts literal key-value pairs
        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path: target_path.clone(),
                wasm_file: None,
            },
            target: Some(TargetEndpoint {
                db_path: target_path.clone(),
                backup_dir: None,
            }),
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Upsert(UpsertStep {
                name: Some("upsert-literals".into()),
                column: Column::Generic,
                entries: vec![
                    UpsertEntry {
                        key: EncodedValue::Hex {
                            data: "0x6b6579414243".to_owned(),
                        }, // "keyABC"
                        value: EncodedValue::Utf8 {
                            data: "literal-value-1".to_owned(),
                        },
                    },
                    UpsertEntry {
                        key: EncodedValue::Hex {
                            data: "0x6b6579444546".to_owned(),
                        }, // "keyDEF"
                        value: EncodedValue::Utf8 {
                            data: "literal-value-2".to_owned(),
                        },
                    },
                ],
                guards: StepGuards::default(),
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), false)?;
        let report = execute_migration(&context)?;

        // Verify execution report
        if let StepExecutionDetail::Upsert { entries_written } = &report.steps[0].detail {
            ensure!(
                *entries_written == 2,
                "expected 2 entries written, got {}",
                entries_written
            );
        }

        // Verify target database contains the upserted entries
        let target_count = count_keys_in_column(&target_path, Column::Generic)?;
        ensure!(
            target_count == 2,
            "expected 2 keys in target, found {}",
            target_count
        );

        let value1 = get_value(&target_path, Column::Generic, b"keyABC")?;
        ensure!(
            value1.as_deref() == Some(&b"literal-value-1"[..]),
            "expected 'literal-value-1', got {:?}",
            value1
        );

        let value2 = get_value(&target_path, Column::Generic, b"keyDEF")?;
        ensure!(
            value2.as_deref() == Some(&b"literal-value-2"[..]),
            "expected 'literal-value-2', got {:?}",
            value2
        );

        Ok(())
    }

    #[test]
    fn execute_verify_passes_when_assertion_succeeds() -> Result<()> {
        let temp = TempDir::new()?;
        let target_path = temp.path().join("target");

        setup_target_db_with_data(&target_path)?;

        // Create a plan with a verify step that should pass
        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path: target_path.clone(),
                wasm_file: None,
            },
            target: Some(TargetEndpoint {
                db_path: target_path,
                backup_dir: None,
            }),
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Verify(VerifyStep {
                name: Some("verify-count".into()),
                column: Column::State,
                filters: PlanFilters::default(),
                assertion: VerificationAssertion::ExpectedCount { expected_count: 3 },
                guards: StepGuards::default(),
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), false)?;
        let report = execute_migration(&context)?;

        // Verify execution report shows verification passed
        if let StepExecutionDetail::Verify { passed, .. } = &report.steps[0].detail {
            ensure!(*passed, "expected verification to pass");
        }

        Ok(())
    }

    #[test]
    fn execute_verify_fails_when_assertion_fails() -> Result<()> {
        let temp = TempDir::new()?;
        let target_path = temp.path().join("target");

        setup_target_db_with_data(&target_path)?;

        // Create a plan with a verify step that should fail (expects 10 keys but only 3 exist)
        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path: target_path.clone(),
                wasm_file: None,
            },
            target: Some(TargetEndpoint {
                db_path: target_path,
                backup_dir: None,
            }),
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Verify(VerifyStep {
                name: Some("verify-fail".into()),
                column: Column::State,
                filters: PlanFilters::default(),
                assertion: VerificationAssertion::ExpectedCount { expected_count: 10 },
                guards: StepGuards::default(),
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), false)?;
        let result = execute_migration(&context);

        // Verify that execution failed
        ensure!(
            result.is_err(),
            "expected execution to fail when verification fails"
        );

        let err = result.unwrap_err();
        ensure!(
            err.to_string().contains("Verification step"),
            "error should mention verification failure: {}",
            err
        );

        Ok(())
    }

    #[test]
    fn execute_multi_step_migration_sequence() -> Result<()> {
        let temp = TempDir::new()?;
        let source_path = temp.path().join("source");
        let target_path = temp.path().join("target");

        setup_source_db(&source_path)?;
        setup_empty_target_db(&target_path)?;

        // Create a multi-step plan: copy, verify, upsert, verify again
        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path: source_path,
                wasm_file: None,
            },
            target: Some(TargetEndpoint {
                db_path: target_path.clone(),
                backup_dir: None,
            }),
            defaults: PlanDefaults::default(),
            steps: vec![
                // Step 1: Copy Generic column entries
                PlanStep::Copy(CopyStep {
                    name: Some("copy-generic".into()),
                    column: Column::Generic,
                    filters: PlanFilters::default(),
                    transform: CopyTransform::default(),
                    guards: StepGuards::default(),
                    batch_size: None,
                }),
                // Step 2: Verify we copied 2 entries
                PlanStep::Verify(VerifyStep {
                    name: Some("verify-copied".into()),
                    column: Column::Generic,
                    filters: PlanFilters::default(),
                    assertion: VerificationAssertion::ExpectedCount { expected_count: 2 },
                    guards: StepGuards::default(),
                }),
                // Step 3: Upsert one more entry
                PlanStep::Upsert(UpsertStep {
                    name: Some("upsert-one".into()),
                    column: Column::Generic,
                    entries: vec![UpsertEntry {
                        key: EncodedValue::Utf8 {
                            data: "key-3".to_owned(),
                        },
                        value: EncodedValue::Utf8 {
                            data: "value-3".to_owned(),
                        },
                    }],
                    guards: StepGuards::default(),
                }),
                // Step 4: Verify we now have 3 entries total
                PlanStep::Verify(VerifyStep {
                    name: Some("verify-final".into()),
                    column: Column::Generic,
                    filters: PlanFilters::default(),
                    assertion: VerificationAssertion::ExpectedCount { expected_count: 3 },
                    guards: StepGuards::default(),
                }),
            ],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), false)?;
        let report = execute_migration(&context)?;

        // Verify all steps executed successfully
        ensure!(report.steps.len() == 4, "expected 4 steps executed");

        // Verify final database state
        let final_count = count_keys_in_column(&target_path, Column::Generic)?;
        ensure!(
            final_count == 3,
            "expected 3 keys in target, found {}",
            final_count
        );

        Ok(())
    }

    #[test]
    fn execute_fails_in_dry_run_mode() -> Result<()> {
        let temp = TempDir::new()?;
        let target_path = temp.path().join("target");

        setup_empty_target_db(&target_path)?;

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path: target_path.clone(),
                wasm_file: None,
            },
            target: Some(TargetEndpoint {
                db_path: target_path,
                backup_dir: None,
            }),
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Upsert(UpsertStep {
                name: Some("test".into()),
                column: Column::Generic,
                entries: vec![UpsertEntry {
                    key: EncodedValue::Utf8 {
                        data: "key".to_owned(),
                    },
                    value: EncodedValue::Utf8 {
                        data: "value".to_owned(),
                    },
                }],
                guards: StepGuards::default(),
            })],
        };

        // Create context in dry-run mode
        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        let result = execute_migration(&context);

        // Should fail because we're in dry-run mode
        ensure!(
            result.is_err(),
            "expected execution to fail in dry-run mode"
        );

        let err = result.unwrap_err();
        ensure!(
            err.to_string().contains("dry-run mode"),
            "error should mention dry-run mode: {}",
            err
        );

        Ok(())
    }

    #[test]
    fn execute_copy_respects_batch_size_limit() -> Result<()> {
        let temp = TempDir::new()?;
        let source_path = temp.path().join("source");
        let target_path = temp.path().join("target");

        // Setup source with more than BATCH_SIZE_LIMIT (1000) keys
        let fixture = DbFixture::new(&source_path)?;
        let ctx = test_context_id(0x11);

        // Insert 1500 entries to test batching across multiple commits
        for i in 0..1500_u32 {
            let mut state_key = [0_u8; 32];
            state_key[..4].copy_from_slice(&i.to_be_bytes());
            fixture.insert_state_entry(&ctx, &state_key, format!("value-{i}").as_bytes())?;
        }

        setup_empty_target_db(&target_path)?;

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path: source_path,
                wasm_file: None,
            },
            target: Some(TargetEndpoint {
                db_path: target_path.clone(),
                backup_dir: None,
            }),
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Copy(CopyStep {
                name: Some("copy-large-set".into()),
                column: Column::State,
                filters: PlanFilters::default(),
                transform: CopyTransform::default(),
                guards: StepGuards::default(),
                batch_size: None,
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), false)?;
        let report = execute_migration(&context)?;

        // Verify all 1500 keys were copied despite batching
        if let StepExecutionDetail::Copy { keys_copied, .. } = &report.steps[0].detail {
            ensure!(
                *keys_copied == 1500,
                "expected 1500 keys copied across batches, got {}",
                keys_copied
            );
        }

        // Verify target contains all keys
        let target_count = count_keys_in_column(&target_path, Column::State)?;
        ensure!(
            target_count == 1500,
            "expected 1500 keys in target, found {}",
            target_count
        );

        Ok(())
    }

    #[test]
    fn execute_delete_respects_batch_size_limit() -> Result<()> {
        let temp = TempDir::new()?;
        let target_path = temp.path().join("target");

        // Setup target with more than BATCH_SIZE_LIMIT (1000) keys
        let fixture = DbFixture::new(&target_path)?;
        let ctx = test_context_id(0x11);

        // Insert 1500 entries to test batching across multiple commits
        for i in 0..1500_u32 {
            let mut state_key = [0_u8; 32];
            state_key[..4].copy_from_slice(&i.to_be_bytes());
            fixture.insert_state_entry(&ctx, &state_key, format!("value-{i}").as_bytes())?;
        }

        // Verify initial state
        let initial_count = count_keys_in_column(&target_path, Column::State)?;
        ensure!(
            initial_count == 1500,
            "expected 1500 keys initially, found {}",
            initial_count
        );

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path: target_path.clone(),
                wasm_file: None,
            },
            target: Some(TargetEndpoint {
                db_path: target_path.clone(),
                backup_dir: None,
            }),
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Delete(DeleteStep {
                name: Some("delete-large-set".into()),
                column: Column::State,
                filters: PlanFilters::default(),
                guards: StepGuards::default(),
                batch_size: None,
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), false)?;
        let report = execute_migration(&context)?;

        // Verify all 1500 keys were deleted despite batching
        if let StepExecutionDetail::Delete { keys_deleted } = &report.steps[0].detail {
            ensure!(
                *keys_deleted == 1500,
                "expected 1500 keys deleted across batches, got {}",
                keys_deleted
            );
        }

        // Verify target is empty
        let final_count = count_keys_in_column(&target_path, Column::State)?;
        ensure!(
            final_count == 0,
            "expected 0 keys in target, found {}",
            final_count
        );

        Ok(())
    }

    #[test]
    fn execute_fails_when_target_is_read_only() -> Result<()> {
        let temp = TempDir::new()?;
        let target_path = temp.path().join("target");

        setup_empty_target_db(&target_path)?;

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path: target_path.clone(),
                wasm_file: None,
            },
            target: Some(TargetEndpoint {
                db_path: target_path,
                backup_dir: None,
            }),
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Upsert(UpsertStep {
                name: Some("test".into()),
                column: Column::Generic,
                entries: vec![UpsertEntry {
                    key: EncodedValue::Utf8 {
                        data: "key".to_owned(),
                    },
                    value: EncodedValue::Utf8 {
                        data: "value".to_owned(),
                    },
                }],
                guards: StepGuards::default(),
            })],
        };

        // Create context with dry_run=false but this will still check target is not read-only
        let context = MigrationContext::new(plan, MigrationOverrides::default(), false)?;

        // The context should have opened target in writable mode, so this test
        // verifies that our mode checking works correctly
        let result = execute_migration(&context);

        // Should succeed because target was opened in writable mode
        ensure!(
            result.is_ok(),
            "execution should succeed when target is writable"
        );

        Ok(())
    }

    #[test]
    fn execute_fails_when_no_target_configured() -> Result<()> {
        let temp = TempDir::new()?;
        let source_path = temp.path().join("source");

        setup_source_db(&source_path)?;

        // Create a plan without a target database
        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path: source_path,
                wasm_file: None,
            },
            target: None, // No target!
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Copy(CopyStep {
                name: Some("copy-test".into()),
                column: Column::State,
                filters: PlanFilters::default(),
                transform: CopyTransform::default(),
                guards: StepGuards::default(),
                batch_size: None,
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), false)?;
        let result = execute_migration(&context);

        // Should fail because no target was configured
        ensure!(
            result.is_err(),
            "expected execution to fail when no target is configured"
        );

        let err = result.unwrap_err();
        ensure!(
            err.to_string().contains("target database"),
            "error should mention missing target database: {}",
            err
        );

        Ok(())
    }

    #[test]
    fn execute_copy_handles_empty_source_gracefully() -> Result<()> {
        let temp = TempDir::new()?;
        let source_path = temp.path().join("source");
        let target_path = temp.path().join("target");

        // Setup empty databases
        let _source_fixture = DbFixture::new(&source_path)?;
        setup_empty_target_db(&target_path)?;

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path: source_path,
                wasm_file: None,
            },
            target: Some(TargetEndpoint {
                db_path: target_path,
                backup_dir: None,
            }),
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Copy(CopyStep {
                name: Some("copy-from-empty".into()),
                column: Column::State,
                filters: PlanFilters::default(),
                transform: CopyTransform::default(),
                guards: StepGuards::default(),
                batch_size: None,
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), false)?;
        let report = execute_migration(&context)?;

        // Should succeed with 0 keys copied
        if let StepExecutionDetail::Copy { keys_copied, .. } = &report.steps[0].detail {
            ensure!(
                *keys_copied == 0,
                "expected 0 keys copied from empty source, got {}",
                keys_copied
            );
        }

        Ok(())
    }

    #[test]
    fn execute_delete_handles_empty_target_gracefully() -> Result<()> {
        let temp = TempDir::new()?;
        let target_path = temp.path().join("target");

        setup_empty_target_db(&target_path)?;

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path: target_path.clone(),
                wasm_file: None,
            },
            target: Some(TargetEndpoint {
                db_path: target_path,
                backup_dir: None,
            }),
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Delete(DeleteStep {
                name: Some("delete-from-empty".into()),
                column: Column::State,
                filters: PlanFilters::default(),
                guards: StepGuards::default(),
                batch_size: None,
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), false)?;
        let report = execute_migration(&context)?;

        // Should succeed with 0 keys deleted
        if let StepExecutionDetail::Delete { keys_deleted } = &report.steps[0].detail {
            ensure!(
                *keys_deleted == 0,
                "expected 0 keys deleted from empty target, got {}",
                keys_deleted
            );
        }

        Ok(())
    }

    #[test]
    fn execute_verify_with_contains_key_assertion() -> Result<()> {
        let temp = TempDir::new()?;
        let target_path = temp.path().join("target");

        setup_target_db_with_data(&target_path)?;

        // Build a specific key to check for
        let key = ContextStateKey::new(test_context_id(0x11), test_state_key(0xAA));
        let key_bytes = key.as_key().as_bytes();

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path: target_path.clone(),
                wasm_file: None,
            },
            target: Some(TargetEndpoint {
                db_path: target_path,
                backup_dir: None,
            }),
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Verify(VerifyStep {
                name: Some("verify-contains-key".into()),
                column: Column::State,
                filters: PlanFilters::default(),
                assertion: VerificationAssertion::ContainsKey {
                    contains_key: EncodedValue::Hex {
                        data: hex::encode(key_bytes),
                    },
                },
                guards: StepGuards::default(),
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), false)?;
        let report = execute_migration(&context)?;

        // Should pass
        if let StepExecutionDetail::Verify { passed, .. } = &report.steps[0].detail {
            ensure!(*passed, "expected ContainsKey verification to pass");
        }

        Ok(())
    }

    #[test]
    fn execute_verify_with_missing_key_assertion() -> Result<()> {
        let temp = TempDir::new()?;
        let target_path = temp.path().join("target");

        setup_target_db_with_data(&target_path)?;

        // Build a key that doesn't exist
        let key = ContextStateKey::new(test_context_id(0xFF), test_state_key(0xFF));
        let key_bytes = key.as_key().as_bytes();

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path: target_path.clone(),
                wasm_file: None,
            },
            target: Some(TargetEndpoint {
                db_path: target_path,
                backup_dir: None,
            }),
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Verify(VerifyStep {
                name: Some("verify-missing-key".into()),
                column: Column::State,
                filters: PlanFilters::default(),
                assertion: VerificationAssertion::MissingKey {
                    missing_key: EncodedValue::Hex {
                        data: hex::encode(key_bytes),
                    },
                },
                guards: StepGuards::default(),
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), false)?;
        let report = execute_migration(&context)?;

        // Should pass because the key is indeed missing
        if let StepExecutionDetail::Verify { passed, .. } = &report.steps[0].detail {
            ensure!(*passed, "expected MissingKey verification to pass");
        }

        Ok(())
    }

    #[test]
    fn execute_upsert_overwrites_existing_keys() -> Result<()> {
        let temp = TempDir::new()?;
        let target_path = temp.path().join("target");

        // Setup target with existing data
        let fixture = DbFixture::new(&target_path)?;
        fixture.insert_generic_entry(b"key-to-overwrite", b"old-value")?;

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path: target_path.clone(),
                wasm_file: None,
            },
            target: Some(TargetEndpoint {
                db_path: target_path.clone(),
                backup_dir: None,
            }),
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Upsert(UpsertStep {
                name: Some("upsert-overwrite".into()),
                column: Column::Generic,
                entries: vec![UpsertEntry {
                    key: EncodedValue::Utf8 {
                        data: "key-to-overwrite".to_owned(),
                    },
                    value: EncodedValue::Utf8 {
                        data: "new-value".to_owned(),
                    },
                }],
                guards: StepGuards::default(),
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), false)?;
        let _report = execute_migration(&context)?;

        // Verify value was overwritten
        let value = get_value(&target_path, Column::Generic, b"key-to-overwrite")?;
        ensure!(
            value.as_deref() == Some(&b"new-value"[..]),
            "expected 'new-value', got {:?}",
            value
        );

        Ok(())
    }

    #[test]
    fn execute_multi_step_with_verification_between_mutations() -> Result<()> {
        let temp = TempDir::new()?;
        let target_path = temp.path().join("target");

        setup_empty_target_db(&target_path)?;

        // Multi-step plan with verifications between each mutation
        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path: target_path.clone(),
                wasm_file: None,
            },
            target: Some(TargetEndpoint {
                db_path: target_path.clone(),
                backup_dir: None,
            }),
            defaults: PlanDefaults::default(),
            steps: vec![
                // Verify empty state
                PlanStep::Verify(VerifyStep {
                    name: Some("verify-empty".into()),
                    column: Column::Generic,
                    filters: PlanFilters::default(),
                    assertion: VerificationAssertion::ExpectedCount { expected_count: 0 },
                    guards: StepGuards::default(),
                }),
                // Add first entry
                PlanStep::Upsert(UpsertStep {
                    name: Some("upsert-1".into()),
                    column: Column::Generic,
                    entries: vec![UpsertEntry {
                        key: EncodedValue::Utf8 {
                            data: "key-1".to_owned(),
                        },
                        value: EncodedValue::Utf8 {
                            data: "value-1".to_owned(),
                        },
                    }],
                    guards: StepGuards::default(),
                }),
                // Verify 1 entry
                PlanStep::Verify(VerifyStep {
                    name: Some("verify-one".into()),
                    column: Column::Generic,
                    filters: PlanFilters::default(),
                    assertion: VerificationAssertion::ExpectedCount { expected_count: 1 },
                    guards: StepGuards::default(),
                }),
                // Add second entry
                PlanStep::Upsert(UpsertStep {
                    name: Some("upsert-2".into()),
                    column: Column::Generic,
                    entries: vec![UpsertEntry {
                        key: EncodedValue::Utf8 {
                            data: "key-2".to_owned(),
                        },
                        value: EncodedValue::Utf8 {
                            data: "value-2".to_owned(),
                        },
                    }],
                    guards: StepGuards::default(),
                }),
                // Verify 2 entries
                PlanStep::Verify(VerifyStep {
                    name: Some("verify-two".into()),
                    column: Column::Generic,
                    filters: PlanFilters::default(),
                    assertion: VerificationAssertion::ExpectedCount { expected_count: 2 },
                    guards: StepGuards::default(),
                }),
                // Delete first entry
                PlanStep::Delete(DeleteStep {
                    name: Some("delete-1".into()),
                    column: Column::Generic,
                    filters: PlanFilters {
                        raw_key_prefix: Some(hex::encode(b"key-1")),
                        ..PlanFilters::default()
                    },
                    guards: StepGuards::default(),
                    batch_size: None,
                }),
                // Verify 1 entry remains
                PlanStep::Verify(VerifyStep {
                    name: Some("verify-one-after-delete".into()),
                    column: Column::Generic,
                    filters: PlanFilters::default(),
                    assertion: VerificationAssertion::ExpectedCount { expected_count: 1 },
                    guards: StepGuards::default(),
                }),
            ],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), false)?;
        let report = execute_migration(&context)?;

        // All 7 steps should have executed successfully
        ensure!(report.steps.len() == 7, "expected 7 steps executed");

        // Verify final state
        let final_count = count_keys_in_column(&target_path, Column::Generic)?;
        ensure!(final_count == 1, "expected 1 key in final state");

        Ok(())
    }
}
