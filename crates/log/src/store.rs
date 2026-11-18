//! Durable storage for every group's log, metadata and state.
//!
//! One RocksDB database per node holds all groups, opened with the column and
//! table options `calimero-store` uses (compression, bloom filters,
//! one bounded block cache), in four column families:
//!
//! | Column | Key | Holds |
//! | --- | --- | --- |
//! | `log` | `group ‖ index` | Raft entries |
//! | `meta` | `group ‖ name` | vote, committed, purged, applied, generation, snapshot |
//! | `state` | `group ‖ generation ‖ machine key` | the state machines |
//! | `blobs` | `name` | content kept outside every log |
//!
//! Values are stored as they are (store format 2). The data directory is on a
//! disk the TD encrypts itself: mero-tee's LUKS2 volume with `hmac-sha256`
//! integrity, keyed by mero-kms under the signed release policy and opened
//! inside the TD, which merod requires in production (`--kms-disk-evidence`).
//! Sealing every value under the storage key as well (format 1 and before)
//! encrypted the same bytes twice, and is no longer read ([`crate::format`]).
//! What leaves this disk is still sealed here: a snapshot file's frames, and the
//! rows in them, under the storage key ([`Db::seal_state`]), so any node of the
//! cluster opens it and a backup copied elsewhere stays sealed. The format row
//! stays sealed too, so a store opened under another key is refused at once.
//! A machine's private keys can be hidden ([`hide`]): the row is keyed by an
//! HMAC of the key, and the key itself travels inside the value.
//!
//! RocksDB takes concurrent writers (one write batch per apply, per group), so
//! groups never queue behind one another; synced writes share one WAL sync.

#![allow(
    clippy::result_large_err,
    reason = "openraft's trait signatures fix these error types"
)]

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::ops::{Bound, RangeBounds};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use openraft::storage::{LogFlushed, RaftLogStorage};
use openraft::{AnyError, LogState, OptionalSend, RaftLogReader, StorageIOError};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};
use rocksdb::{
    BoundColumnFamily, Cache, ColumnFamilyDescriptor, DBWithThreadMode, MultiThreaded, ReadOptions,
    WriteBatch, WriteOptions,
};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::types::{Entry, GroupId, LogId, StorageError, TypeConfig, Vote};

/// Seals and opens values with one key.
pub struct Sealer {
    key: LessSafeKey,
    rng: SystemRandom,
}

impl Sealer {
    #[must_use]
    pub fn new(key: &[u8; 32]) -> Self {
        let unbound = UnboundKey::new(&AES_256_GCM, key).expect("32-byte key is valid for AES-256");
        Self {
            key: LessSafeKey::new(unbound),
            rng: SystemRandom::new(),
        }
    }

    pub fn seal(&self, aad: &[u8], plain: &[u8]) -> eyre::Result<Vec<u8>> {
        let mut nonce = [0_u8; NONCE_LEN];
        self.rng
            .fill(&mut nonce)
            .map_err(|_| eyre::eyre!("no randomness"))?;
        let mut buf = plain.to_vec();
        self.key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad),
                &mut buf,
            )
            .map_err(|_| eyre::eyre!("seal failed"))?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&buf);
        Ok(out)
    }

    pub fn open(&self, aad: &[u8], sealed: &[u8]) -> eyre::Result<Vec<u8>> {
        if sealed.len() < NONCE_LEN {
            eyre::bail!("sealed value too short");
        }
        let (nonce, body) = sealed.split_at(NONCE_LEN);
        let nonce =
            Nonce::try_assume_unique_for_key(nonce).map_err(|_| eyre::eyre!("bad nonce"))?;
        let mut buf = body.to_vec();
        let plain = self
            .key
            .open_in_place(nonce, Aad::from(aad), &mut buf)
            .map_err(|_| eyre::eyre!("value failed authentication"))?;
        Ok(plain.to_vec())
    }
}

/// A key derived from the storage key for one purpose.
pub(crate) fn derive(storage_key: &[u8; 32], label: &str) -> [u8; 32] {
    let tag = hmac::sign(
        &hmac::Key::new(hmac::HMAC_SHA256, storage_key),
        label.as_bytes(),
    );
    let mut out = [0; 32];
    out.copy_from_slice(&tag.as_ref()[..32]);
    out
}

/// The tag of a hidden key's row: `tag ‖ HMAC(group ‖ tag ‖ key)`.
pub const HIDDEN_LEN: usize = 33;

/// Where in the state a hidden key's row lives (see the module docs).
#[must_use]
pub fn hide(hider: &hmac::Key, group: GroupId, tag: u8, key: &[u8]) -> Vec<u8> {
    let mut msg = group.to_be_bytes().to_vec();
    msg.push(tag);
    msg.extend_from_slice(key);
    let mac = hmac::sign(hider, &msg);
    let mut out = Vec::with_capacity(HIDDEN_LEN);
    out.push(tag);
    out.extend_from_slice(&mac.as_ref()[..32]);
    out
}

/// A hidden row's value: the key it stands for, then the value.
pub(crate) fn wrap_hidden(key: &[u8], value: &[u8]) -> Vec<u8> {
    let len = u32::try_from(key.len()).unwrap_or(u32::MAX);
    let mut out = len.to_be_bytes().to_vec();
    out.extend_from_slice(key);
    out.extend_from_slice(value);
    out
}

/// The `(key, value)` a hidden row's value holds.
#[must_use]
pub fn unwrap_hidden(raw: &[u8]) -> Option<(&[u8], &[u8])> {
    let len = usize::try_from(u32::from_be_bytes(raw.get(..4)?.try_into().ok()?)).ok()?;
    let key = raw.get(4..4 + len)?;
    Some((key, &raw[4 + len..]))
}

/// The node's column families.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Column {
    Log,
    Meta,
    State,
    Blobs,
}

impl Column {
    const ALL: [Self; 4] = [Self::Log, Self::Meta, Self::State, Self::Blobs];

    const fn name(self) -> &'static str {
        match self {
            Self::Log => "log",
            Self::Meta => "meta",
            Self::State => "state",
            Self::Blobs => "blobs",
        }
    }
}

/// The block cache every column family shares: what state reads are served
/// from before disk, bounded whatever the state's size.
pub const BLOCK_CACHE_BYTES: usize = 128 * 1024 * 1024;

type Rocks = DBWithThreadMode<MultiThreaded>;

/// Writes staged for one atomic batch.
#[derive(Default)]
pub(crate) struct Transaction {
    ops: Vec<(Column, Vec<u8>, Option<Vec<u8>>)>,
}

impl Transaction {
    fn raw_put(&mut self, col: Column, key: Vec<u8>, value: Vec<u8>) {
        self.ops.push((col, key, Some(value)));
    }

    fn raw_delete(&mut self, col: Column, key: Vec<u8>) {
        self.ops.push((col, key, None));
    }

    fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

/// The group id of the node's own `meta` rows (`Engine::put_node_meta`). The
/// node numbers its groups from 1 upwards, so no group ever takes it.
pub(crate) const NODE_ROWS: GroupId = GroupId::MAX;

/// Key prefix of a segment's row in the `blobs` column.
const SEGMENT_BLOB: &str = "seg.";
/// Key prefix of a segment's empty index row in the `meta` column; no group id starts with it.
const SEGMENT_INDEX: &[u8] = b"seg/";

/// The tags in a sealed row's AAD: a node row's (`meta`), a snapshot row's (`state`).
const TAG_META: u8 = 2;
const TAG_STATE: u8 = 3;

fn group_prefix(group: GroupId) -> [u8; 8] {
    group.to_be_bytes()
}

/// The first key past every key of `group` (exclusive upper bound).
fn group_end(group: GroupId) -> Vec<u8> {
    match group.checked_add(1) {
        Some(next) => next.to_be_bytes().to_vec(),
        None => vec![0xff; 9],
    }
}

fn log_key(group: GroupId, index: u64) -> Vec<u8> {
    let mut k = group_prefix(group).to_vec();
    k.extend_from_slice(&index.to_be_bytes());
    k
}

fn segment_index_key(hash: &str) -> Vec<u8> {
    [SEGMENT_INDEX, hash.as_bytes()].concat()
}

pub(crate) fn meta_key(group: GroupId, name: &str) -> Vec<u8> {
    let mut k = group_prefix(group).to_vec();
    k.extend_from_slice(name.as_bytes());
    k
}

fn state_prefix(group: GroupId, generation: u32) -> Vec<u8> {
    let mut k = group_prefix(group).to_vec();
    k.extend_from_slice(&generation.to_be_bytes());
    k
}

pub(crate) fn state_key(group: GroupId, generation: u32, key: &[u8]) -> Vec<u8> {
    let mut k = state_prefix(group, generation);
    k.extend_from_slice(key);
    k
}

/// The AAD of a row. A state row is bound to its group and machine key but not
/// its generation, so a snapshot moves sealed rows to the next one unopened.
fn aad(tag: u8, group: Option<GroupId>, key: &[u8]) -> Vec<u8> {
    let mut a = vec![b'c', b'l', tag];
    if let Some(g) = group {
        a.extend_from_slice(&g.to_be_bytes());
    }
    a.extend_from_slice(key);
    a
}

/// Opens snapshot files and the state rows in them, with the storage key alone.
pub(crate) struct SnapshotKeys {
    pub(crate) frames: Sealer,
    rows: Sealer,
}

impl SnapshotKeys {
    pub(crate) fn new(storage_key: &[u8; 32]) -> Self {
        Self {
            frames: Sealer::new(&derive(storage_key, "calimero.log.snapshot.v1")),
            rows: Sealer::new(storage_key),
        }
    }

    pub(crate) fn open_row(
        &self,
        group: GroupId,
        key: &[u8],
        sealed: &[u8],
    ) -> eyre::Result<Vec<u8>> {
        self.rows.open(&aad(TAG_STATE, Some(group), key), sealed)
    }
}

/// The node's database, shared by all groups.
/// [`Db::summary`]: sizes are as stored (sealed).
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreSummary {
    pub groups: std::collections::BTreeMap<GroupId, GroupSummary>,
    pub blobs: u64,
    pub blob_bytes: u64,
    pub snapshots: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupSummary {
    pub log_entries: u64,
    pub log_bytes: u64,
    pub state_rows: u64,
    pub state_bytes: u64,
    /// The metadata rows' names (vote, applied index, membership, ...).
    pub meta: Vec<String>,
}

pub struct Db {
    db: Rocks,
    /// Seals the rows of a snapshot file, and the format row; stored rows are not.
    sealer: Sealer,
    /// Seals snapshot files (see [`crate::machine`]): every node of the
    /// cluster shares the storage key, so a follower opens what a leader built.
    pub(crate) snapshot_sealer: Sealer,
    pub(crate) hider: hmac::Key,
    pub(crate) snapshot_dir: PathBuf,
    /// Groups whose unsynced writes (the commit index, applied batches) are
    /// dropped, as a crash before they reached the disk would (feature `testing`).
    #[cfg(feature = "testing")]
    pub(crate) losing: std::sync::RwLock<std::collections::HashSet<GroupId>>,
    /// The directory it was opened in, which names it to [`crate::testing`].
    #[cfg(feature = "testing")]
    pub(crate) dir: PathBuf,
    /// Last field, so it is set after RocksDB has closed and released its lock.
    pub(crate) closed: Closed,
}

/// Set when dropped; see [`Db::closed`].
pub(crate) struct Closed(pub(crate) Arc<AtomicBool>);

impl Drop for Closed {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Groups whose applies are held back, by the directory their node opened
/// (feature `testing`; see [`crate::testing::hold_apply`]).
#[cfg(feature = "testing")]
pub(crate) static HELD: std::sync::Mutex<Vec<(PathBuf, GroupId)>> =
    std::sync::Mutex::new(Vec::new());

/// Applied batches that fail to commit, as a disk error would: `(dir, group,
/// how many more)` (feature `testing`; see [`crate::testing::fail_applies`]).
#[cfg(feature = "testing")]
pub(crate) static FAILING: std::sync::Mutex<Vec<(PathBuf, GroupId, u32)>> =
    std::sync::Mutex::new(Vec::new());

/// Snapshot builds held at a point, by the directory their node opened
/// (feature `testing`; see [`crate::testing::hold_build`]).
#[cfg(feature = "testing")]
pub(crate) static HELD_BUILDS: std::sync::Mutex<Vec<crate::testing::HeldBuild>> =
    std::sync::Mutex::new(Vec::new());

/// Groups whose state reads fail, by the directory their node opened
/// (feature `testing`; see [`crate::testing::fail_state_reads`]).
#[cfg(feature = "testing")]
pub(crate) static FAILING_READS: std::sync::Mutex<Vec<(PathBuf, GroupId)>> =
    std::sync::Mutex::new(Vec::new());

/// Groups whose synced writes take longer, by the directory their node opened
/// (feature `testing`; see [`crate::testing::slow_synced_writes`]).
#[cfg(feature = "testing")]
pub(crate) static SLOW_SYNCS: std::sync::Mutex<Vec<(PathBuf, GroupId, std::time::Duration)>> =
    std::sync::Mutex::new(Vec::new());

/// How long closing a store waits first, by the directory it was opened in
/// (feature `testing`; see [`crate::testing::slow_close`]).
#[cfg(feature = "testing")]
pub(crate) static CLOSE_DELAYS: std::sync::Mutex<Vec<(PathBuf, std::time::Duration)>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(feature = "testing")]
impl Drop for Db {
    fn drop(&mut self) {
        let delay = CLOSE_DELAYS
            .lock()
            .ok()
            .and_then(|d| d.iter().find(|(dir, _)| *dir == self.dir).map(|(_, t)| *t));
        if let Some(delay) = delay {
            std::thread::sleep(delay);
        }
    }
}

impl Db {
    /// Open (or create) the database under `dir`: the store in `dir/store`,
    /// snapshot files in `dir/snapshots`.
    pub fn open(dir: &Path, key: &[u8; 32]) -> eyre::Result<Arc<Self>> {
        std::fs::create_dir_all(dir)?;
        let snapshot_dir = dir.join("snapshots");
        std::fs::create_dir_all(&snapshot_dir)?;
        let cache = Cache::new_lru_cache(BLOCK_CACHE_BYTES);
        let table = crate::tuning::table_options(&cache);
        let mut options = crate::tuning::column_options(&table)?;
        options.create_if_missing(true);
        options.create_missing_column_families(true);
        options.set_max_open_files(256);
        options.set_max_total_wal_size(256 * 1024 * 1024);
        options.set_db_write_buffer_size(256 * 1024 * 1024);
        let descriptors = Column::ALL
            .iter()
            .map(|c| {
                Ok(ColumnFamilyDescriptor::new(
                    c.name(),
                    crate::tuning::column_options(&table)?,
                ))
            })
            .collect::<eyre::Result<Vec<_>>>()?;
        let db = Rocks::open_cf_descriptors(&options, store_path(dir), descriptors)?;
        Ok(Arc::new(Self {
            db,
            sealer: Sealer::new(key),
            snapshot_sealer: Sealer::new(&derive(key, "calimero.log.snapshot.v1")),
            hider: hmac::Key::new(
                hmac::HMAC_SHA256,
                &derive(key, crate::machine::HIDDEN_KEYS_LABEL),
            ),
            snapshot_dir,
            #[cfg(feature = "testing")]
            losing: std::sync::RwLock::default(),
            #[cfg(feature = "testing")]
            dir: dir.to_owned(),
            closed: Closed(Arc::default()),
        }))
    }

    /// Whether `group`'s applies are held back on this node (feature `testing`).
    #[cfg(feature = "testing")]
    #[must_use]
    pub(crate) fn apply_held(&self, group: GroupId) -> bool {
        HELD.lock()
            .map(|h| h.iter().any(|(d, g)| *g == group && *d == self.dir))
            .unwrap_or(false)
    }

    /// Whether `group`'s next applied batch on this node fails to commit
    /// (feature `testing`): counts one failure down.
    #[cfg(feature = "testing")]
    pub(crate) fn apply_fails(&self, group: GroupId) -> bool {
        let Ok(mut f) = FAILING.lock() else {
            return false;
        };
        let Some(i) = f.iter().position(|(d, g, _)| *g == group && *d == self.dir) else {
            return false;
        };
        f[i].2 -= 1;
        if f[i].2 == 0 {
            let _done = f.remove(i);
        }
        true
    }

    /// Wait here while a build of `group` is held at `at` on this node
    /// (feature `testing`; see [`crate::testing::hold_build`]). Blocks the
    /// calling thread, as the build itself does.
    #[cfg(feature = "testing")]
    pub(crate) fn pause_build(&self, group: GroupId, at: crate::testing::BuildPoint) {
        loop {
            let held = HELD_BUILDS
                .lock()
                .map(|mut h| {
                    h.iter_mut()
                        .find(|b| b.group == group && b.at == at && b.dir == self.dir)
                        .map(|b| b.waiting = true)
                        .is_some()
                })
                .unwrap_or(false);
            if !held {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// Wait as long as [`crate::testing::slow_synced_writes`] slows `group`'s
    /// synced writes on this node (feature `testing`). Blocks the calling
    /// thread, as a slow fsync does.
    fn slow_sync(&self, group: GroupId) {
        #[cfg(feature = "testing")]
        {
            let delay = SLOW_SYNCS.lock().ok().and_then(|s| {
                s.iter()
                    .find(|(d, g, _)| *g == group && *d == self.dir)
                    .map(|(_, _, delay)| *delay)
            });
            if let Some(delay) = delay {
                std::thread::sleep(delay);
            }
        }
        #[cfg(not(feature = "testing"))]
        let _ = group;
    }

    /// Whether `group`'s unsynced writes are being dropped (feature `testing`).
    #[must_use]
    pub(crate) fn losing_unsynced(&self, group: GroupId) -> bool {
        #[cfg(feature = "testing")]
        {
            self.losing
                .read()
                .map(|l| l.contains(&group))
                .unwrap_or(false)
        }
        #[cfg(not(feature = "testing"))]
        {
            let _ = group;
            false
        }
    }

    fn cf(&self, col: Column) -> eyre::Result<Arc<BoundColumnFamily<'_>>> {
        self.db
            .cf_handle(col.name())
            .ok_or_else(|| eyre::eyre!("missing column family {}", col.name()))
    }

    fn raw_get(&self, col: Column, key: &[u8]) -> eyre::Result<Option<Vec<u8>>> {
        Ok(self
            .db
            .get_pinned_cf(&self.cf(col)?, key)?
            .map(|v| v.to_vec()))
    }

    fn write(&self, tx: &Transaction, synced: bool) -> eyre::Result<()> {
        let mut batch = WriteBatch::default();
        for (col, key, value) in &tx.ops {
            let cf = self.cf(*col)?;
            match value {
                Some(v) => batch.put_cf(&cf, key, v),
                None => batch.delete_cf(&cf, key),
            }
        }
        let mut opts = WriteOptions::default();
        // RocksDB syncs concurrent writers' batches with one WAL sync.
        opts.set_sync(synced);
        self.db.write_opt(batch, &opts)?;
        Ok(())
    }

    fn delete_range(&self, col: Column, lo: &[u8], hi: &[u8]) -> eyre::Result<()> {
        let mut batch = WriteBatch::default();
        batch.delete_range_cf(&self.cf(col)?, lo, hi);
        self.db.write(batch)?;
        Ok(())
    }

    /// A node row ([`NODE_ROWS`]): sealed, as every one is (see [`Db::stage_meta`]).
    pub(crate) fn put_sealed_meta<T: Serialize>(&self, name: &str, value: &T) -> eyre::Result<()> {
        self.put_meta(NODE_ROWS, name, value)
    }

    /// A node row written by [`Db::put_sealed_meta`]; an error when it does not open
    /// (the store was sealed under another key, or is damaged).
    pub(crate) fn get_sealed_meta<T: DeserializeOwned>(
        &self,
        name: &str,
    ) -> eyre::Result<Option<T>> {
        self.get_meta(NODE_ROWS, name)
    }

    /// Stage `value` as group metadata `name`.
    pub(crate) fn stage_meta<T: Serialize>(
        &self,
        tx: &mut Transaction,
        group: GroupId,
        name: &str,
        value: &T,
    ) -> eyre::Result<()> {
        let key = meta_key(group, name);
        let plain = serde_json::to_vec(value)?;
        // The node's own rows (its format, its rollback counter) stay sealed: they open
        // only under its key, which is how a store opened under another key is refused.
        let value = if group == NODE_ROWS {
            self.sealer.seal(&aad(TAG_META, None, &key), &plain)?
        } else {
            plain
        };
        tx.raw_put(Column::Meta, key, value);
        Ok(())
    }

    /// Stage the removal of group metadata `name`.
    pub(crate) fn stage_drop_meta(&self, tx: &mut Transaction, group: GroupId, name: &str) {
        tx.raw_delete(Column::Meta, meta_key(group, name));
    }

    /// Write group metadata `name`, synced (votes must survive power loss).
    pub fn put_meta<T: Serialize>(
        &self,
        group: GroupId,
        name: &str,
        value: &T,
    ) -> eyre::Result<()> {
        let mut tx = Transaction::default();
        self.stage_meta(&mut tx, group, name, value)?;
        self.slow_sync(group);
        self.write(&tx, true)
    }

    pub fn get_meta<T: DeserializeOwned>(
        &self,
        group: GroupId,
        name: &str,
    ) -> eyre::Result<Option<T>> {
        let key = meta_key(group, name);
        let Some(stored) = self.raw_get(Column::Meta, &key)? else {
            return Ok(None);
        };
        let plain = if group == NODE_ROWS {
            self.sealer.open(&aad(TAG_META, None, &key), &stored)?
        } else {
            stored
        };
        Ok(Some(serde_json::from_slice(&plain)?))
    }

    /// Store a blob (content kept outside every log, e.g. a WASM bundle).
    pub fn put_blob(&self, name: &str, bytes: &[u8]) -> eyre::Result<()> {
        let mut tx = Transaction::default();
        tx.raw_put(Column::Blobs, name.as_bytes().to_vec(), bytes.to_vec());
        self.write(&tx, true)
    }

    /// Store a file segment under `seg.<hash>` and, in the same batch, its empty index row
    /// `seg/<hash>` in `meta`, so segments can be listed without reading a byte of them.
    pub fn put_segment(&self, hash: &str, bytes: &[u8]) -> eyre::Result<()> {
        let mut tx = Transaction::default();
        tx.raw_put(
            Column::Blobs,
            format!("{SEGMENT_BLOB}{hash}").into_bytes(),
            bytes.to_vec(),
        );
        tx.raw_put(Column::Meta, segment_index_key(hash), Vec::new());
        self.write(&tx, true)
    }

    /// Delete a segment and its index row; nothing if there is none.
    pub fn delete_segment(&self, hash: &str) -> eyre::Result<()> {
        let mut tx = Transaction::default();
        tx.raw_delete(Column::Blobs, format!("{SEGMENT_BLOB}{hash}").into_bytes());
        tx.raw_delete(Column::Meta, segment_index_key(hash));
        self.write(&tx, true)
    }

    /// Read a segment stored with [`Db::put_segment`].
    pub fn get_segment(&self, hash: &str) -> eyre::Result<Option<Vec<u8>>> {
        self.get_blob(&format!("{SEGMENT_BLOB}{hash}"))
    }

    /// The hash of every segment stored here, from the index rows alone.
    pub fn list_segments(&self) -> eyre::Result<Vec<String>> {
        let mut out = Vec::new();
        self.scan(Column::Meta, SEGMENT_INDEX, |k, _| {
            let Some(hash) = k.strip_prefix(SEGMENT_INDEX) else {
                return Ok(false);
            };
            out.push(String::from_utf8_lossy(hash).into_owned());
            Ok(true)
        })?;
        Ok(out)
    }

    /// Delete a blob stored with [`Db::put_blob`]; nothing if there is none.
    pub fn delete_blob(&self, name: &str) -> eyre::Result<()> {
        let mut tx = Transaction::default();
        tx.raw_delete(Column::Blobs, name.as_bytes().to_vec());
        self.write(&tx, true)
    }

    /// Read a blob stored with [`Db::put_blob`].
    pub fn get_blob(&self, name: &str) -> eyre::Result<Option<Vec<u8>>> {
        self.raw_get(Column::Blobs, name.as_bytes())
    }

    pub(crate) fn put_entries(&self, group: GroupId, entries: &[Entry]) -> eyre::Result<()> {
        let mut tx = Transaction::default();
        for e in entries {
            tx.raw_put(
                Column::Log,
                log_key(group, e.log_id.index),
                serde_json::to_vec(e)?,
            );
        }
        self.slow_sync(group);
        self.write(&tx, true)
    }

    fn range(&self, group: GroupId, lo: u64, hi_incl: u64) -> eyre::Result<Vec<Entry>> {
        let mut out = Vec::new();
        let end = log_key(group, hi_incl);
        self.scan(Column::Log, &log_key(group, lo), |k, v| {
            if k > end.as_slice() {
                return Ok(false);
            }
            out.push(serde_json::from_slice(v)?);
            Ok(true)
        })?;
        Ok(out)
    }

    fn last(&self, group: GroupId) -> eyre::Result<Option<Entry>> {
        let mut it = self.db.raw_iterator_cf(&self.cf(Column::Log)?);
        // The largest key below the next group's first: this group's last entry.
        it.seek_for_prev(group_end(group));
        while it.valid() {
            let Some(k) = it.key() else { break };
            if !k.starts_with(&group_prefix(group)) {
                if k >= group_end(group).as_slice() {
                    it.prev();
                    continue;
                }
                return Ok(None);
            }
            let v = it.value().unwrap_or_default();
            return Ok(Some(serde_json::from_slice(v)?));
        }
        it.status()?;
        Ok(None)
    }

    fn delete_log(&self, group: GroupId, lo: u64, hi_incl: u64) -> eyre::Result<()> {
        let hi = match hi_incl.checked_add(1) {
            Some(h) => log_key(group, h),
            None => group_end(group),
        };
        self.delete_range(Column::Log, &log_key(group, lo), &hi)
    }

    /// Visit the rows of `col` from `from` on, in key order, until `f` answers
    /// `false` or the column ends.
    pub(crate) fn scan(
        &self,
        col: Column,
        from: &[u8],
        mut f: impl FnMut(&[u8], &[u8]) -> eyre::Result<bool>,
    ) -> eyre::Result<()> {
        let mut it = self.db.raw_iterator_cf(&self.cf(col)?);
        it.seek(from);
        while it.valid() {
            let (Some(k), Some(v)) = (it.key(), it.value()) else {
                break;
            };
            if !f(k, v)? {
                return Ok(());
            }
            it.next();
        }
        it.status()?;
        Ok(())
    }

    /// What the store holds, from the raw keys alone (no row is opened):
    /// per group, its log entries, state rows and metadata names, and the
    /// blobs.
    pub fn summary(&self) -> eyre::Result<StoreSummary> {
        let mut out = StoreSummary::default();
        let group_of = |k: &[u8]| -> Option<GroupId> {
            Some(GroupId::from_be_bytes(k.get(..8)?.try_into().ok()?))
        };
        self.scan(Column::Log, &[], |k, v| {
            if let Some(g) = group_of(k) {
                let e = out.groups.entry(g).or_default();
                e.log_entries += 1;
                e.log_bytes += v.len() as u64;
            }
            Ok(true)
        })?;
        self.scan(Column::State, &[], |k, v| {
            if let Some(g) = group_of(k) {
                let e = out.groups.entry(g).or_default();
                e.state_rows += 1;
                e.state_bytes += (k.len() + v.len()) as u64;
            }
            Ok(true)
        })?;
        self.scan(Column::Meta, &[], |k, _| {
            if k.starts_with(SEGMENT_INDEX) {
                return Ok(true);
            }
            if let Some(g) = group_of(k) {
                let name = String::from_utf8_lossy(&k[8..]).into_owned();
                out.groups.entry(g).or_default().meta.push(name);
            }
            Ok(true)
        })?;
        self.scan(Column::Blobs, &[], |_, v| {
            out.blobs += 1;
            out.blob_bytes += v.len() as u64;
            Ok(true)
        })?;
        for entry in std::fs::read_dir(&self.snapshot_dir)?.flatten() {
            out.snapshots
                .push(entry.file_name().to_string_lossy().into_owned());
        }
        out.snapshots.sort();
        Ok(out)
    }

    /// Whether the store holds no row in any column (new, or created and never written).
    pub(crate) fn is_empty(&self) -> eyre::Result<bool> {
        for col in Column::ALL {
            let mut it = self.db.raw_iterator_cf(&self.cf(col)?);
            it.seek_to_first();
            if it.valid() {
                return Ok(false);
            }
            it.status()?;
        }
        Ok(true)
    }

    /// [`Db::scan`] over a consistent point-in-time view of the state column.
    pub(crate) fn scan_state_snapshot(
        &self,
        from: &[u8],
        mut f: impl FnMut(&[u8], &[u8]) -> eyre::Result<bool>,
    ) -> eyre::Result<()> {
        let snapshot = self.db.snapshot();
        let mut opts = ReadOptions::default();
        opts.set_snapshot(&snapshot);
        let mut it = self.db.raw_iterator_cf_opt(&self.cf(Column::State)?, opts);
        it.seek(from);
        while it.valid() {
            let (Some(k), Some(v)) = (it.key(), it.value()) else {
                break;
            };
            if !f(k, v)? {
                return Ok(());
            }
            it.next();
        }
        it.status()?;
        Ok(())
    }

    /// One row of a group's state, opened.
    pub(crate) fn get_state(
        &self,
        group: GroupId,
        generation: u32,
        key: &[u8],
    ) -> eyre::Result<Option<Vec<u8>>> {
        #[cfg(feature = "testing")]
        if FAILING_READS
            .lock()
            .is_ok_and(|f| f.iter().any(|(d, g)| *g == group && *d == self.dir))
        {
            eyre::bail!("state read failed (injected by a test)");
        }
        self.raw_get(Column::State, &state_key(group, generation, key))
    }

    /// Stage a state row (`None` deletes it).
    pub(crate) fn stage_state(
        &self,
        tx: &mut Transaction,
        group: GroupId,
        generation: u32,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> eyre::Result<()> {
        let full = state_key(group, generation, key);
        match value {
            Some(v) => {
                tx.raw_put(Column::State, full, v.to_vec());
                Ok(())
            }
            None => {
                tx.raw_delete(Column::State, full);
                Ok(())
            }
        }
    }

    /// Seal a state row of `group` for a snapshot file, bound to its group and
    /// machine key (the shape every release's snapshot carries).
    pub(crate) fn seal_state(
        &self,
        group: GroupId,
        key: &[u8],
        plain: &[u8],
    ) -> eyre::Result<Vec<u8>> {
        self.sealer.seal(&aad(TAG_STATE, Some(group), key), plain)
    }

    /// Visit a group generation's state rows from `from` (a machine key) on:
    /// `(machine key, opened value)`.
    pub(crate) fn scan_state(
        &self,
        group: GroupId,
        generation: u32,
        from: &[u8],
        mut f: impl FnMut(&[u8], &[u8]) -> eyre::Result<bool>,
    ) -> eyre::Result<()> {
        let prefix = state_prefix(group, generation);
        self.scan(
            Column::State,
            &state_key(group, generation, from),
            |k, v| {
                let Some(key) = k.strip_prefix(prefix.as_slice()) else {
                    return Ok(false);
                };
                f(key, v)
            },
        )
    }

    /// Open a sealed state row of `group` (a snapshot's).
    pub(crate) fn open_state(
        &self,
        group: GroupId,
        key: &[u8],
        sealed: &[u8],
    ) -> eyre::Result<Vec<u8>> {
        self.sealer.open(&aad(TAG_STATE, Some(group), key), sealed)
    }

    /// Delete group metadata `name`.
    pub(crate) fn drop_meta(&self, group: GroupId, name: &str) -> eyre::Result<()> {
        let mut tx = Transaction::default();
        tx.raw_delete(Column::Meta, meta_key(group, name));
        self.write(&tx, true)
    }

    /// Delete every row of one generation of a group's state.
    pub(crate) fn drop_generation(&self, group: GroupId, generation: u32) -> eyre::Result<()> {
        let hi = match generation.checked_add(1) {
            Some(g) => state_prefix(group, g),
            None => group_end(group),
        };
        self.delete_range(Column::State, &state_prefix(group, generation), &hi)
    }

    /// Delete every generation of a group's state but `keep`.
    pub(crate) fn drop_other_generations(&self, group: GroupId, keep: u32) -> eyre::Result<()> {
        self.delete_range(
            Column::State,
            &group_prefix(group),
            &state_prefix(group, keep),
        )?;
        if let Some(next) = keep.checked_add(1) {
            self.delete_range(Column::State, &state_prefix(group, next), &group_end(group))?;
        }
        Ok(())
    }

    /// Write a staged batch atomically.
    pub(crate) fn commit(&self, tx: &Transaction, synced: bool) -> eyre::Result<()> {
        if tx.is_empty() {
            return Ok(());
        }
        self.write(tx, synced)
    }

    /// Delete everything of a group (used when a group is removed from this node).
    pub fn drop_group(&self, group: GroupId) -> eyre::Result<()> {
        let lo = group_prefix(group);
        let hi = group_end(group);
        for col in [Column::Log, Column::Meta, Column::State] {
            self.delete_range(col, &lo, &hi)?;
        }
        let prefix = format!("g{group}-");
        for entry in std::fs::read_dir(&self.snapshot_dir)?.filter_map(Result::ok) {
            if entry.file_name().to_string_lossy().starts_with(&prefix) {
                let _removed = std::fs::remove_file(entry.path());
            }
        }
        Ok(())
    }
}

/// A group's state, read through the database.
///
/// Cheap to clone; every clone reads the generation the group is on now, so a
/// snapshot install is seen by all at once.
#[derive(Clone)]
pub struct GroupState {
    pub(crate) db: Arc<Db>,
    pub(crate) group: GroupId,
    pub(crate) generation: Arc<AtomicU32>,
    /// Held while the snapshot a group serves is recorded, and while an
    /// install or a replace switches the generation and records (or drops)
    /// it: see [`GroupState::lock_snapshot_record`].
    pub(crate) snapshot_record: Arc<Mutex<()>>,
}

impl GroupState {
    pub(crate) fn new(db: Arc<Db>, group: GroupId, generation: u32) -> Self {
        Self {
            db,
            group,
            generation: Arc::new(AtomicU32::new(generation)),
            snapshot_record: Arc::default(),
        }
    }

    #[must_use]
    pub const fn group(&self) -> GroupId {
        self.group
    }

    /// Hold off every other change to the snapshot this group serves, and
    /// every generation switch (an install, a replace), while the guard
    /// lives. Taken before the group's applied-state lock, never while it is
    /// held, so the two cannot deadlock. It guards no data, so a holder that
    /// panicked leaves nothing torn: a poisoned lock is taken all the same.
    pub(crate) fn lock_snapshot_record(&self) -> MutexGuard<'_, ()> {
        self.snapshot_record
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn generation(&self) -> u32 {
        self.generation.load(Ordering::Acquire)
    }

    /// One row, or the storage fault that kept it from being read. Anything
    /// that decides state (a machine's `restore`) reads with this, never
    /// [`GroupState::get`].
    pub fn try_get(&self, key: &[u8]) -> eyre::Result<Option<Vec<u8>>> {
        self.db.get_state(self.group, self.generation(), key)
    }

    /// One row, for a reader that only answers (a view, a route). A storage
    /// fault reads as absent and is logged; nothing applied may read this way
    /// (an apply reads through its [`StateTx`], which fails the batch instead).
    #[must_use]
    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        match self.db.get_state(self.group, self.generation(), key) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(group = self.group, error = %e, "state read failed");
                None
            }
        }
    }

    /// The row a hidden key stands for (see [`hide`]).
    #[must_use]
    pub fn get_hidden(&self, tag: u8, key: &[u8]) -> Option<Vec<u8>> {
        let raw = self.get(&hide(&self.db.hider, self.group, tag, key))?;
        let (k, v) = unwrap_hidden(&raw)?;
        (k == key).then(|| v.to_vec())
    }

    /// Visit rows from `from` on, `(key, value)`, until `f` answers `false`.
    pub fn scan(
        &self,
        from: &[u8],
        f: impl FnMut(&[u8], &[u8]) -> eyre::Result<bool>,
    ) -> eyre::Result<()> {
        self.db.scan_state(self.group, self.generation(), from, f)
    }

    /// Visit every row hidden under `tag`: `(key, value)` as they were written.
    pub fn scan_hidden(
        &self,
        tag: u8,
        mut f: impl FnMut(&[u8], &[u8]) -> eyre::Result<bool>,
    ) -> eyre::Result<()> {
        self.scan(&[tag], |k, raw| {
            if k.first() != Some(&tag) {
                return Ok(false);
            }
            match unwrap_hidden(raw) {
                Some((key, value)) => f(key, value),
                None => Err(eyre::eyre!(
                    "group {}: a hidden row under tag {tag} is malformed",
                    self.group
                )),
            }
        })
    }
}

/// The writes one batch of applied entries makes to its group's state, read
/// back by later entries of the batch. Committed with the applied index, in
/// one write.
///
/// A read that fails (a storage fault, a hidden row that is malformed) reads
/// as absent *and fails the batch* ([`StateTx::fail`]): the group stops on that
/// entry rather than apply it differently from the other replicas.
pub struct StateTx<'a> {
    state: &'a GroupState,
    staged: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    /// The first fault recorded; set through `&self` too, by a read.
    error: Mutex<Option<String>>,
    /// How the batch's writes so far move the group's state digest
    /// ([`crate::digest`]): each row it writes less the row that write replaces.
    delta: crate::digest::StateDigest,
}

impl<'a> StateTx<'a> {
    #[must_use]
    pub fn new(state: &'a GroupState) -> Self {
        Self {
            state,
            staged: BTreeMap::new(),
            error: Mutex::new(None),
            delta: crate::digest::StateDigest::default(),
        }
    }

    /// How this batch's writes so far move the group's state digest.
    #[must_use]
    pub(crate) const fn digest_delta(&self) -> crate::digest::StateDigest {
        self.delta
    }

    /// Row `key` as this batch left it, for the digest of a write that replaces
    /// it. A row that cannot be read fails the batch: a digest taken over a row
    /// guessed absent would differ from the other replicas' for no fault of the
    /// state.
    fn replaced(&mut self, key: &[u8]) -> Option<Vec<u8>> {
        if let Some(v) = self.staged.get(key) {
            return v.clone();
        }
        match self
            .state
            .db
            .get_state(self.state.group, self.state.generation(), key)
        {
            Ok(v) => v,
            Err(e) => {
                self.fail(format_args!("state read failed: {e}"));
                None
            }
        }
    }

    /// The group's committed state, without this batch's writes.
    #[must_use]
    pub const fn committed(&self) -> &GroupState {
        self.state
    }

    /// One row, as this batch left it. A storage fault fails the batch (see
    /// [`StateTx`]) and reads as absent.
    #[must_use]
    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        match self.staged.get(key) {
            Some(v) => v.clone(),
            None => match self
                .state
                .db
                .get_state(self.state.group, self.state.generation(), key)
            {
                Ok(v) => v,
                Err(e) => {
                    self.fail(format_args!("state read failed: {e}"));
                    None
                }
            },
        }
    }

    pub fn put(&mut self, key: Vec<u8>, value: Vec<u8>) {
        let old = self.replaced(&key);
        self.delta = self.delta.replace(&key, old.as_deref(), Some(&value));
        let _prev = self.staged.insert(key, Some(value));
    }

    pub fn delete(&mut self, key: Vec<u8>) {
        let old = self.replaced(&key);
        self.delta = self.delta.replace(&key, old.as_deref(), None);
        let _prev = self.staged.insert(key, None);
    }

    fn hidden(&self, tag: u8, key: &[u8]) -> Vec<u8> {
        hide(&self.state.db.hider, self.state.group, tag, key)
    }

    /// The row a hidden key stands for (see [`hide`]). A row that is not a
    /// hidden row's shape fails the batch, as a storage fault does.
    #[must_use]
    pub fn get_hidden(&self, tag: u8, key: &[u8]) -> Option<Vec<u8>> {
        let raw = self.get(&self.hidden(tag, key))?;
        let Some((k, v)) = unwrap_hidden(&raw) else {
            self.fail(format_args!("a hidden row under tag {tag} is malformed"));
            return None;
        };
        (k == key).then(|| v.to_vec())
    }

    pub fn put_hidden(&mut self, tag: u8, key: &[u8], value: &[u8]) {
        let row = self.hidden(tag, key);
        self.put(row, wrap_hidden(key, value));
    }

    pub fn delete_hidden(&mut self, tag: u8, key: &[u8]) {
        let row = self.hidden(tag, key);
        self.delete(row);
    }

    /// Record a fault: the batch then fails to commit, which stops the
    /// group's Raft core where it is (the entries after it are not applied).
    /// A machine calls it for a row it read but cannot decode, as a read does
    /// for a storage fault: a replica that cannot read what the others read
    /// stops instead of applying differently.
    pub fn fail(&self, why: impl std::fmt::Display) {
        let mut error = self.error.lock().unwrap_or_else(PoisonError::into_inner);
        if error.is_none() {
            tracing::error!(group = self.state.group, error = %why, "this batch fails");
            *error = Some(why.to_string());
        }
    }

    /// Whether a fault was recorded ([`StateTx::fail`]).
    #[must_use]
    pub fn failed(&self) -> bool {
        self.error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// Write the batch now, outside any group's apply: for tools and tests
    /// working on a group that is not running. A running group commits its
    /// batches itself, with the index they bring it to.
    ///
    /// The group's stored state digest no longer describes its rows, which
    /// changed outside the log: it is dropped, and taken again from the rows
    /// when the group next opens.
    pub fn commit(self) -> eyre::Result<()> {
        let db = Arc::clone(&self.state.db);
        let group = self.state.group;
        let mut tx = Transaction::default();
        self.into_batch(&mut tx)?;
        db.stage_drop_meta(&mut tx, group, crate::machine::DIGEST_META);
        db.commit(&tx, true)
    }

    /// Stage the writes into `tx`, or answer the fault recorded.
    pub(crate) fn into_batch(self, tx: &mut Transaction) -> eyre::Result<()> {
        if let Some(e) = self
            .error
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
        {
            eyre::bail!("the batch failed: {e}");
        }
        for (k, v) in self.staged {
            self.state.db.stage_state(
                tx,
                self.state.group,
                self.state.generation(),
                &k,
                v.as_deref(),
            )?;
        }
        Ok(())
    }
}

pub(crate) fn io(e: impl std::fmt::Display) -> StorageError {
    StorageIOError::write_logs(AnyError::error(e.to_string())).into()
}

/// Run `f`, which waits on the disk (a synced write, a snapshot file), on
/// tokio's blocking pool and wait for it there. Every group of a node shares
/// the async workers: a slow fsync run on one of them would hold up the other
/// groups' heartbeats and elections for as long as it takes. The caller still
/// awaits `f`, so what openraft relies on (an append durable before its
/// callback, a vote before the reply) holds as before.
pub(crate) async fn off_workers<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, StorageError> + Send + 'static,
) -> Result<T, StorageError> {
    tokio::task::spawn_blocking(f).await.map_err(io)?
}

/// One group's view of the log storage.
#[derive(Clone)]
pub struct LogStore {
    group: GroupId,
    db: Arc<Db>,
}

impl LogStore {
    #[must_use]
    pub fn new(group: GroupId, db: Arc<Db>) -> Self {
        Self { group, db }
    }
}

impl RaftLogReader<TypeConfig> for LogStore {
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + OptionalSend>(
        &mut self,
        range: RB,
    ) -> Result<Vec<Entry>, StorageError> {
        let lo = match range.start_bound() {
            Bound::Included(x) => *x,
            Bound::Excluded(x) => x + 1,
            Bound::Unbounded => 0,
        };
        let hi = match range.end_bound() {
            Bound::Included(x) => *x,
            Bound::Excluded(0) => return Ok(Vec::new()),
            Bound::Excluded(x) => x - 1,
            Bound::Unbounded => u64::MAX,
        };
        if hi < lo {
            return Ok(Vec::new());
        }
        self.db.range(self.group, lo, hi).map_err(io)
    }
}

impl RaftLogStorage<TypeConfig> for LogStore {
    type LogReader = Self;

    async fn get_log_state(&mut self) -> Result<LogState<TypeConfig>, StorageError> {
        let purged: Option<LogId> = self.db.get_meta(self.group, "purged").map_err(io)?;
        let last = self.db.last(self.group).map_err(io)?.map(|e| e.log_id);
        Ok(LogState {
            last_purged_log_id: purged,
            last_log_id: last.or(purged),
        })
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        self.clone()
    }

    async fn save_vote(&mut self, vote: &Vote) -> Result<(), StorageError> {
        let (db, group, vote) = (Arc::clone(&self.db), self.group, *vote);
        off_workers(move || db.put_meta(group, "vote", &vote).map_err(io)).await
    }

    async fn read_vote(&mut self) -> Result<Option<Vote>, StorageError> {
        self.db.get_meta(self.group, "vote").map_err(io)
    }

    async fn save_committed(&mut self, committed: Option<LogId>) -> Result<(), StorageError> {
        // Not synced: losing it only means re-learning the commit index. On
        // start openraft raises a record below the applied index to it and
        // replays the log up to one above it; the leader sends the rest.
        // Tests: `a_lost_or_older_commit_index_is_learned_again`,
        // `an_in_memory_machine_applies_each_entry_once_after_losing_its_commit_index`.
        if self.db.losing_unsynced(self.group) {
            return Ok(());
        }
        let mut tx = Transaction::default();
        self.db
            .stage_meta(&mut tx, self.group, "committed", &committed)
            .and_then(|()| self.db.commit(&tx, false))
            .map_err(io)
    }

    async fn read_committed(&mut self) -> Result<Option<LogId>, StorageError> {
        Ok(self
            .db
            .get_meta::<Option<LogId>>(self.group, "committed")
            .map_err(io)?
            .flatten())
    }

    async fn append<I>(
        &mut self,
        entries: I,
        callback: LogFlushed<TypeConfig>,
    ) -> Result<(), StorageError>
    where
        I: IntoIterator<Item = Entry> + OptionalSend,
        I::IntoIter: OptionalSend,
    {
        let entries: Vec<Entry> = entries.into_iter().collect();
        let (db, group) = (Arc::clone(&self.db), self.group);
        // Synced before the callback: openraft counts the entries as this
        // node's only once they are on the disk.
        let res = off_workers(move || db.put_entries(group, &entries).map_err(io)).await;
        match res {
            Ok(()) => {
                callback.log_io_completed(Ok(()));
                Ok(())
            }
            Err(e) => {
                callback.log_io_completed(Err(std::io::Error::other(e.to_string())));
                Err(e)
            }
        }
    }

    async fn truncate(&mut self, log_id: LogId) -> Result<(), StorageError> {
        let (db, group) = (Arc::clone(&self.db), self.group);
        off_workers(move || db.delete_log(group, log_id.index, u64::MAX).map_err(io)).await
    }

    async fn purge(&mut self, log_id: LogId) -> Result<(), StorageError> {
        let (db, group) = (Arc::clone(&self.db), self.group);
        off_workers(move || {
            db.put_meta(group, "purged", &log_id).map_err(io)?;
            db.delete_log(group, 0, log_id.index).map_err(io)
        })
        .await
    }
}

/// The generation of a group's state in use.
pub(crate) fn load_generation(db: &Db, group: GroupId) -> eyre::Result<u32> {
    Ok(db.get_meta::<u32>(group, "generation")?.unwrap_or(0))
}

/// How the data-dir paths are laid out.
#[must_use]
pub fn store_path(dir: &Path) -> PathBuf {
    dir.join("store")
}

#[cfg(test)]
mod known_answers {
    use super::*;

    const KEY: [u8; 32] = [0x42; 32];

    fn vector(name: &str) -> Vec<u8> {
        let text = include_str!("../fixtures/known_answers.txt");
        let hex = text
            .lines()
            .filter(|l| !l.starts_with('#'))
            .find_map(|l| l.strip_prefix(name).and_then(|r| r.strip_prefix(' ')))
            .unwrap_or_else(|| panic!("no vector {name}"));
        hex::decode(hex.trim()).unwrap()
    }

    #[test]
    fn the_keys_derived_from_the_storage_key_are_unchanged() {
        assert_eq!(
            derive(&KEY, "calimero.log.snapshot.v1").to_vec(),
            vector("snapshot_key")
        );
        assert_eq!(
            derive(&KEY, "calimero.log.hidden-keys.v1").to_vec(),
            vector("hider_key")
        );
    }

    #[test]
    fn a_hidden_rows_place_is_unchanged() {
        let hider = hmac::Key::new(
            hmac::HMAC_SHA256,
            &derive(&KEY, "calimero.log.hidden-keys.v1"),
        );
        assert_eq!(hide(&hider, 7, 3, b"app-key"), vector("hidden_row"));
    }

    #[test]
    fn a_row_sealed_before_still_opens() {
        let sealer = Sealer::new(&KEY);
        assert_eq!(
            sealer.open(b"calimero.kat", &vector("sealed_row")).unwrap(),
            b"sealed-under-aes-256-gcm"
        );
        assert!(
            sealer.open(b"other aad", &vector("sealed_row")).is_err(),
            "the aad is bound"
        );
    }
}

#[cfg(test)]
mod node_rows {
    use super::*;

    /// The node's rollback counter lives in the store itself: it survives a
    /// restart, stays sealed (node rows do; other rows are stored as they are), and
    /// no group's rows touch it.
    #[test]
    fn a_node_row_survives_a_reopen_and_opens_only_under_its_key() {
        let dir = tempfile::tempdir().unwrap();
        {
            let db = Db::open(dir.path(), &[1; 32]).unwrap();
            db.put_meta(NODE_ROWS, "rollback", &7_u64).unwrap();
            db.put_meta(1, "rollback", &1_u64).unwrap();
        }
        let db = Db::open(dir.path(), &[1; 32]).unwrap();
        assert_eq!(db.get_meta::<u64>(NODE_ROWS, "rollback").unwrap(), Some(7));
        assert_eq!(db.get_meta::<u64>(1, "rollback").unwrap(), Some(1));
        drop(db);
        let other = Db::open(dir.path(), &[2; 32]).unwrap();
        assert!(other.get_meta::<u64>(NODE_ROWS, "rollback").is_err());
    }
}

#[cfg(test)]
mod segments {
    use super::*;

    const A: &str = "aa";
    const B: &str = "bb";

    #[test]
    fn a_segment_and_its_index_row_are_written_and_deleted_together() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path(), &[1; 32]).unwrap();
        db.put_blob("plain", b"blob").unwrap();
        db.put_segment(B, b"two").unwrap();
        db.put_segment(A, b"one").unwrap();
        assert_eq!(db.get_segment(A).unwrap().as_deref(), Some(&b"one"[..]));
        assert_eq!(db.list_segments().unwrap(), vec![A, B]);
        assert_eq!(db.summary().unwrap().blobs, 3);
        assert!(
            db.summary().unwrap().groups.is_empty(),
            "no group sees the index"
        );
        db.delete_segment(A).unwrap();
        assert_eq!(db.get_segment(A).unwrap(), None);
        assert_eq!(db.list_segments().unwrap(), vec![B]);
        db.delete_segment(A).unwrap();
        assert_eq!(db.get_blob("plain").unwrap().as_deref(), Some(&b"blob"[..]));
    }

    #[test]
    fn the_index_lists_no_node_or_group_row() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path(), &[1; 32]).unwrap();
        db.put_meta(NODE_ROWS, "rollback", &7_u64).unwrap();
        db.put_meta(1, "vote", &1_u64).unwrap();
        assert!(db.list_segments().unwrap().is_empty());
        db.put_segment(A, b"x").unwrap();
        assert_eq!(db.list_segments().unwrap(), vec![A]);
    }
}

#[cfg(test)]
mod apply_reads {
    use super::*;

    fn state(dir: &tempfile::TempDir) -> GroupState {
        GroupState::new(Db::open(dir.path(), &[1; 32]).unwrap(), 5, 0)
    }

    /// Review finding B3: a read that failed inside an apply read as absent and
    /// the batch committed. It now fails the batch.
    #[test]
    fn a_read_fault_fails_the_batch_instead_of_reading_absent() {
        let dir = tempfile::tempdir().unwrap();
        let state = state(&dir);
        let mut tx = StateTx::new(&state);
        tx.put(b"k".to_vec(), b"v".to_vec());
        tx.commit().unwrap();
        crate::testing::fail_state_reads(dir.path(), 5);
        let tx = StateTx::new(&state);
        let read = tx.get(b"k");
        crate::testing::heal_state_reads(dir.path(), 5);
        assert_eq!(read, None);
        assert!(tx.failed(), "the batch goes on as if the row were absent");
        assert!(tx.commit().is_err());
        // A row this batch wrote is read back without touching the disk.
        let mut tx = StateTx::new(&state);
        tx.put(b"k".to_vec(), b"w".to_vec());
        assert_eq!(tx.get(b"k").as_deref(), Some(&b"w"[..]));
        assert!(!tx.failed());
    }

    /// A row under a hidden key that is not a hidden row's shape is unreadable
    /// too: inside an apply it fails the batch, and a scan reports it.
    #[test]
    fn a_malformed_hidden_row_fails_the_batch_and_the_scan() {
        let dir = tempfile::tempdir().unwrap();
        let state = state(&dir);
        let mut tx = StateTx::new(&state);
        let row = tx.hidden(7, b"key");
        tx.put(row, b"\xff".to_vec());
        tx.commit().unwrap();
        let tx = StateTx::new(&state);
        assert_eq!(tx.get_hidden(7, b"key"), None);
        assert!(
            tx.failed(),
            "a malformed row reads as absent and the batch goes on"
        );
        assert!(state.scan_hidden(7, |_k, _v| Ok(true)).is_err());
        // An absent row is no fault.
        let tx = StateTx::new(&state);
        assert_eq!(tx.get_hidden(7, b"other"), None);
        assert!(!tx.failed());
    }
}
