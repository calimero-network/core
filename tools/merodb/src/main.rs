#![allow(
    clippy::struct_excessive_bools,
    reason = "CLI struct with boolean flags is appropriate"
)]

use std::fs;
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use eyre::{Result, WrapErr};
use rocksdb::{DBWithThreadMode, Options, SingleThreaded};

mod abi;
mod dag;
mod deserializer;
mod export;
mod schema;
mod types;
mod validation;

#[cfg(feature = "gui")]
mod gui;

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

#[derive(Args, Debug, Default)]
struct MigrateArgs {
    /// Migration plan file (YAML/JSON)
    #[arg(long, value_name = "PLAN")]
    plan: Option<PathBuf>,

    /// Source RocksDB path
    #[arg(long, value_name = "PATH")]
    db_path: Option<PathBuf>,

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
        Command::Validate(args) => run_validate(args),
        Command::ExportDag(args) => run_export_dag(args),
        #[cfg(feature = "gui")]
        Command::Gui(args) => run_gui(args.port),
        Command::Migrate(args) => run_migrate_placeholder(args),
    }
}

fn open_database(path: &Path) -> Result<DBWithThreadMode<SingleThreaded>> {
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
        let column = match column_name {
            "Meta" => Column::Meta,
            "Config" => Column::Config,
            "Identity" => Column::Identity,
            "State" => Column::State,
            "Delta" => Column::Delta,
            "Blobs" => Column::Blobs,
            "Application" => Column::Application,
            "Alias" => Column::Alias,
            "Generic" => Column::Generic,
            _ => eyre::bail!("Unknown column family: {}", column_name),
        };
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

    let db = open_database(&args.db_path)?;

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

fn run_validate(args: ValidateArgs) -> Result<()> {
    if !args.db_path.exists() {
        eyre::bail!("Database path does not exist: {}", args.db_path.display());
    }

    let db = open_database(&args.db_path)?;
    let validation_result = validation::validate_database(&db)?;
    output_json(&validation_result, args.output.as_deref())
}

fn run_export_dag(args: ExportDagArgs) -> Result<()> {
    if !args.db_path.exists() {
        eyre::bail!("Database path does not exist: {}", args.db_path.display());
    }

    let db = open_database(&args.db_path)?;
    let dag_data = dag::export_dag(&db)?;
    output_json(&dag_data, args.output.as_deref())
}

fn run_migrate_placeholder(args: MigrateArgs) -> Result<()> {
    let mut message = String::from(
        "Migration support is not yet implemented. Track progress in tools/merodb/migrations.md.",
    );

    if args.plan.is_some() || args.db_path.is_some() || args.target_db.is_some() || args.dry_run {
        message.push_str(" Supplied migration arguments are acknowledged but ignored for now.");
    }

    eyre::bail!(message)
}
