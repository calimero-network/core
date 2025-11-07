use std::fs;
use std::path::Path;

use eyre::{Result, WrapErr};
use serde_json::json;

use crate::types::Column;

use super::context::{AbiManifestStatus, MigrationContext};
use super::dry_run::DryRunReport;
use super::plan::PlanFilters;

/// Persist the dry-run report as pretty JSON at the requested destination.
pub fn write_json_report(
    destination: &Path,
    plan_path: &Path,
    context: &MigrationContext,
    report: &DryRunReport,
) -> Result<()> {
    let payload = build_report_payload(plan_path, context, report);

    let json_string = serde_json::to_string_pretty(&payload)?;
    let json_with_newline = format!("{json_string}\n");

    fs::write(destination, json_with_newline).wrap_err_with(|| {
        format!(
            "Failed to write migration report to {}",
            destination.display()
        )
    })?;

    println!("Dry-run report written to {}", destination.display());

    Ok(())
}

/// Build the structured JSON payload used by `--report`.
pub fn build_report_payload(
    plan_path: &Path,
    context: &MigrationContext,
    report: &DryRunReport,
) -> serde_json::Value {
    let plan = context.plan();

    let defaults_columns: Vec<&str> = plan.defaults.columns.iter().map(Column::as_str).collect();

    let plan_steps = plan
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let preview = report.steps.iter().find(|entry| entry.index == index);
            json!({
                "index": index,
                "number": index.saturating_add(1),
                "kind": step.kind(),
                "name": step.name(),
                "column": step.column().as_str(),
                "filters_summary": step.filters().and_then(PlanFilters::summary),
                "dry_run": preview,
            })
        })
        .collect::<Vec<_>>();

    let plan_defaults = json!({
        "columns": defaults_columns,
        "decode_with_abi": plan.defaults.decode_with_abi,
        "write_if_missing": plan.defaults.write_if_missing,
        "filters_summary": plan.defaults.filters.summary(),
    });

    let plan_target = plan.target.as_ref().map(|target| {
        json!({
            "db_path": target.db_path.display().to_string(),
            "backup_dir": target
                .backup_dir
                .as_ref()
                .map(|path| path.display().to_string()),
        })
    });

    let plan_json = json!({
        "version": plan.version.as_u32(),
        "name": plan.name,
        "description": plan.description,
        "source": {
            "db_path": plan.source.db_path.display().to_string(),
            "wasm_file": plan
                .source
                .wasm_file
                .as_ref()
                .map(|path| path.display().to_string()),
        },
        "target": plan_target,
        "defaults": plan_defaults,
        "steps": plan_steps,
    });

    let abi_status = match context.source().abi_status() {
        AbiManifestStatus::NotConfigured => json!({
            "status": "not_configured",
        }),
        AbiManifestStatus::Loaded => json!({
            "status": "loaded",
        }),
        AbiManifestStatus::Pending { wasm_path } => json!({
            "status": "pending",
            "wasm_path": wasm_path.display().to_string(),
        }),
    };

    let target_context = context.target().map(|target| {
        json!({
            "db_path": target.path().display().to_string(),
            "read_only": target.is_read_only(),
            "backup_dir": target
                .backup_dir()
                .map(|path| path.display().to_string()),
        })
    });

    json!({
        "plan_path": plan_path.display().to_string(),
        "mode": if context.is_dry_run() { "dry-run" } else { "apply" },
        "plan": plan_json,
        "context": {
            "source": {
                "db_path": context.source().path().display().to_string(),
                "abi": abi_status,
            },
            "target": target_context,
        },
        "dry_run": report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::context::MigrationOverrides;
    use crate::migration::dry_run::generate_report;
    use crate::migration::plan::{
        CopyStep, CopyTransform, MigrationPlan, PlanDefaults, PlanFilters, PlanStep, PlanVersion,
        SourceEndpoint, StepGuards, VerificationAssertion, VerifyStep,
    };
    use eyre::{ensure, eyre, Result};
    use rocksdb::{ColumnFamilyDescriptor, Options, WriteBatch, DB};
    use tempfile::TempDir;

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

        let mut state_key = [0_u8; 64];
        state_key[..32].copy_from_slice(&[0x11; 32]);
        state_key[32..64].copy_from_slice(&[0x22; 32]);

        let mut batch = WriteBatch::default();
        batch.put_cf(cf_state, state_key, b"value-1");
        db.write(batch)?;

        drop(db);
        Ok(())
    }

    fn basic_plan(path: &Path) -> MigrationPlan {
        MigrationPlan {
            version: PlanVersion::latest(),
            name: Some("sample-plan".into()),
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
    fn json_report_includes_plan_and_preview_sections() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("db");
        setup_db(&db_path)?;

        let plan_path = temp_dir.path().join("plan.yaml");
        fs::write(&plan_path, "version: 1\n")?;

        let plan = basic_plan(&db_path);
        let context = MigrationContext::new(plan, MigrationOverrides::default(), true)?;
        let dry_run_report = generate_report(&context)?;

        let payload = build_report_payload(&plan_path, &context, &dry_run_report);

        let version = payload["plan"]["version"]
            .as_u64()
            .ok_or_else(|| eyre!("plan.version missing"))?;
        ensure!(version == 1, "expected plan version == 1, got {version}");

        let plan_name = payload["plan"]["name"]
            .as_str()
            .ok_or_else(|| eyre!("plan.name missing"))?;
        ensure!(
            plan_name == "sample-plan",
            "unexpected plan name: {plan_name}"
        );

        let step_count = payload["dry_run"]["steps"]
            .as_array()
            .map(Vec::len)
            .ok_or_else(|| eyre!("dry_run.steps missing"))?;
        ensure!(
            step_count == 2,
            "expected 2 dry-run steps, got {step_count}"
        );

        let matched = payload["plan"]["steps"][0]["dry_run"]["matched_keys"]
            .as_u64()
            .ok_or_else(|| eyre!("matched_keys missing"))?;
        ensure!(
            matched == 1,
            "expected first step to match 1 key, got {matched}"
        );

        Ok(())
    }
}
