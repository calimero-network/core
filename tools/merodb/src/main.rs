use std::fs;
use std::path::{Path, PathBuf};

use calimero_primitives as _;
use clap::{Parser, Subcommand};
use eyre::{Result, WrapErr};
use rocksdb::{DBWithThreadMode, Options, SingleThreaded};

mod abi;
mod dag;
mod deserializer;
mod export;
mod migration;
mod schema;
mod types;
mod validation;

#[cfg(feature = "gui")]
use clap::Args;

#[cfg(feature = "gui")]
mod gui;

use dag::cli as dag_cli;
use export::cli as export_cli;
use migration::cli::{run_migrate, MigrateArgs};
use types::Column;
use validation::cli as validation_cli;

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
    Export(export_cli::ExportArgs),
    /// Validate database integrity
    Validate(validation_cli::ValidateArgs),
    /// Export DAG structure from Context DAG deltas
    #[command(name = "export-dag")]
    ExportDag(dag_cli::ExportDagArgs),
    /// Launch interactive GUI (requires 'gui' feature)
    #[cfg(feature = "gui")]
    Gui(GuiArgs),
    /// Placeholder for upcoming migration support
    Migrate(MigrateArgs),
}

#[cfg(feature = "gui")]
#[derive(Args, Debug)]
struct GuiArgs {
    /// Port for the GUI server (default: 8080)
    #[arg(long, default_value = "8080")]
    port: u16,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Schema { output } => {
            let schema = schema::generate_schema();
            output_json(&schema, output.as_deref())?;
            Ok(())
        }
        Command::Export(args) => export_cli::run_export(args),
        Command::Validate(args) => validation_cli::run_validate(&args),
        Command::ExportDag(args) => dag_cli::run_export_dag(&args),
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

pub(crate) fn output_json(value: &serde_json::Value, output_path: Option<&Path>) -> Result<()> {
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
    println!("1. Open the GUI in your browser");
    println!("2. Enter your database path and optionally upload a state schema file");
    println!("3. Use the Data View, DAG View, and State Tree tabs to explore your database");
    println!();

    let rt = Runtime::new()?;
    rt.block_on(gui::start_gui_server(port))?;

    Ok(())
}
