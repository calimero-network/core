use std::fs;
use std::path::PathBuf;

use wasmparser::{Parser as WasmParser, Payload};

pub fn inspect_wasm(wasm_file: &PathBuf) -> eyre::Result<()> {
    // Read the WASM file
    let wasm_bytes = fs::read(wasm_file)?;

    println!("WASM file: {}", wasm_file.display());
    println!("Size: {} bytes\n", wasm_bytes.len());

    // Parse the WASM file
    let parser = WasmParser::new(0);
    let mut sections = Vec::new();
    let mut custom_sections = Vec::new();
    let mut has_get_abi_exports = false;
    let mut export_count = 0;

    for payload in parser.parse_all(&wasm_bytes) {
        match payload? {
            Payload::Version { num, .. } => {
                sections.push(format!("Version: {num}"));
            }
            Payload::TypeSection(reader) => {
                sections.push(format!("TypeSection: {} types", reader.count()));
            }
            Payload::ImportSection(reader) => {
                sections.push(format!("ImportSection: {} imports", reader.count()));
            }
            Payload::FunctionSection(reader) => {
                sections.push(format!("FunctionSection: {} functions", reader.count()));
            }
            Payload::TableSection(reader) => {
                sections.push(format!("TableSection: {} tables", reader.count()));
            }
            Payload::MemorySection(reader) => {
                sections.push(format!("MemorySection: {} memories", reader.count()));
            }
            Payload::GlobalSection(reader) => {
                sections.push(format!("GlobalSection: {} globals", reader.count()));
            }
            Payload::ExportSection(reader) => {
                export_count = reader.count();
                sections.push(format!("ExportSection: {export_count} exports"));

                for export in reader {
                    let export = export?;
                    if export.name == "get_abi_ptr"
                        || export.name == "get_abi_len"
                        || export.name == "get_abi"
                    {
                        has_get_abi_exports = true;
                    }
                }
            }
            Payload::StartSection { func, .. } => {
                sections.push(format!("StartSection: function {func}"));
            }
            Payload::ElementSection(reader) => {
                sections.push(format!("ElementSection: {} elements", reader.count()));
            }
            Payload::DataCountSection { count, .. } => {
                sections.push(format!("DataCountSection: {count} data segments"));
            }
            Payload::DataSection(reader) => {
                sections.push(format!("DataSection: {} data segments", reader.count()));
            }
            Payload::CodeSectionStart { count, .. } => {
                sections.push(format!("CodeSection: {count} functions"));
            }
            Payload::CustomSection(section) => {
                let name = section.name();
                let size = section.data().len();
                sections.push(format!("CustomSection: '{name}' ({size} bytes)"));
                custom_sections.push((name.to_owned(), size));
            }
            _ => {}
        }
    }

    println!("All sections:");
    for (i, section) in sections.iter().enumerate() {
        println!("  {}. {}", i.saturating_add(1), section);
    }

    println!("\nExports: {export_count} total");
    println!("Has get_abi* exports: {has_get_abi_exports}");

    if !custom_sections.is_empty() {
        println!("\nCustom sections summary:");
        for (name, size) in &custom_sections {
            println!("  - '{name}' ({size} bytes)");
        }
    }

    if custom_sections
        .iter()
        .any(|(name, _)| name == "calimero_abi_v1")
    {
        println!("\n✓ 'calimero_abi_v1' section found - ABI extraction available");
    } else {
        println!("\n⚠️  'calimero_abi_v1' section NOT found.");
        println!("This WASM file was built without ABI generation enabled.");
        println!("\nTo enable ABI generation:");
        println!("  - See the build.rs examples in apps/kv-store or apps/abi_conformance");
        println!("  - Build with: cargo build --target wasm32-unknown-unknown --release");
    }

    Ok(())
}
