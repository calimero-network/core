//! The pipeline step cache: a successful step run's outputs and result, kept by the key of what
//! it computed, so an equal run later reuses them instead of running.
//!
//! The leader makes the key when it builds a run; apply never recomputes it. Every entry holds
//! its outputs (one reference each) for its life, and entries leave oldest first, by count and by
//! the timestamps of the entries applied, never a node's clock.

use calimero_log::{Bytes, StateTx};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::runs::RunInputRef;
use super::{get_json, put_json};
use crate::app::{rows, AppMachine};

/// Entries per app; past it the oldest go.
pub const MAX_CACHE_ENTRIES: usize = if cfg!(test) { 6 } else { 4096 };
/// How long an entry is kept after it was written or last used.
pub const CACHE_TTL_MS: u64 = 7 * 24 * 60 * 60 * 1000;
/// Entries one applied entry drops for age; the rest wait for the next ones.
pub const CACHE_EXPIRY_STEP: usize = if cfg!(test) { 2 } else { 256 };
/// Cache hits one step decision carries.
pub const MAX_CACHED_PER_DECISION: usize = 1024;

/// What a cached run made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheEntry {
    pub outputs: Vec<RunInputRef>,
    pub result: Option<Bytes>,
    /// When the entry was written or last used, by the entry that did it.
    pub ts_ms: u64,
    pub source_run: String,
}

/// A run of a step decision answered from the cache instead of started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedRun {
    pub index: u32,
    pub key: String,
}

/// The key of a step run: what it computes, with its input files named by content.
/// `input_hashes` are the plaintext SHA-256s of the inputs in order.
/// Caps are not part of it: a deterministic module gives the same output under any caps that let
/// it finish, so raising a step's limits on a retry must not miss the items that already succeeded.
#[must_use]
pub fn cache_key(bundle: &str, method: &str, args: &[u8], input_hashes: &[String]) -> String {
    let doc = json!({
        "bundle": bundle,
        "method": method,
        "args": hex::encode(calimero_identity::sha256(args)),
        "inputs": input_hashes,
    });
    let bytes = serde_json_canonicalizer::to_vec(&doc).unwrap_or_default();
    hex::encode(calimero_identity::sha256(&bytes))
}

impl AppMachine {
    /// The cache entry under `key`, as the group holds it.
    #[must_use]
    pub fn cache_entry(&self, key: &str) -> Option<CacheEntry> {
        let raw = self
            .state
            .kv
            .group()?
            .get_hidden(rows::CCACHE, key.as_bytes())?;
        serde_json::from_slice(&raw).ok()
    }

    /// Whether `key` has an entry whose outputs are all still held files of the app.
    #[must_use]
    pub fn cache_hit(&self, key: &str) -> bool {
        self.cache_entry(key)
            .is_some_and(|e| self.file_refs(e.outputs.iter().map(|o| &o.id)).is_some())
    }

    /// The plaintext SHA-256 (hex) of the sealed file `id`, if the app holds it.
    #[must_use]
    pub fn file_sha256(&self, id: &str) -> Option<String> {
        self.file_refs([&id.to_owned()])
            .and_then(|f| f.into_iter().next())
            .map(|f| f.sha256)
    }

    /// Makes `key` the newest entry, stamped `ts_ms`.
    pub(super) fn touch_cache(
        &mut self,
        tx: &mut StateTx<'_>,
        key: &str,
        mut entry: CacheEntry,
        ts_ms: u64,
    ) {
        entry.ts_ms = entry.ts_ms.max(ts_ms);
        put_json(tx, rows::CCACHE, key, &entry);
        let order = &mut self.state.component.head.step_cache;
        order.retain(|k| k != key);
        order.push_back(key.to_owned());
    }

    /// A done step run's outputs and result under its key, held until the entry goes. A key that
    /// has an entry already only has it refreshed; a result too large to keep is not cached.
    pub(super) fn cache_put(
        &mut self,
        tx: &mut StateTx<'_>,
        key: &str,
        entry: CacheEntry,
        ts_ms: u64,
    ) {
        if let Some(old) = get_json::<CacheEntry>(tx, rows::CCACHE, key) {
            self.touch_cache(tx, key, old, ts_ms);
            return;
        }
        self.pin_files(tx, entry.outputs.iter().map(|o| &o.id), 1, ts_ms);
        self.touch_cache(tx, key, entry, ts_ms);
        while self.state.component.head.step_cache.len() > MAX_CACHE_ENTRIES {
            let Some(oldest) = self.state.component.head.step_cache.front().cloned() else {
                break;
            };
            self.cache_drop(tx, &oldest, ts_ms);
        }
    }

    /// Removes the entry under `key`, releasing its holds.
    fn cache_drop(&mut self, tx: &mut StateTx<'_>, key: &str, ts_ms: u64) {
        self.state.component.head.step_cache.retain(|k| k != key);
        if let Some(entry) = get_json::<CacheEntry>(tx, rows::CCACHE, key) {
            self.pin_files(tx, entry.outputs.iter().map(|o| &o.id), -1, ts_ms);
            tx.delete_hidden(rows::CCACHE, key.as_bytes());
        }
    }

    /// Drops the entries older than [`CACHE_TTL_MS`] at `now_ms`, oldest first.
    pub(super) fn expire_step_cache(&mut self, tx: &mut StateTx<'_>, now_ms: u64) {
        for _ in 0..CACHE_EXPIRY_STEP {
            let Some(oldest) = self.state.component.head.step_cache.front().cloned() else {
                return;
            };
            let due = get_json::<CacheEntry>(tx, rows::CCACHE, &oldest)
                .is_none_or(|e| e.ts_ms.saturating_add(CACHE_TTL_MS) <= now_ms);
            if !due {
                return;
            }
            self.cache_drop(tx, &oldest, now_ms);
        }
    }
}
