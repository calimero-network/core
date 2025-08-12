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

use std::fs;
use std::path::Path;
use crate::Result;

/// Emit ABI JSON to OUT_DIR if the abi-export feature is enabled
/// 
/// This function writes the ABI JSON to `${OUT_DIR}/calimero/abi/<module>.json`
/// and is a no-op when the abi-export feature is disabled.
pub fn emit_if_enabled(module: &str, abi_json: &[u8]) -> Result<()> {
    let out_dir = std::env::var("OUT_DIR")
        .map_err(|_| crate::AbiError::BuildError("OUT_DIR not set".to_string()))?;
    
    let abi_dir = Path::new(&out_dir).join("calimero").join("abi");
    fs::create_dir_all(&abi_dir)
        .map_err(|e| crate::AbiError::BuildError(format!("Failed to create ABI directory: {}", e)))?;
    
    let abi_path = abi_dir.join(format!("{}.json", module));
    fs::write(&abi_path, abi_json)
        .map_err(|e| crate::AbiError::BuildError(format!("Failed to write ABI file: {}", e)))?;
    
    Ok(())
}

/// Copy ABI file from OUT_DIR to target directory
/// 
/// This function copies the ABI file from `${OUT_DIR}/calimero/abi/<module>.json`
/// to `target/abi/abi.json`, creating the target directory if needed.
pub fn copy_to_target(out_path: &Path, module: &str) -> Result<()> {
    let out_dir = std::env::var("OUT_DIR")
        .map_err(|_| crate::AbiError::BuildError("OUT_DIR not set".to_string()))?;
    
    let source_path = Path::new(&out_dir).join("calimero").join("abi").join(format!("{}.json", module));
    
    if !source_path.exists() {
        return Err(crate::AbiError::BuildError(format!(
            "ABI file not found at {}", source_path.display()
        )));
    }
    
    // Create target directory
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| crate::AbiError::BuildError(format!("Failed to create target directory: {}", e)))?;
    }
    
    // Copy the file
    fs::copy(&source_path, out_path)
        .map_err(|e| crate::AbiError::BuildError(format!("Failed to copy ABI file: {}", e)))?;
    
    Ok(())
} 