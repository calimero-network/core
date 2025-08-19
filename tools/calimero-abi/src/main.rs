use std::fs;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use wasmparser::{Parser as WasmParser, Payload};

#[derive(Parser)]
#[command(name = "calimero-abi")]
#[command(about = "Extract Calimero WASM ABI from compiled applications")]
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
}

fn main() -> eyre::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Extract {
            wasm_file,
            output,
            verify,
        } => {
            extract_abi(&wasm_file, output.as_deref(), verify)?;
        }
    }

    Ok(())
}

fn extract_abi(wasm_file: &PathBuf, output: Option<&Path>, verify: bool) -> eyre::Result<()> {
    // Read the WASM file
    let wasm_bytes = fs::read(wasm_file)?;

    // Parse the WASM file
    let parser = WasmParser::new(0);
    let mut abi_section: Option<Vec<u8>> = None;
    let mut has_get_abi_exports = false;

    for payload in parser.parse_all(&wasm_bytes) {
        match payload? {
            Payload::CustomSection(section) => {
                if section.name() == "calimero_abi_v1" {
                    abi_section = Some(section.data().to_vec());
                }
            }
            Payload::ExportSection(reader) => {
                for export in reader {
                    let export = export?;
                    if export.name == "get_abi_ptr"
                        || export.name == "get_abi_len"
                        || export.name == "get_abi"
                    {
                        has_get_abi_exports = true;
                    }
                }
            }
            _ => {}
        }
    }

    // Check if we found the ABI section
    let abi_json = match abi_section {
        Some(data) => {
            let json_str = String::from_utf8(data)?;

            // Validate JSON
            serde_json::from_str::<serde_json::Value>(&json_str)?;

            json_str
        }
        None => {
            eyre::bail!("No 'calimero_abi_v1' custom section found in WASM file");
        }
    };

    // Verify if requested
    if verify && !has_get_abi_exports {
        eyre::bail!("Verification failed: get_abi* exports not found in WASM file");
    }

    // Determine output path
    let output_path = match output {
        Some(path) => path.to_path_buf(),
        None => {
            let mut path = wasm_file.clone();
            path.set_extension("abi.json");
            path
        }
    };

    // Write the ABI JSON
    fs::write(&output_path, abi_json)?;

    println!("ABI extracted successfully to: {}", output_path.display());

    if verify {
        println!("Verification passed: get_abi* exports found");
    }

    Ok(())
}
