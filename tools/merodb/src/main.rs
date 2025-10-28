#![allow(
    clippy::struct_excessive_bools,
    reason = "CLI struct with boolean flags is appropriate"
)]

use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;
use eyre::{Result, WrapErr};
use rocksdb::{DBWithThreadMode, Options, SingleThreaded};

mod abi;
mod deserializer;
mod export;
mod schema;
mod types;
mod validation;

use types::Column;

#[derive(Parser)]
#[command(name = "merodb")]
#[command(author, version, about = "CLI tool for debugging RocksDB in Calimero", long_about = None)]
struct Cli {
    /// Path to the RocksDB database (not required for --schema)
    #[arg(long, value_name = "PATH", required_unless_present = "schema")]
    db_path: Option<PathBuf>,

    /// Generate JSON schema of the database structure
    #[arg(long, conflicts_with_all = &["export", "validate"])]
    schema: bool,

    /// Export data from the database
    #[arg(long, conflicts_with = "validate")]
    export: bool,

    /// Export all column families
    #[arg(long, requires = "export")]
    all: bool,

    /// Export specific column families (comma-separated)
    #[arg(
        long,
        value_name = "COLUMNS",
        requires = "export",
        conflicts_with = "all"
    )]
    columns: Option<String>,

    /// Validate database integrity
    #[arg(long, conflicts_with_all = &["schema", "export"])]
    validate: bool,

    /// Output file path (defaults to stdout if not specified)
    #[arg(short, long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// WASM file for ABI-guided state deserialization (optional)
    #[arg(long, value_name = "WASM_FILE")]
    wasm_file: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle schema generation (doesn't require opening the database)
    if cli.schema {
        let schema = schema::generate_schema();
        output_json(&schema, cli.output.as_deref())?;
        return Ok(());
    }

    // Ensure the database path exists
    let db_path = cli.db_path.as_ref().ok_or_else(|| {
        eyre::eyre!("Database path is required for export and validate operations")
    })?;

    if !db_path.exists() {
        eyre::bail!("Database path does not exist: {}", db_path.display());
    }

    // Open the database in read-only mode
    let db = open_database(db_path)?;

    // Load ABI manifest if WASM file is provided
    let abi_manifest = if let Some(wasm_path) = &cli.wasm_file {
        if !wasm_path.exists() {
            eyre::bail!("WASM file does not exist: {}", wasm_path.display());
        }
        println!("Loading ABI from WASM file: {}", wasm_path.display());
        match abi::extract_abi_from_wasm(wasm_path) {
            Ok(manifest) => {
                println!("ABI loaded successfully");
                Some(manifest)
            }
            Err(e) => {
                eprintln!("Warning: Failed to load ABI from WASM: {e}");
                eprintln!("Falling back to raw hex output for State and Delta columns");
                None
            }
        }
    } else {
        None
    };

    // Handle different operations
    if cli.export {
        let columns = if cli.all {
            Column::all().to_vec()
        } else if let Some(column_names) = cli.columns {
            parse_columns(&column_names)?
        } else {
            eyre::bail!("Must specify either --all or --columns when using --export");
        };

        let data = export::export_data(&db, &columns, abi_manifest.as_ref())?;
        output_json(&data, cli.output.as_deref())?;
    } else if cli.validate {
        let validation_result = validation::validate_database(&db)?;
        output_json(&validation_result, cli.output.as_deref())?;
    } else {
        eyre::bail!("Must specify one of: --schema, --export, or --validate");
    }

    Ok(())
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

fn parse_columns(column_names: &str) -> Result<Vec<Column>> {
    let mut columns = Vec::new();

    for name in column_names.split(',') {
        let name = name.trim();
        let column = match name {
            "Meta" => Column::Meta,
            "Config" => Column::Config,
            "Identity" => Column::Identity,
            "State" => Column::State,
            "Delta" => Column::Delta,
            "Blobs" => Column::Blobs,
            "Application" => Column::Application,
            "Alias" => Column::Alias,
            "Generic" => Column::Generic,
            _ => eyre::bail!("Unknown column family: {}", name),
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
