use std::path::PathBuf;

use clap::Args;
use eyre::Result;

use crate::dag;
use crate::open_database;

/// Arguments for the export-dag subcommand.
#[derive(Args, Debug)]
pub struct ExportDagArgs {
    /// Path to the RocksDB database
    #[arg(long, value_name = "PATH")]
    pub db_path: PathBuf,

    /// Output file path (defaults to stdout if not specified)
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,
}

/// Execute the export-dag subcommand.
pub fn run_export_dag(args: &ExportDagArgs) -> Result<()> {
    if !args.db_path.exists() {
        eyre::bail!("Database path does not exist: {}", args.db_path.display());
    }

    let db = open_database(&args.db_path)?;
    let dag_data = dag::export_dag(&db)?;
    crate::output_json(&dag_data, args.output.as_deref())
}
