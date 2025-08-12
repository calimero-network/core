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
    /// Extract ABI file for a specific package
    Extract {
        /// Package name to extract ABI for
        #[arg(short, long)]
        package: String,
        
        /// Output path (default: target/abi/abi.json)
        #[arg(short, long, default_value = "target/abi/abi.json")]
        out: PathBuf,
    },
}

fn main() -> eyre::Result<()> {
    let args = Args::parse();
    
    match args.command {
        Commands::Abi { subcommand } => match subcommand {
            AbiCommands::Extract { package, out } => {
                // Ensure output directory exists
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                
                // Build the package with abi-export feature to generate the ABI
                let status = std::process::Command::new("cargo")
                    .args(["build", "-p", &package, "--features", "abi-export"])
                    .status()?;
                
                if !status.success() {
                    return Err(eyre::eyre!("Failed to build package {}", package));
                }
                
                // The ABI should already be at the target location due to build.rs
                if !out.exists() {
                    return Err(eyre::eyre!("ABI file not found at {}", out.display()));
                }
                
                println!("ABI extracted to: {}", out.display());
                
                // Print SHA256 for verification
                let output = std::process::Command::new("shasum")
                    .args(["-a", "256", out.to_str().unwrap()])
                    .output()?;
                
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let sha256 = stdout.trim();
                    println!("SHA256: {}", sha256);
                }
            }
        },
    }
    
    Ok(())
} 