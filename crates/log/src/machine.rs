//! Adapter between openraft's state machine trait and a group's [`Machine`].
//!
//! A machine's state lives in the database ([`GroupState`]); an applied batch
//! commits its writes together with the applied index, so a restart resumes
//! where it stopped instead of replaying the log. A machine that keeps its
//! state in memory ([`Machine::IN_MEMORY`]) is checkpointed with every snapshot
//! instead, and resumes from the last one.
//!
//! Snapshots are files of sealed frames, streamed from a consistent view of
//! the database and never held whole in memory. Installing one writes the next
//! generation of the group's state and switches to it in one batch, so a crash
//! mid-install leaves the previous state intact.

#![allow(
    clippy::result_large_err,
    reason = "openraft's trait signatures fix these error types"
)]

use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

use openraft::storage::RaftStateMachine;
use openraft::{
    AnyError, EntryPayload, OptionalSend, RaftSnapshotBuilder, Snapshot, StorageIOError,
};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::store::{load_generation, Db, GroupState, StateTx, Transaction};
use crate::types::{
    Bytes, Entry, GroupId, LogId, SnapshotMeta, StorageError, StoredMembership, TypeConfig,
};

/// What a group replicates. The engine calls `apply` on every node, in log
/// order, with the committed command bytes; the output goes back to the writer.
pub trait Machine: Send + Sync + 'static {
    /// The machine keeps its state in memory: it is persisted only with
    /// snapshots ([`Machine::snapshot`]), and a restart replays the log from
    /// the last one. Otherwise the state is the group's [`GroupState`].
    const IN_MEMORY: bool = false;

    /// The group's durable state, handed over once before anything else.
    fn attach(&mut self, _state: GroupState) {}

    /// Apply one committed command, writing through `state`. Must not fail: a
    /// command that cannot be applied is recorded as a failure inside the
    /// machine's own output.
    fn apply(&mut self, index: u64, command: &[u8], state: &mut StateTx<'_>) -> Vec<u8>;

    /// An in-memory machine's whole state. Unused otherwise.
    fn snapshot(&self) -> Vec<u8> {
        Vec::new()
    }

    /// An in-memory machine: replace the whole state with `data`. Otherwise
    /// `data` is empty and the machine re-reads what it caches from its
    /// [`GroupState`] (after a restart or a snapshot install).
    fn restore(&mut self, data: &[u8]) -> eyre::Result<()>;

    /// Bring in a state written by the previous storage format (one JSON
    /// document, what [`Machine::snapshot`] used to answer for every machine),
    /// writing it through `state`. In-memory machines restore it as it is.
    fn import(&mut self, data: &[u8], _state: &mut StateTx<'_>) -> eyre::Result<()> {
        self.restore(data)
    }
}

/// The applied state of a group, shared between the Raft core and readers.
pub struct Applied<M> {
    pub machine: M,
    pub last_applied: Option<LogId>,
    pub membership: StoredMembership,
}

impl<M> Applied<M> {
    #[must_use]
    pub fn applied_index(&self) -> u64 {
        self.last_applied.map_or(0, |l| l.index)
    }
}

pub type Shared<M> = Arc<RwLock<Applied<M>>>;

/// Where a group stands, committed with the state it describes.
#[derive(Clone, Default, Serialize, Deserialize)]
struct AppliedMeta {
    last_applied: Option<LogId>,
    membership: StoredMembership,
}

/// The snapshot a group can serve, and the file that holds it.
#[derive(Clone, Serialize, Deserialize)]
struct SnapshotRecord {
    meta: SnapshotMeta,
    file: String,
}

/// A snapshot in the previous storage format, carried over by the migration.
#[derive(Serialize, Deserialize)]
pub(crate) struct LegacySnapshot {
    pub(crate) meta: SnapshotMeta,
    pub(crate) data: Bytes,
}

/// The state row an in-memory machine's checkpoint lives in.
const CHECKPOINT_KEY: &[u8] = b"\0calimero.log.checkpoint";

const SNAPSHOT_MAGIC: &[u8] = b"CALIMERO-SNAPSHOT-1\n";
/// Rows per frame stop growing past this many plaintext bytes.
const FRAME_BYTES: usize = 1024 * 1024;
/// A frame is refused past this size when read back.
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
/// Rows written per batch while installing a snapshot.
const INSTALL_BATCH_BYTES: usize = 4 * 1024 * 1024;

fn io(e: impl std::fmt::Display) -> StorageError {
    StorageIOError::write_state_machine(AnyError::error(e.to_string())).into()
}

fn frame_aad(group: GroupId, snapshot_id: &str, n: u64, last: bool) -> Vec<u8> {
    let mut a = b"calimero.log.snapshot-frame".to_vec();
    a.extend_from_slice(&group.to_be_bytes());
    a.extend_from_slice(&n.to_be_bytes());
    a.push(u8::from(last));
    a.extend_from_slice(snapshot_id.as_bytes());
    a
}

fn put_u32(out: &mut Vec<u8>, n: usize) {
    out.extend_from_slice(&u32::try_from(n).unwrap_or(u32::MAX).to_be_bytes());
}

fn take<'a>(buf: &mut &'a [u8], n: usize) -> eyre::Result<&'a [u8]> {
    if buf.len() < n {
        eyre::bail!("snapshot frame is truncated");
    }
    let (head, rest) = buf.split_at(n);
    *buf = rest;
    Ok(head)
}

fn take_u32(buf: &mut &[u8]) -> eyre::Result<usize> {
    let b = take(buf, 4)?;
    Ok(u32::from_be_bytes(b.try_into()?) as usize)
}

/// Write a snapshot file: the magic, then frames of `(key, sealed row)` pairs,
/// each frame sealed and bound to its group, snapshot, position and whether it
/// is the last, so a frame cannot be moved, dropped or cut off unnoticed.
struct FrameWriter<'a> {
    db: &'a Db,
    group: GroupId,
    id: String,
    out: BufWriter<std::fs::File>,
    buf: Vec<u8>,
    rows: u32,
    n: u64,
}

impl FrameWriter<'_> {
    fn row(&mut self, key: &[u8], sealed: &[u8]) -> eyre::Result<()> {
        put_u32(&mut self.buf, key.len());
        self.buf.extend_from_slice(key);
        put_u32(&mut self.buf, sealed.len());
        self.buf.extend_from_slice(sealed);
        self.rows += 1;
        if self.buf.len() >= FRAME_BYTES {
            self.flush(false)?;
        }
        Ok(())
    }

    fn flush(&mut self, last: bool) -> eyre::Result<()> {
        let mut plain = self.rows.to_be_bytes().to_vec();
        plain.append(&mut self.buf);
        let sealed = self
            .db
            .snapshot_sealer
            .seal(&frame_aad(self.group, &self.id, self.n, last), &plain)?;
        self.out.write_all(&[u8::from(last)])?;
        self.out
            .write_all(&u32::try_from(sealed.len())?.to_be_bytes())?;
        self.out.write_all(&sealed)?;
        self.n += 1;
        self.rows = 0;
        Ok(())
    }

    fn finish(mut self) -> eyre::Result<()> {
        self.flush(true)?;
        let file = self.out.into_inner().map_err(|e| eyre::eyre!("{e}"))?;
        file.sync_all()?;
        Ok(())
    }
}

/// Read a snapshot file back, frame by frame: `f(key, sealed row)`.
fn read_frames(
    frames: &crate::store::Sealer,
    group: GroupId,
    id: &str,
    file: std::fs::File,
    mut f: impl FnMut(&[u8], &[u8]) -> eyre::Result<()>,
) -> eyre::Result<()> {
    let mut r = BufReader::new(file);
    let mut magic = vec![0; SNAPSHOT_MAGIC.len()];
    r.read_exact(&mut magic)?;
    if magic != SNAPSHOT_MAGIC {
        eyre::bail!("not a snapshot file");
    }
    let mut n = 0_u64;
    loop {
        let mut head = [0_u8; 5];
        r.read_exact(&mut head)
            .map_err(|_| eyre::eyre!("snapshot ends before its last frame"))?;
        let last = match head[0] {
            0 => false,
            1 => true,
            _ => eyre::bail!("bad snapshot frame"),
        };
        let len = u32::from_be_bytes(head[1..].try_into()?) as usize;
        if len > MAX_FRAME_BYTES {
            eyre::bail!("snapshot frame of {len} bytes is too large");
        }
        let mut sealed = vec![0; len];
        r.read_exact(&mut sealed)?;
        let plain = frames.open(&frame_aad(group, id, n, last), &sealed)?;
        let mut buf = plain.as_slice();
        let rows = u32::from_be_bytes(take(&mut buf, 4)?.try_into()?);
        for _ in 0..rows {
            let klen = take_u32(&mut buf)?;
            let key = take(&mut buf, klen)?;
            let vlen = take_u32(&mut buf)?;
            let value = take(&mut buf, vlen)?;
            f(key, value)?;
        }
        if !buf.is_empty() {
            eyre::bail!("snapshot frame has trailing bytes");
        }
        n += 1;
        if last {
            let mut rest = [0_u8; 1];
            if r.read(&mut rest)? != 0 {
                eyre::bail!("snapshot has bytes past its last frame");
            }
            return Ok(());
        }
    }
}

/// openraft state machine over a [`Machine`].
pub struct StateMachine<M: Machine> {
    group: GroupId,
    db: Arc<Db>,
    state: GroupState,
    shared: Shared<M>,
    applied_tx: Arc<watch::Sender<u64>>,
    snapshots_built: u64,
    /// The file the snapshot being received is written to.
    receiving: Option<PathBuf>,
}

impl<M: Machine> StateMachine<M> {
    /// Open a group's state: resume a disk machine where it stopped, restore
    /// an in-memory one from its last checkpoint, and bring in a state left by
    /// the previous storage format.
    pub fn open(
        group: GroupId,
        db: Arc<Db>,
        mut machine: M,
        applied_tx: Arc<watch::Sender<u64>>,
    ) -> eyre::Result<Self> {
        let generation = load_generation(&db, group)?;
        // A crash mid-install leaves rows of a generation never switched to.
        db.drop_other_generations(group, generation)?;
        let state = GroupState {
            db: Arc::clone(&db),
            group,
            generation: Arc::new(AtomicU32::new(generation)),
        };
        machine.attach(state.clone());
        let mut applied: AppliedMeta = db.get_meta(group, "applied")?.unwrap_or_default();
        if applied.last_applied.is_none() {
            if let Some(legacy) = db.get_meta::<LegacySnapshot>(group, "legacy-snapshot")? {
                let mut tx = StateTx::new(&state);
                machine.import(&legacy.data.0, &mut tx)?;
                applied = AppliedMeta {
                    last_applied: legacy.meta.last_log_id,
                    membership: legacy.meta.last_membership.clone(),
                };
                let mut batch = Transaction::default();
                tx.into_batch(&mut batch)?;
                if M::IN_MEMORY {
                    db.stage_state(
                        &mut batch,
                        group,
                        generation,
                        CHECKPOINT_KEY,
                        Some(&machine.snapshot()),
                    )?;
                }
                db.stage_meta(&mut batch, group, "applied", &applied)?;
                db.commit(&batch, true)?;
                db.drop_meta(group, "legacy-snapshot")?;
            }
        }
        if M::IN_MEMORY {
            if let Some(data) = state.get(CHECKPOINT_KEY) {
                machine.restore(&data)?;
            }
        } else {
            machine.restore(&[])?;
        }
        let last_applied = applied.last_applied;
        let shared = Arc::new(RwLock::new(Applied {
            machine,
            last_applied,
            membership: applied.membership,
        }));
        let _ = applied_tx.send(last_applied.map_or(0, |l| l.index));
        Ok(Self {
            group,
            db,
            state,
            shared,
            applied_tx,
            snapshots_built: 0,
            receiving: None,
        })
    }

    #[must_use]
    pub fn shared(&self) -> Shared<M> {
        Arc::clone(&self.shared)
    }

    /// The group's durable state.
    #[must_use]
    pub fn state(&self) -> GroupState {
        self.state.clone()
    }

    fn snapshot_path(&self, name: &str) -> PathBuf {
        self.db.snapshot_dir.join(name)
    }

    fn build(&mut self) -> eyre::Result<Snapshot<TypeConfig>> {
        self.snapshots_built += 1;
        let generation = self.state.generation.load(Ordering::Acquire);
        // Under the read lock no batch applies, so the meta below and the view
        // the scan reads are the same state. The scan's view is pinned when it
        // opens, so the lock is not held while the file is written.
        let st = self.shared.read().map_err(|e| eyre::eyre!("{e}"))?;
        let last = st.last_applied;
        let meta = SnapshotMeta {
            last_log_id: last,
            last_membership: st.membership.clone(),
            snapshot_id: format!(
                "{}-{}-{}",
                last.map_or(0, |l| l.index),
                self.snapshots_built,
                hex_id()
            ),
        };
        if M::IN_MEMORY {
            let mut batch = Transaction::default();
            self.db.stage_state(
                &mut batch,
                self.group,
                generation,
                CHECKPOINT_KEY,
                Some(&st.machine.snapshot()),
            )?;
            self.db.stage_meta(
                &mut batch,
                self.group,
                "applied",
                &AppliedMeta {
                    last_applied: last,
                    membership: st.membership.clone(),
                },
            )?;
            self.db.commit(&batch, true)?;
        }
        let name = format!("g{}-{}.snap", self.group, meta.snapshot_id);
        let path = self.snapshot_path(&name);
        let mut out = BufWriter::new(std::fs::File::create(&path)?);
        out.write_all(SNAPSHOT_MAGIC)?;
        let mut w = FrameWriter {
            db: &self.db,
            group: self.group,
            id: meta.snapshot_id.clone(),
            out,
            buf: Vec::new(),
            rows: 0,
            n: 0,
        };
        let prefix = {
            let mut p = self.group.to_be_bytes().to_vec();
            p.extend_from_slice(&generation.to_be_bytes());
            p
        };
        let mut guard = Some(st);
        self.db.scan_state_snapshot(&prefix, |k, v| {
            // The view is open: let applies continue.
            drop(guard.take());
            let Some(key) = k.strip_prefix(prefix.as_slice()) else {
                return Ok(false);
            };
            w.row(key, v)?;
            Ok(true)
        })?;
        drop(guard);
        w.finish()?;
        self.keep_snapshot(SnapshotRecord {
            meta: meta.clone(),
            file: name,
        })?;
        Ok(Snapshot {
            meta,
            snapshot: Box::new(tokio::fs::File::from_std(std::fs::File::open(&path)?)),
        })
    }

    /// Record `record` as the snapshot this group serves, and delete the one
    /// it replaces.
    fn keep_snapshot(&self, record: SnapshotRecord) -> eyre::Result<()> {
        let previous: Option<SnapshotRecord> = self.db.get_meta(self.group, "snapshot")?;
        self.db.put_meta(self.group, "snapshot", &record)?;
        if let Some(p) = previous {
            if p.file != record.file {
                let _removed = std::fs::remove_file(self.snapshot_path(&p.file));
            }
        }
        Ok(())
    }

    fn install(&mut self, meta: &SnapshotMeta) -> eyre::Result<()> {
        let path = self
            .receiving
            .take()
            .ok_or_else(|| eyre::eyre!("no snapshot is being received"))?;
        let old = self.state.generation.load(Ordering::Acquire);
        let next = old.wrapping_add(1);
        self.db.drop_generation(self.group, next)?;
        let mut batch = Transaction::default();
        let mut pending = 0_usize;
        let group = self.group;
        let db = Arc::clone(&self.db);
        let file = std::fs::File::open(&path)?;
        let read = read_frames(
            &db.snapshot_sealer,
            group,
            &meta.snapshot_id,
            file,
            |key, sealed| {
                db.verify_state(group, key, sealed)?;
                Db::stage_sealed_state(&mut batch, group, next, key, sealed.to_vec());
                pending += key.len() + sealed.len();
                if pending >= INSTALL_BATCH_BYTES {
                    db.commit(&batch, false)?;
                    batch = Transaction::default();
                    pending = 0;
                }
                Ok(())
            },
        );
        if let Err(e) = read.and_then(|()| db.commit(&batch, false)) {
            let _ = db.drop_generation(group, next);
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        {
            let mut st = self.shared.write().map_err(|e| eyre::eyre!("{e}"))?;
            let mut switch = Transaction::default();
            db.stage_meta(&mut switch, group, "generation", &next)?;
            db.stage_meta(
                &mut switch,
                group,
                "applied",
                &AppliedMeta {
                    last_applied: meta.last_log_id,
                    membership: meta.last_membership.clone(),
                },
            )?;
            db.commit(&switch, true)?;
            self.state.generation.store(next, Ordering::Release);
            if M::IN_MEMORY {
                let data = self.state.get(CHECKPOINT_KEY).unwrap_or_default();
                st.machine.restore(&data)?;
            } else {
                st.machine.restore(&[])?;
            }
            st.last_applied = meta.last_log_id;
            st.membership = meta.last_membership.clone();
        }
        db.drop_generation(group, old)?;
        self.keep_snapshot(SnapshotRecord {
            meta: meta.clone(),
            file: name,
        })?;
        Ok(())
    }
}

fn hex_id() -> String {
    use ring::rand::SecureRandom;
    let mut b = [0_u8; 6];
    let _ = ring::rand::SystemRandom::new().fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Builds snapshots off the shared state.
pub struct Builder<M: Machine> {
    inner: StateMachine<M>,
}

impl<M: Machine> RaftSnapshotBuilder<TypeConfig> for Builder<M> {
    async fn build_snapshot(&mut self) -> Result<Snapshot<TypeConfig>, StorageError> {
        self.inner.build().map_err(io)
    }
}

impl<M: Machine> RaftStateMachine<TypeConfig> for StateMachine<M> {
    type SnapshotBuilder = Builder<M>;

    async fn applied_state(&mut self) -> Result<(Option<LogId>, StoredMembership), StorageError> {
        let st = self.shared.read().map_err(io)?;
        Ok((st.last_applied, st.membership.clone()))
    }

    async fn apply<I>(&mut self, entries: I) -> Result<Vec<Bytes>, StorageError>
    where
        I: IntoIterator<Item = Entry> + OptionalSend,
        I::IntoIter: OptionalSend,
    {
        let mut out = Vec::new();
        let mut last = 0;
        {
            let mut st = self.shared.write().map_err(io)?;
            let mut tx = StateTx::new(&self.state);
            for entry in entries {
                st.last_applied = Some(entry.log_id);
                last = entry.log_id.index;
                match entry.payload {
                    EntryPayload::Blank => out.push(Bytes::default()),
                    EntryPayload::Normal(cmd) => {
                        let r = st.machine.apply(entry.log_id.index, &cmd.0, &mut tx);
                        out.push(Bytes(r));
                    }
                    EntryPayload::Membership(m) => {
                        st.membership = StoredMembership::new(Some(entry.log_id), m);
                        out.push(Bytes::default());
                    }
                }
            }
            // The batch's writes and the index they bring the state to land
            // together; readers, held off by the lock, see neither until then.
            let mut batch = Transaction::default();
            tx.into_batch(&mut batch).map_err(io)?;
            if !M::IN_MEMORY && last > 0 {
                self.db
                    .stage_meta(
                        &mut batch,
                        self.group,
                        "applied",
                        &AppliedMeta {
                            last_applied: st.last_applied,
                            membership: st.membership.clone(),
                        },
                    )
                    .map_err(io)?;
            }
            // Not synced: the entries are in the synced log, so a lost batch is
            // re-applied from there.
            self.db.commit(&batch, false).map_err(io)?;
        }
        if last > 0 {
            let _ = self.applied_tx.send(last);
        }
        Ok(out)
    }

    async fn get_snapshot_builder(&mut self) -> Self::SnapshotBuilder {
        Builder {
            inner: StateMachine {
                group: self.group,
                db: Arc::clone(&self.db),
                state: self.state.clone(),
                shared: Arc::clone(&self.shared),
                applied_tx: Arc::clone(&self.applied_tx),
                snapshots_built: self.snapshots_built + 1,
                receiving: None,
            },
        }
    }

    async fn begin_receiving_snapshot(&mut self) -> Result<Box<tokio::fs::File>, StorageError> {
        let path = self.snapshot_path(&format!("g{}-recv-{}.snap", self.group, hex_id()));
        if let Some(old) = self.receiving.replace(path.clone()) {
            let _removed = std::fs::remove_file(old);
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(io)?;
        Ok(Box::new(tokio::fs::File::from_std(file)))
    }

    async fn install_snapshot(
        &mut self,
        meta: &SnapshotMeta,
        snapshot: Box<tokio::fs::File>,
    ) -> Result<(), StorageError> {
        {
            use tokio::io::AsyncWriteExt;
            let mut file = snapshot;
            file.flush().await.map_err(io)?;
            file.sync_all().await.map_err(io)?;
        }
        self.install(meta).map_err(io)?;
        let _ = self
            .applied_tx
            .send(meta.last_log_id.map_or(0, |l| l.index));
        Ok(())
    }

    async fn get_current_snapshot(&mut self) -> Result<Option<Snapshot<TypeConfig>>, StorageError> {
        let Some(s) = self
            .db
            .get_meta::<SnapshotRecord>(self.group, "snapshot")
            .map_err(io)?
        else {
            return Ok(None);
        };
        let Ok(file) = std::fs::File::open(self.snapshot_path(&s.file)) else {
            return Ok(None);
        };
        Ok(Some(Snapshot {
            meta: s.meta,
            snapshot: Box::new(tokio::fs::File::from_std(file)),
        }))
    }
}

/// The file of the snapshot a group serves, and the index it is at.
pub(crate) fn current_snapshot_file(
    db: &Db,
    group: GroupId,
) -> eyre::Result<Option<(u64, String, PathBuf)>> {
    let Some(s) = db.get_meta::<SnapshotRecord>(group, "snapshot")? else {
        return Ok(None);
    };
    let path = db.snapshot_dir.join(&s.file);
    Ok(path.exists().then(|| {
        (
            s.meta.last_log_id.map_or(0, |l| l.index),
            s.meta.snapshot_id,
            path,
        )
    }))
}

/// Read a snapshot file this cluster wrote, with its storage key alone:
/// `f(machine key, opened value)` for every row of the group's state.
pub fn read_snapshot_file(
    storage_key: &[u8; 32],
    group: GroupId,
    snapshot_id: &str,
    path: &std::path::Path,
    mut f: impl FnMut(&[u8], &[u8]) -> eyre::Result<()>,
) -> eyre::Result<()> {
    let keys = crate::store::SnapshotKeys::new(storage_key);
    read_frames(
        &keys.frames,
        group,
        snapshot_id,
        std::fs::File::open(path)?,
        |k, sealed| {
            let plain = keys.open_row(group, k, sealed)?;
            f(k, &plain)
        },
    )
}
