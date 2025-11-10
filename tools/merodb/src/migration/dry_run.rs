//!
//! Dry-run engine for the `migrate` command.
//!
//! The goal of this module is to preview a migration plan without mutating RocksDB. It wires the
//! plan, the source database handle, and optional ABI manifest together to produce a
//! per-step summary containing:
//!
//! - **Resolved filters** – merge plan defaults with step overrides, interpret context IDs,
//!   raw key prefixes, alias names, etc., and emit warnings when a filter is not yet supported
//!   (e.g. alias resolution).
//! - **Matched key counts & samples** – iterate the relevant RocksDB column, apply the resolved
//!   filters, keep a running count, and capture a few representative keys (rendered via
//!   `types::parse_key`) so users can sanity-check the scope.
//! - **Step detail** – annotate each step with extra information (copy vs delete vs upsert vs verify).
//!   For copy steps we note whether ABI decoding was requested and available; for verify steps we
//!   evaluate the assertion immediately and display pass/fail state.
//! - **Warnings** – surface anything that might surprise the user (missing ABI while
//!   `decode_with_abi` is true, hex decoding failures, unsupported filters, etc.).
//!
//! The CLI consumes the `DryRunReport` and prints a human-readable preview. A future iteration can
//! serialize the same data structure as JSON for `--report` output.

use eyre::{eyre, Result, WrapErr};
use rocksdb::{DBWithThreadMode, IteratorMode, SingleThreaded};
use serde::Serialize;

use calimero_wasm_abi::schema::Manifest;

use crate::types;
use crate::types::Column;

use super::context::MigrationContext;
use super::filters::ResolvedFilters;
use super::plan::{CopyStep, DeleteStep, PlanDefaults, PlanStep, UpsertStep, VerifyStep};
use super::verification::evaluate_assertion;

const SAMPLE_LIMIT: usize = 3;

/// Aggregated dry-run information for each step in the migration plan.
#[derive(Debug, Serialize)]
pub struct DryRunReport {
    pub steps: Vec<StepReport>,
}

/// Per-step dry-run preview including key counts, sample data, and warnings.
#[derive(Debug, Serialize)]
pub struct StepReport {
    pub index: usize,
    pub matched_keys: usize,
    pub filters_summary: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub detail: StepDetail,
}

/// Additional information that depends on the step type.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepDetail {
    Copy {
        decode_with_abi: bool,
    },
    Delete,
    Upsert {
        entries: usize,
    },
    Verify {
        summary: String,
        passed: Option<bool>,
    },
}

impl StepDetail {}

/// Generate a comprehensive dry-run report for all migration steps without modifying the database.
///
/// This function orchestrates the preview process by:
/// 1. Extracting the migration plan, source database, and optional ABI manifest from context
/// 2. Iterating through each step in the plan sequentially
/// 3. Dispatching each step to its type-specific preview handler
/// 4. Aggregating individual step reports into a complete migration preview
///
/// The dry-run report allows users to:
/// - Verify filter logic and scope before applying destructive changes
/// - Review sample keys that will be affected by each operation
/// - Check verification assertions to predict migration success
/// - Identify potential issues through accumulated warnings
///
/// # Arguments
///
/// * `context` - Migration context containing the plan, source database, and ABI manifest
///
/// # Returns
///
/// A `DryRunReport` containing preview information for all steps in execution order.
///
/// # Errors
///
/// Returns an error if:
/// - Database column families cannot be accessed during scanning
/// - Key iteration fails due to I/O or corruption issues
/// - Verification assertion evaluation encounters database errors
pub fn generate_report(context: &MigrationContext) -> Result<DryRunReport> {
    let plan = context.plan();
    let source_db = context.source().db();
    let target_db = context.target().map(|target| target.db());
    let abi_manifest = context.source().abi_manifest()?;

    let mut steps = Vec::with_capacity(plan.steps.len());

    // Process each step in order, building preview reports
    for (index, step) in plan.steps.iter().enumerate() {
        let report = match step {
            PlanStep::Copy(copy) => {
                preview_copy_step(index, copy, &plan.defaults, source_db, abi_manifest)?
            }
            PlanStep::Delete(delete) => {
                let target_db = target_db.ok_or_else(|| {
                    eyre!(
                        "Step {} ({} column '{}') requires a target database. Provide --target-db or configure target.db_path in the plan.",
                        index + 1,
                        step.kind(),
                        step.column().as_str()
                    )
                })?;

                preview_delete_step(index, delete, &plan.defaults, target_db)?
            }
            PlanStep::Upsert(upsert) => preview_upsert_step(index, upsert, &plan.defaults),
            PlanStep::Verify(verify) => {
                let target_db = target_db.ok_or_else(|| {
                    eyre!(
                        "Step {} ({} column '{}') requires a target database. Provide --target-db or configure target.db_path in the plan.",
                        index + 1,
                        step.kind(),
                        step.column().as_str()
                    )
                })?;

                preview_verify_step(index, verify, &plan.defaults, target_db)?
            }
        };

        steps.push(report);
    }

    Ok(DryRunReport { steps })
}

/// Preview a copy operation by scanning the source column and reporting matched keys.
///
/// This function simulates a copy step without performing any writes. It:
/// 1. Merges plan defaults with step-specific filters to determine the final filter set
/// 2. Resolves filters into byte-oriented predicates (context IDs, prefixes, ranges, etc.)
/// 3. Validates ABI decoding configuration and warns if the manifest is missing
/// 4. Scans the source column to count matching keys and capture representative samples
/// 5. Assembles a detailed report including match counts, samples, and any warnings
///
/// The preview helps users verify:
/// - Filter logic correctly targets the intended key subset
/// - Sample keys represent the expected data
/// - ABI decoding requirements are satisfied
/// - No unexpected warnings indicate configuration issues
///
/// # Arguments
///
/// * `index` - Zero-based position of this step in the migration plan
/// * `step` - The copy step configuration from the plan
/// * `defaults` - Plan-level defaults that may override step settings
/// * `db` - Source database handle for reading keys
/// * `abi_manifest` - Optional ABI manifest for validating decode_with_abi requests
///
/// # Returns
///
/// A `StepReport` containing match counts, sample keys, filter summary, and warnings.
///
/// # Errors
///
/// Returns an error if the column family doesn't exist or key iteration fails.
fn preview_copy_step(
    index: usize,
    step: &CopyStep,
    defaults: &PlanDefaults,
    db: &DBWithThreadMode<SingleThreaded>,
    abi_manifest: Option<&Manifest>,
) -> Result<StepReport> {
    // Merge plan-level and step-level filters to get the effective filter set
    let filters = defaults.merge_filters(&step.filters);
    let mut resolved = ResolvedFilters::resolve(step.column, &filters);

    // Check if ABI decoding is requested and validate manifest availability
    let decode_with_abi = defaults.effective_decode_with_abi(step.transform.decode_with_abi);
    if decode_with_abi && abi_manifest.is_none() {
        resolved
            .warnings
            .push("decode_with_abi requested but source ABI manifest is unavailable".into());
    }

    // Scan the source column to count matches and collect sample keys
    let scan = scan_column(db, step.column, &resolved)?;

    let detail = StepDetail::Copy { decode_with_abi };

    Ok(StepReport {
        index,
        matched_keys: scan.matched,
        filters_summary: filters.summary(),
        samples: scan.samples,
        warnings: resolved.warnings,
        detail,
    })
}

/// Preview a delete operation by identifying which keys would be removed.
///
/// This function simulates a delete step without performing any removals. The preview
/// process is similar to copy steps but without ABI validation since delete operations
/// only need to identify keys, not decode their values. It:
/// 1. Merges plan defaults with step-specific filters
/// 2. Resolves filters into concrete byte predicates
/// 3. Scans the target column to count and sample keys that match deletion criteria
/// 4. Reports the scope of deletion with warnings about any filter resolution issues
///
/// Users can use this preview to:
/// - Verify deletion scope matches expectations before applying destructive changes
/// - Review sample keys to ensure no unintended data would be deleted
/// - Catch filter configuration errors early
///
/// # Arguments
///
/// * `index` - Zero-based position of this step in the migration plan
/// * `step` - The delete step configuration from the plan
/// * `defaults` - Plan-level defaults that may override step settings
/// * `db` - Database handle for scanning keys (source or target depending on context)
///
/// # Returns
///
/// A `StepReport` containing the count of keys to delete, sample keys, and warnings.
///
/// # Errors
///
/// Returns an error if the column family doesn't exist or key iteration fails.
fn preview_delete_step(
    index: usize,
    step: &DeleteStep,
    defaults: &PlanDefaults,
    db: &DBWithThreadMode<SingleThreaded>,
) -> Result<StepReport> {
    // Merge and resolve filters to determine deletion scope
    let filters = defaults.merge_filters(&step.filters);
    let resolved = ResolvedFilters::resolve(step.column, &filters);

    // Scan the column to identify keys that would be deleted
    let scan = scan_column(db, step.column, &resolved)?;

    Ok(StepReport {
        index,
        matched_keys: scan.matched,
        filters_summary: filters.summary(),
        samples: scan.samples,
        warnings: resolved.warnings,
        detail: StepDetail::Delete,
    })
}

/// Preview an upsert operation by summarizing the literal key-value entries to be written.
///
/// Unlike copy and delete steps which scan the database, upsert steps contain explicit
/// key-value pairs defined in the migration plan. This function simply summarizes those
/// entries without any database access. It:
/// 1. Counts the total number of entries to be inserted/updated
/// 2. Generates preview strings for up to SAMPLE_LIMIT entries showing keys and values
/// 3. Returns a report with no filter summary or warnings (since no scanning occurs)
///
/// The preview helps users:
/// - Verify the exact keys and values that will be written
/// - Confirm the number of entries matches expectations
/// - Spot any encoding or formatting issues in the plan data
///
/// # Arguments
///
/// * `index` - Zero-based position of this step in the migration plan
/// * `step` - The upsert step configuration containing literal entries
/// * `_defaults` - Plan-level defaults (unused for upsert but kept for consistency)
///
/// # Returns
///
/// A `StepReport` with entry count, sample previews, and no warnings or filter summary.
fn preview_upsert_step(index: usize, step: &UpsertStep, _defaults: &PlanDefaults) -> StepReport {
    let mut samples = Vec::new();

    // Collect sample previews for the first few entries
    for entry in step.entries.iter().take(SAMPLE_LIMIT) {
        samples.push(format!(
            "key={} value={}",
            entry.key.preview(16),
            entry.value.preview(32)
        ));
    }

    StepReport {
        index,
        matched_keys: step.entries.len(),
        filters_summary: None,
        samples,
        warnings: Vec::new(),
        detail: StepDetail::Upsert {
            entries: step.entries.len(),
        },
    }
}

/// Preview a verification step by evaluating its assertion against the current database state.
///
/// Verification steps allow migration plans to assert preconditions or postconditions
/// about database state. During dry-run, these assertions are evaluated immediately to
/// predict whether the migration would succeed. This function:
/// 1. Merges and resolves filters to identify the key set for verification
/// 2. Scans the database to count matching keys and collect samples
/// 3. Evaluates the assertion (expected count, min/max count, key presence/absence)
/// 4. Reports pass/fail status with a human-readable summary
/// 5. Accumulates warnings from both filter resolution and assertion evaluation
///
/// This preview is crucial for:
/// - Identifying verification failures before applying any changes
/// - Understanding why an assertion might fail based on current data
/// - Validating that filters correctly scope the verification check
///
/// # Arguments
///
/// * `index` - Zero-based position of this step in the migration plan
/// * `step` - The verify step configuration with assertion and filters
/// * `defaults` - Plan-level defaults that may override step settings
/// * `db` - Database handle for counting keys and checking assertions
///
/// # Returns
///
/// A `StepReport` with match counts, assertion summary, pass/fail status, and warnings.
///
/// # Errors
///
/// Returns an error if:
/// - The column family doesn't exist
/// - Key iteration fails during scanning
/// - Assertion evaluation encounters database access errors
fn preview_verify_step(
    index: usize,
    step: &VerifyStep,
    defaults: &PlanDefaults,
    db: &DBWithThreadMode<SingleThreaded>,
) -> Result<StepReport> {
    // Merge and resolve filters to determine verification scope
    let filters = defaults.merge_filters(&step.filters);
    let mut resolved = ResolvedFilters::resolve(step.column, &filters);

    // Scan the column to count matching keys for assertion evaluation
    let scan = scan_column(db, step.column, &resolved)?;

    // Evaluate the assertion against the actual match count
    let outcome = evaluate_assertion(db, step.column, &step.assertion, scan.matched)?;
    resolved.warnings.extend(outcome.warnings);

    Ok(StepReport {
        index,
        matched_keys: scan.matched,
        filters_summary: filters.summary(),
        samples: scan.samples,
        warnings: resolved.warnings,
        detail: StepDetail::Verify {
            summary: outcome.summary,
            passed: outcome.passed,
        },
    })
}

/// Compact scan result containing match count and representative sample keys.
///
/// This struct provides a lightweight summary of a column scan operation without
/// storing all matched keys in memory. It's designed for dry-run previews where
/// users need:
/// - An accurate count of how many keys match the filter criteria
/// - A small set of sample keys to verify the filter logic is correct
struct ScanResult {
    /// Total number of keys that matched the filter predicates
    matched: usize,
    /// Up to SAMPLE_LIMIT representative keys, formatted for display
    samples: Vec<String>,
}

/// Scan a database column family and count/sample keys matching the resolved filters.
///
/// This is the core scanning function used by all dry-run step previews. It iterates
/// through an entire column family from start to finish, applying filter predicates
/// to each key. The function:
/// 1. Obtains a handle to the specified column family
/// 2. Creates a forward iterator starting from the first key
/// 3. Tests each key against all resolved filter predicates (AND logic)
/// 4. Increments the match counter for passing keys (with saturating arithmetic)
/// 5. Captures up to SAMPLE_LIMIT sample keys for preview display
/// 6. Formats sample keys using structured parsing or hex fallback
///
/// # Performance Considerations
///
/// This function performs a full table scan, which may be slow for large column families.
/// However, it's necessary to provide accurate match counts and representative samples.
/// The SAMPLE_LIMIT prevents unbounded memory growth during sampling.
///
/// # Arguments
///
/// * `db` - Database handle with access to all column families
/// * `column` - The specific column family to scan
/// * `filters` - Resolved filter predicates to apply to each key
///
/// # Returns
///
/// A `ScanResult` with the total match count and sample keys for display.
///
/// # Errors
///
/// Returns an error if:
/// - The specified column family doesn't exist in the database
/// - Key iteration fails due to I/O errors or database corruption
fn scan_column(
    db: &DBWithThreadMode<SingleThreaded>,
    column: Column,
    filters: &ResolvedFilters,
) -> Result<ScanResult> {
    // Obtain a handle to the target column family
    let cf = db
        .cf_handle(column.as_str())
        .ok_or_else(|| eyre::eyre!("Column family '{}' not found", column.as_str()))?;

    let mut matched: usize = 0;
    let mut samples = Vec::new();

    // Iterate through all keys in the column family from the beginning
    let iter = db.iterator_cf(cf, IteratorMode::Start);
    for item in iter {
        let (key, _value) = item.wrap_err_with(|| {
            format!(
                "Failed to iterate column family '{}' during dry-run",
                column.as_str()
            )
        })?;

        // Apply all filter predicates (AND logic) to determine if this key matches
        if filters.matches(column, &key) {
            // Use saturating addition to prevent overflow on large datasets
            matched = matched.saturating_add(1);

            // Collect sample keys up to the limit for preview display
            if samples.len() < SAMPLE_LIMIT {
                samples.push(sample_from_key(column, &key));
            }
        }
    }

    Ok(ScanResult { matched, samples })
}

/// Format a database key for human-readable display in dry-run previews.
///
/// This function attempts to parse keys using the column-specific structured format
/// (e.g., extracting context IDs, state keys, alias names) and serializes the result
/// as JSON for readability. If parsing fails (malformed keys, short keys, etc.), it
/// falls back to displaying the raw hex representation.
///
/// # Key Parsing Strategy
///
/// 1. **First attempt**: Use `types::parse_key()` to decode the key structure
///    - Success: Serialize the parsed structure as JSON
///    - JSON serialization failure: Fall back to hex
/// 2. **Parse failure**: Display as `raw_hex=<hex_string>`
///
/// This dual approach ensures that:
/// - Well-formed keys are displayed in a structured, understandable format
/// - Malformed or unexpected keys are still visible for debugging
/// - All keys have a displayable representation (no data loss)
///
/// # Arguments
///
/// * `column` - The column family context for parsing key structure
/// * `key` - Raw key bytes to format
///
/// # Returns
///
/// A human-readable string representation, either as JSON or hex-encoded bytes.
fn sample_from_key(column: Column, key: &[u8]) -> String {
    types::parse_key(column, key).map_or_else(
        |_| format!("raw_hex={}", hex::encode(key)),
        |value| {
            serde_json::to_string(&value)
                .unwrap_or_else(|_| format!("raw_hex={}", hex::encode(key)))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::context::{MigrationContext, MigrationOverrides};
    use crate::migration::plan::{
        CopyStep, CopyTransform, DeleteStep, EncodedValue, KeyRange, MigrationPlan, PlanDefaults,
        PlanFilters, PlanStep, PlanVersion, SourceEndpoint, StepGuards, TargetEndpoint,
        UpsertEntry, UpsertStep, VerificationAssertion, VerifyStep,
    };
    use crate::migration::test_utils::{test_context_id, test_context_meta, DbFixture};
    use crate::types::Column;
    use eyre::ensure;
    use rocksdb::{ColumnFamilyDescriptor, Options, WriteBatch, DB};
    use std::path::Path;
    use tempfile::TempDir;

    /// Create a RocksDB instance with all column families and insert a single state row.
    fn setup_db(path: &Path) -> Result<()> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);

        let descriptors: Vec<_> = Column::all()
            .iter()
            .map(|column| ColumnFamilyDescriptor::new(column.as_str(), Options::default()))
            .collect();

        let db = DB::open_cf_descriptors(&opts, path, descriptors)?;

        let cf_state = db.cf_handle(Column::State.as_str()).unwrap();

        // Create a single State entry with 64-byte key structure
        let mut state_key = [0_u8; 64];
        state_key[..32].copy_from_slice(&[0x11; 32]); // Context ID: 0x1111...1111
        state_key[32..64].copy_from_slice(&[0x22; 32]); // State key: 0x2222...2222

        let mut batch = WriteBatch::default();
        batch.put_cf(cf_state, state_key, b"value-1");
        db.write(batch)?;

        drop(db);
        Ok(())
    }

    fn target_config(path: &Path) -> TargetEndpoint {
        TargetEndpoint {
            db_path: path.to_path_buf(),
            backup_dir: None,
        }
    }

    /// Build a minimal two-step plan (copy + verify) scoped to the synthetic state row we insert.
    fn basic_plan(path: &Path) -> MigrationPlan {
        MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path: path.to_path_buf(),
                wasm_file: None,
            },
            target: Some(target_config(path)),
            defaults: PlanDefaults {
                columns: Vec::new(),
                filters: PlanFilters::default(),
                decode_with_abi: Some(false),
                write_if_missing: Some(false),
                batch_size: None,
            },
            steps: vec![
                PlanStep::Copy(CopyStep {
                    name: Some("copy-state".into()),
                    column: Column::State,
                    filters: PlanFilters {
                        context_ids: vec![hex::encode([0x11; 32])],
                        ..PlanFilters::default()
                    },
                    transform: CopyTransform::default(),
                    guards: StepGuards::default(),
                    batch_size: None,
                }),
                PlanStep::Verify(VerifyStep {
                    name: Some("expect-one".into()),
                    column: Column::State,
                    filters: PlanFilters {
                        context_ids: vec![hex::encode([0x11; 32])],
                        ..PlanFilters::default()
                    },
                    assertion: VerificationAssertion::ExpectedCount { expected_count: 1 },
                    guards: StepGuards::default(),
                }),
            ],
        }
    }

    #[test]
    /// End-to-end smoke test: run the dry-run engine against the synthetic DB and ensure the
    /// copy step and verify step both report the expected counts and statuses.
    fn dry_run_reports_copy_and_verify() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        // Seed RocksDB with a single entry so the plan has a deterministic target.
        setup_db(&db_path)?;

        let plan = basic_plan(&db_path);
        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;

        let report = generate_report(&context)?;
        // Plan contains exactly two steps (copy + verify).
        ensure!(
            report.steps.len() == 2,
            "expected two steps, found {}",
            report.steps.len()
        );

        let copy = &report.steps[0];
        // Dry-run preview should classify the first step as copy and report one matched key.
        ensure!(
            matches!(copy.detail, StepDetail::Copy { .. }),
            "expected copy detail"
        );
        ensure!(
            copy.matched_keys == 1,
            "expected 1 matched key, got {}",
            copy.matched_keys
        );
        ensure!(
            !copy.samples.is_empty(),
            "expected at least one sample for copy step"
        );

        let verify = &report.steps[1];
        // Second step is the verification assertion; it should also match the single seeded key.
        ensure!(
            matches!(verify.detail, StepDetail::Verify { .. }),
            "expected verify detail"
        );
        ensure!(
            verify.matched_keys == 1,
            "expected 1 matched key in verify step, got {}",
            verify.matched_keys
        );
        match &verify.detail {
            StepDetail::Verify { passed, .. } => ensure!(
                passed == &Some(true),
                "expected verification to pass, got {:?}",
                passed
            ),
            other => return Err(eyre::eyre!("expected verify detail, found {other:?}")),
        }

        Ok(())
    }

    #[test]
    /// Test dry-run behavior for delete steps.
    fn dry_run_reports_delete_step() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        setup_db(&db_path)?;

        let target = target_config(&db_path);

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path,
                wasm_file: None,
            },
            target: Some(target),
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Delete(DeleteStep {
                name: Some("delete-test".into()),
                column: Column::State,
                filters: PlanFilters {
                    context_ids: vec![hex::encode([0x11; 32])],
                    ..PlanFilters::default()
                },
                guards: StepGuards::default(),
                batch_size: None,
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        let report = generate_report(&context)?;

        ensure!(report.steps.len() == 1, "expected one step");

        let delete = &report.steps[0];
        ensure!(
            matches!(delete.detail, StepDetail::Delete),
            "expected delete detail"
        );
        ensure!(
            delete.matched_keys == 1,
            "expected 1 matched key, got {}",
            delete.matched_keys
        );

        Ok(())
    }

    #[test]
    fn dry_run_delete_without_target_errors() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        setup_db(&db_path)?;
        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path,
                wasm_file: None,
            },
            target: None,
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Delete(DeleteStep {
                name: Some("delete-test".into()),
                column: Column::State,
                filters: PlanFilters::default(),
                guards: StepGuards::default(),
                batch_size: None,
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        ensure!(
            context.target().is_none(),
            "expected context without target"
        );
        let error = generate_report(&context).expect_err("expected missing target error");
        ensure!(
            error.to_string().contains("requires a target database"),
            "expected error about missing target, got: {}",
            error
        );

        Ok(())
    }

    #[test]
    /// Test dry-run behavior for upsert steps.
    fn dry_run_reports_upsert_step() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        setup_db(&db_path)?;

        let target = target_config(&db_path);

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path,
                wasm_file: None,
            },
            target: Some(target),
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Upsert(UpsertStep {
                name: Some("upsert-test".into()),
                column: Column::Generic,
                entries: vec![
                    UpsertEntry {
                        key: EncodedValue::Hex {
                            data: "aabbcc".into(),
                        },
                        value: EncodedValue::Utf8 {
                            data: "test-value-1".into(),
                        },
                    },
                    UpsertEntry {
                        key: EncodedValue::Hex {
                            data: "ddeeff".into(),
                        },
                        value: EncodedValue::Utf8 {
                            data: "test-value-2".into(),
                        },
                    },
                ],
                guards: StepGuards::default(),
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        let report = generate_report(&context)?;

        ensure!(report.steps.len() == 1, "expected one step");

        let upsert = &report.steps[0];
        match &upsert.detail {
            StepDetail::Upsert { entries } => {
                ensure!(*entries == 2, "expected 2 entries, got {}", entries);
            }
            other => return Err(eyre::eyre!("expected upsert detail, found {other:?}")),
        }

        Ok(())
    }

    #[test]
    /// Test filter resolution with multiple context IDs.
    fn dry_run_filters_multiple_contexts() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        // Create DB with entries for two different contexts
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);

        let descriptors: Vec<_> = Column::all()
            .iter()
            .map(|column| ColumnFamilyDescriptor::new(column.as_str(), Options::default()))
            .collect();

        let db = DB::open_cf_descriptors(&opts, &db_path, descriptors)?;
        let cf_state = db.cf_handle(Column::State.as_str()).unwrap();

        let mut batch = WriteBatch::default();

        // Context 0x11 - 2 entries
        // State keys are 64 bytes: first 32 bytes = context ID, last 32 bytes = state key
        let mut key1 = [0_u8; 64];
        key1[..32].copy_from_slice(&[0x11; 32]); // Context ID: 0x1111...1111
        key1[32..64].copy_from_slice(&[0x22; 32]); // State key: 0x2222...2222
        batch.put_cf(cf_state, key1, b"value-1");

        let mut key2 = [0_u8; 64];
        key2[..32].copy_from_slice(&[0x11; 32]); // Context ID: 0x1111...1111 (same context)
        key2[32..64].copy_from_slice(&[0x33; 32]); // State key: 0x3333...3333
        batch.put_cf(cf_state, key2, b"value-2");

        // Context 0xAA - 1 entry
        let mut key3 = [0_u8; 64];
        key3[..32].copy_from_slice(&[0xAA; 32]); // Context ID: 0xAAAA...AAAA (different context)
        key3[32..64].copy_from_slice(&[0xBB; 32]); // State key: 0xBBBB...BBBB
        batch.put_cf(cf_state, key3, b"value-3");

        db.write(batch)?;
        drop(db);

        // Plan that only targets context 0x11
        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path,
                wasm_file: None,
            },
            target: None,
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Copy(CopyStep {
                name: Some("copy-filtered".into()),
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

        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        let report = generate_report(&context)?;

        ensure!(report.steps.len() == 1, "expected one step");
        let copy = &report.steps[0];
        ensure!(
            copy.matched_keys == 2,
            "expected 2 matched keys for context 0x11, got {}",
            copy.matched_keys
        );

        Ok(())
    }

    #[test]
    /// Test verification with min_count assertion.
    fn dry_run_verify_min_count() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        setup_db(&db_path)?;

        let target = target_config(&db_path);

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path,
                wasm_file: None,
            },
            target: Some(target),
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Verify(VerifyStep {
                name: Some("min-count-check".into()),
                column: Column::State,
                filters: PlanFilters {
                    context_ids: vec![hex::encode([0x11; 32])],
                    ..PlanFilters::default()
                },
                assertion: VerificationAssertion::MinCount { min_count: 1 },
                guards: StepGuards::default(),
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        let report = generate_report(&context)?;

        ensure!(report.steps.len() == 1, "expected one step");
        let verify = &report.steps[0];

        match &verify.detail {
            StepDetail::Verify { passed, .. } => {
                ensure!(
                    passed == &Some(true),
                    "expected min_count verification to pass"
                );
            }
            other => return Err(eyre::eyre!("expected verify detail, found {other:?}")),
        }

        Ok(())
    }

    #[test]
    /// Test verification with max_count assertion that should fail.
    fn dry_run_verify_max_count_fails() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        setup_db(&db_path)?;

        let target = target_config(&db_path);

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path,
                wasm_file: None,
            },
            target: Some(target),
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Verify(VerifyStep {
                name: Some("max-count-check".into()),
                column: Column::State,
                filters: PlanFilters {
                    context_ids: vec![hex::encode([0x11; 32])],
                    ..PlanFilters::default()
                },
                assertion: VerificationAssertion::MaxCount { max_count: 0 },
                guards: StepGuards::default(),
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        let report = generate_report(&context)?;

        ensure!(report.steps.len() == 1, "expected one step");
        let verify = &report.steps[0];

        match &verify.detail {
            StepDetail::Verify { passed, .. } => {
                ensure!(
                    passed == &Some(false),
                    "expected max_count verification to fail"
                );
            }
            other => return Err(eyre::eyre!("expected verify detail, found {other:?}")),
        }

        Ok(())
    }

    #[test]
    /// Test that raw_key_prefix filter works correctly.
    fn dry_run_filters_raw_key_prefix() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        // Create DB with multiple entries having different key prefixes
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);

        let descriptors: Vec<_> = Column::all()
            .iter()
            .map(|column| ColumnFamilyDescriptor::new(column.as_str(), Options::default()))
            .collect();

        let db = DB::open_cf_descriptors(&opts, &db_path, descriptors)?;
        let cf_generic = db.cf_handle(Column::Generic.as_str()).unwrap();

        let mut batch = WriteBatch::default();
        batch.put_cf(cf_generic, b"prefix_aaa", b"value-1");
        batch.put_cf(cf_generic, b"prefix_bbb", b"value-2");
        batch.put_cf(cf_generic, b"other_ccc", b"value-3");
        db.write(batch)?;
        drop(db);

        // Plan that filters by raw_key_prefix
        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path,
                wasm_file: None,
            },
            target: None,
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Copy(CopyStep {
                name: Some("copy-with-prefix".into()),
                column: Column::Generic,
                filters: PlanFilters {
                    raw_key_prefix: Some(hex::encode(b"prefix_")),
                    ..PlanFilters::default()
                },
                transform: CopyTransform::default(),
                guards: StepGuards::default(),
                batch_size: None,
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        let report = generate_report(&context)?;

        ensure!(report.steps.len() == 1, "expected one step");
        let copy = &report.steps[0];
        ensure!(
            copy.matched_keys == 2,
            "expected 2 matched keys with prefix, got {}",
            copy.matched_keys
        );

        Ok(())
    }

    #[test]
    /// Test that requesting ABI decoding without providing an ABI manifest emits a warning.
    fn dry_run_warns_missing_abi_when_decode_requested() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        setup_db(&db_path)?;

        // Plan requests decode_with_abi but no wasm_file is provided
        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path,
                wasm_file: None, // No ABI manifest
            },
            target: None,
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Copy(CopyStep {
                name: Some("copy-with-abi".into()),
                column: Column::State,
                filters: PlanFilters {
                    context_ids: vec![hex::encode([0x11; 32])],
                    ..PlanFilters::default()
                },
                transform: CopyTransform {
                    decode_with_abi: Some(true), // Request ABI decoding
                    jq: None,
                },
                guards: StepGuards::default(),
                batch_size: None,
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        let report = generate_report(&context)?;

        ensure!(report.steps.len() == 1, "expected one step");
        let copy = &report.steps[0];

        // Should emit a warning about missing ABI manifest
        ensure!(
            !copy.warnings.is_empty(),
            "expected warnings about missing ABI"
        );
        ensure!(
            copy.warnings.iter().any(|w| w.contains("ABI manifest")),
            "expected warning to mention ABI manifest, got: {:?}",
            copy.warnings
        );

        Ok(())
    }

    #[test]
    /// Test that an invalid JQ transform is caught during plan validation.
    fn plan_validation_rejects_empty_jq_transform() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        setup_db(&db_path)?;

        // Plan with an empty JQ transform (should fail validation)
        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path,
                wasm_file: None,
            },
            target: None,
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Copy(CopyStep {
                name: Some("copy-with-invalid-jq".into()),
                column: Column::State,
                filters: PlanFilters::default(),
                transform: CopyTransform {
                    decode_with_abi: None,
                    jq: Some("   ".into()), // Empty/whitespace-only JQ expression
                },
                guards: StepGuards::default(),
                batch_size: None,
            })],
        };

        // Validation should fail for empty JQ transform
        let result = plan.validate();
        ensure!(
            result.is_err(),
            "expected validation to fail for empty jq transform"
        );

        let err_msg = result.unwrap_err().to_string();
        ensure!(
            err_msg.contains("jq") || err_msg.contains("empty"),
            "expected error message to mention jq or empty, got: {}",
            err_msg
        );

        Ok(())
    }

    #[test]
    /// Test that a JQ transform referencing a non-existent field would be caught.
    /// Note: JQ validation happens at execution time, not during dry-run, so this test
    /// documents the current behavior where invalid JQ expressions pass dry-run but would
    /// fail during actual execution.
    fn dry_run_accepts_invalid_jq_syntax() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        setup_db(&db_path)?;

        // Plan with a JQ transform that references a non-existent field
        // This is syntactically valid JQ, but semantically invalid for the data
        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path,
                wasm_file: None,
            },
            target: None,
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Copy(CopyStep {
                name: Some("copy-with-nonexistent-field".into()),
                column: Column::State,
                filters: PlanFilters {
                    context_ids: vec![hex::encode([0x11; 32])],
                    ..PlanFilters::default()
                },
                transform: CopyTransform {
                    decode_with_abi: Some(false),
                    jq: Some(".value.parsed.nonexistent_field".into()), // References non-existent field
                },
                guards: StepGuards::default(),
                batch_size: None,
            })],
        };

        // Validation should pass (JQ syntax is valid)
        plan.validate()?;

        // Dry-run should also pass (it doesn't execute JQ transforms)
        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        let report = generate_report(&context)?;

        ensure!(report.steps.len() == 1, "expected one step");
        let copy = &report.steps[0];
        ensure!(
            copy.matched_keys == 1,
            "expected dry-run to count matched keys despite invalid JQ"
        );

        // Note: The actual error would occur during execution (--apply mode)
        // when the JQ transform is actually applied to the data

        Ok(())
    }

    #[test]
    /// Edge case: Test that scanning an empty database (no keys) returns zero matches.
    ///
    /// This test ensures the dry-run engine handles databases with no data gracefully,
    /// which is important for:
    /// - New/empty target databases
    /// - Filters that match nothing
    /// - Verification steps that should detect missing data
    ///
    /// Expected behavior: matched_keys should be 0, no samples, no errors.
    fn dry_run_empty_database_returns_zero_matches() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        // Create an empty database with all column families but no data
        let _fixture = DbFixture::new(&db_path)?;

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path,
                wasm_file: None,
            },
            target: None,
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Copy(CopyStep {
                name: Some("copy-from-empty".into()),
                column: Column::State,
                filters: PlanFilters::default(), // No filters, scan everything
                transform: CopyTransform::default(),
                guards: StepGuards::default(),
                batch_size: None,
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        let report = generate_report(&context)?;

        ensure!(report.steps.len() == 1, "expected one step");
        let copy = &report.steps[0];

        ensure!(
            copy.matched_keys == 0,
            "expected 0 matched keys in empty database, got {}",
            copy.matched_keys
        );
        ensure!(
            copy.samples.is_empty(),
            "expected no samples from empty database"
        );

        Ok(())
    }

    #[test]
    /// Edge case: Test filters that match zero keys even in a populated database.
    ///
    /// This test verifies that when filters are too restrictive and match nothing,
    /// the dry-run engine:
    /// - Returns 0 matches without errors
    /// - Doesn't generate spurious samples
    /// - Properly reports the filter summary
    ///
    /// Real-world scenario: Typo in context_id, or filtering for data that doesn't exist.
    fn dry_run_filters_matching_nothing() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        setup_db(&db_path)?;

        // Use a context ID that doesn't exist in the database
        // setup_db creates entries with context_id 0x11..11
        let nonexistent_context = hex::encode([0xAA; 32]);

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path,
                wasm_file: None,
            },
            target: None,
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Copy(CopyStep {
                name: Some("copy-nonexistent-context".into()),
                column: Column::State,
                filters: PlanFilters {
                    context_ids: vec![nonexistent_context],
                    ..PlanFilters::default()
                },
                transform: CopyTransform::default(),
                guards: StepGuards::default(),
                batch_size: None,
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        let report = generate_report(&context)?;

        ensure!(report.steps.len() == 1, "expected one step");
        let copy = &report.steps[0];

        ensure!(
            copy.matched_keys == 0,
            "expected 0 matches for nonexistent context, got {}",
            copy.matched_keys
        );
        ensure!(
            copy.samples.is_empty(),
            "expected no samples when no keys match"
        );
        ensure!(
            copy.filters_summary.is_some(),
            "expected filters summary to be present"
        );

        Ok(())
    }

    #[test]
    /// Edge case: Test behavior with malformed keys shorter than expected context ID size.
    ///
    /// This test verifies the engine's resilience when encountering keys that don't
    /// conform to expected layouts. Specifically:
    /// - State column keys should be 64 bytes (32 context_id + 32 state_key)
    /// - Other context-based columns need at least 32 bytes for context_id
    /// - Keys shorter than this cannot be parsed for context ID extraction
    ///
    /// Expected behavior: Short keys should not match context_id filters since the
    /// extract_context_id function returns None for keys < 32 bytes, causing the
    /// filter to reject them (see matches() implementation line 495-501).
    fn dry_run_handles_malformed_short_keys() -> Result<()> {
        use super::super::test_utils::short_key;
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        // Create database and insert both valid and invalid keys
        let fixture = DbFixture::new(&db_path)?;

        // Insert a valid State entry
        fixture.insert_state_entry(&test_context_id(0x11), &[0x22; 32], b"valid-value")?;

        // Insert a malformed Generic entry (too short to have a context ID)
        fixture.insert_generic_entry(&short_key(16), b"malformed-value")?;

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path,
                wasm_file: None,
            },
            target: None,
            defaults: PlanDefaults::default(),
            steps: vec![
                // This should match the valid State entry
                PlanStep::Copy(CopyStep {
                    name: Some("copy-state".into()),
                    column: Column::State,
                    filters: PlanFilters {
                        context_ids: vec![hex::encode([0x11; 32])],
                        ..PlanFilters::default()
                    },
                    transform: CopyTransform::default(),
                    guards: StepGuards::default(),
                    batch_size: None,
                }),
                // This should match the malformed Generic entry (no context filter)
                PlanStep::Copy(CopyStep {
                    name: Some("copy-generic-unfiltered".into()),
                    column: Column::Generic,
                    filters: PlanFilters::default(),
                    transform: CopyTransform::default(),
                    guards: StepGuards::default(),
                    batch_size: None,
                }),
            ],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        let report = generate_report(&context)?;

        ensure!(report.steps.len() == 2, "expected two steps");

        // Step 1: Valid State entry should match
        let state_step = &report.steps[0];
        ensure!(
            state_step.matched_keys == 1,
            "expected 1 match for valid State entry, got {}",
            state_step.matched_keys
        );

        // Step 2: Generic short key should be found when no filters applied
        let generic_step = &report.steps[1];
        ensure!(
            generic_step.matched_keys == 1,
            "expected 1 match for unfiltered Generic scan, got {}",
            generic_step.matched_keys
        );

        Ok(())
    }

    #[test]
    /// Edge case: Test that verification with ExpectedCount assertion works correctly.
    ///
    /// This test covers the ExpectedCount verification assertion path which was not
    /// tested before. ExpectedCount requires an exact match of the filtered row count.
    ///
    /// Test strategy:
    /// - Insert known number of entries (2)
    /// - Run ExpectedCount verification with matching count (should pass)
    /// - Run ExpectedCount verification with wrong count (should fail)
    ///
    /// This exercises evaluate_assertion() lines 281-290.
    fn dry_run_verify_expected_count() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        // Create database with exactly 2 entries for the same context
        let fixture = DbFixture::new(&db_path)?;
        fixture.insert_state_entry(&test_context_id(0x11), &[0x22; 32], b"value-1")?;
        fixture.insert_state_entry(&test_context_id(0x11), &[0x33; 32], b"value-2")?;

        let target = target_config(&db_path);

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path,
                wasm_file: None,
            },
            target: Some(target),
            defaults: PlanDefaults::default(),
            steps: vec![
                // Should pass: exact count matches
                PlanStep::Verify(VerifyStep {
                    name: Some("verify-exact-count-pass".into()),
                    column: Column::State,
                    filters: PlanFilters {
                        context_ids: vec![hex::encode([0x11; 32])],
                        ..PlanFilters::default()
                    },
                    assertion: VerificationAssertion::ExpectedCount { expected_count: 2 },
                    guards: StepGuards::default(),
                }),
                // Should fail: count doesn't match
                PlanStep::Verify(VerifyStep {
                    name: Some("verify-exact-count-fail".into()),
                    column: Column::State,
                    filters: PlanFilters {
                        context_ids: vec![hex::encode([0x11; 32])],
                        ..PlanFilters::default()
                    },
                    assertion: VerificationAssertion::ExpectedCount { expected_count: 3 },
                    guards: StepGuards::default(),
                }),
            ],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        let report = generate_report(&context)?;

        ensure!(report.steps.len() == 2, "expected two steps");

        // First verification should pass
        let verify_pass = &report.steps[0];
        match &verify_pass.detail {
            StepDetail::Verify { passed, summary } => {
                ensure!(
                    passed == &Some(true),
                    "expected ExpectedCount(2) to pass with 2 entries"
                );
                ensure!(
                    summary.contains("PASS"),
                    "expected PASS in summary, got: {summary}"
                );
            }
            other => return Err(eyre::eyre!("expected verify detail, found {other:?}")),
        }

        // Second verification should fail
        let verify_fail = &report.steps[1];
        match &verify_fail.detail {
            StepDetail::Verify { passed, summary } => {
                ensure!(
                    passed == &Some(false),
                    "expected ExpectedCount(3) to fail with 2 entries"
                );
                ensure!(
                    summary.contains("FAIL"),
                    "expected FAIL in summary, got: {summary}"
                );
            }
            other => return Err(eyre::eyre!("expected verify detail, found {other:?}")),
        }

        Ok(())
    }

    #[test]
    /// Edge case: Test ContainsKey verification assertion with existing and missing keys.
    ///
    /// This test covers the ContainsKey verification path (evaluate_assertion lines 314-338).
    /// ContainsKey checks if a specific key exists in the database using a direct RocksDB
    /// get operation (not filtering).
    ///
    /// Test cases:
    /// - Key that exists in the database (should pass)
    /// - Key that doesn't exist (should fail)
    ///
    /// This is useful for verifying that critical keys exist after a migration.
    fn dry_run_verify_contains_key() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        let fixture = DbFixture::new(&db_path)?;
        // Insert a known Generic entry with a specific key
        fixture.insert_generic_entry(b"known-key", b"known-value")?;

        // Encode the keys we'll check for
        let existing_key = EncodedValue::Utf8 {
            data: "known-key".into(),
        };
        let missing_key = EncodedValue::Utf8 {
            data: "nonexistent-key".into(),
        };

        let target = target_config(&db_path);

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path,
                wasm_file: None,
            },
            target: Some(target),
            defaults: PlanDefaults::default(),
            steps: vec![
                // Should pass: key exists
                PlanStep::Verify(VerifyStep {
                    name: Some("verify-key-exists".into()),
                    column: Column::Generic,
                    filters: PlanFilters::default(),
                    assertion: VerificationAssertion::ContainsKey {
                        contains_key: existing_key,
                    },
                    guards: StepGuards::default(),
                }),
                // Should fail: key doesn't exist
                PlanStep::Verify(VerifyStep {
                    name: Some("verify-key-missing".into()),
                    column: Column::Generic,
                    filters: PlanFilters::default(),
                    assertion: VerificationAssertion::ContainsKey {
                        contains_key: missing_key,
                    },
                    guards: StepGuards::default(),
                }),
            ],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        let report = generate_report(&context)?;

        ensure!(report.steps.len() == 2, "expected two steps");

        // First verification should pass (key exists)
        let verify_exists = &report.steps[0];
        match &verify_exists.detail {
            StepDetail::Verify { passed, summary } => {
                ensure!(
                    passed == &Some(true),
                    "expected ContainsKey to pass for existing key"
                );
                ensure!(
                    summary.contains("present") && summary.contains("PASS"),
                    "expected 'present' and 'PASS' in summary, got: {summary}"
                );
            }
            other => return Err(eyre::eyre!("expected verify detail, found {other:?}")),
        }

        // Second verification should fail (key missing)
        let verify_missing = &report.steps[1];
        match &verify_missing.detail {
            StepDetail::Verify { passed, summary } => {
                ensure!(
                    passed == &Some(false),
                    "expected ContainsKey to fail for missing key"
                );
                ensure!(
                    summary.contains("missing") && summary.contains("FAIL"),
                    "expected 'missing' and 'FAIL' in summary, got: {summary}"
                );
            }
            other => return Err(eyre::eyre!("expected verify detail, found {other:?}")),
        }

        Ok(())
    }

    #[test]
    /// Edge case: Test MissingKey verification assertion.
    ///
    /// This test covers the MissingKey verification path (evaluate_assertion lines 340-365).
    /// MissingKey is the inverse of ContainsKey - it passes when a key does NOT exist.
    ///
    /// Use case: Verify that certain keys were successfully deleted or never existed
    /// in the target database.
    ///
    /// Test cases:
    /// - Key that doesn't exist (should pass)
    /// - Key that exists (should fail)
    fn dry_run_verify_missing_key() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        let fixture = DbFixture::new(&db_path)?;
        fixture.insert_generic_entry(b"existing-key", b"some-value")?;

        let existing_key = EncodedValue::Utf8 {
            data: "existing-key".into(),
        };
        let truly_missing_key = EncodedValue::Utf8 {
            data: "this-key-does-not-exist".into(),
        };

        let target = target_config(&db_path);

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path,
                wasm_file: None,
            },
            target: Some(target),
            defaults: PlanDefaults::default(),
            steps: vec![
                // Should pass: key is indeed missing
                PlanStep::Verify(VerifyStep {
                    name: Some("verify-key-absent".into()),
                    column: Column::Generic,
                    filters: PlanFilters::default(),
                    assertion: VerificationAssertion::MissingKey {
                        missing_key: truly_missing_key,
                    },
                    guards: StepGuards::default(),
                }),
                // Should fail: key actually exists
                PlanStep::Verify(VerifyStep {
                    name: Some("verify-key-should-be-missing-but-exists".into()),
                    column: Column::Generic,
                    filters: PlanFilters::default(),
                    assertion: VerificationAssertion::MissingKey {
                        missing_key: existing_key,
                    },
                    guards: StepGuards::default(),
                }),
            ],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        let report = generate_report(&context)?;

        ensure!(report.steps.len() == 2, "expected two steps");

        // First verification should pass (key is missing)
        let verify_absent = &report.steps[0];
        match &verify_absent.detail {
            StepDetail::Verify { passed, summary } => {
                ensure!(
                    passed == &Some(true),
                    "expected MissingKey to pass when key doesn't exist"
                );
                ensure!(
                    summary.contains("missing") && summary.contains("PASS"),
                    "expected 'missing' and 'PASS' in summary, got: {summary}"
                );
            }
            other => return Err(eyre::eyre!("expected verify detail, found {other:?}")),
        }

        // Second verification should fail (key exists when it shouldn't)
        let verify_exists = &report.steps[1];
        match &verify_exists.detail {
            StepDetail::Verify { passed, summary } => {
                ensure!(
                    passed == &Some(false),
                    "expected MissingKey to fail when key exists"
                );
                ensure!(
                    summary.contains("present") && summary.contains("FAIL"),
                    "expected 'present' and 'FAIL' in summary, got: {summary}"
                );
            }
            other => return Err(eyre::eyre!("expected verify detail, found {other:?}")),
        }

        Ok(())
    }

    #[test]
    /// Edge case: Test state_key_prefix filter for State column.
    ///
    /// The state_key_prefix filter operates on the second half of State column keys.
    /// State keys are 64 bytes: [context_id: 32 bytes][state_key: 32 bytes]
    /// state_key_prefix matches keys where bytes [32..] start with the prefix.
    ///
    /// This is different from raw_key_prefix which matches from byte 0.
    ///
    /// Test validates:
    /// - Keys with matching state_key prefix are included
    /// - Keys with non-matching state_key prefix are excluded
    /// - Filter only works on State column (implementation lines 504-514)
    fn dry_run_filters_state_key_prefix() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        let fixture = DbFixture::new(&db_path)?;
        let ctx_id = test_context_id(0x11);

        // Insert entries with different state key prefixes
        // State key starting with "user_"
        let mut user_key = [0_u8; 32];
        user_key[..5].copy_from_slice(b"user_");
        fixture.insert_state_entry(&ctx_id, &user_key, b"user-data")?;

        // State key starting with "config_"
        let mut config_key = [0_u8; 32];
        config_key[..7].copy_from_slice(b"config_");
        fixture.insert_state_entry(&ctx_id, &config_key, b"config-data")?;

        // State key starting with "user_admin"
        let mut user_admin_key = [0_u8; 32];
        user_admin_key[..11].copy_from_slice(b"user_admin_");
        fixture.insert_state_entry(&ctx_id, &user_admin_key, b"admin-data")?;

        // Filter for state keys starting with "user_"
        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path,
                wasm_file: None,
            },
            target: None,
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Copy(CopyStep {
                name: Some("copy-user-state".into()),
                column: Column::State,
                filters: PlanFilters {
                    context_ids: vec![hex::encode(*ctx_id)],
                    state_key_prefix: Some("user_".into()),
                    ..PlanFilters::default()
                },
                transform: CopyTransform::default(),
                guards: StepGuards::default(),
                batch_size: None,
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        let report = generate_report(&context)?;

        ensure!(report.steps.len() == 1, "expected one step");
        let copy = &report.steps[0];

        // Should match 2 entries: "user_" and "user_admin_", but not "config_"
        ensure!(
            copy.matched_keys == 2,
            "expected 2 matches for state_key_prefix 'user_', got {}",
            copy.matched_keys
        );

        Ok(())
    }

    #[test]
    /// Edge case: Test key_range filter with start and end bounds.
    ///
    /// The key_range filter applies lexicographic bounds to keys:
    /// - key_range.start: inclusive lower bound (key >= start)
    /// - key_range.end: exclusive upper bound (key < end)
    ///
    /// This is useful for:
    /// - Partitioning large datasets
    /// - Migrating specific ranges of keys
    /// - Testing specific subsets of data
    ///
    /// Implementation: ResolvedFilters::matches() lines 522-532
    fn dry_run_filters_key_range() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        let fixture = DbFixture::new(&db_path)?;

        // Insert Generic entries with keys in known lexicographic order
        fixture.insert_generic_entry(b"aaa", b"value-1")?;
        fixture.insert_generic_entry(b"bbb", b"value-2")?;
        fixture.insert_generic_entry(b"ccc", b"value-3")?;
        fixture.insert_generic_entry(b"ddd", b"value-4")?;
        fixture.insert_generic_entry(b"eee", b"value-5")?;

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path,
                wasm_file: None,
            },
            target: None,
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Copy(CopyStep {
                name: Some("copy-range-b-to-d".into()),
                column: Column::Generic,
                filters: PlanFilters {
                    key_range: Some(KeyRange {
                        start: Some(hex::encode(b"bbb")), // inclusive: >= "bbb"
                        end: Some(hex::encode(b"ddd")),   // exclusive: < "ddd"
                    }),
                    ..PlanFilters::default()
                },
                transform: CopyTransform::default(),
                guards: StepGuards::default(),
                batch_size: None,
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        let report = generate_report(&context)?;

        ensure!(report.steps.len() == 1, "expected one step");
        let copy = &report.steps[0];

        // Should match "bbb" and "ccc" but not "aaa", "ddd", or "eee"
        ensure!(
            copy.matched_keys == 2,
            "expected 2 matches in range [bbb, ddd), got {}",
            copy.matched_keys
        );

        Ok(())
    }

    #[test]
    /// Edge case: Test combining multiple filters (context_id + raw_key_prefix).
    ///
    /// When multiple filters are specified, they are ANDed together - a key must
    /// satisfy ALL filters to match. This test verifies that behavior.
    ///
    /// Test scenario:
    /// - Two contexts with different data
    /// - Apply both context_id AND raw_key_prefix filters
    /// - Only keys matching BOTH conditions should be included
    ///
    /// This exercises the ResolvedFilters::matches() AND logic where each filter
    /// returns false if it doesn't match (lines 494-549).
    fn dry_run_filters_combined_context_and_prefix() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        let fixture = DbFixture::new(&db_path)?;

        // Context 0x11 with various state keys
        fixture.insert_state_entry(&test_context_id(0x11), &[0xAA; 32], b"value-11-aa")?;
        fixture.insert_state_entry(&test_context_id(0x11), &[0xBB; 32], b"value-11-bb")?;

        // Context 0x22 with similar state keys
        fixture.insert_state_entry(&test_context_id(0x22), &[0xAA; 32], b"value-22-aa")?;
        fixture.insert_state_entry(&test_context_id(0x22), &[0xBB; 32], b"value-22-bb")?;

        // Filter for context 0x11 AND keys starting with 0x11 (the context ID bytes)
        // This will match context 0x11 entries because State keys are [context_id][state_key]
        // and raw_key_prefix applies to the full key from byte 0
        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path,
                wasm_file: None,
            },
            target: None,
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Copy(CopyStep {
                name: Some("copy-context-11-with-prefix".into()),
                column: Column::State,
                filters: PlanFilters {
                    context_ids: vec![hex::encode([0x11; 32])],
                    raw_key_prefix: Some(hex::encode([0x11; 32])), // Same as context ID
                    ..PlanFilters::default()
                },
                transform: CopyTransform::default(),
                guards: StepGuards::default(),
                batch_size: None,
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        let report = generate_report(&context)?;

        ensure!(report.steps.len() == 1, "expected one step");
        let copy = &report.steps[0];

        // Should match both entries from context 0x11 (both start with 0x11)
        // Should NOT match context 0x22 entries (different context_id)
        ensure!(
            copy.matched_keys == 2,
            "expected 2 matches for combined filters, got {}",
            copy.matched_keys
        );

        Ok(())
    }

    #[test]
    /// Edge case: Test that non-State columns (like Meta) work with context_id filters.
    ///
    /// Context ID filtering works across multiple column types:
    /// - State, Meta, Config, Identity, Delta all store context_id in first 32 bytes
    /// - Generic and Alias columns have different layouts
    ///
    /// This test verifies the extract_context_id function (lines 563-574) works
    /// correctly for Meta column, which has structure: [context_id: 32][meta_key: variable]
    fn dry_run_filters_context_id_on_meta_column() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");

        let fixture = DbFixture::new(&db_path)?;

        // Insert Meta entries for two different contexts
        // Note: Meta column has one entry per context (context_id is the key)
        fixture.insert_meta_entry(&test_context_id(0x11), &test_context_meta(0xAA))?;
        fixture.insert_meta_entry(&test_context_id(0x22), &test_context_meta(0xBB))?;

        let plan = MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path,
                wasm_file: None,
            },
            target: None,
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Copy(CopyStep {
                name: Some("copy-meta-context-11".into()),
                column: Column::Meta,
                filters: PlanFilters {
                    context_ids: vec![hex::encode([0x11; 32])],
                    ..PlanFilters::default()
                },
                transform: CopyTransform::default(),
                guards: StepGuards::default(),
                batch_size: None,
            })],
        };

        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        let report = generate_report(&context)?;

        ensure!(report.steps.len() == 1, "expected one step");
        let copy = &report.steps[0];

        // Should match 1 Meta entry from context 0x11 (Meta column has one entry per context)
        ensure!(
            copy.matched_keys == 1,
            "expected 1 Meta entry for context 0x11, got {}",
            copy.matched_keys
        );

        Ok(())
    }
}
