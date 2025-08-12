use std::fs;
use std::path::Path;
use abi_core::schema::{Abi, AbiTypeRef, TypeDef, FieldDef, VariantDef, VariantKind, MapMode, AbiFunction, AbiParameter, ErrorAbi, FunctionKind, ParameterDirection};
use serde_json::Value;
use sha2::Digest;

#[test]
fn test_struct_s_fixture() {
    let fixture_path = Path::new("tests/fixtures/v011/struct_s.json");
    let sha_path = Path::new("tests/fixtures/v011/struct_s.sha256");
    
    let fixture_content = fs::read_to_string(fixture_path).expect("Failed to read fixture");
    let expected_sha = fs::read_to_string(sha_path).expect("Failed to read SHA");
    
    // Parse the fixture
    let abi: Abi = serde_json::from_str(&fixture_content).expect("Failed to parse fixture");
    
    // Verify schema version
    assert_eq!(abi.metadata.schema_version, "0.1.1");
    
    // Verify module info
    assert_eq!(abi.module_name, "test");
    assert_eq!(abi.module_version, "0.1.0");
    
    // Verify type registry
    let types = abi.types.clone().expect("Types should be present");
    let s_type = types.get("test::S").expect("S type should be present");
    
    match s_type {
        TypeDef::Struct { fields, newtype } => {
            assert_eq!(*newtype, false);
            assert_eq!(fields.len(), 2);
            
            // Check field order is preserved
            assert_eq!(fields[0].name, "a");
            assert_eq!(fields[1].name, "b");
        }
        _ => panic!("Expected struct type"),
    }
    
    // Verify canonical serialization
    let canonical = serde_json::to_string_pretty(&abi).expect("Failed to serialize");
    let mut hasher = sha2::Sha256::new();
    hasher.update(canonical.as_bytes());
    let actual_sha = format!("{:x}", hasher.finalize());
    
    assert_eq!(actual_sha, expected_sha.trim());
}

#[test]
fn test_enum_e_fixture() {
    let fixture_path = Path::new("tests/fixtures/v011/enum_e.json");
    let sha_path = Path::new("tests/fixtures/v011/enum_e.sha256");
    
    let fixture_content = fs::read_to_string(fixture_path).expect("Failed to read fixture");
    let expected_sha = fs::read_to_string(sha_path).expect("Failed to read SHA");
    
    // Parse the fixture
    let abi: Abi = serde_json::from_str(&fixture_content).expect("Failed to parse fixture");
    
    // Verify type registry
    let types = abi.types.clone().expect("Types should be present");
    let e_type = types.get("test::E").expect("E type should be present");
    
    match e_type {
        TypeDef::Enum { variants } => {
            assert_eq!(variants.len(), 3);
            
            // Check variant order is preserved
            assert_eq!(variants[0].name, "A");
            assert_eq!(variants[1].name, "B");
            assert_eq!(variants[2].name, "C");
            
            // Check variant kinds
            assert!(matches!(variants[0].kind, VariantKind::Unit));
            assert!(matches!(variants[1].kind, VariantKind::Tuple { .. }));
            assert!(matches!(variants[2].kind, VariantKind::Struct { .. }));
        }
        _ => panic!("Expected enum type"),
    }
    
    // Verify canonical serialization
    let canonical = serde_json::to_string_pretty(&abi).expect("Failed to serialize");
    let mut hasher = sha2::Sha256::new();
    hasher.update(canonical.as_bytes());
    let actual_sha = format!("{:x}", hasher.finalize());
    
    assert_eq!(actual_sha, expected_sha.trim());
}

#[test]
fn test_map_string_key_fixture() {
    let fixture_path = Path::new("tests/fixtures/v011/map_string_key.json");
    let sha_path = Path::new("tests/fixtures/v011/map_string_key.sha256");
    
    let fixture_content = fs::read_to_string(fixture_path).expect("Failed to read fixture");
    let expected_sha = fs::read_to_string(sha_path).expect("Failed to read SHA");
    
    // Parse the fixture
    let abi: Abi = serde_json::from_str(&fixture_content).expect("Failed to parse fixture");
    
    // Verify type registry
    let types = abi.types.clone().expect("Types should be present");
    let map_type = types.get("test::StringMap").expect("StringMap type should be present");
    
    match map_type {
        TypeDef::Map { key, value, mode } => {
            assert_eq!(*mode, MapMode::Object);
            // Verify key and value types
        }
        _ => panic!("Expected map type"),
    }
    
    // Verify canonical serialization
    let canonical = serde_json::to_string_pretty(&abi).expect("Failed to serialize");
    let mut hasher = sha2::Sha256::new();
    hasher.update(canonical.as_bytes());
    let actual_sha = format!("{:x}", hasher.finalize());
    
    assert_eq!(actual_sha, expected_sha.trim());
}

#[test]
fn test_map_u64_key_fixture() {
    let fixture_path = Path::new("tests/fixtures/v011/map_u64_key.json");
    let sha_path = Path::new("tests/fixtures/v011/map_u64_key.sha256");
    
    let fixture_content = fs::read_to_string(fixture_path).expect("Failed to read fixture");
    let expected_sha = fs::read_to_string(sha_path).expect("Failed to read SHA");
    
    // Parse the fixture
    let abi: Abi = serde_json::from_str(&fixture_content).expect("Failed to parse fixture");
    
    // Verify type registry
    let types = abi.types.clone().expect("Types should be present");
    let map_type = types.get("test::U64Map").expect("U64Map type should be present");
    
    match map_type {
        TypeDef::Map { key, value, mode } => {
            assert_eq!(*mode, MapMode::Entries);
            // Verify key and value types
        }
        _ => panic!("Expected map type"),
    }
    
    // Verify canonical serialization
    let canonical = serde_json::to_string_pretty(&abi).expect("Failed to serialize");
    let mut hasher = sha2::Sha256::new();
    hasher.update(canonical.as_bytes());
    let actual_sha = format!("{:x}", hasher.finalize());
    
    assert_eq!(actual_sha, expected_sha.trim());
}

#[test]
fn test_tuple_and_array_fixture() {
    let fixture_path = Path::new("tests/fixtures/v011/tuple_and_array.json");
    let sha_path = Path::new("tests/fixtures/v011/tuple_and_array.sha256");
    
    let fixture_content = fs::read_to_string(fixture_path).expect("Failed to read fixture");
    let expected_sha = fs::read_to_string(sha_path).expect("Failed to read SHA");
    
    // Parse the fixture
    let abi: Abi = serde_json::from_str(&fixture_content).expect("Failed to parse fixture");
    
    // Verify type registry
    let types = abi.types.clone().expect("Types should be present");
    
    // Check tuple type
    let tuple_type = types.get("test::Tuple").expect("Tuple type should be present");
    match tuple_type {
        TypeDef::Tuple { items } => {
            assert_eq!(items.len(), 2);
        }
        _ => panic!("Expected tuple type"),
    }
    
    // Check array type
    let array_type = types.get("test::Array").expect("Array type should be present");
    match array_type {
        TypeDef::Array { item, len } => {
            assert_eq!(*len, 4);
        }
        _ => panic!("Expected array type"),
    }
    
    // Check newtype
    let newtype = types.get("test::Newtype").expect("Newtype should be present");
    match newtype {
        TypeDef::Struct { fields, newtype } => {
            assert_eq!(*newtype, true);
            assert_eq!(fields.len(), 1);
            assert_eq!(fields[0].name, "0");
        }
        _ => panic!("Expected newtype struct"),
    }
    
    // Verify canonical serialization
    let canonical = serde_json::to_string_pretty(&abi).expect("Failed to serialize");
    let mut hasher = sha2::Sha256::new();
    hasher.update(canonical.as_bytes());
    let actual_sha = format!("{:x}", hasher.finalize());
    
    assert_eq!(actual_sha, expected_sha.trim());
}

#[test]
fn test_function_returns_fixture() {
    let fixture_path = Path::new("tests/fixtures/v011/function_returns.json");
    let sha_path = Path::new("tests/fixtures/v011/function_returns.sha256");
    
    let fixture_content = fs::read_to_string(fixture_path).expect("Failed to read fixture");
    let expected_sha = fs::read_to_string(sha_path).expect("Failed to read SHA");
    
    // Parse the fixture
    let abi: Abi = serde_json::from_str(&fixture_content).expect("Failed to parse fixture");
    
    // Verify functions
    let functions = &abi.functions;
    
    // Check plain return function
    let plain_return = functions.get("plain_return").expect("plain_return should be present");
    assert_eq!(plain_return.kind, FunctionKind::Query);
    assert!(plain_return.returns.is_some());
    assert_eq!(plain_return.errors.len(), 0);
    
    // Check unit return function
    let unit_return = functions.get("unit_return").expect("unit_return should be present");
    assert_eq!(unit_return.kind, FunctionKind::Command);
    assert!(unit_return.returns.is_none());
    assert_eq!(unit_return.errors.len(), 2);
    
    // Check result return function
    let result_return = functions.get("result_return").expect("result_return should be present");
    assert_eq!(result_return.kind, FunctionKind::Query);
    assert!(result_return.returns.is_some());
    assert_eq!(result_return.errors.len(), 2);
    
    // Verify no Result types in JSON
    let json_value: Value = serde_json::from_str(&fixture_content).expect("Failed to parse as Value");
    let json_str = serde_json::to_string(&json_value).expect("Failed to serialize");
    assert!(!json_str.contains("Result"));
    
    // Verify canonical serialization
    let canonical = serde_json::to_string_pretty(&abi).expect("Failed to serialize");
    let mut hasher = sha2::Sha256::new();
    hasher.update(canonical.as_bytes());
    let actual_sha = format!("{:x}", hasher.finalize());
    
    assert_eq!(actual_sha, expected_sha.trim());
}

#[test]
fn test_no_absolute_paths() {
    // Test that generated JSON contains no absolute paths or backslashes
    let fixtures = [
        "tests/fixtures/v011/struct_s.json",
        "tests/fixtures/v011/enum_e.json",
        "tests/fixtures/v011/map_string_key.json",
        "tests/fixtures/v011/map_u64_key.json",
        "tests/fixtures/v011/tuple_and_array.json",
        "tests/fixtures/v011/function_returns.json",
    ];
    
    for fixture_path in fixtures {
        let content = fs::read_to_string(fixture_path).expect("Failed to read fixture");
        
        // Check for no backslashes
        assert!(!content.contains("\\"), "Fixture {} contains backslashes", fixture_path);
        
        // Check for no absolute paths (Windows style)
        assert!(!content.contains(":\\"), "Fixture {} contains Windows absolute paths", fixture_path);
        
        // Check for no current working directory
        let cwd = std::env::current_dir().expect("Failed to get current directory");
        let cwd_str = cwd.to_string_lossy();
        assert!(!content.contains(&*cwd_str), "Fixture {} contains current working directory", fixture_path);
    }
} 