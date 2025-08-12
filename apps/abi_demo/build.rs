use std::{fs, path::PathBuf};

fn main() {
    let out = PathBuf::from("target").join("abi");
    fs::create_dir_all(&out).expect("create target/abi");
    println!("cargo:rustc-env=CALIMERO_ABI_OUT={}", out.join("abi.json").display());
    println!("cargo:rerun-if-changed=src/lib.rs");
    
    // Generate ABI JSON if the abi-export feature is enabled
    #[cfg(feature = "abi-export")]
    {
        let abi_json = generate_demo_abi();
        if let Ok(json_bytes) = serde_json::to_vec_pretty(&abi_json) {
            let abi_path = out.join("abi.json");
            if let Err(e) = fs::write(&abi_path, json_bytes) {
                eprintln!("Warning: Could not write ABI file: {}", e);
            } else {
                println!("Generated ABI: {}", abi_path.display());
            }
        }
    }
}

#[cfg(feature = "abi-export")]
fn generate_demo_abi() -> serde_json::Value {
    use abi_core::schema::{Abi, AbiFunction, AbiEvent, AbiParameter, AbiTypeRef, TypeDef, FieldDef, VariantDef, VariantKind, MapMode, FunctionKind, ParameterDirection, ErrorAbi};
    use sha2::{Digest, Sha256};
    
    // Generate source hash
    let mut hasher = Sha256::new();
    hasher.update("demo0.1.0".as_bytes());
    let source_hash = hex::encode(hasher.finalize());
    
    // Create ABI
    let mut abi = Abi::new(
        "demo".to_string(),
        "0.1.0".to_string(),
        "1.85.0".to_string(),
        source_hash,
    );
    
    // Add ScoreMap type (BTreeMap<String, u64>)
    let score_map_type = TypeDef::Map {
        key: AbiTypeRef::inline_primitive("string".to_string()),
        value: AbiTypeRef::inline_primitive("u64".to_string()),
        mode: MapMode::Object,
    };
    abi.add_type("ScoreMap".to_string(), score_map_type);
    
    // Add DemoError type
    let demo_error_type = TypeDef::Enum {
        variants: vec![
            VariantDef {
                name: "Empty".to_string(),
                kind: VariantKind::Unit,
            },
            VariantDef {
                name: "TooLong".to_string(),
                kind: VariantKind::Struct {
                    fields: vec![
                        FieldDef {
                            name: "max".to_string(),
                            ty: AbiTypeRef::inline_primitive("u8".to_string()),
                        },
                        FieldDef {
                            name: "got".to_string(),
                            ty: AbiTypeRef::inline_primitive("u8".to_string()),
                        },
                    ],
                },
            },
        ],
    };
    abi.add_type("DemoError".to_string(), demo_error_type);
    
    // Add functions
    let get_greeting = AbiFunction {
        name: "get_greeting".to_string(),
        kind: FunctionKind::Query,
        parameters: vec![
            AbiParameter {
                name: "name".to_string(),
                ty: AbiTypeRef::inline_primitive("string".to_string()),
                direction: ParameterDirection::Input,
            },
        ],
        returns: Some(AbiTypeRef::inline_primitive("string".to_string())),
        errors: vec![],
    };
    abi.add_function(get_greeting);
    
    let set_greeting = AbiFunction {
        name: "set_greeting".to_string(),
        kind: FunctionKind::Command,
        parameters: vec![
            AbiParameter {
                name: "new_value".to_string(),
                ty: AbiTypeRef::inline_primitive("string".to_string()),
                direction: ParameterDirection::Input,
            },
        ],
        returns: None,
        errors: vec![
            ErrorAbi {
                name: "Empty".to_string(),
                code: "EMPTY".to_string(),
                ty: None,
            },
            ErrorAbi {
                name: "TooLong".to_string(),
                code: "TOO_LONG".to_string(),
                ty: Some(AbiTypeRef::ref_("DemoError".to_string())),
            },
        ],
    };
    abi.add_function(set_greeting);
    
    let get_scores = AbiFunction {
        name: "get_scores".to_string(),
        kind: FunctionKind::Query,
        parameters: vec![],
        returns: Some(AbiTypeRef::ref_("ScoreMap".to_string())),
        errors: vec![],
    };
    abi.add_function(get_scores);
    
    let set_score = AbiFunction {
        name: "set_score".to_string(),
        kind: FunctionKind::Command,
        parameters: vec![
            AbiParameter {
                name: "player".to_string(),
                ty: AbiTypeRef::inline_primitive("string".to_string()),
                direction: ParameterDirection::Input,
            },
            AbiParameter {
                name: "score".to_string(),
                ty: AbiTypeRef::inline_primitive("u64".to_string()),
                direction: ParameterDirection::Input,
            },
        ],
        returns: None,
        errors: vec![],
    };
    abi.add_function(set_score);
    
    // Add events
    let greeting_changed = AbiEvent {
        name: "GreetingChanged".to_string(),
        payload_type: Some(AbiTypeRef::inline_composite(
            "struct".to_string(),
            None,
            None,
            None,
            None,
            None,
            Some(vec![
                FieldDef {
                    name: "old".to_string(),
                    ty: AbiTypeRef::inline_primitive("string".to_string()),
                },
                FieldDef {
                    name: "new".to_string(),
                    ty: AbiTypeRef::inline_primitive("string".to_string()),
                },
            ]),
            None,
            None,
        )),
    };
    abi.add_event(greeting_changed);
    
    // Convert to JSON
    serde_json::to_value(abi).unwrap()
} 