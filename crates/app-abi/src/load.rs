//! Reading an app's ABI from what a developer has at hand: the ABI JSON (or the node's
//! `GET /apps/{app}/abi` answer), a built component (`.wasm`), a bundle (`.mpk`), or a bundle's
//! unpacked `manifest.json` beside its artifacts.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

use calimero_bundle::BundleManifest;
use eyre::{bail, eyre, Result as EyreResult, WrapErr};
use flate2::read::GzDecoder;
use serde_json::Value;

use crate::abi::AppAbi;

/// The most bytes read from one input file or one archive entry.
pub const MAX_INPUT_BYTES: u64 = 256 * 1024 * 1024;

/// The custom section `#[app::logic]` writes the ABI into.
pub const ABI_SECTION: &str = "calimero:abi";

/// One ABI of an input: a multi-service bundle has one per service.
#[derive(Clone, Debug)]
pub struct Loaded {
    /// The service's name; `None` for a single-service bundle or a bare component or ABI.
    pub service: Option<String>,
    pub abi: AppAbi,
    /// The ABI JSON as read.
    pub json: Value,
    /// The bundle's package and app version, when it came from one.
    pub package: Option<String>,
    pub app_version: Option<String>,
}

/// Every ABI `path` holds.
pub fn load_path(path: &Path) -> EyreResult<Vec<Loaded>> {
    let bytes = read_capped(path)?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    load_bytes(&bytes, Some(base)).wrap_err_with(|| format!("reading {}", path.display()))
}

/// Every ABI `bytes` hold; `base` is where a `manifest.json`'s artifacts are.
pub fn load_bytes(bytes: &[u8], base: Option<&Path>) -> EyreResult<Vec<Loaded>> {
    if bytes.starts_with(b"\0asm") {
        return Ok(vec![loaded(None, wasm_abi(bytes)?)?]);
    }
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return mpk(bytes);
    }
    let json: Value =
        serde_json::from_slice(bytes).map_err(|e| eyre!("not a component, bundle or JSON: {e}"))?;
    if json.get("package").is_some() && json.get("methods").is_none() {
        let Some(base) = base else {
            bail!("a manifest.json is read from a file, beside its artifacts");
        };
        let manifest: BundleManifest =
            serde_json::from_value(json).map_err(|e| eyre!("bad manifest.json: {e}"))?;
        return manifest_abis(&manifest, |path| read_capped(&base.join(path)));
    }
    Ok(vec![loaded(None, json)?])
}

/// The single ABI of `path`, or the one of `service` in a multi-service bundle.
pub fn load_one(path: &Path, service: Option<&str>) -> EyreResult<Loaded> {
    let all = load_path(path)?;
    pick(all, service).wrap_err_with(|| format!("reading {}", path.display()))
}

fn pick(mut all: Vec<Loaded>, service: Option<&str>) -> EyreResult<Loaded> {
    if let Some(service) = service {
        let names: Vec<String> = all.iter().filter_map(|l| l.service.clone()).collect();
        return all
            .into_iter()
            .find(|l| l.service.as_deref() == Some(service))
            .ok_or_else(|| eyre!("no service {service:?} (it has {names:?})"));
    }
    if all.len() != 1 {
        let names: Vec<String> = all.iter().filter_map(|l| l.service.clone()).collect();
        bail!("it has several services ({names:?}): name one with --service");
    }
    all.pop().ok_or_else(|| eyre!("no ABI"))
}

fn loaded(service: Option<String>, json: Value) -> EyreResult<Loaded> {
    Ok(Loaded {
        service,
        abi: AppAbi::from_json(&json)?,
        json,
        package: None,
        app_version: None,
    })
}

fn read_capped(path: &Path) -> EyreResult<Vec<u8>> {
    let file = fs::File::open(path).wrap_err_with(|| format!("opening {}", path.display()))?;
    let mut bytes = Vec::new();
    let _ = file.take(MAX_INPUT_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        bail!("{} is over {MAX_INPUT_BYTES} bytes", path.display());
    }
    Ok(bytes)
}

/// The `calimero:abi` section of a component: its top level, where `mero-app-build` puts it,
/// or the core module inside, where a plain `cargo build` leaves it.
pub fn wasm_abi(bytes: &[u8]) -> EyreResult<Value> {
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        if let wasmparser::Payload::CustomSection(section) =
            payload.map_err(|e| eyre!("not a valid WebAssembly binary: {e}"))?
        {
            if section.name() == ABI_SECTION {
                return serde_json::from_slice(section.data())
                    .map_err(|e| eyre!("the {ABI_SECTION} section is not JSON: {e}"));
            }
        }
    }
    bail!("no {ABI_SECTION} section: build the app with calimero-sdk's #[app::logic]")
}

fn mpk(bytes: &[u8]) -> EyreResult<Vec<Loaded>> {
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut total = 0_u64;
    let mut archive = tar::Archive::new(GzDecoder::new(bytes));
    for entry in archive.entries()? {
        let entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?.to_string_lossy().into_owned();
        let path = path.strip_prefix("./").unwrap_or(&path).to_owned();
        let mut data = Vec::new();
        let _ = entry.take(MAX_INPUT_BYTES + 1).read_to_end(&mut data)?;
        total += data.len() as u64;
        if data.len() as u64 > MAX_INPUT_BYTES || total > MAX_INPUT_BYTES {
            bail!("the bundle is over {MAX_INPUT_BYTES} bytes unpacked");
        }
        let _ = files.insert(path, data);
    }
    let manifest = files
        .get("manifest.json")
        .ok_or_else(|| eyre!("the bundle has no manifest.json"))?;
    let manifest: BundleManifest =
        serde_json::from_slice(manifest).map_err(|e| eyre!("bad manifest.json: {e}"))?;
    manifest_abis(&manifest, |path| {
        files
            .get(path.strip_prefix("./").unwrap_or(path))
            .cloned()
            .ok_or_else(|| eyre!("the bundle has no {path}"))
    })
}

fn manifest_abis(
    manifest: &BundleManifest,
    read: impl Fn(&str) -> EyreResult<Vec<u8>>,
) -> EyreResult<Vec<Loaded>> {
    let artifacts: Vec<_> = manifest
        .wasm_artifacts()
        .into_iter()
        .filter_map(|a| Some((a.name, a.wasm?)))
        .collect();
    if artifacts.is_empty() {
        bail!("the manifest names no wasm");
    }
    artifacts
        .iter()
        .map(|(name, artifact)| {
            let wasm = read(&artifact.path)?;
            let mut l = loaded(name.map(str::to_owned), wasm_abi(&wasm)?)?;
            l.package = Some(manifest.package.clone());
            l.app_version = Some(manifest.app_version.clone());
            Ok(l)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A wasm module holding only the ABI custom section.
    fn wasm_with_abi(abi: &Value) -> Vec<u8> {
        let data = serde_json::to_vec(abi).unwrap();
        let mut section = Vec::new();
        section.push(u8::try_from(ABI_SECTION.len()).unwrap());
        section.extend_from_slice(ABI_SECTION.as_bytes());
        section.extend_from_slice(&data);
        let mut out = b"\0asm\x01\0\0\0".to_vec();
        out.push(0);
        let mut len = section.len();
        loop {
            let byte = u8::try_from(len & 0x7f).unwrap();
            len >>= 7;
            if len == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
        out.extend_from_slice(&section);
        out
    }

    fn abi() -> Value {
        json!({ "abiVersion": 1, "methods": { "len": { "kind": "view", "params": [], "returns": "u64" } } })
    }

    #[test]
    fn json_wasm_and_bundle() {
        let from_json = load_bytes(&serde_json::to_vec(&abi()).unwrap(), None).unwrap();
        assert_eq!(from_json.len(), 1);

        let wasm = wasm_with_abi(&abi());
        let from_wasm = load_bytes(&wasm, None).unwrap();
        assert_eq!(from_wasm[0].abi, from_json[0].abi);

        let manifest = json!({
            "version": "1.0", "package": "com.example.kv", "appVersion": "1.0.0",
            "minRuntimeVersion": "0.0.1",
            "wasm": { "path": "app.wasm", "hash": "00", "size": wasm.len() },
        });
        let mut tar = tar::Builder::new(Vec::new());
        for (name, data) in [
            ("manifest.json", serde_json::to_vec(&manifest).unwrap()),
            ("app.wasm", wasm),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, data.as_slice()).unwrap();
        }
        let tar = tar.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &tar).unwrap();
        let from_mpk = load_bytes(&gz.finish().unwrap(), None).unwrap();
        assert_eq!(from_mpk[0].abi, from_json[0].abi);
        assert_eq!(from_mpk[0].service, None);
        assert_eq!(from_mpk[0].package.as_deref(), Some("com.example.kv"));
    }

    #[test]
    fn a_wasm_without_the_section_is_refused() {
        let err = load_bytes(b"\0asm\x01\0\0\0", None).unwrap_err();
        assert!(err.to_string().contains("no calimero:abi section"));
    }
}
