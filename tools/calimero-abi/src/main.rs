use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "mero-abi")]
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
    },
    /// Extract only the types schema from a WASM file
    Types {
        /// Input WASM file
        #[arg(value_name = "WASM_FILE")]
        wasm_file: PathBuf,

        /// Output JSON file
        #[arg(short, long, value_name = "OUTPUT")]
        output: Option<PathBuf>,
    },
    /// Extract the state schema (state root and all its type dependencies)
    State {
        /// Input WASM file
        #[arg(value_name = "WASM_FILE")]
        wasm_file: PathBuf,

        /// Output JSON file
        #[arg(short, long, value_name = "OUTPUT")]
        output: Option<PathBuf>,
    },
    /// Inspect WASM file sections
    Inspect {
        /// Input WASM file
        #[arg(value_name = "WASM_FILE")]
        wasm_file: PathBuf,
    },
    /// Embed an ABI manifest (abi.json or state-schema.json) into a wasm as the calimero_abi_v1 section (in place).
    Embed {
        /// The wasm file to modify in place.
        wasm: std::path::PathBuf,
        /// The ABI manifest to embed (must be name-sorted; `cargo mero build` embeds the canonicalized full ABI).
        schema: std::path::PathBuf,
    },
    /// Diff two state-schema.json versions; flags breaking + unsafe identity
    /// downgrades (an AuthoredMap/AuthoredVector/SharedStorage field replaced by
    /// a plain type, which silently strips authorship / writer-ACL).
    Diff {
        /// The new (current build) state-schema.json
        #[arg(value_name = "CURRENT")]
        current: PathBuf,

        /// The previous (baseline) state-schema.json to compare against
        #[arg(value_name = "BASELINE")]
        baseline: PathBuf,

        /// Report findings but always exit 0 (don't fail CI)
        #[arg(long)]
        exit_zero: bool,
    },
}

fn main() -> eyre::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Extract { wasm_file, output } => {
            mero_abi::extract_abi(&wasm_file, output.as_deref())?;
        }
        Commands::Types { wasm_file, output } => {
            mero_abi::extract_types_schema(&wasm_file, output.as_deref())?;
        }
        Commands::State { wasm_file, output } => {
            mero_abi::extract_state_schema(&wasm_file, output.as_deref())?;
        }
        Commands::Inspect { wasm_file } => {
            mero_abi::inspect_wasm(&wasm_file)?;
        }
        Commands::Embed { wasm, schema } => mero_abi::run_embed(&wasm, &schema)?,
        Commands::Diff {
            current,
            baseline,
            exit_zero,
        } => {
            if mero_abi::run_diff(&current, &baseline, exit_zero)? {
                std::process::exit(1);
            }
        }
    }

    Ok(())
}
