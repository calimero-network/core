//! The on-disk cache of compiled components (spec section 5 step 1, S8).
//!
//! An artifact is named `<component sha256>-<wasmtime tag>-<config hash>.cwasm`, beside a
//! `.cwasm.sha256` sidecar holding the artifact's own SHA-256 written at compile time.
//! A load reads the artifact into memory, checks it against the sidecar and only then
//! deserializes those same bytes; any mismatch or error recompiles and rewrites both.

use std::fs;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use calimero_app_host::PrepareError;
use eyre::Result as EyreResult;
use sha2::Digest;
use sha2::Sha256;
use wasmtime::component::Component;
use wasmtime::Engine;

/// The wasmtime release line the artifacts were made by. The config hash below also
/// covers the exact wasmtime version, so a patch release recompiles too.
const WASMTIME_TAG: &str = "wasmtime50";
/// Hex characters of the config hash kept in a file name.
const CONFIG_HASH_CHARS: usize = 16;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) struct Cache {
    dir: PathBuf,
    config_hash: String,
}

impl Cache {
    pub(crate) fn new(dir: PathBuf, engine: &Engine) -> EyreResult<Self> {
        fs::create_dir_all(&dir)?;
        let mut hasher = Sha256Hasher(Sha256::new());
        engine.precompile_compatibility_hash().hash(&mut hasher);
        let mut config_hash = hex::encode(hasher.0.finalize());
        config_hash.truncate(CONFIG_HASH_CHARS);
        Ok(Self { dir, config_hash })
    }

    /// The compiled component, from the cache when its artifact is intact.
    pub(crate) fn load_or_compile(
        &self,
        engine: &Engine,
        component: &[u8],
    ) -> Result<Component, PrepareError> {
        let key = hex::encode(Sha256::digest(component));
        let artifact = self
            .dir
            .join(format!("{key}-{WASMTIME_TAG}-{}.cwasm", self.config_hash));
        let sidecar = artifact.with_extension("cwasm.sha256");
        if let Some(compiled) = load(engine, &artifact, &sidecar) {
            return Ok(compiled);
        }
        let compiled =
            Component::new(engine, component).map_err(|e| PrepareError::Compile(e.to_string()))?;
        // The cache is an optimisation: a failed write only costs a recompile next time.
        if let Ok(bytes) = compiled.serialize() {
            let _ = store(&artifact, &sidecar, &bytes);
        }
        Ok(compiled)
    }
}

fn load(engine: &Engine, artifact: &Path, sidecar: &Path) -> Option<Component> {
    let bytes = fs::read(artifact).ok()?;
    let expected = fs::read_to_string(sidecar).ok()?;
    if hex::encode(Sha256::digest(&bytes)) != expected.trim() {
        return None;
    }
    // SAFETY: `Component::deserialize` trusts its input to be an artifact this engine
    // produced. These exact in-memory bytes (not a re-read of the file) match the SHA-256
    // written beside them when this node compiled and serialized them, and wasmtime still
    // checks its own version and config header before using them.
    unsafe { Component::deserialize(engine, &bytes) }.ok()
}

/// Writes the artifact, then its sidecar, each through a temp file and a rename, so a
/// reader never sees a half-written file pass the check.
fn store(artifact: &Path, sidecar: &Path, bytes: &[u8]) -> EyreResult<()> {
    write_atomic(artifact, bytes)?;
    write_atomic(sidecar, hex::encode(Sha256::digest(bytes)).as_bytes())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> EyreResult<()> {
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = path.with_extension(format!("tmp-{}-{n}", std::process::id()));
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path)?;
    Ok(())
}

/// Feeds `Hash` output into SHA-256, so the config hash is stable across processes.
struct Sha256Hasher(Sha256);

impl Hasher for Sha256Hasher {
    fn finish(&self) -> u64 {
        let digest = self.0.clone().finalize();
        let mut first = [0; 8];
        first.copy_from_slice(&digest[..8]);
        u64::from_le_bytes(first)
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }
}
