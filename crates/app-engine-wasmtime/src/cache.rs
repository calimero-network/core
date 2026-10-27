//! The on-disk cache of compiled components (spec section 5 step 1, S8).
//!
//! An artifact is named `<component sha256>-<wasmtime tag>-<config hash>.cwasm`, beside a
//! `.cwasm.mac` sidecar: HMAC-SHA256 under the caller's cache key over the artifact's file
//! name and bytes, written at compile time (S8, Ruling 38). The key comes from the node's
//! storage key, so a party that can write the directory but does not hold the key can
//! neither forge an artifact nor move a valid one under another component's name. A load
//! reads the artifact into memory, verifies the MAC and only then deserializes those same
//! bytes; any mismatch or error recompiles and rewrites both.

use std::fs;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use calimero_app_host::PrepareError;
use eyre::Result as EyreResult;
use ring::hmac;
use sha2::Digest;
use sha2::Sha256;
use subtle::ConstantTimeEq;
use wasmtime::component::Component;
use wasmtime::Engine;

/// The wasmtime release line the artifacts were made by. The config hash below also
/// covers the exact wasmtime version, so a patch release recompiles too.
const WASMTIME_TAG: &str = "wasmtime50";
/// Domain separation of the artifact MAC.
const MAC_DOMAIN: &[u8] = b"calimero.cwasm.v1\0";
/// Hex characters of the config hash kept in a file name.
const CONFIG_HASH_CHARS: usize = 16;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) struct Cache {
    dir: PathBuf,
    config_hash: String,
    key: hmac::Key,
}

impl Cache {
    /// `key` authenticates the artifacts (S8): the node derives it from its storage key.
    pub(crate) fn new(dir: PathBuf, engine: &Engine, key: &[u8; 32]) -> EyreResult<Self> {
        fs::create_dir_all(&dir)?;
        let mut hasher = Sha256Hasher(Sha256::new());
        engine.precompile_compatibility_hash().hash(&mut hasher);
        let mut config_hash = hex::encode(hasher.0.finalize());
        config_hash.truncate(CONFIG_HASH_CHARS);
        Ok(Self {
            dir,
            config_hash,
            key: hmac::Key::new(hmac::HMAC_SHA256, key),
        })
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
        let sidecar = artifact.with_extension("cwasm.mac");
        if let Some(compiled) = self.load(engine, &artifact, &sidecar) {
            return Ok(compiled);
        }
        let compiled =
            Component::new(engine, component).map_err(|e| PrepareError::Compile(e.to_string()))?;
        // The cache is an optimisation: a failed write only costs a recompile next time.
        if let Ok(bytes) = compiled.serialize() {
            let _ = self.store(&artifact, &sidecar, &bytes);
        }
        Ok(compiled)
    }

    /// The MAC of `bytes` stored as `artifact`: the file name is covered, so a valid
    /// artifact of one component does not pass under another's name.
    fn mac(&self, artifact: &Path, bytes: &[u8]) -> hmac::Tag {
        let name = artifact
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut ctx = hmac::Context::with_key(&self.key);
        ctx.update(MAC_DOMAIN);
        ctx.update(name.as_bytes());
        ctx.update(b"\0");
        ctx.update(bytes);
        ctx.sign()
    }

    fn load(&self, engine: &Engine, artifact: &Path, sidecar: &Path) -> Option<Component> {
        let bytes = fs::read(artifact).ok()?;
        let expected = hex::decode(fs::read_to_string(sidecar).ok()?.trim()).ok()?;
        let tag = self.mac(artifact, &bytes);
        // `hmac::verify` takes the whole message in one slice, which would copy the
        // artifact; the tag is computed over its parts and compared in constant time
        // (`subtle`; `ring::constant_time` is deprecated in the ring this tree uses).
        if !bool::from(tag.as_ref().ct_eq(expected.as_slice())) {
            return None;
        }
        // SAFETY: `Component::deserialize` trusts its input to be an artifact this engine
        // produced. These exact in-memory bytes (not a re-read of the file) carry a MAC under
        // the node's cache key, which only this node (holding its storage key) could have
        // written when it compiled and serialized them (S8); wasmtime still checks its own
        // version and config header before using them.
        unsafe { Component::deserialize(engine, &bytes) }.ok()
    }

    /// Writes the artifact, then its sidecar, each through a temp file and a rename, so a
    /// reader never sees a half-written file pass the check.
    fn store(&self, artifact: &Path, sidecar: &Path, bytes: &[u8]) -> EyreResult<()> {
        write_atomic(artifact, bytes)?;
        write_atomic(
            sidecar,
            hex::encode(self.mac(artifact, bytes).as_ref()).as_bytes(),
        )
    }
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
