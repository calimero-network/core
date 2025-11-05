use std::env;
use std::path::{Path, PathBuf};

use clap::Args;
use eyre::Result;
use rocksdb::properties;

use crate::types::Column;

use super::context::{AbiManifestStatus, MigrationContext, MigrationOverrides};
use super::dry_run::{generate_report as generate_dry_run_report, DryRunReport, StepDetail};
use super::loader::load_plan;
use super::plan::{MigrationPlan, PlanStep};
use super::report::write_json_report;

/// Clap-backed argument struct for the `migrate` subcommand.
#[derive(Args, Debug)]
pub struct MigrateArgs {
    /// Migration plan file (YAML)
    #[arg(long, value_name = "PLAN")]
    pub plan: PathBuf,

    /// Source RocksDB path
    #[arg(long, value_name = "PATH")]
    pub db_path: Option<PathBuf>,

    /// WASM file providing the ABI manifest for the source database
    #[arg(long, value_name = "WASM_FILE")]
    pub wasm_file: Option<PathBuf>,

    /// Target RocksDB path
    #[arg(long, value_name = "PATH")]
    pub target_db: Option<PathBuf>,

    /// Perform a dry run without writing to the target
    #[arg(long)]
    pub dry_run: bool,

    /// Write dry-run results as JSON to this path
    #[arg(long, value_name = "FILE")]
    pub report: Option<PathBuf>,
}

/// Entrypoint for the `migrate` subcommand.
pub fn run_migrate(args: &MigrateArgs) -> Result<()> {
    let plan_path = &args.plan;
    if !plan_path.exists() {
        eyre::bail!(
            "Migration plan file does not exist: {}",
            plan_path.display()
        );
    }

    if !args.dry_run {
        eprintln!("note: mutating migrations are not yet supported; running in dry-run mode only.");
    }

    let plan = load_plan(plan_path)?;

    let overrides = MigrationOverrides {
        source_db: args.db_path.clone(),
        wasm_file: args.wasm_file.clone(),
        target_db: args.target_db.clone(),
    };

    let context = MigrationContext::new(plan, overrides, true)?;

    print_plan_summary(context.plan(), plan_path);

    println!();
    println!(
        "Source database opened (read-only): {}",
        context.source().path().display()
    );
    match context.source().abi_status() {
        AbiManifestStatus::NotConfigured => {
            println!("  ABI manifest: <not configured>");
        }
        AbiManifestStatus::Pending { wasm_path } => {
            println!(
                "  ABI manifest configured (lazy load): {}",
                wasm_path.display()
            );
        }
        AbiManifestStatus::Loaded => {
            println!("  ABI manifest: loaded (cached)");
        }
    }

    if let Ok(Some(estimate)) = context
        .source()
        .db()
        .property_int_value(properties::ESTIMATE_NUM_KEYS)
    {
        println!("  Approximate key count: {estimate}");
    }

    if env::var_os("MERODB_EAGER_ABI").is_some() {
        match context.source().abi_manifest() {
            Ok(Some(_)) => println!("  ABI manifest eagerly loaded via MERODB_EAGER_ABI"),
            Ok(None) => println!("  MERODB_EAGER_ABI set but no WASM manifest configured"),
            Err(error) => println!("  Failed to load ABI manifest eagerly: {error:?}"),
        }
    }

    if let Some(target) = context.target() {
        println!(
            "Target database opened ({}): {}",
            if target.is_read_only() {
                "read-only"
            } else {
                "read-write"
            },
            target.path().display()
        );
        if let Ok(Some(estimate)) = target
            .db()
            .property_int_value(properties::ESTIMATE_NUM_KEYS)
        {
            println!("  Approximate key count (target): {estimate}");
        }
        if let Some(backup_dir) = target.backup_dir() {
            println!("  Backup directory: {}", backup_dir.display());
        }
    } else {
        println!("Target database: <not configured>");
    }

    println!(
        "Dry run mode: {}",
        if context.is_dry_run() {
            "enabled"
        } else {
            "disabled"
        }
    );

    let dry_run_report = generate_dry_run_report(&context)?;
    println!();
    print_dry_run_report(&context, &dry_run_report);

    if let Some(report_path) = args.report.as_deref() {
        write_json_report(report_path, plan_path, &context, &dry_run_report)?;
    }

    Ok(())
}

fn print_plan_summary(plan: &MigrationPlan, plan_path: &Path) {
    println!("Loaded migration plan: {}", plan_path.display());
    println!("  Version: {}", plan.version.as_u32());
    if let Some(name) = plan.name.as_deref() {
        println!("  Name: {name}");
    }
    if let Some(description) = plan.description.as_deref() {
        println!("  Description: {description}");
    }
    println!("  Source DB: {}", plan.source.db_path.display());
    if let Some(wasm) = plan.source.wasm_file.as_ref() {
        println!("  Source WASM: {}", wasm.display());
    }
    if let Some(target) = plan.target.as_ref() {
        println!("  Target DB: {}", target.db_path.display());
        if let Some(backup) = target.backup_dir.as_ref() {
            println!("  Target backup dir: {}", backup.display());
        }
    } else {
        println!("  Target DB: <not specified>");
    }

    if plan.defaults.columns.is_empty() {
        println!("  Default columns: <none>");
    } else {
        let columns = plan
            .defaults
            .columns
            .iter()
            .map(Column::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        println!("  Default columns: {columns}");
    }

    if let Some(filters) = plan.defaults.filters.summary() {
        println!("  Default filters: {filters}");
    } else {
        println!("  Default filters: <none>");
    }

    if let Some(flag) = plan.defaults.decode_with_abi {
        println!("  Default decode_with_abi: {flag}");
    }
    if let Some(flag) = plan.defaults.write_if_missing {
        println!("  Default write_if_missing: {flag}");
    }

    println!("  Steps: {}", plan.steps.len());
    for (index, step) in plan.steps.iter().enumerate() {
        let step_number = index.saturating_add(1);
        println!("    {step_number}. {}", format_plan_step(step));
    }
}

fn format_plan_step(step: &PlanStep) -> String {
    let mut details = Vec::new();

    details.push(format!("column {}", step.column().as_str()));

    if let Some(filters) = step.filters() {
        if let Some(summary) = filters.summary() {
            details.push(format!("filters: {summary}"));
        } else if filters.is_empty() {
            details.push("filters: <none>".to_owned());
        }
    }

    match step {
        PlanStep::Copy(copy) => {
            if let Some(transform) = copy.transform.summary() {
                details.push(format!("transform: {transform}"));
            }
        }
        PlanStep::Delete(_) => {}
        PlanStep::Upsert(upsert) => {
            details.push(format!("entries={}", upsert.entries.len()));
            if let Some(first) = upsert.entries.first() {
                details.push(format!(
                    "first_key={} ({})",
                    first.key.encoding_label(),
                    first.key.preview(16)
                ));
            }
        }
        PlanStep::Verify(verify) => {
            details.push(verify.assertion.summary());
        }
    }

    let mut label = step.name().map_or_else(
        || step.kind().to_owned(),
        |name| format!("{name} [{}]", step.kind()),
    );

    if !details.is_empty() {
        label.push_str(" - ");
        label.push_str(&details.join("; "));
    }

    label
}

fn print_dry_run_report(context: &MigrationContext, report: &DryRunReport) {
    println!("Dry-run preview:");
    let plan = context.plan();

    for step_report in &report.steps {
        if step_report.index >= plan.steps.len() {
            continue;
        }

        let label = format_plan_step(&plan.steps[step_report.index]);
        let step_number = step_report.index.saturating_add(1);
        println!("  {step_number}. {label}");
        println!("       matched keys: {}", step_report.matched_keys);

        if let Some(summary) = step_report.filters_summary.as_deref() {
            if !summary.is_empty() {
                println!("       filters applied: {summary}");
            }
        }

        match &step_report.detail {
            StepDetail::Copy { decode_with_abi } => {
                println!("       action: copy (decode_with_abi={decode_with_abi})");
            }
            StepDetail::Delete => {
                println!("       action: delete preview");
            }
            StepDetail::Upsert { entries } => {
                println!("       entries to write: {entries}");
            }
            StepDetail::Verify { summary, passed } => {
                println!("       verify: {summary}");
                if passed.is_none() {
                    println!("       verify outcome: unknown (see warnings)");
                }
            }
        }

        for sample in &step_report.samples {
            println!("       sample: {sample}");
        }
        for warning in &step_report.warnings {
            println!("       warning: {warning}");
        }

        if !step_report.samples.is_empty() || !step_report.warnings.is_empty() {
            println!();
        }
    }
}
