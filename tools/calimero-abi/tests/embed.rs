use std::fs;

const VALID_MANIFEST_JSON: &str =
    r#"{"schema_version":"wasm-abi/1","types":{},"methods":[],"events":[]}"#;

#[test]
fn embed_adds_custom_section() {
    let dir = tempfile::tempdir().unwrap();
    let wasm_path = dir.path().join("app.wasm");
    // minimal empty module
    fs::write(&wasm_path, wat::parse_str("(module)").unwrap()).unwrap();

    let schema_path = dir.path().join("abi.json");
    fs::write(&schema_path, VALID_MANIFEST_JSON).unwrap();

    mero_abi::run_embed(&wasm_path, &schema_path).unwrap();

    let bytes = fs::read(&wasm_path).unwrap();
    let mut found = false;
    for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
        if let wasmparser::Payload::CustomSection(s) = payload.unwrap() {
            if s.name() == "calimero_abi_v1" {
                found = true;
                assert_eq!(s.data(), fs::read(&schema_path).unwrap().as_slice());
            }
        }
    }
    assert!(found, "calimero_abi_v1 section missing");
}
