use std::path::PathBuf;

use clap::Args;
use eyre::Result;

use crate::open_database;
use crate::validation;

/// Validate command arguments.
#[derive(Args, Debug)]
pub struct ValidateArgs {
    /// Path to the RocksDB database
    #[arg(long, value_name = "PATH")]
    pub db_path: PathBuf,

    /// Output file path (defaults to stdout if not specified)
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,
}

/// Execute the validate subcommand.
pub fn run_validate(args: &ValidateArgs) -> Result<()> {
    if !args.db_path.exists() {
        eyre::bail!("Database path does not exist: {}", args.db_path.display());
    }

    let db = open_database(args.db_path.as_path())?;
    let validation_result = validation::validate_database(&db)?;
    crate::output_json(&validation_result, args.output.as_deref())
}
