//! The on-disk cache of compiled components (spec section 5 step 1, S8).
//!
//! An artifact is named `<component sha256>-<wasmtime tag>-<config hash>.cwasm`, beside a
//! `.cwasm.mac` sidecar: HMAC-SHA256 under the caller's cache key over the artifact's file
//! name and bytes, written at compile time (S8). The key comes from the node's
//! storage key, so a party that can write the directory but does not hold the key can
//! neither forge an artifact nor move a valid one under another component's name. A load
//! reads the artifact into memory, verifies the MAC and only then deserializes those same
//! bytes; any mismatch or error recompiles and rewrites both.
//!
//! A new wasmtime or engine config names every artifact anew, and a component no app runs
//! any more keeps its artifact, so the directory is swept ([`Cache::sweep`]): at engine start
//! the artifacts of another wasmtime or config go, and the node's periodic sweep also
//! removes those of components it no longer runs. A file modified within
//! [`CACHE_SWEEP_GRACE`] is never deleted, so a process still writing or about to load one
//! keeps it; deleting a file another process has open costs it at most a recompile, as a
//! load reads the whole artifact before checking it.
//!
//! Ids in these comments cite `docs/specs/app-runtime-v0.14.md` (S security rules in section
//! 9): S8 (compiled artifacts are checked before they load).

use std::fs;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::SystemTime;

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

/// A file in the cache directory modified more recently than this is never swept: another
/// process sharing the directory may still be writing it, or a component just installed may
/// not yet be named by any app.
pub const CACHE_SWEEP_GRACE: Duration = Duration::from_secs(60 * 60);

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// What one sweep of the cache directory deleted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheSweep {
    /// Files deleted: artifacts, their `.mac` files and leftover temp files.
    pub files: usize,
    /// Their bytes.
    pub bytes: u64,
}

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

    /// Deletes, once past [`CACHE_SWEEP_GRACE`], the artifacts (and `.mac` files) of another
    /// wasmtime or engine config, those of components `live` refuses (by SHA-256, lowercase
    /// hex), and temp files a crashed write left. Names this cache never writes are left.
    pub(crate) fn sweep(&self, live: &dyn Fn(&str) -> bool) -> CacheSweep {
        let mut swept = CacheSweep::default();
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return swept;
        };
        let now = SystemTime::now();
        for entry in entries.flatten() {
            let name = entry.file_name();
            if !name.to_str().is_some_and(|n| self.doomed(n, live)) {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let age = meta
                .modified()
                .ok()
                .and_then(|t| now.duration_since(t).ok());
            if !meta.is_file() || age.is_none_or(|a| a < CACHE_SWEEP_GRACE) {
                continue;
            }
            // Another process may have deleted it first: only what this one deleted counts.
            if fs::remove_file(entry.path()).is_ok() {
                swept.files += 1;
                swept.bytes += meta.len();
            }
        }
        swept
    }

    /// Whether the sweep deletes the file `name`, age aside; never a name this cache does
    /// not write.
    fn doomed(&self, name: &str, live: &dyn Fn(&str) -> bool) -> bool {
        let Some((stem, kind)) = name.split_once('.') else {
            return false;
        };
        let mut parts = stem.splitn(3, '-');
        let (Some(component), Some(tag), Some(config)) = (parts.next(), parts.next(), parts.next())
        else {
            return false;
        };
        let ours = component.len() == 64
            && component
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            && tag.starts_with("wasmtime")
            && !config.is_empty();
        if !ours {
            return false;
        }
        match kind {
            "cwasm" | "cwasm.mac" => {
                tag != WASMTIME_TAG || config != self.config_hash || !live(component)
            }
            // `write_atomic`'s temp names: `<stem>.tmp-..` and `<stem>.cwasm.tmp-..`.
            k => k.starts_with("tmp-") || k.starts_with("cwasm.tmp-"),
        }
    }

    /// The compiled component, from the cache when its artifact is intact.
    pub(crate) fn load_or_compile(
        &self,
        engine: &Engine,
        component: &[u8],
        compile: impl FnOnce() -> Result<Component, PrepareError>,
    ) -> Result<Component, PrepareError> {
        let key = hex::encode(Sha256::digest(component));
        let artifact = self
            .dir
            .join(format!("{key}-{WASMTIME_TAG}-{}.cwasm", self.config_hash));
        let sidecar = artifact.with_extension("cwasm.mac");
        if let Some(compiled) = self.load(engine, &artifact, &sidecar) {
            return Ok(compiled);
        }
        let compiled = compile()?;
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
