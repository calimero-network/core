#![allow(dead_code)]

use std::fmt;
use std::path::PathBuf;

use eyre::{bail, ensure, Result};
use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::types::Column;

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
    pub fn validate(&self) -> Result<()> {
        self.version.ensure_supported()?;
        self.source.validate("source")?;
        if let Some(target) = &self.target {
            target.validate("target")?;
        }
        self.defaults.validate("defaults")?;

        if self.steps.is_empty() {
            bail!("Migration plan must define at least one step");
        }

        for (index, step) in self.steps.iter().enumerate() {
            step.validate(index)?;
        }

        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct SourceEndpoint {
    pub db_path: PathBuf,
    #[serde(default)]
    pub wasm_file: Option<PathBuf>,
}

impl SourceEndpoint {
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

#[derive(Debug, Deserialize)]
pub struct TargetEndpoint {
    pub db_path: PathBuf,
    #[serde(default)]
    pub backup_dir: Option<PathBuf>,
}

impl TargetEndpoint {
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
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PlanFilters {
    #[serde(default)]
    pub context_ids: Vec<String>,
    #[serde(default)]
    pub context_aliases: Vec<String>,
    #[serde(default)]
    pub state_key_prefix: Option<String>,
    #[serde(default)]
    pub raw_key_prefix: Option<String>,
    #[serde(default)]
    pub alias_name: Option<String>,
    #[serde(default)]
    pub key_range: Option<KeyRange>,
}

impl PlanFilters {
    fn validate(&self, context: &str) -> Result<()> {
        if let Some(range) = &self.key_range {
            range.validate(&format!("{context}.key_range"))?;
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.context_ids.is_empty()
            && self.context_aliases.is_empty()
            && self.state_key_prefix.is_none()
            && self.raw_key_prefix.is_none()
            && self.alias_name.is_none()
            && self.key_range.is_none()
    }
}

#[derive(Debug, Deserialize)]
pub struct KeyRange {
    pub start: Option<String>,
    pub end: Option<String>,
}

impl KeyRange {
    fn validate(&self, context: &str) -> Result<()> {
        if self.start.as_deref().map_or(true, str::is_empty)
            && self.end.as_deref().map_or(true, str::is_empty)
        {
            bail!("{context}: key_range requires at least 'start' or 'end'");
        }
        Ok(())
    }
}

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
            EncodedValue::Hex { data }
            | EncodedValue::Base64 { data }
            | EncodedValue::Utf8 { data } => {
                ensure!(
                    !data.trim().is_empty(),
                    "{context}: value must not be empty"
                );
            }
            EncodedValue::Json { .. } => {}
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct VerifyStep {
    #[serde(default)]
    pub name: Option<String>,
    pub column: Column,
    #[serde(default)]
    pub filters: PlanFilters,
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

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum VerificationAssertion {
    ExpectedCount { expected_count: u64 },
    MinCount { min_count: u64 },
    MaxCount { max_count: u64 },
    ContainsKey { contains_key: EncodedValue },
    MissingKey { missing_key: EncodedValue },
}

impl VerificationAssertion {
    fn validate(&self, context: &str) -> Result<()> {
        match self {
            VerificationAssertion::ExpectedCount { .. }
            | VerificationAssertion::MinCount { .. }
            | VerificationAssertion::MaxCount { .. } => {}
            VerificationAssertion::ContainsKey { contains_key } => {
                contains_key.validate(&format!("{context}.contains_key"))?;
            }
            VerificationAssertion::MissingKey { missing_key } => {
                missing_key.validate(&format!("{context}.missing_key"))?;
            }
        }
        Ok(())
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
