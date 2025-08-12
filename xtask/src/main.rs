// Copyright 2024 Calimero Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "xtask")]
#[command(about = "Build and development tasks for Calimero Core")]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Extract ABI files from compiled modules
    Abi {
        #[command(subcommand)]
        subcommand: AbiCommands,
    },
}

#[derive(Subcommand)]
enum AbiCommands {
    /// Extract ABI file for a specific module
    Extract {
        /// Module name to extract ABI for
        #[arg(short, long)]
        module: String,
        
        /// Output path (default: target/abi/abi.json)
        #[arg(short, long, default_value = "target/abi/abi.json")]
        out: PathBuf,
    },
}

fn main() -> eyre::Result<()> {
    let args = Args::parse();
    
    match args.command {
        Commands::Abi { subcommand } => match subcommand {
            AbiCommands::Extract { module, out } => {
                // Ensure output directory exists
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                
                // First, build the module to generate the ABI
                let status = std::process::Command::new("cargo")
                    .args(["build", "-p", &module])
                    .status()?;
                
                if !status.success() {
                    return Err(eyre::eyre!("Failed to build module {}", module));
                }
                
                // Find the generated ABI file in the build output
                let target_dir = std::path::Path::new("target");
                let debug_dir = target_dir.join("debug");
                let build_dir = debug_dir.join("build");
                
                // Look for the ABI file in the build output
                let mut abi_source = None;
                if build_dir.exists() {
                    for entry in std::fs::read_dir(build_dir)? {
                        let entry = entry?;
                        let path = entry.path();
                        if path.is_dir() && path.file_name().unwrap().to_string_lossy().starts_with(&format!("{}-", module)) {
                            // Look for any .json file in the abi directory
                            let abi_dir = path.join("out/calimero/abi");
                            if abi_dir.exists() {
                                for abi_entry in std::fs::read_dir(abi_dir)? {
                                    let abi_entry = abi_entry?;
                                    let abi_path = abi_entry.path();
                                    if abi_path.extension().map(|ext| ext == "json").unwrap_or(false) {
                                        abi_source = Some(abi_path);
                                        break;
                                    }
                                }
                            }
                            if abi_source.is_some() {
                                break;
                            }
                        }
                    }
                }
                
                let abi_source = abi_source.ok_or_else(|| eyre::eyre!("Could not find ABI file for module {}", module))?;
                
                // Copy the ABI file to the target location
                std::fs::copy(&abi_source, &out)?;
                
                println!("ABI extracted to: {}", out.display());
            }
        },
    }
    
    Ok(())
} 