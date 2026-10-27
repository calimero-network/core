//! Builds a Rust guest into a `calimero:app` component and inspects the result.
//!
//! `cargo build --release --target wasm32-wasip2` emits a component whose
//! `calimero:abi` custom section sits inside a nested core module. The build
//! lifts it to the component's top level so a reader looks in one place.

use std::path::Path;
use std::process::Command;
use std::process::Stdio;

use eyre::Result as EyreResult;
use serde_json::Value;
use wasmparser::ComponentTypeRef;
use wasmparser::Parser;
use wasmparser::Payload;

/// Name of the custom section carrying the app's ABI JSON.
pub const ABI_SECTION: &str = "calimero:abi";

const TARGET: &str = "wasm32-wasip2";
const HEADER_LEN: usize = 8;
const SECTION_CUSTOM: u8 = 0;
const SECTION_MODULE: u8 = 1;
const SECTION_COMPONENT: u8 = 4;

/// A built component and what was read out of it.
#[derive(Debug)]
pub struct Built {
    pub component: Vec<u8>,
    pub abi: Option<Value>,
    pub imports: Vec<String>,
}

/// Builds the crate at `crate_dir` into `target_dir` and returns the component.
pub fn build_component(crate_dir: &Path, target_dir: &Path) -> EyreResult<Built> {
    let output = Command::new("cargo")
        .args(["build", "--release", "--target", TARGET])
        .args(["--message-format", "json-render-diagnostics"])
        .arg("--target-dir")
        .arg(target_dir)
        .current_dir(crate_dir)
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        eyre::bail!(
            "cargo build failed ({}):\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let artifact = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|msg| msg["reason"] == "compiler-artifact")
        .filter_map(|msg| msg["filenames"].as_array().cloned())
        .flatten()
        .filter_map(|name| name.as_str().map(str::to_owned))
        .find(|name| name.ends_with(".wasm"))
        .ok_or_else(|| eyre::eyre!("cargo produced no .wasm artifact"))?;

    let raw = std::fs::read(&artifact)?;
    let (stripped, abi) = lift_abi(&raw)?;
    let mut component = stripped;
    if let Some(abi) = &abi {
        append_custom_section(&mut component, ABI_SECTION, abi);
    }
    let imports = imports_of(&component)?;
    let abi = abi_of(&component)?;
    Ok(Built {
        component,
        abi,
        imports,
    })
}

/// Lists the component's top-level imports, fully qualified.
pub fn imports_of(component: &[u8]) -> EyreResult<Vec<String>> {
    let mut imports = Vec::new();
    for payload in Parser::new(0).parse_all(component) {
        if let Payload::ComponentImportSection(reader) = payload? {
            for import in reader {
                let import = import?;
                // Type imports (e.g. a bare `invocation-kind`) are not interfaces.
                if matches!(import.ty, ComponentTypeRef::Instance(_)) {
                    imports.push(import.name.full_name().into_owned());
                }
            }
        }
    }
    Ok(imports)
}

/// Reads the ABI JSON from the component's top-level `calimero:abi` section only.
pub fn abi_of(component: &[u8]) -> EyreResult<Option<Value>> {
    for payload in Parser::new(0).parse_all(component) {
        if let Payload::CustomSection(section) = payload? {
            if section.name() == ABI_SECTION {
                return Ok(Some(serde_json::from_slice(section.data())?));
            }
        }
    }
    Ok(None)
}

/// Splits `bytes` (a module or component) into `(id, content)` sections.
fn sections(bytes: &[u8]) -> EyreResult<Vec<(u8, &[u8])>> {
    eyre::ensure!(bytes.len() >= HEADER_LEN, "truncated wasm binary");
    let mut out = Vec::new();
    let mut pos = HEADER_LEN;
    while pos < bytes.len() {
        let id = bytes[pos];
        pos += 1;
        let (len, used) = read_leb(&bytes[pos..])?;
        pos += used;
        let end = pos
            .checked_add(len)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| eyre::eyre!("section overruns the binary"))?;
        out.push((id, &bytes[pos..end]));
        pos = end;
    }
    Ok(out)
}

fn read_leb(bytes: &[u8]) -> EyreResult<(usize, usize)> {
    let mut value = 0usize;
    for (i, byte) in bytes.iter().enumerate().take(5) {
        value |= usize::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok((value, i + 1));
        }
    }
    eyre::bail!("bad section length")
}

fn push_leb(out: &mut Vec<u8>, mut value: usize) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn push_section(out: &mut Vec<u8>, id: u8, content: &[u8]) {
    out.push(id);
    push_leb(out, content.len());
    out.extend_from_slice(content);
}

fn append_custom_section(out: &mut Vec<u8>, name: &str, abi: &Value) {
    let mut content = Vec::new();
    push_leb(&mut content, name.len());
    content.extend_from_slice(name.as_bytes());
    // SAFETY: serializing a `Value` cannot fail.
    content.extend_from_slice(&serde_json::to_vec(abi).expect("Value serializes"));
    push_section(out, SECTION_CUSTOM, &content);
}

/// Returns the binary without any nested `calimero:abi` section, and the ABI found.
fn lift_abi(bytes: &[u8]) -> EyreResult<(Vec<u8>, Option<Value>)> {
    // Component sections 1 and 4 hold nested binaries; in a core module they are
    // the type and table sections, so only descend from a component (layer 1).
    let is_component = bytes.get(6..8) == Some(&[1, 0]);
    let mut out = bytes[..HEADER_LEN.min(bytes.len())].to_vec();
    let mut abi = None;
    for (id, content) in sections(bytes)? {
        match id {
            SECTION_CUSTOM => {
                let (name_len, used) = read_leb(content)?;
                let name = content.get(used..used + name_len);
                if let Some(name) = name.filter(|n| *n == ABI_SECTION.as_bytes()) {
                    let _ = name;
                    abi = Some(serde_json::from_slice(&content[used + name_len..])?);
                    continue;
                }
                push_section(&mut out, id, content);
            }
            SECTION_MODULE | SECTION_COMPONENT if is_component => {
                let (inner, found) = lift_abi(content)?;
                if found.is_some() {
                    abi = found;
                }
                push_section(&mut out, id, &inner);
            }
            _ => push_section(&mut out, id, content),
        }
    }
    Ok((out, abi))
}
