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

fn main() {
    // Set up rerun triggers
    println!("cargo:rerun-if-changed=src/lib.rs");
    
    // Generate ABI JSON if the abi-export feature is enabled
    #[cfg(feature = "abi-export")]
    {
        let abi_json = generate_demo_abi();
        if let Ok(json_bytes) = serde_json::to_vec_pretty(&abi_json) {
            // Write to target/abi/abi.json
            let target_dir = std::path::Path::new("target/abi");
            if let Err(_) = std::fs::create_dir_all(target_dir) {
                eprintln!("Warning: Could not create target/abi directory");
                return;
            }
            
            let abi_path = target_dir.join("abi.json");
            if let Err(e) = std::fs::write(&abi_path, json_bytes) {
                eprintln!("Warning: Could not write ABI file: {}", e);
            } else {
                println!("Generated ABI: {}", abi_path.display());
            }
        }
    }
    
    // Copy ABI file to target directory if it exists
    if let Err(e) = abi_core::build::copy_to_target(
        std::path::Path::new("target/abi/abi.json"),
        "demo"
    ) {
        eprintln!("Warning: Could not copy ABI file: {}", e);
    }
}

#[cfg(feature = "abi-export")]
fn generate_demo_abi() -> serde_json::Value {
    serde_json::json!({
        "metadata": {
            "schema_version": "0.1.1",
            "toolchain_version": "1.75.0",
            "source_hash": "a1b2c3d4e5f6789012345678901234567890abcdef1234567890abcdef123456"
        },
        "module_name": "demo",
        "module_version": "0.1.0",
        "functions": {
            "get_greeting": {
                "name": "get_greeting",
                "kind": "query",
                "parameters": [
                    {
                        "name": "name",
                        "ty": { "type": "string" },
                        "direction": "input"
                    }
                ],
                "returns": { "type": "string" },
                "errors": []
            },
            "set_greeting": {
                "name": "set_greeting",
                "kind": "command",
                "parameters": [
                    {
                        "name": "new_value",
                        "ty": { "type": "string" },
                        "direction": "input"
                    }
                ],
                "returns": null,
                "errors": [
                    {
                        "name": "InvalidGreeting",
                        "code": "INVALID_GREETING",
                        "ty": { "type": "string" }
                    },
                    {
                        "name": "GreetingTooLong",
                        "code": "GREETING_TOO_LONG",
                        "ty": { "type": "u64" }
                    }
                ]
            },
            "compute": {
                "name": "compute",
                "kind": "query",
                "parameters": [
                    {
                        "name": "value",
                        "ty": { "type": "u64" },
                        "direction": "input"
                    },
                    {
                        "name": "divisor",
                        "ty": { "type": "u64" },
                        "direction": "input"
                    }
                ],
                "returns": { "type": "u64" },
                "errors": [
                    {
                        "name": "DivisionByZero",
                        "code": "DIVISION_BY_ZERO",
                        "ty": null
                    },
                    {
                        "name": "Overflow",
                        "code": "OVERFLOW",
                        "ty": null
                    },
                    {
                        "name": "InvalidInput",
                        "code": "INVALID_INPUT",
                        "ty": { "type": "string" }
                    }
                ]
            }
        },
        "events": {
            "GreetingChanged": {
                "name": "GreetingChanged",
                "payload_type": {
                    "kind": "struct",
                    "fields": [
                        {
                            "name": "old",
                            "ty": { "type": "string" }
                        },
                        {
                            "name": "new",
                            "ty": { "type": "string" }
                        }
                    ],
                    "newtype": false
                }
            }
        }
    })
} 