use std::process::Command;

#[test]
fn embed_then_read_back_finds_section() {
    let dir = std::env::temp_dir().join(format!("mero_abi_embed_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wasm = dir.join("m.wasm");
    let schema = dir.join("s.json");
    std::fs::write(&wasm, [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]).unwrap();
    std::fs::write(
        &schema,
        r#"{"schema_version":"wasm-abi/1","types":{"Root":{"kind":"record","fields":[]}},"methods":[],"events":[],"state_root":"Root"}"#,
    ).unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_mero-abi"))
        .args(["embed", wasm.to_str().unwrap(), schema.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success(), "embed exited non-zero");

    let bytes = std::fs::read(&wasm).unwrap();
    let found = wasmparser::Parser::new(0)
        .parse_all(&bytes)
        .filter_map(Result::ok)
        .any(|p| matches!(p, wasmparser::Payload::CustomSection(c) if c.name() == "calimero_abi_v1"));
    assert!(found, "calimero_abi_v1 section present after embed");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn embed_rejects_bad_schema() {
    let dir = std::env::temp_dir().join(format!("mero_abi_embed_bad_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wasm = dir.join("m.wasm");
    let schema = dir.join("bad.json");
    std::fs::write(&wasm, [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]).unwrap();
    std::fs::write(&schema, b"not json").unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_mero-abi"))
        .args(["embed", wasm.to_str().unwrap(), schema.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(!status.success(), "embed must fail on invalid schema json");
    let _ = std::fs::remove_dir_all(&dir);
}
