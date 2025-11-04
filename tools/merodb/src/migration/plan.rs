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
    /// Ensure version compatibility, non-empty endpoints, and that every step passes its own invariants.
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
    /// Ensure provided paths are not empty strings so downstream checks may trust them.
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
    /// Ensure the plan points at usable path strings.
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
}

impl PlanDefaults {
    fn validate(&self, context: &str) -> Result<()> {
        self.filters.validate(&format!("{context}.filters"))?;
        Ok(())
    }

    /// Combine default filters with step-specific filters, preferring step overrides when present.
    pub fn merge_filters(&self, overrides: &PlanFilters) -> PlanFilters {
        self.filters.merged_with(overrides)
    }

    /// Resolve the effective `decode_with_abi` flag, allowing step overrides to take precedence.
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
    fn validate(&self, context: &str) -> Result<()> {
        if let Some(range) = &self.key_range {
            range.validate(&format!("{context}.key_range"))?;
        }
        Ok(())
    }

    pub const fn is_empty(&self) -> bool {
        self.context_ids.is_empty()
            && self.context_aliases.is_empty()
            && self.state_key_prefix.is_none()
            && self.raw_key_prefix.is_none()
            && self.alias_name.is_none()
            && self.key_range.is_none()
    }

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

    /// Merge two filter sets, where empty/`None` fields in `overrides` inherit from `self`.
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
    fn validate(&self, context: &str) -> Result<()> {
        if self.start.as_deref().is_none_or(str::is_empty)
            && self.end.as_deref().is_none_or(str::is_empty)
        {
            bail!("{context}: key_range requires at least 'start' or 'end'");
        }
        Ok(())
    }

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
    fn validate(&self, index: usize) -> Result<()> {
        match self {
            Self::Copy(step) => step.validate(index),
            Self::Delete(step) => step.validate(index),
            Self::Upsert(step) => step.validate(index),
            Self::Verify(step) => step.validate(index),
        }
    }

    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Copy(_) => "copy",
            Self::Delete(_) => "delete",
            Self::Upsert(_) => "upsert",
            Self::Verify(_) => "verify",
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Copy(step) => step.name.as_deref(),
            Self::Delete(step) => step.name.as_deref(),
            Self::Upsert(step) => step.name.as_deref(),
            Self::Verify(step) => step.name.as_deref(),
        }
    }

    /// Column against which the step operates.
    pub const fn column(&self) -> Column {
        match self {
            Self::Copy(step) => step.column,
            Self::Delete(step) => step.column,
            Self::Upsert(step) => step.column,
            Self::Verify(step) => step.column,
        }
    }

    /// Optional column filters that apply to the step.
    pub const fn filters(&self) -> Option<&PlanFilters> {
        match self {
            Self::Copy(step) => Some(&step.filters),
            Self::Delete(step) => Some(&step.filters),
            Self::Verify(step) => Some(&step.filters),
            Self::Upsert(_) => None,
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
}

impl CopyStep {
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
    fn validate(&self, context: &str) -> Result<()> {
        if let Some(jq) = &self.jq {
            ensure!(
                !jq.trim().is_empty(),
                "{context}: jq transform must not be an empty string"
            );
        }
        Ok(())
    }

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
}

impl DeleteStep {
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
}

impl UpsertStep {
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

    /// Decode the encoded value into raw bytes for key comparison.
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

    pub const fn encoding_label(&self) -> &'static str {
        match self {
            Self::Hex { .. } => "hex",
            Self::Base64 { .. } => "base64",
            Self::Utf8 { .. } => "utf8",
            Self::Json { .. } => "json",
        }
    }

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
}

impl VerifyStep {
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
