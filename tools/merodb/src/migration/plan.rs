use std::fmt;
use std::path::PathBuf;

use base64::engine::general_purpose::STANDARD as BASE64_ENGINE;
use base64::Engine;
use eyre::{bail, ensure, Result};
use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::types::Column;

/// Discrete version number attached to migration plans for compatibility gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlanVersion(u32);

impl PlanVersion {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn latest() -> Self {
        Self(1)
    }

    pub const fn as_u32(self) -> u32 {
        self.0
    }

    /// Validate that this plan version is supported by the current migration engine.
    ///
    /// Migration plans include a version number to enable compatibility checks and allow
    /// graceful evolution of the plan schema over time. This method ensures the plan version
    /// matches the current engine's supported version, preventing issues from running plans
    /// with incompatible features or semantics.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the version matches the latest supported version.
    ///
    /// # Errors
    ///
    /// Returns an error with a descriptive message if the plan version is not supported,
    /// indicating both the plan's version and the currently supported version.
    pub fn ensure_supported(self) -> Result<()> {
        if self == Self::latest() {
            return Ok(());
        }
        bail!(
            "Unsupported migration plan version {version}. Supported versions: {supported}",
            version = self.0,
            supported = Self::latest().0,
        );
    }
}

impl Default for PlanVersion {
    fn default() -> Self {
        Self::latest()
    }
}

impl fmt::Display for PlanVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<'de> Deserialize<'de> for PlanVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = u32::deserialize(deserializer)?;
        Ok(Self::new(value))
    }
}

/// Top-level representation of a YAML migration plan document.
#[derive(Debug, Deserialize)]
pub struct MigrationPlan {
    pub version: PlanVersion,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub source: SourceEndpoint,
    #[serde(default)]
    pub target: Option<TargetEndpoint>,
    #[serde(default)]
    pub defaults: PlanDefaults,
    #[serde(default)]
    pub steps: Vec<PlanStep>,
}

impl MigrationPlan {
    /// Perform comprehensive validation of the entire migration plan structure.
    ///
    /// This method orchestrates validation across all components of a migration plan to
    /// ensure it's well-formed and safe to execute. The validation process:
    /// 1. **Version check**: Ensures the plan version is supported by this engine
    /// 2. **Endpoint validation**: Verifies source and target database paths are non-empty
    /// 3. **Defaults validation**: Checks plan-level defaults for consistency
    /// 4. **Non-empty steps**: Requires at least one migration step
    /// 5. **Step validation**: Recursively validates each step's configuration
    ///
    /// Validation catches common errors early:
    /// - Empty or missing database paths
    /// - Malformed filter configurations
    /// - Invalid key ranges (start >= end)
    /// - Empty upsert entry lists
    /// - Blank jq transformation expressions
    /// - Unsupported plan versions
    ///
    /// # Returns
    ///
    /// `Ok(())` if the plan is valid and ready for execution.
    ///
    /// # Errors
    ///
    /// Returns a detailed error describing the first validation failure encountered,
    /// including context about which part of the plan failed validation (e.g., step index,
    /// field name).
    pub fn validate(&self) -> Result<()> {
        // Reject plans that target an unsupported schema version.
        self.version.ensure_supported()?;

        // Source, target, and defaults perform their own path/filter validations.
        self.source.validate("source")?;
        if let Some(target) = &self.target {
            target.validate("target")?;
        }
        self.defaults.validate("defaults")?;

        // Plans must declare at least one step to be meaningful.
        if self.steps.is_empty() {
            bail!("Migration plan must define at least one step");
        }

        // Each step enforces its own shape and required fields.
        for (index, step) in self.steps.iter().enumerate() {
            step.validate(index)?;
        }

        Ok(())
    }
}

/// Location of the source RocksDB and its optional ABI.
#[derive(Debug, Deserialize)]
pub struct SourceEndpoint {
    pub db_path: PathBuf,
    #[serde(default)]
    pub wasm_file: Option<PathBuf>,
}

impl SourceEndpoint {
    /// Validate that the source database path is non-empty and properly configured.
    ///
    /// This method performs early validation on the source endpoint configuration to
    /// ensure that:
    /// 1. The database path is not an empty string
    /// 2. If a WASM ABI file is specified, its path is not empty
    ///
    /// This prevents downstream errors during database opening and ensures that path
    /// strings are usable for filesystem operations.
    ///
    /// # Arguments
    ///
    /// * `context` - Error context string for detailed failure messages (e.g., "source")
    ///
    /// # Returns
    ///
    /// `Ok(())` if all paths are valid and non-empty.
    ///
    /// # Errors
    ///
    /// Returns an error if the db_path is empty or if wasm_file is Some but empty.
    fn validate(&self, context: &str) -> Result<()> {
        ensure!(
            !self.db_path.as_os_str().is_empty(),
            "{context}: db_path must not be empty",
        );
        if let Some(path) = &self.wasm_file {
            ensure!(
                !path.as_os_str().is_empty(),
                "{context}: wasm_file must not be empty when provided",
            );
        }
        Ok(())
    }
}

/// Location of the target RocksDB and optional backup directory.
#[derive(Debug, Deserialize)]
pub struct TargetEndpoint {
    pub db_path: PathBuf,
    #[serde(default)]
    pub backup_dir: Option<PathBuf>,
}

impl TargetEndpoint {
    /// Validate that the target database path and optional backup directory are non-empty.
    ///
    /// This method performs early validation on the target endpoint configuration to
    /// ensure that:
    /// 1. The database path is not an empty string
    /// 2. If a backup directory is specified, its path is not empty
    ///
    /// This prevents downstream errors during database opening and backup operations,
    /// ensuring that path strings are usable for filesystem operations.
    ///
    /// # Arguments
    ///
    /// * `context` - Error context string for detailed failure messages (e.g., "target")
    ///
    /// # Returns
    ///
    /// `Ok(())` if all paths are valid and non-empty.
    ///
    /// # Errors
    ///
    /// Returns an error if the db_path is empty or if backup_dir is Some but empty.
    fn validate(&self, context: &str) -> Result<()> {
        ensure!(
            !self.db_path.as_os_str().is_empty(),
            "{context}: db_path must not be empty",
        );
        if let Some(path) = &self.backup_dir {
            ensure!(
                !path.as_os_str().is_empty(),
                "{context}: backup_dir must not be empty when provided",
            );
        }
        Ok(())
    }
}

/// Default columns, filters, and options that individual steps may inherit.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PlanDefaults {
    #[serde(default)]
    pub columns: Vec<Column>,
    #[serde(default)]
    pub filters: PlanFilters,
    #[serde(default)]
    pub decode_with_abi: Option<bool>,
    #[serde(default)]
    pub write_if_missing: Option<bool>,
    #[serde(default)]
    pub batch_size: Option<usize>,
}

impl PlanDefaults {
    /// Validate that the default filters are well-formed.
    ///
    /// This method delegates to the filter validation logic to ensure that plan-level
    /// default filters are properly configured. Since defaults can be inherited by
    /// multiple steps, early validation prevents propagating invalid filter configurations
    /// throughout the migration.
    ///
    /// # Arguments
    ///
    /// * `context` - Error context string for detailed failure messages (e.g., "defaults")
    ///
    /// # Returns
    ///
    /// `Ok(())` if the default filters are valid.
    ///
    /// # Errors
    ///
    /// Returns an error if the default filters are invalid (e.g., invalid key range bounds).
    fn validate(&self, context: &str) -> Result<()> {
        self.filters.validate(&format!("{context}.filters"))?;
        Ok(())
    }

    /// Merge plan-level default filters with step-specific filter overrides.
    ///
    /// This method implements the filter inheritance mechanism where steps can override
    /// plan-level defaults while keeping unspecified filters from defaults. The merging
    /// strategy ensures that:
    /// - Step-specific filters take precedence over plan defaults
    /// - Unspecified step filters inherit from plan defaults
    /// - Empty filter collections replace defaults (explicit override, not inheritance)
    ///
    /// # Arguments
    ///
    /// * `overrides` - Step-specific filters that may override plan defaults
    ///
    /// # Returns
    ///
    /// A new `PlanFilters` instance with the merged configuration.
    pub fn merge_filters(&self, overrides: &PlanFilters) -> PlanFilters {
        self.filters.merged_with(overrides)
    }

    /// Determine the effective decode_with_abi flag considering both defaults and overrides.
    ///
    /// This method implements a three-level priority system for the `decode_with_abi` setting:
    /// 1. **Step override** (highest priority): If the step explicitly sets this flag, use it
    /// 2. **Plan default**: If the plan defaults specify this flag, use it
    /// 3. **Engine default** (lowest priority): Fall back to `false` if neither is specified
    ///
    /// # Arguments
    ///
    /// * `override_flag` - Optional step-level override for decode_with_abi
    ///
    /// # Returns
    ///
    /// The effective boolean value to use for ABI decoding in this step.
    pub fn effective_decode_with_abi(&self, override_flag: Option<bool>) -> bool {
        override_flag.or(self.decode_with_abi).unwrap_or(false)
    }
}

/// Common filters that can be referenced by multiple steps.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PlanFilters {
    #[serde(default)]
    /// Exact context IDs (hex strings) a step may touch; empty means any context ID is allowed.
    pub context_ids: Vec<String>,
    #[serde(default)]
    /// Human-friendly aliases that resolve to context IDs; use when you prefer names over hashes.
    pub context_aliases: Vec<String>,
    #[serde(default)]
    /// Application state records must have decoded keys beginning with this prefix (State column only).
    pub state_key_prefix: Option<String>,
    #[serde(default)]
    /// Low-level RocksDB keys must start with this byte prefix; bypasses ABI/key decoding.
    pub raw_key_prefix: Option<String>,
    #[serde(default)]
    /// Limit alias-column mutations to a single alias entry with this name.
    pub alias_name: Option<String>,
    #[serde(default)]
    /// Optional lexicographic slice (start/end strings) applied to the column's raw keys.
    pub key_range: Option<KeyRange>,
}

impl PlanFilters {
    /// Validate that all filter fields are well-formed and usable.
    ///
    /// This method checks filter configuration for common errors:
    /// - **Key ranges**: Must have at least one bound (start or end) specified
    /// - Other filters are validated during resolution (hex decoding, etc.)
    ///
    /// # Arguments
    ///
    /// * `context` - Error context string for detailed failure messages
    ///
    /// # Returns
    ///
    /// `Ok(())` if all filters are valid.
    ///
    /// # Errors
    ///
    /// Returns an error if the key_range is Some but has both start and end empty.
    fn validate(&self, context: &str) -> Result<()> {
        if let Some(range) = &self.key_range {
            range.validate(&format!("{context}.key_range"))?;
        }
        Ok(())
    }

    /// Check whether any filters are configured in this filter set.
    ///
    /// This method determines if the filter set is completely empty (no filters specified)
    /// or if at least one filter is configured. Empty filter sets match all keys in the
    /// column, while non-empty sets apply filtering logic.
    ///
    /// # Returns
    ///
    /// `true` if no filters are configured (matches all keys), `false` if any filter is set.
    pub const fn is_empty(&self) -> bool {
        self.context_ids.is_empty()
            && self.context_aliases.is_empty()
            && self.state_key_prefix.is_none()
            && self.raw_key_prefix.is_none()
            && self.alias_name.is_none()
            && self.key_range.is_none()
    }

    /// Generate a human-readable summary of active filters for display in reports.
    ///
    /// This method creates a concise, comma-separated representation of all non-empty
    /// filters configured in this filter set. It's used in dry-run reports and execution
    /// logs to help users understand which filters are being applied to each step.
    ///
    /// The summary includes:
    /// - **context_ids**: Count of specified context IDs (not the IDs themselves)
    /// - **context_aliases**: Pipe-separated list of alias names
    /// - **state_key_prefix**: The prefix string for State column filtering
    /// - **raw_key_prefix**: The raw byte prefix for low-level filtering
    /// - **alias_name**: The specific alias name filter
    /// - **key_range**: Lexicographic range bounds in abbreviated form
    ///
    /// # Returns
    ///
    /// `Some(String)` with a formatted summary if any filters are active,
    /// `None` if no filters are configured (matches all keys).
    pub fn summary(&self) -> Option<String> {
        let mut parts = Vec::new();

        if !self.context_ids.is_empty() {
            parts.push(format!("context_ids={}", self.context_ids.len()));
        }
        if !self.context_aliases.is_empty() {
            parts.push(format!(
                "context_aliases={}",
                self.context_aliases.join("|")
            ));
        }
        if let Some(prefix) = &self.state_key_prefix {
            parts.push(format!("state_key_prefix={prefix}"));
        }
        if let Some(prefix) = &self.raw_key_prefix {
            parts.push(format!("raw_key_prefix={prefix}"));
        }
        if let Some(name) = &self.alias_name {
            parts.push(format!("alias_name={name}"));
        }
        if let Some(range) = &self.key_range {
            parts.push(format!("key_range={}", range.summary()));
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(", "))
        }
    }

    /// Merge two filter sets with override semantics for step-level customization.
    ///
    /// This method implements the core filter inheritance mechanism used throughout the
    /// migration system. It combines plan-level default filters with step-specific overrides,
    /// using the following rules for each filter type:
    ///
    /// **For collection filters** (context_ids, context_aliases):
    /// - If override is **empty**: Inherit from base
    /// - If override is **non-empty**: Use override (replaces base entirely)
    ///
    /// **For optional filters** (state_key_prefix, raw_key_prefix, alias_name, key_range):
    /// - If override is **None**: Inherit from base
    /// - If override is **Some**: Use override value
    ///
    /// This allows steps to:
    /// - Inherit default filters by leaving fields unspecified
    /// - Override specific filters while keeping others from defaults
    /// - Explicitly clear inherited filters (e.g., empty vec, None)
    ///
    /// # Arguments
    ///
    /// * `overrides` - Step-specific filter configuration that may override base filters
    ///
    /// # Returns
    ///
    /// A new `PlanFilters` instance with the merged configuration.
    pub fn merged_with(&self, overrides: &Self) -> Self {
        Self {
            context_ids: if overrides.context_ids.is_empty() {
                self.context_ids.clone()
            } else {
                overrides.context_ids.clone()
            },
            context_aliases: if overrides.context_aliases.is_empty() {
                self.context_aliases.clone()
            } else {
                overrides.context_aliases.clone()
            },
            state_key_prefix: overrides
                .state_key_prefix
                .clone()
                .or_else(|| self.state_key_prefix.clone()),
            raw_key_prefix: overrides
                .raw_key_prefix
                .clone()
                .or_else(|| self.raw_key_prefix.clone()),
            alias_name: overrides
                .alias_name
                .clone()
                .or_else(|| self.alias_name.clone()),
            key_range: overrides
                .key_range
                .clone()
                .or_else(|| self.key_range.clone()),
        }
    }
}

/// Inclusive/exclusive key range filter for byte prefixes.
#[derive(Clone, Debug, Deserialize)]
pub struct KeyRange {
    pub start: Option<String>,
    pub end: Option<String>,
}

impl KeyRange {
    /// Validate that the key range has at least one non-empty bound.
    ///
    /// This method ensures that a KeyRange filter is meaningful by requiring at least
    /// one of `start` or `end` to be specified and non-empty. A key range with both
    /// bounds missing or empty would match all keys, which should be expressed by
    /// omitting the key_range filter entirely.
    ///
    /// # Arguments
    ///
    /// * `context` - Error context string for detailed failure messages
    ///
    /// # Returns
    ///
    /// `Ok(())` if at least one bound is specified and non-empty.
    ///
    /// # Errors
    ///
    /// Returns an error if both start and end are None or empty strings.
    fn validate(&self, context: &str) -> Result<()> {
        if self.start.as_deref().is_none_or(str::is_empty)
            && self.end.as_deref().is_none_or(str::is_empty)
        {
            bail!("{context}: key_range requires at least 'start' or 'end'");
        }
        Ok(())
    }

    /// Generate a compact string representation of the key range for display.
    ///
    /// This method creates a human-readable summary using Rust's range syntax
    /// (e.g., "aaa..zzz") for use in logs, reports, and error messages. Empty
    /// bounds are shown as empty strings.
    ///
    /// # Returns
    ///
    /// A string in the format "start..end" where either bound may be empty if unspecified.
    pub fn summary(&self) -> String {
        let start = self.start.as_deref().unwrap_or("");
        let end = self.end.as_deref().unwrap_or("");
        format!("{start}..{end}")
    }
}

/// Supported step kinds within a migration plan.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PlanStep {
    Copy(CopyStep),
    Delete(DeleteStep),
    Upsert(UpsertStep),
    Verify(VerifyStep),
}

impl PlanStep {
    /// Delegate validation to the specific step type implementation.
    ///
    /// This method performs comprehensive validation for the specific step type,
    /// ensuring that all required fields are present and properly configured. Each
    /// step type has its own validation requirements:
    ///
    /// - **Copy**: Validates filters and transformations (jq expression non-empty)
    /// - **Delete**: Validates filters for proper configuration
    /// - **Upsert**: Ensures at least one entry is present and validates each entry
    /// - **Verify**: Validates filters and assertion configuration
    ///
    /// # Arguments
    ///
    /// * `index` - The step's position in the migration plan (0-based), used for error context
    ///
    /// # Returns
    ///
    /// `Ok(())` if the step is valid and ready for execution.
    ///
    /// # Errors
    ///
    /// Returns a detailed error describing the validation failure, including the step
    /// index and specific field that failed validation.
    fn validate(&self, index: usize) -> Result<()> {
        match self {
            Self::Copy(step) => step.validate(index),
            Self::Delete(step) => step.validate(index),
            Self::Upsert(step) => step.validate(index),
            Self::Verify(step) => step.validate(index),
        }
    }

    /// Return the kebab-case kind string for this step type.
    ///
    /// This method provides the canonical string representation of each step type,
    /// matching the YAML tag names used in migration plan documents.
    ///
    /// # Returns
    ///
    /// One of: "copy", "delete", "upsert", or "verify".
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Copy(_) => "copy",
            Self::Delete(_) => "delete",
            Self::Upsert(_) => "upsert",
            Self::Verify(_) => "verify",
        }
    }

    /// Extract the optional human-readable name from any step type.
    ///
    /// This method retrieves the user-specified name field (if present) from a step.
    /// Step names are optional and appear in CLI output, logs, and error messages to
    /// help users identify specific steps in multi-step migrations.
    ///
    /// # Returns
    ///
    /// `Some(&str)` if the step has a name, `None` otherwise.
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Copy(step) => step.name.as_deref(),
            Self::Delete(step) => step.name.as_deref(),
            Self::Upsert(step) => step.name.as_deref(),
            Self::Verify(step) => step.name.as_deref(),
        }
    }

    /// Extract the column family that this step operates on.
    ///
    /// Every migration step targets a specific RocksDB column family (State, Meta,
    /// Config, Identity, Delta, Alias, Application, or Generic). This method provides
    /// uniform access to the column field across all step types.
    ///
    /// # Returns
    ///
    /// The `Column` that this step will read from or write to.
    pub const fn column(&self) -> Column {
        match self {
            Self::Copy(step) => step.column,
            Self::Delete(step) => step.column,
            Self::Upsert(step) => step.column,
            Self::Verify(step) => step.column,
        }
    }

    /// Extract the filter configuration from steps that support filtering.
    ///
    /// This method provides access to the filter configuration for steps that scan
    /// database contents (Copy, Delete, Verify). Upsert steps don't have filters
    /// because they write literal key-value entries without scanning.
    ///
    /// The returned filters may be merged with plan-level defaults before being
    /// applied during execution.
    ///
    /// # Returns
    ///
    /// `Some(&PlanFilters)` for Copy, Delete, and Verify steps, `None` for Upsert.
    pub const fn filters(&self) -> Option<&PlanFilters> {
        match self {
            Self::Copy(step) => Some(&step.filters),
            Self::Delete(step) => Some(&step.filters),
            Self::Verify(step) => Some(&step.filters),
            Self::Upsert(_) => None,
        }
    }

    /// Extract the safety guards configuration from any step type.
    ///
    /// This method provides uniform access to the guards field across all step types.
    /// Guards control when a step can execute, enforcing safety requirements like:
    /// - Target database must pass validation checks
    /// - Target database must be empty
    ///
    /// # Returns
    ///
    /// A reference to the step's `StepGuards` configuration.
    pub const fn guards(&self) -> &StepGuards {
        match self {
            Self::Copy(step) => &step.guards,
            Self::Delete(step) => &step.guards,
            Self::Upsert(step) => &step.guards,
            Self::Verify(step) => &step.guards,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CopyStep {
    #[serde(default)]
    pub name: Option<String>,
    pub column: Column,
    #[serde(default)]
    pub filters: PlanFilters,
    #[serde(default)]
    pub transform: CopyTransform,
    #[serde(default)]
    pub guards: StepGuards,
    #[serde(default)]
    pub batch_size: Option<usize>,
}

impl CopyStep {
    /// Validate that filters and transformation configuration are well-formed.
    ///
    /// This method ensures that the copy step's configuration is valid by:
    /// 1. Validating the filters (key ranges, context IDs, prefixes)
    /// 2. Validating the transformation (jq expression must be non-empty if specified)
    ///
    /// # Arguments
    ///
    /// * `index` - The step's position in the migration plan (0-based), used for error context
    ///
    /// # Returns
    ///
    /// `Ok(())` if the copy step is valid and ready for execution.
    ///
    /// # Errors
    ///
    /// Returns an error if filters or transformations are invalid (e.g., empty jq expression,
    /// invalid key range bounds).
    fn validate(&self, index: usize) -> Result<()> {
        self.filters
            .validate(&format!("steps[{index}].copy.filters"))?;
        self.transform
            .validate(&format!("steps[{index}].copy.transform"))?;
        Ok(())
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CopyTransform {
    #[serde(default)]
    pub decode_with_abi: Option<bool>,
    #[serde(default)]
    pub jq: Option<String>,
}

impl CopyTransform {
    /// Validate that the transformation configuration is well-formed.
    ///
    /// This method ensures that if a jq transformation expression is specified, it
    /// contains actual content and is not just whitespace. Empty jq expressions would
    /// cause runtime errors during transformation, so they're rejected early.
    ///
    /// # Arguments
    ///
    /// * `context` - Error context string for detailed failure messages
    ///
    /// # Returns
    ///
    /// `Ok(())` if the transformation is valid or not specified.
    ///
    /// # Errors
    ///
    /// Returns an error if the jq field is Some but contains only whitespace.
    fn validate(&self, context: &str) -> Result<()> {
        if let Some(jq) = &self.jq {
            ensure!(
                !jq.trim().is_empty(),
                "{context}: jq transform must not be an empty string"
            );
        }
        Ok(())
    }

    /// Generate a human-readable summary of active transformations for display.
    ///
    /// This method creates a concise representation of the copy transformation
    /// configuration, showing which transformations are enabled. It's used in
    /// dry-run reports and execution logs.
    ///
    /// # Returns
    ///
    /// `Some(String)` with transformation details if any are configured,
    /// `None` if no transformations are active (straight copy).
    pub fn summary(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(flag) = self.decode_with_abi {
            parts.push(format!("decode_with_abi={flag}"));
        }
        if let Some(jq) = &self.jq {
            parts.push(format!("jq={jq}"));
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(", "))
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct DeleteStep {
    #[serde(default)]
    pub name: Option<String>,
    pub column: Column,
    #[serde(default)]
    pub filters: PlanFilters,
    #[serde(default)]
    pub guards: StepGuards,
    #[serde(default)]
    pub batch_size: Option<usize>,
}

impl DeleteStep {
    /// Validate that the delete step's filter configuration is well-formed.
    ///
    /// This method ensures that the delete step's filters are valid by checking:
    /// 1. Key range bounds are properly specified (at least start or end)
    /// 2. All filter fields are consistent and non-empty where required
    ///
    /// # Arguments
    ///
    /// * `index` - The step's position in the migration plan (0-based), used for error context
    ///
    /// # Returns
    ///
    /// `Ok(())` if the delete step is valid and ready for execution.
    ///
    /// # Errors
    ///
    /// Returns an error if filters are invalid (e.g., invalid key range configuration).
    fn validate(&self, index: usize) -> Result<()> {
        self.filters
            .validate(&format!("steps[{index}].delete.filters"))?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct UpsertStep {
    #[serde(default)]
    pub name: Option<String>,
    pub column: Column,
    pub entries: Vec<UpsertEntry>,
    #[serde(default)]
    pub guards: StepGuards,
}

impl UpsertStep {
    /// Validate that the upsert step has at least one entry and all entries are well-formed.
    ///
    /// This method ensures that the upsert step's configuration is valid by:
    /// 1. Requiring at least one key-value entry (empty upserts are meaningless)
    /// 2. Validating each entry's key and value encoding (hex, base64, utf8, json)
    /// 3. Ensuring encoded values are non-empty where required
    ///
    /// # Arguments
    ///
    /// * `index` - The step's position in the migration plan (0-based), used for error context
    ///
    /// # Returns
    ///
    /// `Ok(())` if the upsert step has valid entries.
    ///
    /// # Errors
    ///
    /// Returns an error if the entries list is empty or if any entry has invalid encoding.
    fn validate(&self, index: usize) -> Result<()> {
        if self.entries.is_empty() {
            bail!("steps[{index}].upsert.entries must contain at least one entry");
        }
        for (entry_index, entry) in self.entries.iter().enumerate() {
            entry.validate(index, entry_index)?;
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct UpsertEntry {
    pub key: EncodedValue,
    pub value: EncodedValue,
}

impl UpsertEntry {
    /// Validate that both the key and value are properly encoded and non-empty.
    ///
    /// This method ensures that an upsert entry's key-value pair is well-formed by
    /// validating both the key and value encodings. For hex, base64, and utf8 encodings,
    /// this checks that the data is non-empty after trimming whitespace.
    ///
    /// # Arguments
    ///
    /// * `step_index` - The parent step's position in the migration plan (0-based)
    /// * `entry_index` - This entry's position within the upsert entries list (0-based)
    ///
    /// # Returns
    ///
    /// `Ok(())` if both key and value are valid.
    ///
    /// # Errors
    ///
    /// Returns an error with detailed context if either the key or value is invalid
    /// (e.g., empty string for hex/base64/utf8 encoding).
    fn validate(&self, step_index: usize, entry_index: usize) -> Result<()> {
        self.key.validate(&format!(
            "steps[{step_index}].upsert.entries[{entry_index}].key"
        ))?;
        self.value.validate(&format!(
            "steps[{step_index}].upsert.entries[{entry_index}].value"
        ))?;
        Ok(())
    }
}

/// Safety guards that control when a step can execute.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct StepGuards {
    /// Require the target database to pass existing validation logic before executing this step.
    #[serde(default)]
    pub requires_validation: bool,
    /// Require the target database to be empty before executing this step.
    #[serde(default)]
    pub requires_empty_target: bool,
}

impl StepGuards {
    /// Check if any guards are configured.
    pub const fn has_guards(&self) -> bool {
        self.requires_validation || self.requires_empty_target
    }
}

/// How values are provided inline within YAML plan documents.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "encoding", rename_all = "kebab-case")]
pub enum EncodedValue {
    Hex { data: String },
    Base64 { data: String },
    Utf8 { data: String },
    Json { value: JsonValue },
}

impl EncodedValue {
    /// Validate that the encoded value contains actual data.
    ///
    /// This method ensures that encoded values are meaningful and not just whitespace.
    /// For hex, base64, and utf8 encodings, it checks that the data string is non-empty
    /// after trimming. JSON values are always considered valid as they're structured data.
    ///
    /// # Arguments
    ///
    /// * `context` - Error context string for detailed failure messages
    ///
    /// # Returns
    ///
    /// `Ok(())` if the value contains actual data.
    ///
    /// # Errors
    ///
    /// Returns an error if a hex/base64/utf8 data field is empty or contains only whitespace.
    fn validate(&self, context: &str) -> Result<()> {
        match self {
            Self::Hex { data } | Self::Base64 { data } | Self::Utf8 { data } => {
                ensure!(
                    !data.trim().is_empty(),
                    "{context}: value must not be empty"
                );
            }
            Self::Json { .. } => {}
        }
        Ok(())
    }

    /// Decode the encoded value into raw bytes for database operations.
    ///
    /// This method converts the encoded representation (hex, base64, utf8, or json)
    /// into the raw byte sequence that will be used as a database key or value.
    ///
    /// **Encoding rules**:
    /// - **Hex**: Decodes hex string (with or without "0x" prefix) to bytes
    /// - **Base64**: Decodes standard base64 string to bytes
    /// - **Utf8**: Converts string to UTF-8 bytes
    /// - **Json**: Serializes JSON value to string, then converts to bytes
    ///
    /// # Returns
    ///
    /// A `Vec<u8>` containing the decoded bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Hex string contains invalid hex characters
    /// - Base64 string is malformed
    /// - (Utf8 and Json encodings always succeed)
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        match self {
            Self::Hex { data } => {
                let trimmed = data.trim_start_matches("0x");
                Ok(hex::decode(trimmed)?)
            }
            Self::Base64 { data } => Ok(BASE64_ENGINE.decode(data)?),
            Self::Utf8 { data } => Ok(data.as_bytes().to_vec()),
            Self::Json { value } => Ok(value.to_string().into_bytes()),
        }
    }

    /// Return the kebab-case encoding type name for this value.
    ///
    /// This method provides the canonical string representation of the encoding type,
    /// matching the YAML tag names used in migration plan documents.
    ///
    /// # Returns
    ///
    /// One of: "hex", "base64", "utf8", or "json".
    pub const fn encoding_label(&self) -> &'static str {
        match self {
            Self::Hex { .. } => "hex",
            Self::Base64 { .. } => "base64",
            Self::Utf8 { .. } => "utf8",
            Self::Json { .. } => "json",
        }
    }

    /// Generate a truncated preview of the encoded value for display in logs and reports.
    ///
    /// This method creates a human-readable preview of the value by truncating it to
    /// the specified maximum character length. If the value exceeds the limit, it's
    /// truncated and an ellipsis (…) is appended. This is useful for displaying keys
    /// and values in verification summaries without overwhelming the output.
    ///
    /// **Character counting**:
    /// - Counts Unicode characters, not bytes
    /// - Truncates at character boundaries (won't split grapheme clusters incorrectly)
    /// - Uses Unicode ellipsis (…) for truncation indicator
    ///
    /// # Arguments
    ///
    /// * `max_len` - Maximum number of characters to include before truncation
    ///
    /// # Returns
    ///
    /// A string representation of the value, truncated if longer than `max_len`.
    pub fn preview(&self, max_len: usize) -> String {
        fn truncate(value: &str, max_len: usize) -> String {
            let mut truncated = String::new();
            for (idx, ch) in value.chars().enumerate() {
                if idx >= max_len {
                    truncated.push('…');
                    break;
                }
                truncated.push(ch);
            }
            truncated
        }

        match self {
            Self::Hex { data } | Self::Base64 { data } | Self::Utf8 { data } => {
                truncate(data, max_len)
            }
            Self::Json { value } => truncate(&value.to_string(), max_len),
        }
    }
}

/// Represents a `type: verify` step that evaluates assertions against filtered column data.
#[derive(Debug, Deserialize)]
pub struct VerifyStep {
    #[serde(default)]
    /// Optional human-readable label that appears in CLI output and logs.
    pub name: Option<String>,
    /// Column family to scan when evaluating the assertion.
    pub column: Column,
    #[serde(default)]
    /// Column/row filters that scope the verification to a subset of keys.
    pub filters: PlanFilters,
    /// Condition that must hold true for the filtered data; failure aborts the plan.
    pub assertion: VerificationAssertion,
    #[serde(default)]
    /// Safety guards that control when this verification step can execute.
    pub guards: StepGuards,
}

impl VerifyStep {
    /// Validate that filters and assertion configuration are well-formed.
    ///
    /// This method ensures that the verify step's configuration is valid by:
    /// 1. Validating the filters (key ranges, context IDs, prefixes)
    /// 2. Validating the assertion (encoded keys must be decodable)
    ///
    /// Verification steps are critical for ensuring database state integrity, so
    /// early validation prevents runtime failures during migration execution.
    ///
    /// # Arguments
    ///
    /// * `index` - The step's position in the migration plan (0-based), used for error context
    ///
    /// # Returns
    ///
    /// `Ok(())` if the verify step is valid and ready for execution.
    ///
    /// # Errors
    ///
    /// Returns an error if filters or assertions are invalid (e.g., empty key encoding,
    /// invalid key range bounds).
    fn validate(&self, index: usize) -> Result<()> {
        self.filters
            .validate(&format!("steps[{index}].verify.filters"))?;
        self.assertion
            .validate(&format!("steps[{index}].verify.assertion"))?;
        Ok(())
    }
}

/// Declarative assertions checked at the end of a verify step.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum VerificationAssertion {
    /// Expect the filtered row count to be exactly `expected_count`.
    ExpectedCount { expected_count: u64 },
    /// Require the filtered row count to be at least `min_count`.
    MinCount { min_count: u64 },
    /// Require the filtered row count to be at most `max_count`.
    MaxCount { max_count: u64 },
    /// Ensure a specific key exists within the filtered results.
    ContainsKey { contains_key: EncodedValue },
    /// Ensure a specific key is absent from the filtered results.
    MissingKey { missing_key: EncodedValue },
}

impl VerificationAssertion {
    /// Validate that the assertion is well-formed and can be evaluated.
    ///
    /// This method ensures that assertion parameters are valid:
    /// - **Count assertions** (ExpectedCount, MinCount, MaxCount): Always valid (numeric checks)
    /// - **Key assertions** (ContainsKey, MissingKey): Validates that encoded keys are non-empty
    ///
    /// # Arguments
    ///
    /// * `context` - Error context string for detailed failure messages
    ///
    /// # Returns
    ///
    /// `Ok(())` if the assertion can be evaluated.
    ///
    /// # Errors
    ///
    /// Returns an error if a ContainsKey or MissingKey assertion has an empty key encoding.
    fn validate(&self, context: &str) -> Result<()> {
        match self {
            Self::ExpectedCount { .. } | Self::MinCount { .. } | Self::MaxCount { .. } => {}
            Self::ContainsKey { contains_key } => {
                contains_key.validate(&format!("{context}.contains_key"))?;
            }
            Self::MissingKey { missing_key } => {
                missing_key.validate(&format!("{context}.missing_key"))?;
            }
        }
        Ok(())
    }

    /// Generate a human-readable summary of the assertion for display in reports.
    ///
    /// This method creates a concise, descriptive string explaining what the assertion
    /// checks. It's used in dry-run previews and verification step output to help users
    /// understand what conditions will be verified.
    ///
    /// **Summary formats**:
    /// - **ExpectedCount**: "expect count == N"
    /// - **MinCount**: "expect count >= N"
    /// - **MaxCount**: "expect count <= N"
    /// - **ContainsKey**: "expect key present (encoding, preview: ...)"
    /// - **MissingKey**: "expect key missing (encoding, preview: ...)"
    ///
    /// # Returns
    ///
    /// A human-readable assertion summary string.
    pub fn summary(&self) -> String {
        match self {
            Self::ExpectedCount { expected_count } => {
                format!("expect count == {expected_count}")
            }
            Self::MinCount { min_count } => {
                format!("expect count >= {min_count}")
            }
            Self::MaxCount { max_count } => {
                format!("expect count <= {max_count}")
            }
            Self::ContainsKey { contains_key } => format!(
                "expect key present ({}, preview: {})",
                contains_key.encoding_label(),
                contains_key.preview(16)
            ),
            Self::MissingKey { missing_key } => format!(
                "expect key missing ({}, preview: {})",
                missing_key.encoding_label(),
                missing_key.preview(16)
            ),
        }
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;
    use std::path::PathBuf;

    fn valid_plan() -> MigrationPlan {
        MigrationPlan {
            version: PlanVersion::latest(),
            name: None,
            description: None,
            source: SourceEndpoint {
                db_path: PathBuf::from("/tmp/source-db"),
                wasm_file: None,
            },
            target: None,
            defaults: PlanDefaults::default(),
            steps: vec![PlanStep::Copy(CopyStep {
                name: None,
                column: Column::State,
                filters: PlanFilters::default(),
                transform: CopyTransform::default(),
                guards: StepGuards::default(),
                batch_size: None,
            })],
        }
    }

    #[test]
    fn rejects_unsupported_version() {
        let mut plan = valid_plan();
        plan.version = PlanVersion::new(42);
        let error = plan.validate().unwrap_err().to_string();
        assert!(error.contains("Unsupported migration plan version"));
    }

    #[test]
    fn rejects_empty_step_list() {
        let mut plan = valid_plan();
        plan.steps.clear();
        let error = plan.validate().unwrap_err().to_string();
        assert!(error.contains("Migration plan must define at least one step"));
    }

    #[test]
    fn rejects_empty_source_path() {
        let mut plan = valid_plan();
        plan.source.db_path = PathBuf::new();
        let error = plan.validate().unwrap_err().to_string();
        assert!(error.contains("db_path must not be empty"));
    }

    #[test]
    fn rejects_invalid_key_range() {
        let mut plan = valid_plan();
        plan.steps = vec![PlanStep::Copy(CopyStep {
            name: None,
            column: Column::State,
            filters: PlanFilters {
                key_range: Some(KeyRange {
                    start: None,
                    end: None,
                }),
                ..PlanFilters::default()
            },
            transform: CopyTransform::default(),
            guards: StepGuards::default(),
            batch_size: None,
        })];

        let error = plan.validate().unwrap_err().to_string();
        assert!(error.contains("key_range requires at least 'start' or 'end'"));
    }

    #[test]
    fn rejects_blank_jq_transform() {
        let mut plan = valid_plan();
        plan.steps = vec![PlanStep::Copy(CopyStep {
            name: None,
            column: Column::State,
            filters: PlanFilters::default(),
            transform: CopyTransform {
                decode_with_abi: None,
                jq: Some("   ".into()),
            },
            guards: StepGuards::default(),
            batch_size: None,
        })];

        let error = plan.validate().unwrap_err().to_string();
        assert!(error.contains("jq transform must not be an empty string"));
    }

    #[test]
    fn rejects_upsert_with_no_entries() {
        let mut plan = valid_plan();
        plan.steps = vec![PlanStep::Upsert(UpsertStep {
            name: None,
            column: Column::Alias,
            entries: Vec::new(),
            guards: StepGuards::default(),
        })];

        let error = plan.validate().unwrap_err().to_string();
        assert!(error.contains("upsert.entries must contain at least one entry"));
    }

    #[test]
    fn accepts_valid_plan() {
        let plan = valid_plan();
        assert!(plan.validate().is_ok());
    }

    #[test]
    fn plan_filters_is_empty_and_summary() {
        let default_filters = PlanFilters::default();
        assert!(default_filters.is_empty());

        let filters = PlanFilters {
            context_ids: vec!["0xabc".into()],
            context_aliases: vec!["marketing".into()],
            ..PlanFilters::default()
        };
        assert!(!filters.is_empty());
        let summary = filters.summary().expect("summary should exist");
        assert!(summary.contains("context_ids=1"));
        assert!(summary.contains("context_aliases=marketing"));
    }

    #[test]
    fn plan_filters_merge_prefers_overrides() {
        let defaults = PlanFilters {
            context_ids: vec!["default".into()],
            state_key_prefix: Some("aaaa".into()),
            ..PlanFilters::default()
        };

        let overrides = PlanFilters {
            state_key_prefix: Some("bbbb".into()),
            ..PlanFilters::default()
        };

        let merged = defaults.merged_with(&overrides);
        assert_eq!(merged.context_ids, defaults.context_ids);
        assert_eq!(merged.state_key_prefix.as_deref(), Some("bbbb"));
    }

    #[test]
    fn encoded_value_to_bytes_decodes() {
        let hex_value = EncodedValue::Hex {
            data: "0x0a0b".into(),
        }
        .to_bytes()
        .expect("hex decode");
        assert_eq!(hex_value, vec![0x0a, 0x0b]);

        let base64_value = EncodedValue::Base64 {
            data: BASE64_ENGINE.encode("hi"),
        }
        .to_bytes()
        .expect("base64 decode");
        assert_eq!(base64_value, b"hi");

        let utf8_value = EncodedValue::Utf8 {
            data: "hello".into(),
        }
        .to_bytes()
        .expect("utf8 decode");
        assert_eq!(utf8_value, b"hello");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_plan_document() {
        let yaml = r#"
version: 1
name: sample-plan
description: Copy a subset of state and assert record counts
source:
  db_path: /var/lib/source-db
  wasm_file: ./contracts/sample.wasm
target:
  db_path: /var/lib/target-db
  backup_dir: ./backups/latest
defaults:
  columns: ["State", "Delta"]
  decode_with_abi: true
  filters:
    context_ids:
      - "0xabc123"
steps:
  - type: copy
    name: copy-context-state
    column: State
    filters:
      state_key_prefix: claims/
    transform:
      decode_with_abi: true
      jq: ".value.parsed | del(.metadata)"
  - type: delete
    name: purge-old-alias
    column: Alias
    filters:
      alias_name: marketplace-old
  - type: upsert
    name: seed-alias
    column: Alias
    entries:
      - key:
          encoding: utf8
          data: marketplace
        value:
          encoding: hex
          data: deadbeef
  - type: verify
    name: ensure-delta-count
    column: Delta
    assertion:
      expected_count: 12
"#;

        let plan: MigrationPlan = serde_yaml::from_str(yaml).expect("plan should deserialize");
        assert_eq!(plan.version, PlanVersion::latest());
        assert_eq!(plan.steps.len(), 4);
        plan.validate().expect("plan should validate");
    }
}
