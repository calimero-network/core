//! The previous storage format (one redb file, `log.redb`), kept only to
//! migrate a node's data out of it.
//!
//! Every value was sealed with AES-256-GCM under the storage key, bound to its
//! group and kind (`group ‖ "log"`, `group ‖ name`, `"blob|" ‖ name`). A
//! snapshot held a machine's whole state as one JSON document.

use std::collections::BTreeSet;
use std::path::Path;

use redb::{Database, ReadableTable, TableDefinition};
use serde::de::DeserializeOwned;
#[cfg(test)]
use serde::Serialize;

use crate::machine::LegacySnapshot;
use crate::store::{Db, Sealer, Transaction};
use crate::types::{Entry, GroupId, LogId, Vote};

const LOG: TableDefinition<'_, (u64, u64), &[u8]> = TableDefinition::new("log");
const META: TableDefinition<'_, (u64, &str), &[u8]> = TableDefinition::new("meta");
const BLOBS: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("blobs");

/// The file name the previous format used inside a node's data directory.
pub const FILE: &str = "log.redb";

fn aad(group: GroupId, kind: &str) -> Vec<u8> {
    let mut aad = group.to_be_bytes().to_vec();
    aad.extend_from_slice(kind.as_bytes());
    aad
}

pub(crate) struct Legacy {
    db: Database,
    sealer: Sealer,
}

impl Legacy {
    pub(crate) fn open(path: &Path, key: &[u8; 32]) -> eyre::Result<Self> {
        Ok(Self {
            db: Database::open(path)?,
            sealer: Sealer::new(key),
        })
    }

    fn meta<T: DeserializeOwned>(&self, group: GroupId, name: &str) -> eyre::Result<Option<T>> {
        let tx = self.db.begin_read()?;
        let t = tx.open_table(META)?;
        let Some(v) = t.get((group, name))? else {
            return Ok(None);
        };
        let plain = self.sealer.open(&aad(group, name), v.value())?;
        Ok(Some(serde_json::from_slice(&plain)?))
    }

    fn groups(&self) -> eyre::Result<BTreeSet<GroupId>> {
        let tx = self.db.begin_read()?;
        let mut out = BTreeSet::new();
        for item in tx.open_table(META)?.iter()? {
            let (k, _v) = item?;
            let _new = out.insert(k.value().0);
        }
        for item in tx.open_table(LOG)?.iter()? {
            let (k, _v) = item?;
            let _new = out.insert(k.value().0);
        }
        Ok(out)
    }

    /// Copy everything into `db`: each group's entries, vote, commit and purge
    /// marks, and its snapshot (kept as `legacy-snapshot`, which the group's
    /// machine imports when it starts), and every blob.
    pub(crate) fn migrate_into(&self, db: &Db) -> eyre::Result<MigrationReport> {
        let mut report = MigrationReport::default();
        for group in self.groups()? {
            let mut entries = Vec::new();
            {
                let tx = self.db.begin_read()?;
                let t = tx.open_table(LOG)?;
                for item in t.range((group, 0)..=(group, u64::MAX))? {
                    let (_k, v) = item?;
                    let plain = self.sealer.open(&aad(group, "log"), v.value())?;
                    entries.push(serde_json::from_slice::<Entry>(&plain)?);
                    if entries.len() >= 1024 {
                        report.entries += entries.len();
                        db.put_entries(group, &entries)?;
                        entries.clear();
                    }
                }
            }
            report.entries += entries.len();
            db.put_entries(group, &entries)?;
            let mut batch = Transaction::default();
            if let Some(vote) = self.meta::<Vote>(group, "vote")? {
                db.stage_meta(&mut batch, group, "vote", &vote)?;
            }
            if let Some(committed) = self.meta::<Option<LogId>>(group, "committed")? {
                db.stage_meta(&mut batch, group, "committed", &committed)?;
            }
            if let Some(purged) = self.meta::<LogId>(group, "purged")? {
                db.stage_meta(&mut batch, group, "purged", &purged)?;
            }
            if let Some(snap) = self.meta::<LegacySnapshot>(group, "snapshot")? {
                db.stage_meta(&mut batch, group, "legacy-snapshot", &snap)?;
                report.snapshots += 1;
            }
            db.commit(&batch, true)?;
            report.groups += 1;
        }
        let tx = self.db.begin_read()?;
        for item in tx.open_table(BLOBS)?.iter()? {
            let (k, v) = item?;
            let name = k.value().to_owned();
            let bytes = self
                .sealer
                .open(format!("blob|{name}").as_bytes(), v.value())?;
            db.put_blob(&name, &bytes)?;
            report.blobs += 1;
        }
        Ok(report)
    }
}

/// What a migration carried over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MigrationReport {
    pub groups: usize,
    pub entries: usize,
    pub snapshots: usize,
    pub blobs: usize,
}

/// Writes the previous format, for the migration's tests.
#[cfg(test)]
pub(crate) struct LegacyWriter {
    db: Database,
    sealer: Sealer,
}

#[cfg(test)]
impl LegacyWriter {
    pub(crate) fn create(path: &Path, key: &[u8; 32]) -> eyre::Result<Self> {
        let db = Database::create(path)?;
        let tx = db.begin_write()?;
        {
            let _log = tx.open_table(LOG)?;
            let _meta = tx.open_table(META)?;
            let _blobs = tx.open_table(BLOBS)?;
        }
        tx.commit()?;
        Ok(Self {
            db,
            sealer: Sealer::new(key),
        })
    }

    pub(crate) fn put_meta<T: Serialize>(
        &self,
        group: GroupId,
        name: &str,
        value: &T,
    ) -> eyre::Result<()> {
        let sealed = self
            .sealer
            .seal(&aad(group, name), &serde_json::to_vec(value)?)?;
        let tx = self.db.begin_write()?;
        {
            let mut t = tx.open_table(META)?;
            let _prev = t.insert((group, name), sealed.as_slice())?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn put_entry(&self, group: GroupId, entry: &Entry) -> eyre::Result<()> {
        let sealed = self
            .sealer
            .seal(&aad(group, "log"), &serde_json::to_vec(entry)?)?;
        let tx = self.db.begin_write()?;
        {
            let mut t = tx.open_table(LOG)?;
            let _prev = t.insert((group, entry.log_id.index), sealed.as_slice())?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn put_blob(&self, name: &str, bytes: &[u8]) -> eyre::Result<()> {
        let sealed = self.sealer.seal(format!("blob|{name}").as_bytes(), bytes)?;
        let tx = self.db.begin_write()?;
        {
            let mut t = tx.open_table(BLOBS)?;
            let _prev = t.insert(name, sealed.as_slice())?;
        }
        tx.commit()?;
        Ok(())
    }
}
