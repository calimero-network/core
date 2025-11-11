use std::fs;
use std::path::Path;

use eyre::{ensure, Result, WrapErr};

use super::plan::MigrationPlan;

/// Parse a migration plan from the provided YAML string.
pub fn parse_plan_str(contents: &str) -> Result<MigrationPlan> {
    let plan: MigrationPlan =
        serde_yaml::from_str(contents).wrap_err("Failed to parse migration plan as YAML")?;

    plan.validate()
        .wrap_err("Migration plan validation failed")?;

    Ok(plan)
}

/// Load and validate a migration plan from disk.
pub fn load_plan(path: &Path) -> Result<MigrationPlan> {
    let ext_matches = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml"));

    ensure!(
        ext_matches,
        "Migration plan must be a .yaml or .yml file: {}",
        path.display()
    );

    let contents = fs::read_to_string(path)
        .wrap_err_with(|| format!("Failed to read migration plan file {}", path.display()))?;

    parse_plan_str(&contents).wrap_err_with(|| {
        format!(
            "Failed to decode migration plan from YAML (file: {})",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_PLAN: &str = r#"
version: 1
source:
  db_path: /source
defaults:
  columns: ["State"]
steps:
  - type: copy
    column: State
  - type: verify
    column: State
    assertion:
      min_count: 1
"#;

    #[test]
    fn parses_yaml_plan() {
        let plan = parse_plan_str(SAMPLE_PLAN).expect("valid plan");
        assert_eq!(plan.steps.len(), 2);
    }
}
