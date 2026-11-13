//! Builds a Rust guest into a `calimero:app` component and inspects the result.
//!
//! `cargo build --release --target wasm32-wasip2` emits a component whose
//! `calimero:abi` custom section sits inside a nested core module. The build
//! lifts it to the component's top level so a reader looks in one place.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Stdio;

use eyre::Result as EyreResult;
use serde_json::Value;
use sha2::{Digest, Sha256};
use wasmparser::ComponentTypeRef;
use wasmparser::Parser;
use wasmparser::Payload;

pub mod abi_tools;
pub mod codegen;

/// Name of the custom section carrying the app's ABI JSON.
pub const ABI_SECTION: &str = "calimero:abi";

const TARGET: &str = "wasm32-wasip2";
const COMPUTE_CFG: &str = "calimero_compute"; // selects the compute world in the SDK and macros
const COMPUTE_TARGET_SUBDIR: &str = "compute"; // inside the target dir, so the two builds never invalidate each other
const TARGET_LOCK: &str = ".calimero-app-build.lock"; // inside the target dir
const SHARED_TARGET: &str = "shared"; // the cache's one cargo target dir, under `<cache>/target`
const HEADER_LEN: usize = 8;
const SECTION_CUSTOM: u8 = 0;
const SECTION_MODULE: u8 = 1;
const SECTION_COMPONENT: u8 = 4;

/// Opens `path` (creating it) and takes an exclusive lock, released when the file drops.
fn lock_exclusive(path: &Path) -> EyreResult<std::fs::File> {
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(|e| eyre::eyre!("{}: {e}", path.display()))?;
    lock.lock()
        .map_err(|e| eyre::eyre!("{}: {e}", path.display()))?;
    Ok(lock)
}

/// A built component and what was read out of it.
#[derive(Debug)]
pub struct Built {
    pub component: Vec<u8>,
    pub abi: Option<Value>,
    pub imports: Vec<String>,
}

impl Built {
    /// A component as [`build_component`] answers it, read back from its bytes (a cached one).
    pub fn from_component(component: Vec<u8>) -> EyreResult<Self> {
        let abi = abi_of(&component)?;
        let imports = imports_of(&component)?;
        Ok(Self {
            component,
            abi,
            imports,
        })
    }
}

/// An app's components: the one replicas run and, when it declares compute methods, the one
/// job nodes run.
#[derive(Debug)]
pub struct BuiltApp {
    pub app: Built,
    pub compute: Option<Built>,
}

/// Builds the crate at `crate_dir` into `target_dir` and returns the component.
pub fn build_component(crate_dir: &Path, target_dir: &Path) -> EyreResult<Built> {
    build(crate_dir, target_dir, false)
}

/// Builds the compute component of the crate at `crate_dir`: the same crate with
/// `--cfg calimero_compute`, into `target_dir/compute`.
pub fn build_compute_component(crate_dir: &Path, target_dir: &Path) -> EyreResult<Built> {
    build(crate_dir, &target_dir.join(COMPUTE_TARGET_SUBDIR), true)
}

/// Builds the crate's app component and, when its ABI declares compute methods, its compute
/// component.
pub fn build_components(crate_dir: &Path, target_dir: &Path) -> EyreResult<BuiltApp> {
    let app = build_component(crate_dir, target_dir)?;
    let has_compute = app
        .abi
        .as_ref()
        .and_then(|abi| abi["methods"].as_object())
        .is_some_and(|methods| methods.values().any(|m| m["kind"] == "compute"));
    let compute = has_compute
        .then(|| build_compute_component(crate_dir, target_dir))
        .transpose()?;
    Ok(BuiltApp { app, compute })
}

/// `CARGO_ENCODED_RUSTFLAGS` as cargo would read it (`RUSTFLAGS` when unset), plus the compute cfg.
fn compute_rustflags() -> String {
    let mut flags: Vec<String> = match (
        std::env::var("CARGO_ENCODED_RUSTFLAGS"),
        std::env::var("RUSTFLAGS"),
    ) {
        (Ok(encoded), _) => encoded.split('\x1f').map(str::to_owned).collect(),
        (Err(_), Ok(plain)) => plain.split_whitespace().map(str::to_owned).collect(),
        _ => Vec::new(),
    };
    flags.retain(|flag| !flag.is_empty());
    flags.extend(["--cfg".to_owned(), COMPUTE_CFG.to_owned()]);
    flags.join("\x1f")
}

/// The `cargo build` of `crate_dir` into `target_dir`. Cargo runs in
/// `crate_dir`, so a relative `target_dir` (the caller's, relative to the
/// caller's directory) is made absolute first; passed as is, it would land
/// under the crate (`<crate>/<crate>/target`).
fn cargo_build(crate_dir: &Path, target_dir: &Path, compute: bool) -> EyreResult<Command> {
    let mut cargo = Command::new("cargo");
    let _ = cargo
        .args(["build", "--release", "--target", TARGET])
        .args(["--message-format", "json-render-diagnostics"])
        .arg("--target-dir")
        .arg(std::path::absolute(target_dir)?)
        .current_dir(crate_dir)
        .stdin(Stdio::null());
    if compute {
        let _ = cargo.env("CARGO_ENCODED_RUSTFLAGS", compute_rustflags());
    }
    Ok(cargo)
}

fn build(crate_dir: &Path, target_dir: &Path, compute: bool) -> EyreResult<Built> {
    // Builds into one target dir race on its artifacts across threads and processes.
    std::fs::create_dir_all(target_dir)?;
    let _lock = lock_exclusive(&target_dir.join(TARGET_LOCK))?;
    let output = cargo_build(crate_dir, target_dir, compute)?.output()?;
    if !output.status.success() {
        eyre::bail!(
            "cargo build failed ({}):\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let manifest = crate_dir.join("Cargo.toml").canonicalize()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    // A cdylib dependency also emits a .wasm, so pick the one of the crate being built.
    let artifact = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|msg| msg["reason"] == "compiler-artifact")
        .filter(|msg| {
            msg["manifest_path"]
                .as_str()
                .and_then(|path| Path::new(path).canonicalize().ok())
                .is_some_and(|path| path == manifest)
        })
        .filter_map(|msg| msg["filenames"].as_array().cloned())
        .flatten()
        .filter_map(|name| name.as_str().map(str::to_owned))
        .find(|name| name.ends_with(".wasm"))
        .ok_or_else(|| eyre::eyre!("{} produced no .wasm artifact", crate_dir.display()))?;

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

/// An app's components as the cache keeps them: the app component and, when it declares
/// compute methods, the compute component.
#[derive(Clone, Debug)]
pub struct CachedApp {
    pub app: Vec<u8>,
    pub compute: Option<Vec<u8>>,
}

/// Builds `crate_dir` like [`build_component`], reusing a result cached under `cache_dir`.
///
/// The component is `cache_dir/<crate dir name>-<key>.wasm` (key: see [`cache_key`]); cargo's
/// own target directory is `cache_dir/target/shared`, one for every crate built through the cache. Use [`build_components_cached`]
/// to get the compute component too.
pub fn build_component_cached(
    crate_dir: &Path,
    also_hash: &[&Path],
    cache_dir: &Path,
) -> EyreResult<Vec<u8>> {
    Ok(build_components_cached(crate_dir, also_hash, cache_dir)?.app)
}

/// Builds `crate_dir` like [`build_components`], reusing a result cached under `cache_dir`.
///
/// The app component is `<name>-<key>.wasm` and the compute one `<name>-<key>.compute.wasm`,
/// written first, so an app file that exists says the compute decision is final too.
pub fn build_components_cached(
    crate_dir: &Path,
    also_hash: &[&Path],
    cache_dir: &Path,
) -> EyreResult<CachedApp> {
    let name = crate_dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| eyre::eyre!("{} has no name", crate_dir.display()))?;
    let cached = |key: &str, ext: &str| cache_dir.join(format!("{name}-{key}.{ext}"));
    let read = |key: &str| {
        let app = std::fs::read(cached(key, "wasm")).ok()?;
        let compute = std::fs::read(cached(key, "compute.wasm")).ok();
        Some(CachedApp { app, compute })
    };
    if let Some(hit) = read(&cache_key(crate_dir, also_hash)?) {
        return Ok(hit);
    }
    // A miss builds under an exclusive lock per crate, held across
    // processes (nextest runs each test in its own): the first builds and writes the
    // cache, the others wait, find it, and reuse it. The key is computed again under
    // the lock, since the first build may have changed what it hashes.
    // One cargo target dir for every crate the cache builds: the guests share most of their
    // dependencies (the SDK, wit-bindgen, serde), which then compile once instead of once per
    // crate. `build` locks it, so builds of different crates take turns, each still using
    // every core.
    let target_dir = cache_dir.join("target").join(SHARED_TARGET);
    std::fs::create_dir_all(&target_dir)?;
    let lock = lock_exclusive(&cache_dir.join("target").join(format!("{name}.build.lock")))?;
    let key = cache_key(crate_dir, also_hash)?;
    if let Some(hit) = read(&key) {
        return Ok(hit);
    }
    let built = build_components(crate_dir, &target_dir)?;
    // Write beside, then rename: a reader that skips the lock never sees half a file.
    let write = |ext: &str, bytes: &[u8]| -> EyreResult<()> {
        let tmp = cache_dir.join(format!("{name}-{key}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, cached(&key, ext))?;
        Ok(())
    };
    if let Some(compute) = &built.compute {
        write("compute.wasm", &compute.component)?;
    }
    write("wasm", &built.app.component)?;
    // Dropping `lock` closes it, which releases the lock.
    drop(lock);
    Ok(CachedApp {
        app: built.app.component,
        compute: built.compute.map(|c| c.component),
    })
}

/// The guest crate at `crate_dir` (a path in this repository) built through the repository's
/// one component cache, `target/app-components` (under `CARGO_TARGET_DIR` when set), for tests.
///
/// Every test binary that builds a guest goes through it (the node's cluster tests, the
/// engine's, the SDK's, mero-compute's), so a guest is compiled once per tree, by whichever
/// test process gets there first, rather than once per test binary into its own target dir,
/// and every other process reads the result instead of running cargo again. The key hashes
/// the crate and the SDK, its macros, its old name and the WIT (`calimero:app`), which are
/// every path a guest here reaches outside its own directory; see [`cache_key`].
pub fn build_in_repo_cache(crate_dir: &Path) -> EyreResult<BuiltApp> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let crates = root.join("crates");
    let also = ["sdk", "sdk-macros", "app-sdk", "app-wit"].map(|c| crates.join(c));
    let also: Vec<&Path> = also.iter().map(PathBuf::as_path).collect();
    // The target dir cargo would use, as the node's test harness reads it.
    let target =
        std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    let cached = build_components_cached(crate_dir, &also, &target.join("app-components"))?;
    Ok(BuiltApp {
        app: Built::from_component(cached.app)?,
        compute: cached.compute.map(Built::from_component).transpose()?,
    })
}

/// Bump when [`build_component`]'s post-processing (the ABI lift) changes behaviour without
/// its source text changing; the source of this file is hashed too.
const POST_PROCESSING_VERSION: u32 = 2;

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

/// The component with its top-level `calimero:abi` section replaced by `abi`, for tests that
/// need a component whose ABI says something its code does not.
pub fn with_abi(component: &[u8], abi: &Value) -> EyreResult<Vec<u8>> {
    let (mut out, _) = lift_abi(component)?;
    append_custom_section(&mut out, ABI_SECTION, abi);
    Ok(out)
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::cargo_build;

    // `meroctl app build apps/kv-store/logic` from the repository root used to
    // hand cargo `--target-dir apps/kv-store/logic/target` while running it in
    // the crate, so the build landed in `apps/kv-store/logic/apps/kv-store/logic/target`.
    #[test]
    fn a_relative_target_dir_is_the_callers_not_the_crates() {
        let cargo = cargo_build(Path::new("apps/kv"), Path::new("apps/kv/target"), false).unwrap();
        let args: Vec<_> = cargo.get_args().collect();
        let at = args.iter().position(|a| *a == "--target-dir").unwrap();
        let target = Path::new(args[at + 1]);
        assert!(target.is_absolute(), "{}", target.display());
        assert_eq!(
            target,
            std::env::current_dir().unwrap().join("apps/kv/target")
        );
        assert_eq!(cargo.get_current_dir(), Some(Path::new("apps/kv")));
    }
}
