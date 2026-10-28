//! Builds a Rust guest into a `calimero:app` component and inspects the result.
//!
//! `cargo build --release --target wasm32-wasip2` emits a component whose
//! `calimero:abi` custom section sits inside a nested core module. The build
//! lifts it to the component's top level so a reader looks in one place.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Stdio;

use eyre::Result as EyreResult;
use fs4::fs_std::FileExt;
use serde_json::Value;
use sha2::{Digest, Sha256};
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

/// Builds `crate_dir` like [`build_component`], reusing a result cached under `cache_dir`.
///
/// The component is `cache_dir/<crate dir name>-<key>.wasm` (key: see [`cache_key`]); cargo's
/// own target directory is `cache_dir/target/<crate dir name>`.
pub fn build_component_cached(
    crate_dir: &Path,
    also_hash: &[&Path],
    cache_dir: &Path,
) -> EyreResult<Vec<u8>> {
    let name = crate_dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| eyre::eyre!("{} has no name", crate_dir.display()))?;
    let cached = |key: &str| cache_dir.join(format!("{name}-{key}.wasm"));
    if let Ok(bytes) = std::fs::read(cached(&cache_key(crate_dir, also_hash)?)) {
        return Ok(bytes);
    }
    // A miss builds under an exclusive lock per (crate, target dir), held across
    // processes (nextest runs each test in its own): the first builds and writes the
    // cache, the others wait, find it, and reuse it. The key is computed again under
    // the lock, since the first build may have changed what it hashes.
    let target_dir = cache_dir.join("target").join(name);
    std::fs::create_dir_all(&target_dir)?;
    let lock_path = cache_dir.join("target").join(format!("{name}.build.lock"));
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|e| eyre::eyre!("{}: {e}", lock_path.display()))?;
    FileExt::lock_exclusive(&lock).map_err(|e| eyre::eyre!("{}: {e}", lock_path.display()))?;
    let key = cache_key(crate_dir, also_hash)?;
    let cached = cached(&key);
    if let Ok(bytes) = std::fs::read(&cached) {
        return Ok(bytes);
    }
    let built = build_component(crate_dir, &target_dir)?;
    // Write beside, then rename: a reader that skips the lock never sees half a file.
    let tmp = cache_dir.join(format!("{name}-{key}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, &built.component)?;
    std::fs::rename(&tmp, &cached)?;
    // Dropping `lock` closes it, which releases the lock.
    drop(lock);
    Ok(built.component)
}

/// Bump when [`build_component`]'s post-processing (the ABI lift) changes behaviour without
/// its source text changing; the source of this file is hashed too.
const POST_PROCESSING_VERSION: u32 = 1;

/// The first 16 hex digits of a SHA-256 over everything that decides the built component:
/// - every file under `crate_dir` and under each of `also_hash` (the guest's path
///   dependencies, e.g. the SDK crates), skipping any `target/` directory, as
///   (path relative to its root, length, bytes), so `Cargo.toml`, `Cargo.lock` and `src/` count;
/// - the nearest `rust-toolchain.toml` / `rust-toolchain` at or above `crate_dir`;
/// - the output of `rustc -V` run in `crate_dir` (the toolchain that builds the guest);
/// - this crate's version, [`POST_PROCESSING_VERSION`] and the source of this file.
pub fn cache_key(crate_dir: &Path, also_hash: &[&Path]) -> EyreResult<String> {
    let mut hasher = Sha256::new();
    let mut part = |label: &str, bytes: &[u8]| {
        hasher.update((label.len() as u64).to_be_bytes());
        hasher.update(label.as_bytes());
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    };
    for (i, root) in std::iter::once(&crate_dir).chain(also_hash).enumerate() {
        let mut files = Vec::new();
        collect_files(root, &mut files)?;
        files.sort();
        for file in files {
            let rel = file.strip_prefix(root).unwrap_or(&file);
            // A file another process removed while this walked (a temp file) is not part
            // of the tree.
            let bytes = match std::fs::read(&file) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => eyre::bail!("{}: {e}", file.display()),
            };
            part(&format!("{i}:{}", rel.to_string_lossy()), &bytes);
        }
    }
    let toolchain = crate_dir
        .canonicalize()
        .unwrap_or_else(|_| crate_dir.to_path_buf())
        .ancestors()
        .flat_map(|d| ["rust-toolchain.toml", "rust-toolchain"].map(|n| d.join(n)))
        .find_map(|f| std::fs::read(f).ok());
    part("toolchain-file", &toolchain.unwrap_or_default());
    let rustc = Command::new("rustc")
        .arg("-V")
        .current_dir(crate_dir)
        .stdin(Stdio::null())
        .output()?;
    eyre::ensure!(rustc.status.success(), "rustc -V failed ({})", rustc.status);
    part("rustc", &rustc.stdout);
    part("app-build-version", env!("CARGO_PKG_VERSION").as_bytes());
    part("post-processing", &POST_PROCESSING_VERSION.to_be_bytes());
    part("app-build-source", include_str!("lib.rs").as_bytes());
    let digest = hasher.finalize();
    Ok(digest.iter().take(8).map(|b| format!("{b:02x}")).collect())
}

/// Every file under `dir` (none when it does not exist), skipping build output.
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> EyreResult<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => eyre::bail!("{}: {e}", dir.display()),
    };
    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => eyre::bail!("{}: {e}", dir.display()),
        };
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
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
