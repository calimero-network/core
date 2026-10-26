//! Durable, encrypted storage for every group's log, vote and snapshot.
//!
//! One redb database per node holds all groups. Every value is sealed with
//! AES-256-GCM under the node's storage key, so the disk holds only ciphertext.

use std::fmt::Debug;
use std::ops::{Bound, RangeBounds};
use std::path::Path;
use std::sync::Arc;

use openraft::storage::{LogFlushed, RaftLogStorage};
use openraft::{AnyError, LogState, OptionalSend, RaftLogReader, StorageIOError};
use redb::{Database, ReadableTable, TableDefinition};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::types::{Entry, GroupId, LogId, StorageError, TypeConfig, Vote};

const LOG: TableDefinition<'_, (u64, u64), &[u8]> = TableDefinition::new("log");
const META: TableDefinition<'_, (u64, &str), &[u8]> = TableDefinition::new("meta");

/// Seals and opens values with the node's storage key.
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
            .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(aad), &mut buf)
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
        let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| eyre::eyre!("bad nonce"))?;
        let mut buf = body.to_vec();
        let plain = self
            .key
            .open_in_place(nonce, Aad::from(aad), &mut buf)
            .map_err(|_| eyre::eyre!("value failed authentication"))?;
        Ok(plain.to_vec())
    }
}

/// The node's database, shared by all groups.
pub struct Db {
    db: Database,
    sealer: Sealer,
}

impl Db {
    pub fn open(path: &Path, key: &[u8; 32]) -> eyre::Result<Arc<Self>> {
        let db = Database::create(path)?;
        let tx = db.begin_write()?;
        {
            let _log = tx.open_table(LOG)?;
            let _meta = tx.open_table(META)?;
        }
        tx.commit()?;
        Ok(Arc::new(Self {
            db,
            sealer: Sealer::new(key),
        }))
    }

    fn aad(group: GroupId, kind: &str) -> Vec<u8> {
        let mut aad = group.to_be_bytes().to_vec();
        aad.extend_from_slice(kind.as_bytes());
        aad
    }

    pub fn put_meta<T: Serialize>(&self, group: GroupId, name: &str, value: &T) -> eyre::Result<()> {
        let plain = serde_json::to_vec(value)?;
        let sealed = self.sealer.seal(&Self::aad(group, name), &plain)?;
        let tx = self.db.begin_write()?;
        {
            let mut t = tx.open_table(META)?;
            let _prev = t.insert((group, name), sealed.as_slice())?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_meta<T: DeserializeOwned>(&self, group: GroupId, name: &str) -> eyre::Result<Option<T>> {
        let tx = self.db.begin_read()?;
        let t = tx.open_table(META)?;
        let Some(v) = t.get((group, name))? else {
            return Ok(None);
        };
        let plain = self.sealer.open(&Self::aad(group, name), v.value())?;
        Ok(Some(serde_json::from_slice(&plain)?))
    }

    fn put_entries(&self, group: GroupId, entries: &[Entry]) -> eyre::Result<()> {
        let tx = self.db.begin_write()?;
        {
            let mut t = tx.open_table(LOG)?;
            for e in entries {
                let plain = serde_json::to_vec(e)?;
                let sealed = self.sealer.seal(&Self::aad(group, "log"), &plain)?;
                let _prev = t.insert((group, e.log_id.index), sealed.as_slice())?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn range(&self, group: GroupId, lo: u64, hi_incl: u64) -> eyre::Result<Vec<Entry>> {
        let tx = self.db.begin_read()?;
        let t = tx.open_table(LOG)?;
        let mut out = Vec::new();
        for item in t.range((group, lo)..=(group, hi_incl))? {
            let (_k, v) = item?;
            let plain = self.sealer.open(&Self::aad(group, "log"), v.value())?;
            out.push(serde_json::from_slice(&plain)?);
        }
        Ok(out)
    }

    fn last(&self, group: GroupId) -> eyre::Result<Option<Entry>> {
        let tx = self.db.begin_read()?;
        let t = tx.open_table(LOG)?;
        let mut it = t.range((group, 0)..=(group, u64::MAX))?;
        match it.next_back() {
            Some(item) => {
                let (_k, v) = item?;
                let plain = self.sealer.open(&Self::aad(group, "log"), v.value())?;
                Ok(Some(serde_json::from_slice(&plain)?))
            }
            None => Ok(None),
        }
    }

    fn delete_range(&self, group: GroupId, lo: u64, hi_incl: u64) -> eyre::Result<()> {
        let tx = self.db.begin_write()?;
        {
            let mut t = tx.open_table(LOG)?;
            let keys: Vec<u64> = t
                .range((group, lo)..=(group, hi_incl))?
                .map(|r| r.map(|(k, _)| k.value().1))
                .collect::<Result<_, _>>()?;
            for k in keys {
                let _prev = t.remove((group, k))?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Delete everything of a group (used when a group is removed from this node).
    pub fn drop_group(&self, group: GroupId) -> eyre::Result<()> {
        self.delete_range(group, 0, u64::MAX)?;
        let tx = self.db.begin_write()?;
        {
            let mut t = tx.open_table(META)?;
            for name in ["vote", "committed", "purged", "snapshot"] {
                let _prev = t.remove((group, name))?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

fn io(e: impl std::fmt::Display) -> StorageError {
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
        self.db.put_meta(self.group, "committed", &committed).map_err(io)
    }

    async fn read_committed(&mut self) -> Result<Option<LogId>, StorageError> {
        Ok(self
            .db
            .get_meta::<Option<LogId>>(self.group, "committed")
            .map_err(io)?
            .flatten())
    }

    async fn append<I>(&mut self, entries: I, callback: LogFlushed<TypeConfig>) -> Result<(), StorageError>
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
            .delete_range(self.group, log_id.index, u64::MAX)
            .map_err(io)
    }

    async fn purge(&mut self, log_id: LogId) -> Result<(), StorageError> {
        self.db.put_meta(self.group, "purged", &log_id).map_err(io)?;
        self.db.delete_range(self.group, 0, log_id.index).map_err(io)
    }
}
