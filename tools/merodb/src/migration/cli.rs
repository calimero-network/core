use std::env;
use std::path::{Path, PathBuf};

use clap::Args;
use eyre::{Result, WrapErr};
use rocksdb::properties;

use crate::types::Column;

use super::context::{MigrationContext, MigrationOverrides, SchemaStatus};
use super::dry_run::{generate_report as generate_dry_run_report, DryRunReport, StepDetail};
use super::execute::{execute_migration, ExecutionReport, StepExecutionDetail};
use super::loader::load_plan;
use super::plan::PlanStep;
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

    /// State schema JSON file (extracted using `calimero-abi state`)
    ///
    /// This includes the state root type and its dependencies, sufficient for state deserialization.
    #[arg(long, value_name = "SCHEMA_FILE")]
    pub state_schema_file: Option<PathBuf>,

    /// Target RocksDB path
    #[arg(long, value_name = "PATH")]
    pub target_db: Option<PathBuf>,

    /// Backup directory for target database (overrides plan configuration)
    #[arg(long, value_name = "PATH")]
    pub backup_dir: Option<PathBuf>,

    /// Skip backup creation before applying migration (use with caution)
    #[arg(long)]
    pub no_backup: bool,

    /// Perform a dry run without writing to the target (default behavior)
    #[arg(long)]
    pub dry_run: bool,

    /// Apply the migration plan and write changes to the target database
    #[arg(long, conflicts_with = "dry_run")]
    pub apply: bool,

    /// Write results (dry-run or execution) as JSON to this path
    #[arg(long, value_name = "FILE")]
    pub report: Option<PathBuf>,
}

/// Entrypoint for the `migrate` subcommand.
///
/// This function orchestrates the entire migration workflow:
/// 1. Validates the plan file exists
/// 2. Determines whether to run in dry-run or apply mode
/// 3. Loads the plan and builds the migration context
/// 4. Prints plan summary and database information
/// 5. Executes either dry-run preview or actual migration
/// 6. Optionally writes results to JSON report file
///
/// # Modes
///
/// - **Dry-run mode** (default): Preview what would happen without making changes
/// - **Apply mode** (`--apply`): Execute the migration and write changes to target
#[expect(
    clippy::too_many_lines,
    reason = "CLI function with extensive user feedback"
)]
pub fn run_migrate(args: &MigrateArgs) -> Result<()> {
    let plan_path = &args.plan;
    if !plan_path.exists() {
        eyre::bail!(
            "Migration plan file does not exist: {}",
            plan_path.display()
        );
    }

    // Determine execution mode: dry-run is default, --apply enables mutations
    let dry_run = !args.apply;

    let plan = load_plan(plan_path)?;

    let overrides = MigrationOverrides {
        source_db: args.db_path.clone(),
        state_schema_file: args.state_schema_file.clone(),
        target_db: args.target_db.clone(),
        backup_dir: args.backup_dir.clone(),
        no_backup: args.no_backup,
    };

    let context = MigrationContext::new(plan, overrides, dry_run)?;

    print_plan_summary(&context, plan_path);

    println!();
    println!(
        "Source database opened (read-only): {}",
        context.source().path().display()
    );
    match context.source().schema_status() {
        SchemaStatus::NotConfigured => {
            println!("  State schema: <not configured>");
        }
        SchemaStatus::PendingStateSchema { schema_path } => {
            println!(
                "  State schema configured (lazy load): {}",
                schema_path.display()
            );
        }
        SchemaStatus::Loaded => {
            println!("  State schema: loaded (cached)");
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
        match context.source().schema() {
            Ok(Some(_)) => println!("  State schema eagerly loaded via MERODB_EAGER_ABI"),
            Ok(None) => println!("  MERODB_EAGER_ABI set but no state schema configured"),
            Err(error) => println!("  Failed to load state schema eagerly: {error:?}"),
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
        "Execution mode: {}",
        if context.is_dry_run() {
            "DRY-RUN (preview only, no changes will be made)"
        } else {
            "APPLY (changes will be written to target database)"
        }
    );
    println!();

    // Execute migration based on mode
    if context.is_dry_run() {
        // Dry-run mode: generate preview report
        let dry_run_report = generate_dry_run_report(&context)?;
        print_dry_run_report(&context, &dry_run_report);

        if let Some(report_path) = args.report.as_deref() {
            write_json_report(report_path, plan_path, &context, &dry_run_report)?;
        }

        println!();
        println!("Dry-run complete. No changes were made to the target database.");
        println!("To apply these changes, run with --apply flag.");
    } else {
        // Apply mode: execute migration and write changes
        println!("Starting migration execution...");
        println!();

        let execution_report = execute_migration(&context)?;
        print_execution_report(&context, &execution_report);

        if let Some(report_path) = args.report.as_deref() {
            write_execution_json_report(report_path, plan_path, &context, &execution_report)?;
        }

        println!();
        println!("Migration execution complete. Changes have been written to the target database.");
    }

    Ok(())
}

fn print_plan_summary(context: &MigrationContext, plan_path: &Path) {
    let plan = context.plan();
    println!("Loaded migration plan: {}", plan_path.display());
    println!("  Version: {}", plan.version.as_u32());
    if let Some(name) = plan.name.as_deref() {
        println!("  Name: {name}");
    }
    if let Some(description) = plan.description.as_deref() {
        println!("  Description: {description}");
    }

    // Show actual source paths being used (from context, not plan)
    println!("  Source DB: {}", context.source().path().display());
    match context.source().schema_status() {
        SchemaStatus::NotConfigured => {}
        SchemaStatus::PendingStateSchema { schema_path } => {
            println!("  Source State Schema: {}", schema_path.display());
        }
        SchemaStatus::Loaded => {}
    }

    // Show actual target paths being used (from context, not plan)
    if let Some(target) = context.target() {
        println!("  Target DB: {}", target.path().display());
        if let Some(backup) = target.backup_dir() {
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

/// Print the execution report to stdout after a migration has been applied.
///
/// This function displays detailed statistics about each executed step, including:
/// - Number of keys processed
/// - Step-specific metrics (keys copied, bytes copied, entries written, etc.)
/// - Filter summaries
/// - Warnings encountered during execution
fn print_execution_report(context: &MigrationContext, report: &ExecutionReport) {
    println!("Execution results:");
    let plan = context.plan();

    for step_report in &report.steps {
        if step_report.index >= plan.steps.len() {
            continue;
        }

        let label = format_plan_step(&plan.steps[step_report.index]);
        let step_number = step_report.index.saturating_add(1);
        println!("  {step_number}. {label}");
        println!("       keys processed: {}", step_report.keys_processed);

        if let Some(summary) = step_report.filters_summary.as_deref() {
            if !summary.is_empty() {
                println!("       filters applied: {summary}");
            }
        }

        match &step_report.detail {
            StepExecutionDetail::Copy {
                keys_copied,
                bytes_copied,
            } => {
                println!("       action: copy");
                println!("       keys copied: {keys_copied}");
                println!("       bytes copied: {bytes_copied}");
            }
            StepExecutionDetail::Delete { keys_deleted } => {
                println!("       action: delete");
                println!("       keys deleted: {keys_deleted}");
            }
            StepExecutionDetail::Upsert { entries_written } => {
                println!("       action: upsert");
                println!("       entries written: {entries_written}");
            }
            StepExecutionDetail::Verify { summary, passed } => {
                println!("       action: verify");
                println!("       verify: {summary}");
                println!(
                    "       status: {}",
                    if *passed { "PASSED" } else { "FAILED" }
                );
            }
        }

        for warning in &step_report.warnings {
            println!("       warning: {warning}");
        }

        if !step_report.warnings.is_empty() {
            println!();
        }
    }
}

/// Write the execution report to a JSON file.
///
/// This function creates a machine-readable report containing:
/// - Plan metadata (path, version, name, description)
/// - Execution mode (always "apply" for this function)
/// - Per-step execution results with detailed statistics
fn write_execution_json_report(
    report_path: &Path,
    plan_path: &Path,
    context: &MigrationContext,
    report: &ExecutionReport,
) -> Result<()> {
    use std::fs;

    let plan = context.plan();

    let json_report = serde_json::json!({
        "plan_path": plan_path.display().to_string(),
        "plan_version": plan.version.as_u32(),
        "plan_name": plan.name,
        "plan_description": plan.description,
        "execution_mode": "apply",
        "source_db": context.source().path().display().to_string(),
        "target_db": context.target().map(|t| t.path().display().to_string()),
        "steps": report.steps,
    });

    let json_string = serde_json::to_string_pretty(&json_report)?;
    fs::write(report_path, json_string).wrap_err_with(|| {
        format!(
            "Failed to write execution report to {}",
            report_path.display()
        )
    })?;

    println!("Execution report written to: {}", report_path.display());

    Ok(())
}
