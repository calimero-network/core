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

use crate::{Abi, AbiError, Result};
use sha2::{Digest, Sha256};
use std::io::Write;

/// Write canonical JSON representation of ABI to writer
pub fn write_canonical<W: Write>(abi: &Abi, mut writer: W) -> Result<()> {
    // Use serde_json with sorted keys for deterministic output
    let json = serde_json::to_string_pretty(abi)
        .map_err(AbiError::Serialization)?;
    
    // Normalize line endings to \n for cross-platform determinism
    let normalized = json.replace("\r\n", "\n");
    
    writer.write_all(normalized.as_bytes())?;
    Ok(())
}

/// Compute SHA256 hash of canonical JSON representation
pub fn sha256(abi: &Abi) -> Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    write_canonical(abi, &mut hasher)?;
    Ok(hasher.finalize().into())
}

/// Copy module ABI from OUT_DIR to target directory
pub fn copy_module_abi(out_dir: &std::path::Path, module_abi_path: &str) -> Result<()> {
    use std::fs;
    use std::path::PathBuf;
    
    // Read the ABI file from OUT_DIR
    let abi_path = std::env::var("OUT_DIR")
        .map(PathBuf::from)
        .map(|out_dir| out_dir.join("calimero").join("abi").join(module_abi_path))
        .map_err(|_| AbiError::InvalidAbi {
            message: "OUT_DIR environment variable not set".to_string(),
        })?;
    
    if !abi_path.exists() {
        return Err(AbiError::InvalidAbi {
            message: format!("ABI file not found: {}", abi_path.display()),
        });
    }
    
    // Create target/abi directory
    let target_abi_dir = PathBuf::from("target").join("abi");
    fs::create_dir_all(&target_abi_dir)?;
    
    // Copy the file
    let target_path = target_abi_dir.join("abi.json");
    fs::copy(&abi_path, &target_path)?;
    
    Ok(())
} 