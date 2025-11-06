//!
//! Shared verification logic for migration assertions.
//!
//! This module provides common verification functionality used by both dry-run preview
//! and actual migration execution. Verification steps allow migration plans to assert
//! expected database state and abort if conditions are not met.
//!
//! ## Assertion Types
//!
//! - **ExpectedCount**: Exact count match (count == expected)
//! - **MinCount**: Minimum threshold (count >= min)
//! - **MaxCount**: Maximum threshold (count <= max)
//! - **ContainsKey**: Specific key must exist
//! - **MissingKey**: Specific key must not exist
//!
//! ## Usage
//!
//! ```ignore
//! let matched_count = /* scan and count matching keys */;
//! let outcome = evaluate_assertion(db, column, &assertion, matched_count)?;
//!
//! if outcome.passed == Some(false) {
//!     bail!("Verification failed: {}", outcome.summary);
//! }
//! ```

use eyre::Result;
use rocksdb::{DBWithThreadMode, SingleThreaded};

use core::convert::TryFrom;

use crate::types::Column;

use super::plan::VerificationAssertion;

/// Verification outcome containing summary, pass/fail status, and warnings.
pub struct VerificationOutcome {
    /// Human-readable summary of the verification result
    pub summary: String,
    /// Pass (true), fail (false), or could not determine (None)
    pub passed: Option<bool>,
    /// Warnings accumulated during verification (e.g., decode failures)
    pub warnings: Vec<String>,
}

/// Evaluate a verification assertion against the target database.
///
/// This function checks one of the following assertion types:
/// - `ExpectedCount`: Exact count match
/// - `MinCount`: Count is at least the specified minimum
/// - `MaxCount`: Count is at most the specified maximum
/// - `ContainsKey`: Specific key exists in the database
/// - `MissingKey`: Specific key does not exist in the database
///
/// # Arguments
///
/// * `db` - The database to verify against
/// * `column` - The column family being verified
/// * `assertion` - The assertion to evaluate
/// * `matched_count` - The number of keys that matched the filters (for count assertions)
///
/// # Returns
///
/// A `VerificationOutcome` with the result summary, pass/fail status, and any warnings.
///
/// # Errors
///
/// Returns an error if database access fails or the column family doesn't exist.
pub fn evaluate_assertion(
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use eyre::ensure;
    use rocksdb::{ColumnFamilyDescriptor, Options, DB};
    use tempfile::TempDir;

    use super::*;
    use crate::migration::plan::EncodedValue;
    use crate::migration::test_utils::DbFixture;
    use crate::types::Column;

    fn open_test_db(path: &Path) -> Result<DB> {
        let mut opts = Options::default();
        opts.create_if_missing(false);

        let descriptors: Vec<_> = Column::all()
            .iter()
            .map(|column| ColumnFamilyDescriptor::new(column.as_str(), Options::default()))
            .collect();

        Ok(DB::open_cf_descriptors(&opts, path, descriptors)?)
    }

    #[test]
    fn test_evaluate_assertion_expected_count_pass() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");
        let _fixture = DbFixture::new(&db_path)?;
        let db = open_test_db(&db_path)?;

        let assertion = VerificationAssertion::ExpectedCount { expected_count: 0 };
        let outcome = evaluate_assertion(&db, Column::State, &assertion, 0)?;

        ensure!(outcome.passed == Some(true), "expected pass");
        ensure!(outcome.summary.contains("PASS"), "expected PASS in summary");
        Ok(())
    }

    #[test]
    fn test_evaluate_assertion_expected_count_fail() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");
        let _fixture = DbFixture::new(&db_path)?;
        let db = open_test_db(&db_path)?;

        let assertion = VerificationAssertion::ExpectedCount { expected_count: 5 };
        let outcome = evaluate_assertion(&db, Column::State, &assertion, 3)?;

        ensure!(outcome.passed == Some(false), "expected fail");
        ensure!(outcome.summary.contains("FAIL"), "expected FAIL in summary");
        Ok(())
    }

    #[test]
    fn test_evaluate_assertion_min_count() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");
        let _fixture = DbFixture::new(&db_path)?;
        let db = open_test_db(&db_path)?;

        // Pass case
        let assertion = VerificationAssertion::MinCount { min_count: 2 };
        let outcome = evaluate_assertion(&db, Column::State, &assertion, 5)?;
        ensure!(outcome.passed == Some(true), "expected pass for 5 >= 2");

        // Fail case
        let outcome = evaluate_assertion(&db, Column::State, &assertion, 1)?;
        ensure!(outcome.passed == Some(false), "expected fail for 1 < 2");

        Ok(())
    }

    #[test]
    fn test_evaluate_assertion_max_count() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");
        let _fixture = DbFixture::new(&db_path)?;
        let db = open_test_db(&db_path)?;

        // Pass case
        let assertion = VerificationAssertion::MaxCount { max_count: 10 };
        let outcome = evaluate_assertion(&db, Column::State, &assertion, 5)?;
        ensure!(outcome.passed == Some(true), "expected pass for 5 <= 10");

        // Fail case
        let outcome = evaluate_assertion(&db, Column::State, &assertion, 15)?;
        ensure!(outcome.passed == Some(false), "expected fail for 15 > 10");

        Ok(())
    }

    #[test]
    fn test_evaluate_assertion_contains_key() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");
        let fixture = DbFixture::new(&db_path)?;
        fixture.insert_generic_entry(b"test-key", b"test-value")?;
        let db = open_test_db(&db_path)?;

        // Pass case - key exists
        let key = EncodedValue::Utf8 {
            data: "test-key".into(),
        };
        let assertion = VerificationAssertion::ContainsKey { contains_key: key };
        let outcome = evaluate_assertion(&db, Column::Generic, &assertion, 0)?;
        ensure!(
            outcome.passed == Some(true),
            "expected pass for existing key"
        );
        ensure!(outcome.summary.contains("present"), "expected 'present'");

        // Fail case - key doesn't exist
        let missing = EncodedValue::Utf8 {
            data: "missing-key".into(),
        };
        let assertion = VerificationAssertion::ContainsKey {
            contains_key: missing,
        };
        let outcome = evaluate_assertion(&db, Column::Generic, &assertion, 0)?;
        ensure!(
            outcome.passed == Some(false),
            "expected fail for missing key"
        );
        ensure!(outcome.summary.contains("missing"), "expected 'missing'");

        Ok(())
    }

    #[test]
    fn test_evaluate_assertion_missing_key() -> Result<()> {
        let temp = TempDir::new()?;
        let db_path = temp.path().join("db");
        let fixture = DbFixture::new(&db_path)?;
        fixture.insert_generic_entry(b"existing-key", b"value")?;
        let db = open_test_db(&db_path)?;

        // Pass case - key is indeed missing
        let missing = EncodedValue::Utf8 {
            data: "truly-missing".into(),
        };
        let assertion = VerificationAssertion::MissingKey {
            missing_key: missing,
        };
        let outcome = evaluate_assertion(&db, Column::Generic, &assertion, 0)?;
        ensure!(
            outcome.passed == Some(true),
            "expected pass for missing key"
        );
        ensure!(outcome.summary.contains("missing"), "expected 'missing'");

        // Fail case - key exists
        let existing = EncodedValue::Utf8 {
            data: "existing-key".into(),
        };
        let assertion = VerificationAssertion::MissingKey {
            missing_key: existing,
        };
        let outcome = evaluate_assertion(&db, Column::Generic, &assertion, 0)?;
        ensure!(
            outcome.passed == Some(false),
            "expected fail for existing key"
        );
        ensure!(outcome.summary.contains("present"), "expected 'present'");

        Ok(())
    }
}
