use std::path::PathBuf;

use clap::{Parser, Subcommand};

mod extract;
mod inspect;

#[derive(Parser)]
#[command(name = "calimero-abi")]
#[command(author, version = env!("CARGO_PKG_VERSION"), about = "Extract Calimero WASM ABI from compiled applications")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Extract ABI from a WASM file
    Extract {
        /// Input WASM file
        #[arg(value_name = "WASM_FILE")]
        wasm_file: PathBuf,

        /// Output JSON file
        #[arg(short, long, value_name = "OUTPUT")]
        output: Option<PathBuf>,

        /// Verify ABI using get_abi* exports
        #[arg(long)]
        verify: bool,
    },
    /// Inspect WASM file sections
    Inspect {
        /// Input WASM file
        #[arg(value_name = "WASM_FILE")]
        wasm_file: PathBuf,
    },
}

fn main() -> eyre::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Extract {
            wasm_file,
            output,
            verify,
        } => {
            extract::extract_abi(&wasm_file, output.as_deref(), verify)?;
        }
        Commands::Inspect { wasm_file } => {
            inspect::inspect_wasm(&wasm_file)?;
        }
    }

    Ok(())
}
