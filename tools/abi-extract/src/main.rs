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

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "abi-extract")]
#[command(about = "Extract ABI files from compiled modules")]
struct Args {
    /// Module name to extract ABI for
    #[arg(short, long)]
    module: String,
    
    /// Output directory (default: target/abi)
    #[arg(short, long, default_value = "target/abi")]
    output: PathBuf,
}

fn main() -> eyre::Result<()> {
    let args = Args::parse();
    
    // Extract ABI file
    abi_core::serializer::copy_module_abi(&args.output, &format!("{}.json", args.module))?;
    
    println!("ABI extracted to: {}", args.output.join("abi.json").display());
    
    Ok(())
} 