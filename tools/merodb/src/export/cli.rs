use std::path::PathBuf;

use clap::Args;
use eyre::Result;

use crate::export;
use crate::types::Column;
use crate::{abi, open_database};

/// Export-specific CLI arguments.
#[derive(Args, Debug)]
pub struct ExportArgs {
    /// Path to the RocksDB database
    #[arg(long, value_name = "PATH")]
    pub db_path: PathBuf,

    /// Export all column families
    #[arg(long)]
    pub all: bool,

    /// Export specific column families (comma-separated)
    #[arg(
        long,
        value_name = "COLUMNS",
        conflicts_with = "all",
        value_delimiter = ',',
        use_value_delimiter = true
    )]
    pub columns: Option<Vec<String>>,

    /// WASM file providing the ABI schema (required for export)
    #[arg(long, value_name = "WASM_FILE")]
    pub wasm_file: Option<PathBuf>,

    /// Output file path (defaults to stdout if not specified)
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,
}

/// Execute the export subcommand.
pub fn run_export(args: ExportArgs) -> Result<()> {
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
    crate::output_json(&data, args.output.as_deref())
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
