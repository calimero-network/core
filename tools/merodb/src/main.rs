#![allow(
    clippy::struct_excessive_bools,
    reason = "CLI struct with boolean flags is appropriate"
)]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use eyre::{Result, WrapErr};
use rocksdb::{properties, DBWithThreadMode, Options, SingleThreaded};

mod abi;
mod dag;
mod deserializer;
mod export;
mod migration;
mod schema;
mod types;
mod validation;

#[cfg(feature = "gui")]
mod gui;

use migration::context::{AbiManifestStatus, MigrationContext, MigrationOverrides};
use migration::loader::load_plan;
use migration::plan::{MigrationPlan, PlanStep};
use types::Column;

#[derive(Parser, Debug)]
#[command(
    name = "merodb",
    author,
    version,
    about = "CLI tool for debugging RocksDB in Calimero",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Generate JSON schema of the database structure
    Schema {
        /// Output file path (defaults to stdout if not specified)
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
    },
    /// Export data from the database
    Export(ExportArgs),
    /// Validate database integrity
    Validate(ValidateArgs),
    /// Export DAG structure from Context DAG deltas
    #[command(name = "export-dag")]
    ExportDag(ExportDagArgs),
    /// Launch interactive GUI (requires 'gui' feature)
    #[cfg(feature = "gui")]
    Gui(GuiArgs),
    /// Placeholder for upcoming migration support
    Migrate(MigrateArgs),
}

#[derive(Args, Debug)]
struct ExportArgs {
    /// Path to the RocksDB database
    #[arg(long, value_name = "PATH")]
    db_path: PathBuf,

    /// Export all column families
    #[arg(long)]
    all: bool,

    /// Export specific column families (comma-separated)
    #[arg(
        long,
        value_name = "COLUMNS",
        conflicts_with = "all",
        value_delimiter = ',',
        use_value_delimiter = true
    )]
    columns: Option<Vec<String>>,

    /// WASM file providing the ABI schema (required for export)
    #[arg(long, value_name = "WASM_FILE")]
    wasm_file: Option<PathBuf>,

    /// Output file path (defaults to stdout if not specified)
    #[arg(short, long, value_name = "FILE")]
    output: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct ValidateArgs {
    /// Path to the RocksDB database
    #[arg(long, value_name = "PATH")]
    db_path: PathBuf,

    /// Output file path (defaults to stdout if not specified)
    #[arg(short, long, value_name = "FILE")]
    output: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct ExportDagArgs {
    /// Path to the RocksDB database
    #[arg(long, value_name = "PATH")]
    db_path: PathBuf,

    /// Output file path (defaults to stdout if not specified)
    #[arg(short, long, value_name = "FILE")]
    output: Option<PathBuf>,
}

#[cfg(feature = "gui")]
#[derive(Args, Debug)]
struct GuiArgs {
    /// Port for the GUI server (default: 8080)
    #[arg(long, default_value = "8080")]
    port: u16,
}

#[derive(Args, Debug)]
struct MigrateArgs {
    /// Migration plan file (YAML)
    #[arg(long, value_name = "PLAN")]
    plan: PathBuf,

    /// Source RocksDB path
    #[arg(long, value_name = "PATH")]
    db_path: Option<PathBuf>,

    /// WASM file providing the ABI manifest for the source database
    #[arg(long, value_name = "WASM_FILE")]
    wasm_file: Option<PathBuf>,

    /// Target RocksDB path
    #[arg(long, value_name = "PATH")]
    target_db: Option<PathBuf>,

    /// Perform a dry run without writing to the target
    #[arg(long)]
    dry_run: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Schema { output } => {
            let schema = schema::generate_schema();
            output_json(&schema, output.as_deref())?;
            Ok(())
        }
        Command::Export(args) => run_export(args),
        Command::Validate(args) => run_validate(&args),
        Command::ExportDag(args) => run_export_dag(&args),
        #[cfg(feature = "gui")]
        Command::Gui(args) => run_gui(args.port),
        Command::Migrate(args) => run_migrate(&args),
    }
}

pub(crate) fn open_database(path: &Path) -> Result<DBWithThreadMode<SingleThreaded>> {
    let options = Options::default();

    // Get all column families
    let cf_names: Vec<String> = Column::all()
        .iter()
        .map(|c| c.as_str().to_owned())
        .collect();

    // Open database in read-only mode
    let db = DBWithThreadMode::<SingleThreaded>::open_cf_for_read_only(
        &options, path, &cf_names, false, // error_if_log_file_exist
    )
    .wrap_err_with(|| format!("Failed to open database at {}", path.display()))?;

    Ok(db)
}

fn parse_columns(column_names: &[String]) -> Result<Vec<Column>> {
    let mut columns = Vec::new();

    for name in column_names {
        let column_name = name.trim();
        let column = Column::from_name(column_name)
            .ok_or_else(|| eyre::eyre!("Unknown column family: {column_name}"))?;
        columns.push(column);
    }

    if columns.is_empty() {
        eyre::bail!("No column families specified");
    }

    Ok(columns)
}

fn output_json(value: &serde_json::Value, output_path: Option<&Path>) -> Result<()> {
    let json_string = serde_json::to_string_pretty(value)?;

    if let Some(path) = output_path {
        fs::write(path, &json_string)
            .wrap_err_with(|| format!("Failed to write to {}", path.display()))?;
        println!("Output written to: {}", path.display());
    } else {
        println!("{json_string}");
    }

    Ok(())
}

#[cfg(feature = "gui")]
fn run_gui(port: u16) -> Result<()> {
    use tokio::runtime::Runtime;

    println!("Starting MeroDB GUI...");
    println!("The GUI will be available at http://127.0.0.1:{port}");
    println!();
    println!("Instructions:");
    println!("1. Export your database using: merodb export --db-path /path/to/db --all --wasm-file contract.wasm --output export.json");
    println!("2. Open the GUI in your browser and load the exported JSON file");
    println!("3. Use JQ queries to explore and analyze your database");
    println!();

    let rt = Runtime::new()?;
    rt.block_on(gui::start_gui_server(port))?;

    Ok(())
}

fn run_export(args: ExportArgs) -> Result<()> {
    if !args.db_path.exists() {
        eyre::bail!("Database path does not exist: {}", args.db_path.display());
    }

    let db = open_database(args.db_path.as_path())?;

    let manifest = if let Some(wasm_path) = args.wasm_file {
        if !wasm_path.exists() {
            eyre::bail!("WASM file does not exist: {}", wasm_path.display());
        }
        println!("Loading ABI from WASM file: {}", wasm_path.display());
        match abi::extract_abi_from_wasm(&wasm_path) {
            Ok(manifest) => {
                println!("ABI loaded successfully");
                manifest
            }
            Err(e) => eyre::bail!("Failed to load ABI from WASM: {e}"),
        }
    } else {
        eyre::bail!("--wasm-file is required when exporting data");
    };

    let columns = if args.all {
        Column::all().to_vec()
    } else if let Some(column_names) = args.columns {
        parse_columns(&column_names)?
    } else {
        eyre::bail!("Must specify either --all or --columns when using export");
    };

    let data = export::export_data(&db, &columns, &manifest)?;
    output_json(&data, args.output.as_deref())
}

fn run_validate(args: &ValidateArgs) -> Result<()> {
    if !args.db_path.exists() {
        eyre::bail!("Database path does not exist: {}", args.db_path.display());
    }

    let db = open_database(args.db_path.as_path())?;
    let validation_result = validation::validate_database(&db)?;
    output_json(&validation_result, args.output.as_deref())
}

fn run_export_dag(args: &ExportDagArgs) -> Result<()> {
    if !args.db_path.exists() {
        eyre::bail!("Database path does not exist: {}", args.db_path.display());
    }

    let db = open_database(&args.db_path)?;
    let dag_data = dag::export_dag(&db)?;
    output_json(&dag_data, args.output.as_deref())
}

fn run_migrate(args: &MigrateArgs) -> Result<()> {
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
