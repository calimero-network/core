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

use std::collections::HashSet;

use eyre::{bail, Result, WrapErr};
use rocksdb::{DBWithThreadMode, IteratorMode, SingleThreaded};
use serde::Serialize;

use core::convert::TryFrom;

use calimero_wasm_abi::schema::Manifest;

use crate::types;
use crate::types::Column;

use super::context::MigrationContext;
use super::plan::{
    CopyStep, DeleteStep, PlanDefaults, PlanFilters, PlanStep, UpsertStep, VerificationAssertion,
    VerifyStep,
};

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

/// Walk every plan step, producing a read-only preview without mutating RocksDB.
pub fn generate_report(context: &MigrationContext) -> Result<DryRunReport> {
    let plan = context.plan();
    let db = context.source().db();
    let abi_manifest = context.source().abi_manifest()?;

    let mut steps = Vec::with_capacity(plan.steps.len());

    for (index, step) in plan.steps.iter().enumerate() {
        let report = match step {
            PlanStep::Copy(copy) => {
                preview_copy_step(index, copy, &plan.defaults, db, abi_manifest)?
            }
            PlanStep::Delete(delete) => preview_delete_step(index, delete, &plan.defaults, db)?,
            PlanStep::Upsert(upsert) => preview_upsert_step(index, upsert, &plan.defaults),
            PlanStep::Verify(verify) => preview_verify_step(index, verify, &plan.defaults, db)?,
        };

        steps.push(report);
    }

    Ok(DryRunReport { steps })
}

/// Preview a `copy` plan step by applying filters, counting matches, and capturing sample keys.
fn preview_copy_step(
    index: usize,
    step: &CopyStep,
    defaults: &PlanDefaults,
    db: &DBWithThreadMode<SingleThreaded>,
    abi_manifest: Option<&Manifest>,
) -> Result<StepReport> {
    let filters = defaults.merge_filters(&step.filters);
    let mut resolved = ResolvedFilters::resolve(step.column, &filters);

    let decode_with_abi = defaults.effective_decode_with_abi(step.transform.decode_with_abi);
    if decode_with_abi && abi_manifest.is_none() {
        resolved
            .warnings
            .push("decode_with_abi requested but source ABI manifest is unavailable".into());
    }

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

/// Preview a `delete` plan step using the same scan routine as copy.
fn preview_delete_step(
    index: usize,
    step: &DeleteStep,
    defaults: &PlanDefaults,
    db: &DBWithThreadMode<SingleThreaded>,
) -> Result<StepReport> {
    let filters = defaults.merge_filters(&step.filters);
    let resolved = ResolvedFilters::resolve(step.column, &filters);
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

/// Summarise an `upsert` plan step; iterates literal entries to build previews.
fn preview_upsert_step(index: usize, step: &UpsertStep, _defaults: &PlanDefaults) -> StepReport {
    let mut samples = Vec::new();

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

/// Inspect a `verify` plan step by scanning matches and evaluating the assertion immediately.
fn preview_verify_step(
    index: usize,
    step: &VerifyStep,
    defaults: &PlanDefaults,
    db: &DBWithThreadMode<SingleThreaded>,
) -> Result<StepReport> {
    let filters = defaults.merge_filters(&step.filters);
    let mut resolved = ResolvedFilters::resolve(step.column, &filters);
    let scan = scan_column(db, step.column, &resolved)?;

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

/// Lightweight summary of a column scan.
struct ScanResult {
    matched: usize,
    samples: Vec<String>,
}

/// Iterate a column family, applying resolved filters to count matches and capture samples.
/// Capture samples are used as sanity checks
fn scan_column(
    db: &DBWithThreadMode<SingleThreaded>,
    column: Column,
    filters: &ResolvedFilters,
) -> Result<ScanResult> {
    let cf = db
        .cf_handle(column.as_str())
        .ok_or_else(|| eyre::eyre!("Column family '{}' not found", column.as_str()))?;

    let mut matched: usize = 0;
    let mut samples = Vec::new();

    let iter = db.iterator_cf(cf, IteratorMode::Start);
    for item in iter {
        let (key, _value) = item.wrap_err_with(|| {
            format!(
                "Failed to iterate column family '{}' during dry-run",
                column.as_str()
            )
        })?;

        if filters.matches(column, &key) {
            matched = matched.saturating_add(1);
            if samples.len() < SAMPLE_LIMIT {
                samples.push(sample_from_key(column, &key));
            }
        }
    }

    Ok(ScanResult { matched, samples })
}

/// Render a key either via structured parsing or as a raw hex fallback.
fn sample_from_key(column: Column, key: &[u8]) -> String {
    types::parse_key(column, key).map_or_else(
        |_| format!("raw_hex={}", hex::encode(key)),
        |value| {
            serde_json::to_string(&value)
                .unwrap_or_else(|_| format!("raw_hex={}", hex::encode(key)))
        },
    )
}

struct VerificationOutcome {
    summary: String,
    passed: Option<bool>,
    warnings: Vec<String>,
}

/// Execute the verify assertion logic and describe the outcome.
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
                        passed: None,
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
                        passed: None,
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

        if !filters.context_aliases.is_empty() {
            warnings.push(
                "context_aliases filter is not yet applied during dry-run (preview may be broader than expected)"
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
    fn matches(&self, column: Column, key: &[u8]) -> bool {
        if let Some(set) = &self.context_ids {
            let Some(context_slice) = extract_context_id(column, key) else {
                return false;
            };

            if !set.contains(&context_slice.to_vec()) {
                return false;
            }
        }

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

        if let Some(prefix) = &self.raw_key_prefix {
            if !key.starts_with(prefix) {
                return false;
            }
        }

        if let Some(start) = &self.key_range_start {
            if key < start.as_slice() {
                return false;
            }
        }

        if let Some(end) = &self.key_range_end {
            if key >= end.as_slice() {
                return false;
            }
        }

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
fn decode_hex_string(value: &str) -> Result<Vec<u8>> {
    let trimmed = value.trim().trim_start_matches("0x");
    if trimmed.len() % 2 != 0 {
        bail!("hex string has odd length");
    }
    Ok(hex::decode(trimmed)?)
}

/// Extract the context ID portion from a key when the column layout supports it.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::context::{MigrationContext, MigrationOverrides};
    use crate::migration::plan::{
        CopyStep, CopyTransform, DeleteStep, EncodedValue, MigrationPlan, PlanDefaults,
        PlanFilters, PlanStep, PlanVersion, SourceEndpoint, UpsertEntry, UpsertStep,
        VerificationAssertion, VerifyStep,
    };
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
            target: None,
            defaults: PlanDefaults {
                columns: Vec::new(),
                filters: PlanFilters::default(),
                decode_with_abi: Some(false),
                write_if_missing: Some(false),
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
                }),
                PlanStep::Verify(VerifyStep {
                    name: Some("expect-one".into()),
                    column: Column::State,
                    filters: PlanFilters {
                        context_ids: vec![hex::encode([0x11; 32])],
                        ..PlanFilters::default()
                    },
                    assertion: VerificationAssertion::ExpectedCount { expected_count: 1 },
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
                filters: PlanFilters {
                    context_ids: vec![hex::encode([0x11; 32])],
                    ..PlanFilters::default()
                },
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
    /// Test dry-run behavior for upsert steps.
    fn dry_run_reports_upsert_step() -> Result<()> {
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
            steps: vec![PlanStep::Verify(VerifyStep {
                name: Some("min-count-check".into()),
                column: Column::State,
                filters: PlanFilters {
                    context_ids: vec![hex::encode([0x11; 32])],
                    ..PlanFilters::default()
                },
                assertion: VerificationAssertion::MinCount { min_count: 1 },
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
            steps: vec![PlanStep::Verify(VerifyStep {
                name: Some("max-count-check".into()),
                column: Column::State,
                filters: PlanFilters {
                    context_ids: vec![hex::encode([0x11; 32])],
                    ..PlanFilters::default()
                },
                assertion: VerificationAssertion::MaxCount { max_count: 0 },
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
}
