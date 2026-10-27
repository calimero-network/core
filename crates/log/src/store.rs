//! Durable, encrypted storage for every group's log, metadata and state.
//!
//! One RocksDB database per node holds all groups, opened with the column and
//! table options `calimero-store` gives v1's store (compression, bloom filters,
//! one bounded block cache), in four column families:
//!
//! | Column | Key | Holds |
//! | --- | --- | --- |
//! | `log` | `group ‖ index` | Raft entries |
//! | `meta` | `group ‖ name` | vote, committed, purged, applied, generation, snapshot |
//! | `state` | `group ‖ generation ‖ machine key` | the state machines |
//! | `blobs` | `name` | content kept outside every log |
//!
//! Every value is sealed with AES-256-GCM under the node's storage key, bound
//! (as AAD) to its column, its group and its key, so a value moved to another
//! row, group or column fails authentication. A machine's private keys can be
//! hidden too ([`hide`]): the row is keyed by an HMAC of the key, and the key
//! itself travels inside the sealed value.
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
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

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
fn derive(storage_key: &[u8; 32], label: &str) -> [u8; 32] {
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
fn wrap_hidden(key: &[u8], value: &[u8]) -> Vec<u8> {
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

    /// The column's tag in every AAD.
    const fn tag(self) -> u8 {
        match self {
            Self::Log => TAG_LOG,
            Self::Meta => TAG_META,
            Self::State => TAG_STATE,
            Self::Blobs => TAG_BLOB,
        }
    }

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

const TAG_LOG: u8 = 1;
const TAG_META: u8 = 2;
const TAG_STATE: u8 = 3;
const TAG_BLOB: u8 = 4;

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

fn meta_key(group: GroupId, name: &str) -> Vec<u8> {
    let mut k = group_prefix(group).to_vec();
    k.extend_from_slice(name.as_bytes());
    k
}

fn state_prefix(group: GroupId, generation: u32) -> Vec<u8> {
    let mut k = group_prefix(group).to_vec();
    k.extend_from_slice(&generation.to_be_bytes());
    k
}

fn state_key(group: GroupId, generation: u32, key: &[u8]) -> Vec<u8> {
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
    sealer: Sealer,
    /// Seals snapshot files (see [`crate::machine`]): every node of the
    /// cluster shares the storage key, so a follower opens what a leader built.
    pub(crate) snapshot_sealer: Sealer,
    pub(crate) hider: hmac::Key,
    pub(crate) snapshot_dir: PathBuf,
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
                &derive(key, "calimero.log.hidden-keys.v1"),
            ),
            snapshot_dir,
        }))
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

    fn get_open(
        &self,
        col: Column,
        group: Option<GroupId>,
        key: &[u8],
        aad_key: &[u8],
    ) -> eyre::Result<Option<Vec<u8>>> {
        let Some(v) = self.raw_get(col, key)? else {
            return Ok(None);
        };
        Ok(Some(self.sealer.open(&aad(col.tag(), group, aad_key), &v)?))
    }

    fn sealed_put(
        &self,
        tx: &mut Transaction,
        col: Column,
        group: Option<GroupId>,
        key: Vec<u8>,
        aad_key: &[u8],
        plain: &[u8],
    ) -> eyre::Result<()> {
        let sealed = self.sealer.seal(&aad(col.tag(), group, aad_key), plain)?;
        tx.raw_put(col, key, sealed);
        Ok(())
    }

    /// Stage `value` as group metadata `name`.
    pub(crate) fn stage_meta<T: Serialize>(
        &self,
        tx: &mut Transaction,
        group: GroupId,
        name: &str,
        value: &T,
    ) -> eyre::Result<()> {
        let plain = serde_json::to_vec(value)?;
        let key = meta_key(group, name);
        self.sealed_put(tx, Column::Meta, None, key.clone(), &key, &plain)
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
        self.write(&tx, true)
    }

    pub fn get_meta<T: DeserializeOwned>(
        &self,
        group: GroupId,
        name: &str,
    ) -> eyre::Result<Option<T>> {
        let key = meta_key(group, name);
        let Some(plain) = self.get_open(Column::Meta, None, &key, &key)? else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_slice(&plain)?))
    }

    /// Store a blob (content kept outside every log, e.g. a WASM bundle).
    pub fn put_blob(&self, name: &str, bytes: &[u8]) -> eyre::Result<()> {
        let mut tx = Transaction::default();
        let key = name.as_bytes().to_vec();
        self.sealed_put(&mut tx, Column::Blobs, None, key.clone(), &key, bytes)?;
        self.write(&tx, true)
    }

    /// Delete a blob stored with [`Db::put_blob`]; nothing if there is none.
    pub fn delete_blob(&self, name: &str) -> eyre::Result<()> {
        let mut tx = Transaction::default();
        tx.raw_delete(Column::Blobs, name.as_bytes().to_vec());
        self.write(&tx, true)
    }

    /// Read a blob stored with [`Db::put_blob`].
    pub fn get_blob(&self, name: &str) -> eyre::Result<Option<Vec<u8>>> {
        let key = name.as_bytes();
        self.get_open(Column::Blobs, None, key, key)
    }

    pub(crate) fn put_entries(&self, group: GroupId, entries: &[Entry]) -> eyre::Result<()> {
        let mut tx = Transaction::default();
        for e in entries {
            let plain = serde_json::to_vec(e)?;
            let key = log_key(group, e.log_id.index);
            self.sealed_put(&mut tx, Column::Log, None, key.clone(), &key, &plain)?;
        }
        self.write(&tx, true)
    }

    fn range(&self, group: GroupId, lo: u64, hi_incl: u64) -> eyre::Result<Vec<Entry>> {
        let mut out = Vec::new();
        let end = log_key(group, hi_incl);
        self.scan(Column::Log, &log_key(group, lo), |k, v| {
            if k > end.as_slice() {
                return Ok(false);
            }
            let plain = self.sealer.open(&aad(TAG_LOG, None, k), v)?;
            out.push(serde_json::from_slice(&plain)?);
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
            let plain = self.sealer.open(&aad(TAG_LOG, None, k), v)?;
            return Ok(Some(serde_json::from_slice(&plain)?));
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
    /// blobs. v1's `merodb` read the same from its own columns.
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
        self.get_open(
            Column::State,
            Some(group),
            &state_key(group, generation, key),
            key,
        )
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
            Some(v) => self.sealed_put(tx, Column::State, Some(group), full, key, v),
            None => {
                tx.raw_delete(Column::State, full);
                Ok(())
            }
        }
    }

    /// Stage an already sealed state row (a snapshot's, moved as it is).
    pub(crate) fn stage_sealed_state(
        tx: &mut Transaction,
        group: GroupId,
        generation: u32,
        key: &[u8],
        sealed: Vec<u8>,
    ) {
        tx.raw_put(Column::State, state_key(group, generation, key), sealed);
    }

    /// Check a sealed state row without keeping the plaintext.
    pub(crate) fn verify_state(
        &self,
        group: GroupId,
        key: &[u8],
        sealed: &[u8],
    ) -> eyre::Result<()> {
        let _plain = self.open_state(group, key, sealed)?;
        Ok(())
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
                let plain = self.sealer.open(&aad(TAG_STATE, Some(group), key), v)?;
                f(key, &plain)
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
}

impl GroupState {
    #[must_use]
    pub const fn group(&self) -> GroupId {
        self.group
    }

    fn generation(&self) -> u32 {
        self.generation.load(Ordering::Acquire)
    }

    /// One row. A storage fault reads as absent and is logged.
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
                None => Ok(true),
            }
        })
    }
}

/// The writes one batch of applied entries makes to its group's state, read
/// back by later entries of the batch. Committed with the applied index, in
/// one write.
pub struct StateTx<'a> {
    state: &'a GroupState,
    staged: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    error: Option<String>,
}

impl<'a> StateTx<'a> {
    #[must_use]
    pub fn new(state: &'a GroupState) -> Self {
        Self {
            state,
            staged: BTreeMap::new(),
            error: None,
        }
    }

    /// The group's committed state, without this batch's writes.
    #[must_use]
    pub const fn committed(&self) -> &GroupState {
        self.state
    }

    /// One row, as this batch left it.
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
                    tracing::error!(group = self.state.group, error = %e, "state read failed");
                    None
                }
            },
        }
    }

    pub fn put(&mut self, key: Vec<u8>, value: Vec<u8>) {
        let _prev = self.staged.insert(key, Some(value));
    }

    pub fn delete(&mut self, key: Vec<u8>) {
        let _prev = self.staged.insert(key, None);
    }

    fn hidden(&self, tag: u8, key: &[u8]) -> Vec<u8> {
        hide(&self.state.db.hider, self.state.group, tag, key)
    }

    #[must_use]
    pub fn get_hidden(&self, tag: u8, key: &[u8]) -> Option<Vec<u8>> {
        let raw = self.get(&self.hidden(tag, key))?;
        let (k, v) = unwrap_hidden(&raw)?;
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

    /// Record a fault: the batch then fails to commit.
    pub fn fail(&mut self, why: impl std::fmt::Display) {
        if self.error.is_none() {
            self.error = Some(why.to_string());
        }
    }

    /// Write the batch now, outside any group's apply: for tools and tests
    /// working on a group that is not running. A running group commits its
    /// batches itself, with the index they bring it to.
    pub fn commit(self) -> eyre::Result<()> {
        let db = Arc::clone(&self.state.db);
        let mut tx = Transaction::default();
        self.into_batch(&mut tx)?;
        db.commit(&tx, true)
    }

    /// Stage the writes into `tx`, or answer the fault recorded.
    pub(crate) fn into_batch(self, tx: &mut Transaction) -> eyre::Result<()> {
        if let Some(e) = self.error {
            eyre::bail!("state write failed: {e}");
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
        self.db.put_meta(self.group, "vote", vote).map_err(io)
    }

    async fn read_vote(&mut self) -> Result<Option<Vote>, StorageError> {
        self.db.get_meta(self.group, "vote").map_err(io)
    }

    async fn save_committed(&mut self, committed: Option<LogId>) -> Result<(), StorageError> {
        // Not synced: losing it only means re-learning the commit index.
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
        let res = self.db.put_entries(self.group, &entries);
        match res {
            Ok(()) => {
                callback.log_io_completed(Ok(()));
                Ok(())
            }
            Err(e) => {
                callback.log_io_completed(Err(std::io::Error::other(e.to_string())));
                Err(io(e))
            }
        }
    }

    async fn truncate(&mut self, log_id: LogId) -> Result<(), StorageError> {
        self.db
            .delete_log(self.group, log_id.index, u64::MAX)
            .map_err(io)
    }

    async fn purge(&mut self, log_id: LogId) -> Result<(), StorageError> {
        self.db
            .put_meta(self.group, "purged", &log_id)
            .map_err(io)?;
        self.db.delete_log(self.group, 0, log_id.index).map_err(io)
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
