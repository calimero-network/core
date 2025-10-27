use std::fs;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use wasmparser::{Parser as WasmParser, Payload};

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
            extract_abi(&wasm_file, output.as_deref(), verify)?;
        }
        Commands::Inspect { wasm_file } => {
            inspect_wasm(&wasm_file)?;
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
            drop(serde_json::from_str::<serde_json::Value>(&json_str)?);

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
            let _ = path.set_extension("abi.json");
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

fn inspect_wasm(wasm_file: &PathBuf) -> eyre::Result<()> {
    // Read the WASM file
    let wasm_bytes = fs::read(wasm_file)?;

    println!("WASM file: {}", wasm_file.display());
    println!("Size: {} bytes\n", wasm_bytes.len());

    // Parse the WASM file
    let parser = WasmParser::new(0);
    let mut sections = Vec::new();
    let mut custom_sections = Vec::new();
    let mut has_get_abi_exports = false;
    let mut export_count = 0;

    for payload in parser.parse_all(&wasm_bytes) {
        match payload? {
            Payload::Version { num, .. } => {
                sections.push(format!("Version: {num}"));
            }
            Payload::TypeSection(reader) => {
                sections.push(format!("TypeSection: {} types", reader.count()));
            }
            Payload::ImportSection(reader) => {
                sections.push(format!("ImportSection: {} imports", reader.count()));
            }
            Payload::FunctionSection(reader) => {
                sections.push(format!("FunctionSection: {} functions", reader.count()));
            }
            Payload::TableSection(reader) => {
                sections.push(format!("TableSection: {} tables", reader.count()));
            }
            Payload::MemorySection(reader) => {
                sections.push(format!("MemorySection: {} memories", reader.count()));
            }
            Payload::GlobalSection(reader) => {
                sections.push(format!("GlobalSection: {} globals", reader.count()));
            }
            Payload::ExportSection(reader) => {
                export_count = reader.count();
                sections.push(format!("ExportSection: {export_count} exports"));

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
            Payload::StartSection { func, .. } => {
                sections.push(format!("StartSection: function {func}"));
            }
            Payload::ElementSection(reader) => {
                sections.push(format!("ElementSection: {} elements", reader.count()));
            }
            Payload::DataCountSection { count, .. } => {
                sections.push(format!("DataCountSection: {count} data segments"));
            }
            Payload::DataSection(reader) => {
                sections.push(format!("DataSection: {} data segments", reader.count()));
            }
            Payload::CodeSectionStart { count, .. } => {
                sections.push(format!("CodeSection: {count} functions"));
            }
            Payload::CustomSection(section) => {
                let name = section.name();
                let size = section.data().len();
                sections.push(format!("CustomSection: '{name}' ({size} bytes)"));
                custom_sections.push((name.to_owned(), size));
            }
            _ => {}
        }
    }

    println!("All sections:");
    for (i, section) in sections.iter().enumerate() {
        println!("  {}. {}", i + 1, section);
    }

    println!("\nExports: {export_count} total");
    println!("Has get_abi* exports: {has_get_abi_exports}");

    if !custom_sections.is_empty() {
        println!("\nCustom sections summary:");
        for (name, size) in &custom_sections {
            println!("  - '{}' ({} bytes)", name, size);
        }
    }

    if !custom_sections.iter().any(|(name, _)| name == "calimero_abi_v1") {
        println!("\n⚠️  'calimero_abi_v1' section NOT found.");
        println!("This WASM file was built without ABI generation enabled.");
        println!("\nTo enable ABI generation:");
        println!("  1. Ensure you're using calimero-sdk");
        println!("  2. Build with: cargo build --target wasm32-unknown-unknown --release");
        println!("  3. The SDK will automatically embed the ABI during build");
    } else {
        println!("\n✓ 'calimero_abi_v1' section found - ABI extraction available");
    }

    Ok(())
}
