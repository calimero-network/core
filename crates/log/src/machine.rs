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

use std::collections::VecDeque;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

use openraft::storage::RaftStateMachine;
use openraft::{
    AnyError, EntryPayload, OptionalSend, RaftSnapshotBuilder, Snapshot, StorageIOError,
};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::digest::{DigestAt, StateDigest, DIGEST_EVERY, DIGEST_KEPT};
use crate::store::{load_generation, off_workers, Db, GroupState, StateTx, Transaction};
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
    ///
    /// An error fails the snapshot build (or the import that checkpoints it), which
    /// stops the group where it happened: a checkpoint or snapshot of no bytes would
    /// otherwise be served to lagging nodes and learners, and fail on them instead,
    /// far from the cause.
    fn snapshot(&self) -> eyre::Result<Vec<u8>> {
        Ok(Vec::new())
    }

    /// An in-memory machine: replace the whole state with `data`, or with the
    /// state it starts with when `data` is empty (its group stopped before its
    /// first checkpoint). Otherwise `data` is empty and the machine re-reads
    /// everything it holds from its [`GroupState`] (after a restart, a snapshot
    /// install, or its group's Raft core stopping, when what it applied in
    /// memory may be ahead of the disk).
    fn restore(&mut self, data: &[u8]) -> eyre::Result<()>;
}

/// The applied state of a group, shared between the Raft core and readers.
pub struct Applied<M> {
    pub machine: M,
    pub last_applied: Option<LogId>,
    pub membership: StoredMembership,
    /// The digest of a disk machine's state rows at `last_applied`
    /// ([`crate::digest`]); the empty digest for an in-memory machine.
    pub digest: StateDigest,
    /// Its digests at the latest indexes that are multiples of
    /// [`crate::digest::DIGEST_EVERY`] this node applied, oldest first.
    pub recorded: VecDeque<DigestAt>,
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

/// The group meta row the state digest at the applied index lives in
/// ([`DigestAt`]), committed with every batch that moves it.
pub(crate) const DIGEST_META: &str = "digest";

/// Keep `at` among the recorded digests, at most [`DIGEST_KEPT`] of them.
fn record(recorded: &mut VecDeque<DigestAt>, at: DigestAt) {
    recorded.push_back(at);
    while recorded.len() > DIGEST_KEPT {
        let _oldest = recorded.pop_front();
    }
}

/// The digest of a group generation's state rows, taken from all of them: for a
/// state whose stored digest is missing or stale.
fn digest_of_rows(db: &Db, group: GroupId, generation: u32) -> eyre::Result<StateDigest> {
    let mut d = StateDigest::default();
    db.scan_state(group, generation, &[], |k, v| {
        d = d.plus(StateDigest::of_row(k, v));
        Ok(true)
    })?;
    Ok(d)
}

/// The state row an in-memory machine's checkpoint lives in (and so where a
/// snapshot file holds that machine's whole state).
pub const CHECKPOINT_KEY: &[u8] = b"\0calimero.log.checkpoint";

/// The newest snapshot file format this release reads, and the one it writes when a row spans
/// frames: the number in the file's first line, `CALIMERO-SNAPSHOT-<format>\n`. Format 1 (every
/// release since the store, backups included) holds each row in one frame, so a row past
/// [`MAX_FRAME_BYTES`] (an in-memory machine's checkpoint: the whole system state) could not be
/// read back. Format 2 lets a row span frames ([`PIECE`]). A file with no such row is written
/// as format 1 ([`WHOLE_ROWS_FORMAT`]), which the release before reads: a rollout never sends
/// a node a format it cannot read that it read before. A change to the framing, or to the rows
/// a snapshot carries (`format::STORE_FORMAT`), raises it.
pub const SNAPSHOT_FORMAT: u32 = 2;
/// The format a snapshot file with every row in one frame says: format 1, as before rows
/// could span frames.
const WHOLE_ROWS_FORMAT: u32 = 1;
// The first line is written before the rows and patched to [`SNAPSHOT_FORMAT`] in place when a
// row was split: both numbers take one digit.
const _: () = assert!(WHOLE_ROWS_FORMAT < 10 && SNAPSHOT_FORMAT < 10);
/// In format 2, a row entry whose value length has this bit set is a piece of the row: the
/// next entry, under the same key, continues it. Format 1 never sets it (a frame's values are
/// far below 2 GiB).
const PIECE: u32 = 1 << 31;
/// The first line of a snapshot file, before its format number.
const SNAPSHOT_MAGIC_PREFIX: &[u8] = b"CALIMERO-SNAPSHOT-";
/// The longest format number a first line may carry (a `u32`).
const MAX_FORMAT_DIGITS: usize = 10;
/// Rows per frame stop growing past this many plaintext bytes.
const FRAME_BYTES: usize = 1024 * 1024;
/// A frame is refused past this size when read back (smaller in this crate's tests, so a
/// state past it is cheap to build).
const MAX_FRAME_BYTES: usize = if cfg!(test) {
    4 * 1024 * 1024
} else {
    64 * 1024 * 1024
};
/// A row (key, value and their lengths) larger than this is written in pieces of
/// [`FRAME_BYTES`] across frames (format 2): whole, its frame could pass [`MAX_FRAME_BYTES`]
/// once sealed.
const MAX_WHOLE_ROW_BYTES: usize = MAX_FRAME_BYTES - FRAME_BYTES;
/// Rows written per batch while installing a snapshot.
const INSTALL_BATCH_BYTES: usize = 4 * 1024 * 1024;
/// Builds that may be thrown away because an install or a replace switched
/// the group's state while their file was written, before a build holds off
/// installs and replaces for its whole length ([`StateMachine::build`]).
const BUILDS_BEFORE_HOLDING_INSTALLS: usize = 2;

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
    /// A row was written in pieces: the file is format 2.
    split: bool,
}

impl FrameWriter<'_> {
    fn row(&mut self, key: &[u8], sealed: &[u8]) -> eyre::Result<()> {
        let size = 8 + key.len() + sealed.len();
        // A large row starts a frame of its own, so a whole one stays under the cap.
        if size > FRAME_BYTES && !self.buf.is_empty() {
            self.flush(false)?;
        }
        if size <= MAX_WHOLE_ROW_BYTES {
            return self.entry(key, sealed, false);
        }
        self.split = true;
        let mut pieces = sealed.chunks(FRAME_BYTES).peekable();
        while let Some(piece) = pieces.next() {
            self.entry(key, piece, pieces.peek().is_some())?;
        }
        Ok(())
    }

    /// One entry: a whole row, or a piece of one that the next entry continues (`more`).
    fn entry(&mut self, key: &[u8], value: &[u8], more: bool) -> eyre::Result<()> {
        put_u32(&mut self.buf, key.len());
        self.buf.extend_from_slice(key);
        let len = u32::try_from(value.len())?;
        self.buf
            .extend_from_slice(&(if more { len | PIECE } else { len }).to_be_bytes());
        self.buf.extend_from_slice(value);
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
        let mut file = self.out.into_inner().map_err(|e| eyre::eyre!("{e}"))?;
        if self.split {
            file.seek(SeekFrom::Start(SNAPSHOT_MAGIC_PREFIX.len() as u64))?;
            file.write_all(SNAPSHOT_FORMAT.to_string().as_bytes())?;
        }
        file.sync_all()?;
        Ok(())
    }
}

/// The first line of a snapshot file this release writes, until a row is split
/// ([`FrameWriter::finish`] then makes it [`SNAPSHOT_FORMAT`]).
fn first_line() -> Vec<u8> {
    [
        SNAPSHOT_MAGIC_PREFIX,
        format!("{WHOLE_ROWS_FORMAT}\n").as_bytes(),
    ]
    .concat()
}

/// The first line of a snapshot file: its format number. Anything else is not a snapshot file.
fn read_format(r: &mut impl Read) -> eyre::Result<u32> {
    let not_a_snapshot = || eyre::eyre!("not a snapshot file");
    let mut prefix = [0; SNAPSHOT_MAGIC_PREFIX.len()];
    r.read_exact(&mut prefix).map_err(|_| not_a_snapshot())?;
    if prefix != SNAPSHOT_MAGIC_PREFIX {
        return Err(not_a_snapshot());
    }
    let mut digits = String::new();
    loop {
        let mut b = [0_u8; 1];
        r.read_exact(&mut b).map_err(|_| not_a_snapshot())?;
        match b[0] {
            b'\n' if !digits.is_empty() => break,
            d @ b'0'..=b'9' if digits.len() < MAX_FORMAT_DIGITS => digits.push(char::from(d)),
            _ => return Err(not_a_snapshot()),
        }
    }
    digits.parse().map_err(|_| not_a_snapshot())
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
    let format = read_format(&mut r)?;
    if format > SNAPSHOT_FORMAT {
        eyre::bail!(
            "snapshot file format {format}, which a later release wrote; this release reads \
             snapshot format {SNAPSHOT_FORMAT}"
        );
    }
    if format == 0 {
        eyre::bail!("snapshot file format {format}, which no release wrote");
    }
    let pieces = format >= 2;
    // The row whose pieces are being read: its key and its value so far.
    let mut pending: Option<(Vec<u8>, Vec<u8>)> = None;
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
            let raw = u32::from_be_bytes(take(&mut buf, 4)?.try_into()?);
            let more = pieces && raw & PIECE != 0;
            let vlen = if pieces { raw & !PIECE } else { raw };
            let value = take(&mut buf, vlen as usize)?;
            match pending.take() {
                None if !more => f(key, value)?,
                None => pending = Some((key.to_vec(), value.to_vec())),
                Some((k, mut v)) => {
                    if k != key {
                        eyre::bail!("a snapshot row's pieces name different keys");
                    }
                    v.extend_from_slice(value);
                    if more {
                        pending = Some((k, v));
                    } else {
                        f(&k, &v)?;
                    }
                }
            }
        }
        if !buf.is_empty() {
            eyre::bail!("snapshot frame has trailing bytes");
        }
        n += 1;
        if last {
            if pending.is_some() {
                eyre::bail!("snapshot ends inside a row");
            }
            let mut rest = [0_u8; 1];
            if r.read(&mut rest)? != 0 {
                eyre::bail!("snapshot has bytes past its last frame");
            }
            return Ok(());
        }
    }
}

/// Read a group's applied state from the disk into `st`: the index and
/// membership its last committed batch (or checkpoint) recorded, and the
/// machine's state as of then.
fn load<M: Machine>(db: &Db, state: &GroupState, st: &mut Applied<M>) -> eyre::Result<()> {
    let applied: AppliedMeta = db.get_meta(state.group(), "applied")?.unwrap_or_default();
    if M::IN_MEMORY {
        st.machine
            .restore(&state.get(CHECKPOINT_KEY).unwrap_or_default())?;
    } else {
        st.machine.restore(&[])?;
    }
    st.last_applied = applied.last_applied;
    st.membership = applied.membership;
    // What was recorded in memory may be of batches that never committed.
    st.recorded.clear();
    st.digest = if M::IN_MEMORY {
        StateDigest::default()
    } else {
        let group = state.group();
        let index = st.applied_index();
        match db.get_meta::<DigestAt>(group, DIGEST_META)? {
            Some(d) if d.index == index => d.digest,
            stored => {
                // Rows changed outside the log (`StateTx::commit`), or kept by a
                // release before the digest: taken from the rows once.
                let d = digest_of_rows(db, group, state.generation.load(Ordering::Acquire))?;
                tracing::info!(
                    group,
                    index,
                    had = stored.is_some(),
                    "the state digest was taken from the rows"
                );
                d
            }
        }
    };
    Ok(())
}

/// Drop what a group applied in memory and read its state from the disk
/// again: after its Raft core stopped, memory may hold a batch that never
/// committed, or half an entry a panicking machine left. The group then
/// serves (and its next core builds on) only what the disk holds, as a
/// replica that lags would, and its applied index says so: a read that waits
/// for an index ([`crate::Group::wait_applied`]) waits for the new core to
/// apply it again.
pub(crate) fn reload<M: Machine>(
    db: &Db,
    state: &GroupState,
    st: &mut Applied<M>,
    applied_tx: &watch::Sender<u64>,
) -> eyre::Result<()> {
    let generation = load_generation(db, state.group())?;
    state.generation.store(generation, Ordering::Release);
    load(db, state, st)?;
    let _previous = applied_tx.send_replace(st.applied_index());
    Ok(())
}

/// What a panic in a machine's apply carried, for the log.
fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic".to_owned())
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

impl<M: Machine> Clone for StateMachine<M> {
    /// Another handle on the same group state, to run disk work on tokio's
    /// blocking pool ([`crate::store::off_workers`]).
    fn clone(&self) -> Self {
        Self {
            group: self.group,
            db: Arc::clone(&self.db),
            state: self.state.clone(),
            shared: Arc::clone(&self.shared),
            applied_tx: Arc::clone(&self.applied_tx),
            snapshots_built: self.snapshots_built,
            receiving: self.receiving.clone(),
        }
    }
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
        let state = GroupState::new(Arc::clone(&db), group, generation);
        machine.attach(state.clone());
        let mut applied = Applied {
            machine,
            last_applied: None,
            membership: StoredMembership::default(),
            digest: StateDigest::default(),
            recorded: VecDeque::new(),
        };
        load(&db, &state, &mut applied)?;
        let _ = applied_tx.send(applied.applied_index());
        Ok(Self::reopened(
            group,
            db,
            state,
            Arc::new(RwLock::new(applied)),
            applied_tx,
        ))
    }

    /// The state machine of a group whose applied state `shared` already holds
    /// what the disk does ([`reload`]): what a stopped group's new Raft core
    /// runs on.
    pub(crate) fn reopened(
        group: GroupId,
        db: Arc<Db>,
        state: GroupState,
        shared: Shared<M>,
        applied_tx: Arc<watch::Sender<u64>>,
    ) -> Self {
        Self {
            group,
            db,
            state,
            shared,
            applied_tx,
            snapshots_built: 0,
            receiving: None,
        }
    }

    /// Apply `entries` and commit their writes with the index they bring the
    /// state to: the outputs, and that index (0: none). A machine that panics
    /// fails the batch, as a write the disk refused does.
    fn apply_batch(
        &self,
        st: &mut Applied<M>,
        entries: impl IntoIterator<Item = Entry>,
    ) -> Result<(Vec<Bytes>, u64), StorageError> {
        let mut out = Vec::new();
        let mut last = 0;
        let mut tx = StateTx::new(&self.state);
        let mut recorded = Vec::new();
        for entry in entries {
            st.last_applied = Some(entry.log_id);
            last = entry.log_id.index;
            match entry.payload {
                EntryPayload::Blank => out.push(Bytes::default()),
                EntryPayload::Normal(cmd) => {
                    let machine = &mut st.machine;
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        machine.apply(entry.log_id.index, &cmd.0, &mut tx)
                    }))
                    .map_err(|p| {
                        io(format!(
                            "the machine panicked applying entry {last}: {}",
                            panic_message(p.as_ref())
                        ))
                    })?;
                    out.push(Bytes(r));
                }
                EntryPayload::Membership(m) => {
                    st.membership = StoredMembership::new(Some(entry.log_id), m);
                    out.push(Bytes::default());
                }
            }
            // A machine that could not apply an entry (one it cannot decode,
            // a row it cannot read) stops the group there: applying what
            // follows would build on a state the other nodes do not have.
            if tx.failed() {
                break;
            }
            // Each entry's digest, wherever the batches split: replicas meet
            // at the same indexes.
            if !M::IN_MEMORY && last.is_multiple_of(DIGEST_EVERY) {
                recorded.push(DigestAt {
                    index: last,
                    digest: st.digest.plus(tx.digest_delta()),
                });
            }
        }
        let digest = st.digest.plus(tx.digest_delta());
        // The batch's writes and the index they bring the state to land
        // together; readers, held off by the lock, see neither until then.
        let mut batch = Transaction::default();
        tx.into_batch(&mut batch).map_err(io)?;
        if !M::IN_MEMORY && last > 0 {
            self.db
                .stage_meta(
                    &mut batch,
                    self.group,
                    DIGEST_META,
                    &DigestAt {
                        index: last,
                        digest,
                    },
                )
                .map_err(io)?;
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
        #[cfg(feature = "testing")]
        if self.db.apply_fails(self.group) {
            return Err(io("an injected write error (testing::fail_applies)"));
        }
        // Not synced: the entries are in the synced log, so a lost batch is
        // re-applied from there.
        if !self.db.losing_unsynced(self.group) {
            self.db.commit(&batch, false).map_err(io)?;
        }
        if !M::IN_MEMORY {
            st.digest = digest;
            for at in recorded {
                record(&mut st.recorded, at);
            }
        }
        Ok((out, last))
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

    /// Build a snapshot of the applied state and record it as the one the
    /// group serves.
    ///
    /// openraft runs a build beside the installs and applies of its state
    /// machine worker, so an install (or a replace) can switch the group's
    /// state while a build writes its file. A build whose generation was
    /// switched meanwhile is of a state the group no longer holds: it is
    /// thrown away and built again, and the record the install left stands.
    /// After [`BUILDS_BEFORE_HOLDING_INSTALLS`] such builds, the next one holds
    /// off installs and replaces while it runs. Never an error for this:
    /// openraft treats a failed build as fatal to the group's Raft core.
    fn build(&mut self) -> eyre::Result<Snapshot<TypeConfig>> {
        for _ in 0..BUILDS_BEFORE_HOLDING_INSTALLS {
            let (record, generation) = self.write_snapshot()?;
            #[cfg(feature = "testing")]
            self.db
                .pause_build(self.group, crate::testing::BuildPoint::Record);
            let held = self.state.lock_snapshot_record();
            self.refuse_if_removed(&record.file)?;
            if self.state.generation.load(Ordering::Acquire) == generation {
                return self.serve_built(record);
            }
            drop(held);
            let _removed = std::fs::remove_file(self.snapshot_path(&record.file));
        }
        // Installs or replaces keep switching the state under the builds: this
        // one holds them off (and the applies queued behind an install on
        // openraft's worker) until it is recorded.
        let state = self.state.clone();
        let _held = state.lock_snapshot_record();
        let (record, _) = self.write_snapshot()?;
        self.serve_built(record)
    }

    /// Write a snapshot file of the applied state: its record, and the state
    /// generation it holds the rows of.
    fn write_snapshot(&mut self) -> eyre::Result<(SnapshotRecord, u32)> {
        self.snapshots_built += 1;
        #[cfg(feature = "testing")]
        self.db
            .pause_build(self.group, crate::testing::BuildPoint::Read);
        // Under the read lock no batch applies and no install or replace
        // switches the generation (each does under the write lock), so the
        // generation, the meta below and the view the scan pins are one state.
        // The scan's view is pinned when it opens, before the guard drops, so
        // the lock is not held while the file is written, and an install that
        // then drops this generation's rows does not take them from the view.
        let st = self.shared.read().map_err(|e| eyre::eyre!("{e}"))?;
        // Removed meanwhile: its rows and files are gone, and nothing (an
        // in-memory machine's checkpoint below, a file) is written again.
        // Under the read lock, so the removal cannot happen between this and
        // the checkpoint.
        self.refuse_if_removed("")?;
        let generation = self.state.generation.load(Ordering::Acquire);
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
                Some(&st.machine.snapshot().map_err(|e| {
                    tracing::error!(group = self.group, error = %e,
                        "the machine could not serialize its state: no snapshot is built");
                    e
                })?),
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
        out.write_all(&first_line())?;
        let mut w = FrameWriter {
            db: &self.db,
            group: self.group,
            id: meta.snapshot_id.clone(),
            out,
            buf: Vec::new(),
            rows: 0,
            n: 0,
            split: false,
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
            // Stored as it is; sealed in the file, which may leave this disk.
            let sealed = self.db.seal_state(self.group, key, v)?;
            w.row(key, &sealed)?;
            Ok(true)
        })?;
        drop(guard);
        w.finish()?;
        Ok((SnapshotRecord { meta, file: name }, generation))
    }

    /// Fail the build of a group the engine removed (`Engine::remove_group`),
    /// deleting the file it wrote (`file`, if any) so nothing of the group is
    /// left. The caller holds the snapshot-record lock or the applied-state
    /// lock, which the removal takes both of.
    fn refuse_if_removed(&self, file: &str) -> eyre::Result<()> {
        if !self.state.removed.load(Ordering::Acquire) {
            return Ok(());
        }
        if !file.is_empty() {
            let _removed = std::fs::remove_file(self.snapshot_path(file));
        }
        eyre::bail!("group {} was removed: no snapshot is built", self.group)
    }

    /// Record a snapshot this node built as the one the group serves, and
    /// hand it to openraft. The caller holds the snapshot-record lock.
    fn serve_built(&self, record: SnapshotRecord) -> eyre::Result<Snapshot<TypeConfig>> {
        // Opened first: a record that is not kept has its file deleted, and
        // openraft reads only the meta of a built snapshot.
        let file = std::fs::File::open(self.snapshot_path(&record.file))?;
        let meta = record.meta.clone();
        self.keep_snapshot(record)?;
        Ok(Snapshot {
            meta,
            snapshot: Box::new(tokio::fs::File::from_std(file)),
        })
    }

    /// Record `record` as the snapshot this group serves, and delete the one
    /// it replaces. A record no newer than the one served is not kept and its
    /// file is deleted instead: the group already serves that state or a
    /// later one (an install's, for a build that ended after it), unless its
    /// file is gone. The caller holds the snapshot-record lock.
    fn keep_snapshot(&self, record: SnapshotRecord) -> eyre::Result<()> {
        let previous: Option<SnapshotRecord> = self.db.get_meta(self.group, "snapshot")?;
        if let Some(p) = &previous {
            if p.file != record.file
                && p.meta.last_log_id >= record.meta.last_log_id
                && self.snapshot_path(&p.file).exists()
            {
                let _removed = std::fs::remove_file(self.snapshot_path(&record.file));
                return Ok(());
            }
        }
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
        let mut digest = StateDigest::default();
        let read = read_frames(
            &db.snapshot_sealer,
            group,
            &meta.snapshot_id,
            file,
            |key, sealed| {
                let plain = db.open_state(group, key, sealed)?;
                digest = digest.plus(StateDigest::of_row(key, &plain));
                db.stage_state(&mut batch, group, next, key, Some(&plain))?;
                pending += key.len() + plain.len();
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
        // A build that read the old generation sees the switch when it comes
        // to record its file, and never records over the snapshot kept here.
        let _held = self.state.lock_snapshot_record();
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
            let at = DigestAt {
                index: meta.last_log_id.map_or(0, |l| l.index),
                digest,
            };
            if !M::IN_MEMORY {
                db.stage_meta(&mut switch, group, DIGEST_META, &at)?;
            }
            db.commit(&switch, true)?;
            self.state.generation.store(next, Ordering::Release);
            // A state that came by snapshot, not by the entries recorded before.
            st.recorded.clear();
            if !M::IN_MEMORY {
                st.digest = digest;
                if at.index.is_multiple_of(DIGEST_EVERY) {
                    record(&mut st.recorded, at);
                }
            }
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

/// Delete the files in the snapshot directory that no group serves: a
/// snapshot being received (`g<group>-recv-*.snap`) or built when the node
/// stopped, one whose record a crash kept from being replaced or deleted, and
/// those of a group whose rows are gone. Runs as the engine opens, before any
/// group starts, so no receive or build is under way. A file whose group's
/// record cannot be read is kept, and anything not named like a snapshot is
/// left alone.
pub(crate) fn clean_snapshot_dir(db: &Db) -> eyre::Result<()> {
    for entry in std::fs::read_dir(&db.snapshot_dir)?.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(group) = name
            .strip_suffix(".snap")
            .and_then(|n| n.strip_prefix('g'))
            .and_then(|n| n.split_once('-'))
            .and_then(|(g, _)| g.parse::<GroupId>().ok())
        else {
            continue;
        };
        let served = match db.get_meta::<SnapshotRecord>(group, "snapshot") {
            Ok(record) => record.is_some_and(|r| r.file == name),
            Err(e) => {
                tracing::warn!(group, file = %name, error = %e,
                    "the group's snapshot record does not read: its file is kept");
                continue;
            }
        };
        if !served {
            tracing::info!(group, file = %name, "deleting a snapshot file no group serves");
            let _removed = std::fs::remove_file(entry.path());
        }
    }
    Ok(())
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
        // The file is written on the blocking pool, not on an async worker.
        let mut inner = self.inner.clone();
        let (built, inner) = off_workers(move || Ok((inner.build().map_err(io), inner))).await?;
        self.inner = inner;
        built
    }
}

impl<M: Machine> RaftStateMachine<TypeConfig> for StateMachine<M> {
    type SnapshotBuilder = Builder<M>;

    async fn applied_state(&mut self) -> Result<(Option<LogId>, StoredMembership), StorageError> {
        // A std lock: never waited for on an async worker.
        let shared = Arc::clone(&self.shared);
        off_workers(move || {
            let st = shared.read().map_err(io)?;
            Ok((st.last_applied, st.membership.clone()))
        })
        .await
    }

    async fn apply<I>(&mut self, entries: I) -> Result<Vec<Bytes>, StorageError>
    where
        I: IntoIterator<Item = Entry> + OptionalSend,
        I::IntoIter: OptionalSend,
    {
        #[cfg(feature = "testing")]
        while self.db.apply_held(self.group) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        // On the blocking pool: the write lock waits for readers (a proposal
        // built under the read lock may run a WASM call), and the machine and
        // the commit may take long; neither may hold an async worker.
        let entries: Vec<Entry> = entries.into_iter().collect();
        let sm = self.clone();
        let (out, last) = off_workers(move || {
            let mut st = sm.shared.write().map_err(io)?;
            sm.apply_batch(&mut st, entries).inspect_err(|e| {
                // The core stops on this error ([`crate::Group`] restarts
                // it); until then nothing may read what never committed.
                tracing::error!(group = sm.group, error = %e,
                    "a batch did not apply: this group's Raft core stops, and restarts from the disk");
                if let Err(r) = reload(&sm.db, &sm.state, &mut st, &sm.applied_tx) {
                    tracing::error!(group = sm.group, error = %r,
                        "the group's state could not be read again from the disk");
                }
            })
        })
        .await?;
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
        // Reads the file and writes the next generation on the blocking pool.
        let mut sm = self.clone();
        self.receiving = None;
        let owned = meta.clone();
        off_workers(move || sm.install(&owned).map_err(io)).await?;
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

/// Replace a disk machine's state with the rows of a snapshot file this
/// cluster wrote for group `from` (a backup), keeping where the group stands
/// in its log. Each row is opened and sealed again for the group, and a hidden
/// key is hidden again under it, so a backup of one group seeds another. The
/// rows go to the next generation, which the group switches to in one batch,
/// as a snapshot install does; the snapshot the group served is dropped, as it
/// holds the state before. Answers how many rows it wrote.
///
/// Only the group's sole voter may do this: every other replica would keep
/// the state before (`calimero_log::Group::replace_state`).
pub(crate) fn replace_state<M: Machine>(
    state: &GroupState,
    shared: &Shared<M>,
    from: GroupId,
    snapshot_id: &str,
    path: &std::path::Path,
) -> eyre::Result<u64> {
    if M::IN_MEMORY {
        eyre::bail!("an in-memory machine's state changes only through its log");
    }
    let db = &state.db;
    let group = state.group;
    let old = state.generation.load(Ordering::Acquire);
    let next = old.wrapping_add(1);
    db.drop_generation(group, next)?;
    let mut batch = Transaction::default();
    let mut pending = 0_usize;
    let mut rows = 0_u64;
    let mut digest = StateDigest::default();
    let read = read_frames(
        &db.snapshot_sealer,
        from,
        snapshot_id,
        std::fs::File::open(path)?,
        |key, sealed| {
            let plain = db.open_state(from, key, sealed)?;
            let key = rehide(db, from, group, key, &plain);
            digest = digest.plus(StateDigest::of_row(&key, &plain));
            db.stage_state(&mut batch, group, next, &key, Some(&plain))?;
            pending += key.len() + plain.len();
            rows += 1;
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
        return Err(e);
    }
    // A build that read the old generation sees the switch when it comes to
    // record its file, and never brings back the snapshot dropped here.
    let _held = state.lock_snapshot_record();
    {
        let mut st = shared.write().map_err(|e| eyre::eyre!("{e}"))?;
        let mut switch = Transaction::default();
        db.stage_meta(&mut switch, group, "generation", &next)?;
        let at = DigestAt {
            index: st.applied_index(),
            digest,
        };
        db.stage_meta(&mut switch, group, DIGEST_META, &at)?;
        db.commit(&switch, true)?;
        state.generation.store(next, Ordering::Release);
        st.machine.restore(&[])?;
        // Another state at the same index: what was recorded before no longer holds.
        st.digest = digest;
        st.recorded.clear();
    }
    db.drop_generation(group, old)?;
    if let Some(served) = db.get_meta::<SnapshotRecord>(group, "snapshot")? {
        db.drop_meta(group, "snapshot")?;
        let _removed = std::fs::remove_file(db.snapshot_dir.join(&served.file));
    }
    Ok(rows)
}

/// Replace an in-memory machine's whole state with `data` (what
/// [`Machine::snapshot`] answers), keeping where the group stands in its log:
/// the caller then builds a snapshot, which checkpoints it. Only the group's
/// sole voter may do this (`calimero_log::Group::replace_machine_data`).
pub(crate) fn replace_machine_data<M: Machine>(
    shared: &Shared<M>,
    data: &[u8],
) -> eyre::Result<()> {
    if !M::IN_MEMORY {
        eyre::bail!("a disk machine's state is replaced from a snapshot file");
    }
    let mut st = shared.write().map_err(|e| eyre::eyre!("{e}"))?;
    st.machine.restore(data)
}

/// Where a row of group `from` lives in group `to`: a hidden key (its row is an
/// HMAC over the group, the tag and the key, [`crate::store::hide`]) is hidden
/// again under `to`; any other key stays as it is.
fn rehide(db: &Db, from: GroupId, to: GroupId, key: &[u8], plain: &[u8]) -> Vec<u8> {
    use crate::store::{hide, unwrap_hidden, HIDDEN_LEN};
    let (Some(tag), Some((inner, _))) = (key.first(), unwrap_hidden(plain)) else {
        return key.to_vec();
    };
    if from == to || key.len() != HIDDEN_LEN || hide(&db.hider, from, *tag, inner) != key {
        return key.to_vec();
    }
    hide(&db.hider, to, *tag, inner)
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

/// A state row as any cluster can take it: a hidden row ([`crate::store::hide`])
/// by the key it hides, so another cluster's key can hide it again; any other
/// row by its machine key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortableRow {
    Plain {
        key: Vec<u8>,
        value: Vec<u8>,
    },
    Hidden {
        tag: u8,
        key: Vec<u8>,
        value: Vec<u8>,
    },
}

/// [`read_snapshot_file`], each row as a [`PortableRow`]: what a backup that
/// does not depend on this cluster's key holds.
pub fn read_snapshot_rows(
    storage_key: &[u8; 32],
    group: GroupId,
    snapshot_id: &str,
    path: &std::path::Path,
    mut f: impl FnMut(PortableRow) -> eyre::Result<()>,
) -> eyre::Result<()> {
    let hider = ring::hmac::Key::new(
        ring::hmac::HMAC_SHA256,
        &crate::store::derive(storage_key, HIDDEN_KEYS_LABEL),
    );
    read_snapshot_file(storage_key, group, snapshot_id, path, |k, plain| {
        f(portable_row(&hider, group, k, plain))
    })
}

/// The label the hider key is derived under (`Db::open`).
pub(crate) const HIDDEN_KEYS_LABEL: &str = "calimero.log.hidden-keys.v1";

fn portable_row(hider: &ring::hmac::Key, group: GroupId, key: &[u8], plain: &[u8]) -> PortableRow {
    use crate::store::{hide, unwrap_hidden, HIDDEN_LEN};
    if let (Some(tag), Some((inner, value))) = (key.first(), unwrap_hidden(plain)) {
        if key.len() == HIDDEN_LEN && hide(hider, group, *tag, inner) == key {
            return PortableRow::Hidden {
                tag: *tag,
                key: inner.to_vec(),
                value: value.to_vec(),
            };
        }
    }
    PortableRow::Plain {
        key: key.to_vec(),
        value: plain.to_vec(),
    }
}

/// [`replace_state`] from portable rows (a recovery backup's): each hidden
/// row hidden again under this node's key, for this group.
pub(crate) fn replace_state_rows<M: Machine>(
    state: &GroupState,
    shared: &Shared<M>,
    rows: impl FnOnce(&mut dyn FnMut(PortableRow) -> eyre::Result<()>) -> eyre::Result<()>,
) -> eyre::Result<u64> {
    use crate::store::{hide, wrap_hidden};
    if M::IN_MEMORY {
        eyre::bail!("an in-memory machine's state changes only through its log");
    }
    let db = &state.db;
    let group = state.group;
    let old = state.generation.load(Ordering::Acquire);
    let next = old.wrapping_add(1);
    db.drop_generation(group, next)?;
    let mut batch = Transaction::default();
    let mut pending = 0_usize;
    let mut count = 0_u64;
    let mut digest = StateDigest::default();
    let mut stage = |row: PortableRow| -> eyre::Result<()> {
        let (key, value) = match row {
            PortableRow::Plain { key, value } => (key, value),
            PortableRow::Hidden { tag, key, value } => {
                (hide(&db.hider, group, tag, &key), wrap_hidden(&key, &value))
            }
        };
        digest = digest.plus(StateDigest::of_row(&key, &value));
        db.stage_state(&mut batch, group, next, &key, Some(&value))?;
        pending += key.len() + value.len();
        count += 1;
        if pending >= INSTALL_BATCH_BYTES {
            db.commit(&batch, false)?;
            batch = Transaction::default();
            pending = 0;
        }
        Ok(())
    };
    let staged = rows(&mut stage).and_then(|()| db.commit(&batch, false));
    if let Err(e) = staged {
        let _ = db.drop_generation(group, next);
        return Err(e);
    }
    // A build that read the old generation sees the switch when it comes to
    // record its file, and never brings back the snapshot dropped here.
    let _held = state.lock_snapshot_record();
    {
        let mut st = shared.write().map_err(|e| eyre::eyre!("{e}"))?;
        let mut switch = Transaction::default();
        db.stage_meta(&mut switch, group, "generation", &next)?;
        let at = DigestAt {
            index: st.applied_index(),
            digest,
        };
        db.stage_meta(&mut switch, group, DIGEST_META, &at)?;
        db.commit(&switch, true)?;
        state.generation.store(next, Ordering::Release);
        st.machine.restore(&[])?;
        // Another state at the same index: what was recorded before no longer holds.
        st.digest = digest;
        st.recorded.clear();
    }
    db.drop_generation(group, old)?;
    if let Some(served) = db.get_meta::<SnapshotRecord>(group, "snapshot")? {
        db.drop_meta(group, "snapshot")?;
        let _removed = std::fs::remove_file(db.snapshot_dir.join(&served.file));
    }
    Ok(count)
}

#[cfg(all(test, feature = "testing"))]
mod build_races;

#[cfg(test)]
mod snapshot_format {
    use super::*;

    fn read_header(bytes: &[u8]) -> eyre::Result<u32> {
        read_format(&mut &bytes[..])
    }

    /// Every snapshot and backup written so far begins `CALIMERO-SNAPSHOT-1\n`.
    #[test]
    fn the_first_line_written_so_far_reads_as_format_1() {
        assert_eq!(read_header(b"CALIMERO-SNAPSHOT-1\n\x01").unwrap(), 1);
        assert_eq!(
            first_line(),
            b"CALIMERO-SNAPSHOT-1\n",
            "what this release writes, a node of the release before reads"
        );
    }

    #[test]
    fn anything_else_is_not_a_snapshot() {
        for bad in [
            &b""[..],
            b"CALIMERO-SNAPSHOT-",
            b"CALIMERO-SNAPSHOT-\n",
            b"CALIMERO-SNAPSHOT-1",
            b"CALIMERO-SNAPSHOT-x\n",
            b"CALIMERO-SNAPSHOT-12345678901\n",
            b"calimero-snapshot-1\n",
        ] {
            let e = read_header(bad).unwrap_err().to_string();
            assert_eq!(e, "not a snapshot file", "{bad:?}");
        }
    }

    /// A node on this release that is sent (or restores) a snapshot from a later one says so,
    /// instead of failing on its frames as if the file were damaged.
    #[test]
    fn a_snapshot_from_a_later_release_is_refused_by_its_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g1-x.snap");
        std::fs::write(&path, b"CALIMERO-SNAPSHOT-3\n\x01\x00\x00\x00\x00").unwrap();
        let e = read_snapshot_file(&[0; 32], 1, "x", &path, |_, _| Ok(()))
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("format 3") && e.contains("snapshot format 2"),
            "{e}"
        );
    }
}

#[cfg(test)]
mod snapshot_errors {
    use std::sync::atomic::AtomicBool;

    use openraft::CommittedLeaderId;

    use super::*;

    /// An in-memory machine counting its entries, whose state stops serializing on demand.
    struct Counter {
        n: u64,
        broken: Arc<AtomicBool>,
    }

    impl Machine for Counter {
        const IN_MEMORY: bool = true;

        fn apply(&mut self, _index: u64, _command: &[u8], _state: &mut StateTx<'_>) -> Vec<u8> {
            self.n += 1;
            Vec::new()
        }

        fn snapshot(&self) -> eyre::Result<Vec<u8>> {
            if self.broken.load(Ordering::Acquire) {
                eyre::bail!("the state does not serialize");
            }
            Ok(self.n.to_string().into_bytes())
        }

        fn restore(&mut self, data: &[u8]) -> eyre::Result<()> {
            self.n = if data.is_empty() {
                0
            } else {
                std::str::from_utf8(data)?.parse()?
            };
            Ok(())
        }
    }

    /// Review finding A6: the system group's snapshot fell back to empty bytes when its
    /// state did not serialize, so the group checkpointed and served an empty state, which
    /// failed to restore on the node that received it, far from the cause. The build now
    /// fails where it happens, and the checkpoint keeps the last good state.
    #[tokio::test]
    async fn a_state_that_does_not_serialize_builds_no_snapshot_and_keeps_the_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path(), &[3; 32]).unwrap();
        let broken = Arc::new(AtomicBool::new(false));
        let machine = Counter {
            n: 0,
            broken: Arc::clone(&broken),
        };
        let mut sm = StateMachine::open(1, db, machine, Arc::new(watch::channel(0).0)).unwrap();
        let entry = |index| Entry {
            log_id: LogId::new(CommittedLeaderId::new(1, 1), index),
            payload: EntryPayload::Normal(Bytes(b"+1".to_vec())),
        };
        let _ = sm.apply(vec![entry(1), entry(2)]).await.unwrap();
        let _good = sm.build().unwrap();
        assert_eq!(sm.state.get(CHECKPOINT_KEY), Some(b"2".to_vec()));

        let _ = sm.apply(vec![entry(3)]).await.unwrap();
        broken.store(true, Ordering::Release);
        let e = sm.build().unwrap_err().to_string();
        assert!(e.contains("does not serialize"), "{e}");
        assert_eq!(
            sm.state.get(CHECKPOINT_KEY),
            Some(b"2".to_vec()),
            "the last good checkpoint stays; no empty one replaces it"
        );
    }
}

#[cfg(test)]
mod large_rows {
    use openraft::CommittedLeaderId;
    use tokio::io::AsyncWriteExt;

    use super::*;

    /// An in-memory machine whose whole state is `len` bytes (a system state that grew).
    struct Blob(Vec<u8>);

    impl Machine for Blob {
        const IN_MEMORY: bool = true;

        fn apply(&mut self, _index: u64, command: &[u8], _state: &mut StateTx<'_>) -> Vec<u8> {
            let len: usize = std::str::from_utf8(command).unwrap().parse().unwrap();
            self.0 = (0..len).map(|i| (i % 251) as u8).collect();
            Vec::new()
        }

        fn snapshot(&self) -> eyre::Result<Vec<u8>> {
            Ok(self.0.clone())
        }

        fn restore(&mut self, data: &[u8]) -> eyre::Result<()> {
            self.0 = data.to_vec();
            Ok(())
        }
    }

    fn open(dir: &std::path::Path) -> StateMachine<Blob> {
        let db = Db::open(dir, &[5; 32]).unwrap();
        StateMachine::open(1, db, Blob(Vec::new()), Arc::new(watch::channel(0).0)).unwrap()
    }

    /// Build a snapshot of a state of `len` bytes on one node, send it to a new one and
    /// install it there: the file's format, and the state the new node holds.
    async fn send(len: usize) -> eyre::Result<(u32, Vec<u8>)> {
        let a = tempfile::tempdir().unwrap();
        let mut leader = open(a.path());
        let entry = Entry {
            log_id: LogId::new(CommittedLeaderId::new(1, 1), 1),
            payload: EntryPayload::Normal(Bytes(len.to_string().into_bytes())),
        };
        let _ = leader.apply(vec![entry]).await.unwrap();
        let built = leader.build().unwrap();
        let record: SnapshotRecord = leader.db.get_meta(1, "snapshot").unwrap().unwrap();
        let bytes = std::fs::read(leader.snapshot_path(&record.file)).unwrap();
        let format = read_format(&mut &bytes[..]).unwrap();

        let b = tempfile::tempdir().unwrap();
        let mut joining = open(b.path());
        let mut file = joining.begin_receiving_snapshot().await.unwrap();
        file.write_all(&bytes).await.unwrap();
        joining
            .install_snapshot(&built.meta, file)
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        let held = joining.shared.read().unwrap().machine.0.clone();
        Ok((format, held))
    }

    /// Review finding B1: an in-memory machine's state is one row, and its snapshot wrote
    /// that row into one frame, which every reader refuses past `MAX_FRAME_BYTES`: once the
    /// system state grew past it, no new or lagging node could install the system snapshot,
    /// and no system backup could be restored. Such a row now spans frames.
    #[tokio::test]
    async fn a_state_larger_than_a_frame_installs_on_a_new_node() {
        let len = MAX_FRAME_BYTES + 3 * FRAME_BYTES + 17;
        let (_, held) = send(len).await.unwrap();
        assert_eq!(held.len(), len);
        assert!(held.iter().enumerate().all(|(i, b)| *b == (i % 251) as u8));
    }

    /// A state whose rows each fit a frame is written as format 1, byte for byte as before
    /// rows could span frames: the release before reads it, so a rollout needs no gate. Only
    /// a file with a split row says format 2, and a format 1 file of that row was refused by
    /// every reader anyway.
    #[tokio::test]
    async fn only_a_file_with_a_split_row_says_format_2() {
        let (format, held) = send(FRAME_BYTES * 2).await.unwrap();
        assert_eq!((format, held.len()), (1, FRAME_BYTES * 2));
        let whole = MAX_WHOLE_ROW_BYTES - 64;
        let (format, held) = send(whole).await.unwrap();
        assert_eq!((format, held.len()), (1, whole));
        let (format, held) = send(MAX_FRAME_BYTES + 1).await.unwrap();
        assert_eq!((format, held.len()), (2, MAX_FRAME_BYTES + 1));
    }

    /// A snapshot file as the release before wrote it (format 1, built here byte by byte,
    /// not with this release's writer) reads back row for row.
    #[test]
    fn a_format_1_file_from_the_release_before_reads() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path(), &[7; 32]).unwrap();
        let rows: [(&[u8], &[u8]); 3] = [(b"a", b"1"), (CHECKPOINT_KEY, b"{\"n\":2}"), (b"z", b"")];
        let mut plain = 3_u32.to_be_bytes().to_vec();
        for (k, v) in rows {
            let sealed = db.seal_state(4, k, v).unwrap();
            plain.extend_from_slice(&u32::try_from(k.len()).unwrap().to_be_bytes());
            plain.extend_from_slice(k);
            plain.extend_from_slice(&u32::try_from(sealed.len()).unwrap().to_be_bytes());
            plain.extend_from_slice(&sealed);
        }
        let frame = db
            .snapshot_sealer
            .seal(&frame_aad(4, "old", 0, true), &plain)
            .unwrap();
        let mut file = b"CALIMERO-SNAPSHOT-1\n\x01".to_vec();
        file.extend_from_slice(&u32::try_from(frame.len()).unwrap().to_be_bytes());
        file.extend_from_slice(&frame);
        let path = dir.path().join("old.snap");
        std::fs::write(&path, file).unwrap();

        let mut read = Vec::new();
        read_frames(
            &db.snapshot_sealer,
            4,
            "old",
            std::fs::File::open(&path).unwrap(),
            |k, sealed| {
                read.push((k.to_vec(), db.open_state(4, k, sealed)?));
                Ok(())
            },
        )
        .unwrap();
        let want: Vec<(Vec<u8>, Vec<u8>)> =
            rows.iter().map(|(k, v)| (k.to_vec(), v.to_vec())).collect();
        assert_eq!(read, want);
    }
}
