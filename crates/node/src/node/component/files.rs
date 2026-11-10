//! The files of a component app on the node: the provider a run reads and writes them through,
//! and their bytes on the app's replicas (pushed before the entry that marks them is proposed).

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Weak};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use calimero_app_host::{
    parse_blob_id, FileError, FileInfo, FileRead, FileWrite, Files, InvocationKind, NewFile,
    NewSegment, PublishedFile, ReadError, FILE_CORRUPT, FILE_FAILED, MAX_READ_CHUNK,
    NOT_A_RUN_OUTPUT, NO_SUCH_FILE,
};
use calimero_log::{Group, NodeId, Proposal, WriteError};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::runtime::{Handle, RuntimeFlavor};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;
use tracing::warn;
use zeroize::Zeroizing;

use crate::app::AppMachine;
use crate::component::blobs::{BlobRecord, Blobs, FileSeal};
use crate::component::{parse_hex32, ComponentCmd, FileMark};
use crate::error::ApiError;
use crate::files::{
    sealed_len, unwrap_key, wrap_key, Segment, SegmentReader, SegmentWriter, CHUNK_BYTES,
    SEGMENT_BYTES, SEGMENT_CHUNKS, TAG_BYTES,
};
use crate::node::blob_guards::Pins;
use crate::node::{off_workers, status_of, Node, FORWARD_TIMEOUT, MAX_BLOB_BYTES};
use crate::system::SystemCmd;
use crate::types::Caller;

/// The longest a run waits to fetch a file it does not hold from the app's replicas.
const FETCH_WAIT: Duration = Duration::from_secs(5);
/// The longest a call waits for its files to be taken: inside one forward, so the client's
/// idempotency key makes a retry safe.
const PUSH_WAIT: Duration = Duration::from_secs(4);
/// The longest a logged entry waits for its new files to be charged to the quota: charge and
/// push together stay inside one forward.
const CHARGE_WAIT: Duration = Duration::from_millis(800);
/// The longest the removal of files' bytes runs in the background.
const DELETE_WAIT: Duration = Duration::from_secs(30);
/// How long a logged run waits for the node-wide open-bytes budget before it is aborted.
const OPEN_WAIT: Duration = Duration::from_secs(1);
/// The ciphertext bytes open file readers hold across all runs of the node.
pub(in crate::node) const MAX_OPEN_FILE_BYTES: usize = 1 << 30;
/// The ciphertext bytes the readers of one run hold.
pub(in crate::node) const MAX_RUN_OPEN_BYTES: usize = 256 << 20;
/// The ciphertext bytes the new files of all runs of the node hold until their entries are proposed.
pub(in crate::node) const MAX_PENDING_FILE_BYTES: usize = 256 << 20;
/// Bytes of the account id a peer names as a file's uploader.
const MAX_UPLOADER_BYTES: usize = 128;
/// The bytes being pushed to replicas at once, across all calls of the node.
pub(in crate::node) const MAX_PUSH_BYTES: usize = 512 << 20;
/// What an open reader holds at most: a segment's ciphertext and the plaintext it opens to.
const OPEN_READER_BYTES: u64 = (SEGMENT_BYTES + SEGMENT_CHUNKS * CHUNK_BYTES) as u64;
/// Segments or blobs of one push sent at once.
const MAX_PUSHING_BLOBS: usize = 2;
/// Files whose bytes a removal deletes at once.
const MAX_DELETE_CONCURRENCY: usize = 8;
/// The code of a call whose files did not reach a majority of the replicas; the leader's
/// forward loop does not retry it, the client does, with its key.
pub const FILES_UNAVAILABLE: &str = "FILES_UNAVAILABLE";
/// The code of an answer that names the app's leader in `details.leaderUrl`.
pub(super) const NOT_LEADER: &str = "NOT_LEADER";

const _: () = assert!(
    MAX_PENDING_FILE_BYTES
        >= 2 * (calimero_app_host::MAX_RUN_FILE_BYTES + calimero_app_host::MAX_PUBLISHED_BYTES)
            as usize
);
const _: () =
    assert!(PUSH_WAIT.as_millis() + CHARGE_WAIT.as_millis() < FORWARD_TIMEOUT.as_millis());

/// The node-wide budget for the ciphertext one run's new files hold in memory: bytes are taken
/// as the guest writes and return when this drops, once the run's entry is proposed or the run
/// failed. A run that cannot get them fails and holds nothing, so runs never hold more than the
/// budget between them.
pub(super) struct PendingBytes {
    node: Arc<Semaphore>,
    held: std::sync::Mutex<Option<OwnedSemaphorePermit>>,
    #[cfg(feature = "testing")]
    low: Option<Arc<std::sync::atomic::AtomicUsize>>,
}

impl PendingBytes {
    pub(super) fn new(node: Arc<Semaphore>) -> Self {
        Self {
            node,
            held: std::sync::Mutex::new(None),
            #[cfg(feature = "testing")]
            low: None,
        }
    }

    /// Gives back `bytes` of what this run took.
    fn release(&self, bytes: usize) {
        let mut held = self
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(part) = held.as_mut().and_then(|total| total.split(bytes)) {
            drop(part);
        }
    }

    /// Takes `bytes` more from the node's budget, or none of them.
    fn reserve(&self, bytes: usize) -> bool {
        let Ok(bytes) = u32::try_from(bytes) else {
            return false;
        };
        let Ok(permit) = Arc::clone(&self.node).try_acquire_many_owned(bytes) else {
            return false;
        };
        let mut held = self
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match held.as_mut() {
            Some(total) => total.merge(permit),
            None => *held = Some(permit),
        }
        #[cfg(feature = "testing")]
        if let Some(low) = &self.low {
            let _was = low.fetch_min(
                self.node.available_permits(),
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        true
    }
}

/// Bytes a run leaves to store: a published plain blob (named by its hex id) or a segment of a
/// sealed file (named by its hex hash).
#[derive(Clone)]
pub struct Blob {
    pub id: String,
    /// `None` for a segment a writer already stored here: it is read back to be pushed.
    pub bytes: Option<axum::body::Bytes>,
    pub segment: bool,
}

impl Blob {
    #[must_use]
    pub fn plain(id: String, bytes: Vec<u8>) -> Self {
        Self {
            id,
            bytes: Some(bytes.into()),
            segment: false,
        }
    }
}

/// What a logged entry stored and counted against the quota: undone if it does not commit.
#[derive(Default)]
pub(super) struct Stored {
    /// The ids of the plain blobs stored for the entry.
    pub ids: Vec<String>,
    /// The hashes of the segments stored for the entry.
    pub segments: Vec<String>,
    /// Plaintext bytes charged for them.
    pub charged: u64,
    /// The segments are a compute run's outputs, which another leader may have registered: deleted
    /// only once the blob records are known ([`Node::delete_unmarked_outputs`]).
    pub outputs: bool,
    /// Keeps a collection from deleting the plain blobs until the entry commits or fails.
    pub pin: Option<Pins>,
    /// Holds the entry's segments out of the sweep until the entry is decided.
    pub _pin: Option<crate::node::SegmentPin>,
}

/// What an upload answers.
#[derive(Serialize, Deserialize)]
pub(crate) struct Uploaded {
    pub mark: FileMark,
    pub index: u64,
}

/// Knobs and counters for tests (feature `testing`).
#[cfg(feature = "testing")]
pub(crate) struct Probes {
    pub peer_blobs: std::sync::Mutex<crate::node::PeerBlobs>,
    pub file_pushes: std::sync::atomic::AtomicU64,
    pub run_kicks: std::sync::atomic::AtomicU64,
    pub blob_puts_seen: std::sync::atomic::AtomicU64,
    pub conflict_next_call: std::sync::atomic::AtomicBool,
    /// While set, every run final this node logs conflicts.
    pub conflict_finals: std::sync::atomic::AtomicBool,
    /// Names a bogus source in the next call's publish marks, as a faulty leader would.
    pub forge_next_publish: std::sync::atomic::AtomicBool,
    pub run_open_cap: std::sync::atomic::AtomicUsize,
    /// Fails the next mark commit: 1 before it is proposed, 2 after it applied, 3 after
    /// another upload committed under the same key.
    pub commit_fault: std::sync::atomic::AtomicU8,
    /// The waits of the leader's run driving, shortened by tests.
    pub limits: std::sync::Mutex<crate::node::ComputeLimits>,
    /// While set, this node's system reads fail.
    pub system_outage: std::sync::atomic::AtomicBool,
    /// The fewest bytes the pending budget had left.
    pub pending_low: Arc<std::sync::atomic::AtomicUsize>,
    /// The fewest bytes the push budget had left.
    pub push_low: std::sync::atomic::AtomicUsize,
    /// How many unpushed segments an upload session may hold.
    pub upload_window: std::sync::atomic::AtomicUsize,
    /// Fails the next segment an upload session stores.
    pub segment_store_fault: std::sync::atomic::AtomicBool,
    /// Fails the next barrier a failed mark commit asks for, leaving its outcome unknown.
    pub barrier_fault: std::sync::atomic::AtomicBool,
    /// Holds the next push of files on this node after they are stored, before its entry commits.
    pub push_hold: std::sync::Mutex<Option<Hold>>,
    /// Holds the next blob deletion on this node between its checks and its delete.
    pub deletion_hold: std::sync::Mutex<Option<Hold>>,
}

/// A test's hold on a code path: it notifies `.0` on arrival and waits for `.1` to go on.
#[cfg(feature = "testing")]
pub type Hold = (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>);

/// Sets a hold in `slot` for the next arrival and returns the test's ends of it.
#[cfg(feature = "testing")]
pub(crate) fn set_hold(slot: &std::sync::Mutex<Option<Hold>>) -> Hold {
    let hold: Hold = Hold::default();
    *slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hold.clone());
    hold
}

/// Stops at a hold a test set in `slot`, once.
#[cfg(feature = "testing")]
pub(crate) async fn checkpoint(slot: &std::sync::Mutex<Option<Hold>>) {
    let hold = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some((reached, resume)) = hold {
        reached.notify_one();
        resume.notified().await;
    }
}

#[cfg(feature = "testing")]
impl Default for Probes {
    fn default() -> Self {
        Self {
            peer_blobs: std::sync::Mutex::new(crate::node::PeerBlobs::Accept),
            file_pushes: std::sync::atomic::AtomicU64::new(0),
            run_kicks: std::sync::atomic::AtomicU64::new(0),
            blob_puts_seen: std::sync::atomic::AtomicU64::new(0),
            conflict_next_call: std::sync::atomic::AtomicBool::new(false),
            conflict_finals: std::sync::atomic::AtomicBool::new(false),
            forge_next_publish: std::sync::atomic::AtomicBool::new(false),
            run_open_cap: std::sync::atomic::AtomicUsize::new(MAX_RUN_OPEN_BYTES),
            commit_fault: std::sync::atomic::AtomicU8::new(0),
            limits: std::sync::Mutex::new(crate::node::ComputeLimits::default()),
            system_outage: std::sync::atomic::AtomicBool::new(false),
            pending_low: Arc::new(std::sync::atomic::AtomicUsize::new(MAX_PENDING_FILE_BYTES)),
            push_low: std::sync::atomic::AtomicUsize::new(MAX_PUSH_BYTES),
            upload_window: std::sync::atomic::AtomicUsize::new(super::uploads::UPLOAD_WINDOW),
            segment_store_fault: std::sync::atomic::AtomicBool::new(false),
            barrier_fault: std::sync::atomic::AtomicBool::new(false),
            push_hold: std::sync::Mutex::new(None),
            deletion_hold: std::sync::Mutex::new(None),
        }
    }
}

/// Runs `fut` to completion from a run's blocking thread. A current-thread runtime runs the
/// run in place on its only thread, where blocking would panic: refused with `None`.
fn block_on<T>(runtime: &Handle, fut: impl Future<Output = T>) -> Option<T> {
    (runtime.runtime_flavor() != RuntimeFlavor::CurrentThread).then(|| runtime.block_on(fut))
}

pub(super) fn files_unavailable() -> ApiError {
    ApiError::new(
        503,
        FILES_UNAVAILABLE,
        "the replicas could not take the files; retry with the same key",
    )
    .with_details(json!({ "retryAfter": 1 }))
}

/// A peer's blob put: the bytes in base64, as `calimero_log::Bytes` serializes them.
#[derive(Serialize)]
struct BlobPut<'a> {
    hash: &'a str,
    wasm: String,
}

/// Where a push goes: the app group's voters and learners but this node.
#[derive(Debug, PartialEq, Eq)]
struct Targets {
    voters: Vec<String>,
    learners: Vec<String>,
    /// Every voter of the group: this node and the ones with no address included.
    voter_count: usize,
    self_voter: bool,
}

impl Targets {
    fn of(
        me: NodeId,
        voters: &[NodeId],
        learners: &[NodeId],
        addr: impl Fn(NodeId) -> Option<String>,
    ) -> Self {
        let others = |ids: &[NodeId]| -> Vec<String> {
            ids.iter()
                .filter(|id| **id != me)
                .filter_map(|id| addr(*id))
                .collect()
        };
        Self {
            voters: others(voters),
            learners: others(learners),
            voter_count: voters.len(),
            self_voter: voters.contains(&me),
        }
    }

    /// Whether `taken` voters other than this node holding a file make a majority of the voters.
    fn is_majority(&self, taken: usize) -> bool {
        (usize::from(self.self_voter) + taken) * 2 > self.voter_count
    }
}

/// The files one run sees: its app's marks, and the node's store behind them. Cheap to build:
/// keys, budgets and bytes are only touched when the run touches a file.
pub(super) struct NodeFiles {
    node: Weak<Node>,
    runtime: Option<Handle>,
    app_id: String,
    blobs: Blobs,
    node_open: Arc<Semaphore>,
    run_open: Arc<Semaphore>,
    pending: Arc<PendingBytes>,
    /// The hashes of the segments this run's writers stored here.
    made: Arc<std::sync::Mutex<Vec<String>>>,
    /// Logged runs wait a little for the node-wide budget; views do not.
    waits: bool,
}

fn pending_of(node: &Node) -> PendingBytes {
    #[cfg_attr(not(feature = "testing"), allow(unused_mut))]
    let mut pending = PendingBytes::new(Arc::clone(&node.pending_file_bytes));
    #[cfg(feature = "testing")]
    {
        pending.low = Some(Arc::clone(&node.probes.pending_low));
    }
    pending
}

impl NodeFiles {
    pub(super) fn new(node: &Node, app_id: &str, blobs: Blobs, kind: InvocationKind) -> Self {
        Self {
            node: node.this.lock().map(|w| w.clone()).unwrap_or_default(),
            runtime: Handle::try_current().ok(),
            app_id: app_id.to_owned(),
            blobs,
            node_open: Arc::clone(&node.open_file_bytes),
            run_open: Arc::new(Semaphore::new(node.run_open_cap())),
            pending: Arc::new(pending_of(node)),
            made: Arc::default(),
            waits: !matches!(
                kind,
                InvocationKind::View | InvocationKind::Action | InvocationKind::Migrate
            ),
        }
    }

    /// The budget the new files of this run draw on; the caller keeps it while it holds their bytes.
    pub(super) fn pending(&self) -> Arc<PendingBytes> {
        Arc::clone(&self.pending)
    }

    /// Deletes the segments this run's writers stored, but those in `kept`: a file that no entry
    /// will mark leaves no bytes here.
    pub(super) fn forget_made(&self, kept: &[Blob]) {
        let made = std::mem::take(
            &mut *self
                .made
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        let Some(node) = self.node.upgrade() else {
            return;
        };
        for hash in made {
            if !kept.iter().any(|b| b.segment && b.id == hash) {
                let _gone = node.engine.delete_segment(&hash);
            }
        }
    }

    /// The key the app's data keys are wrapped under, derived when a run touches a file.
    fn file_key(&self) -> Option<Zeroizing<[u8; 32]>> {
        let node = self.node.upgrade()?;
        Some(Zeroizing::new(crate::backup::file_key(
            &node.cluster_key,
            &self.app_id,
        )))
    }

    /// Takes `held` bytes from the run's budget and from the node's.
    fn reserve(&self, held: u32) -> Option<(OwnedSemaphorePermit, OwnedSemaphorePermit)> {
        let run = Arc::clone(&self.run_open)
            .try_acquire_many_owned(held)
            .ok()?;
        let node = Arc::clone(&self.node_open);
        let permit = if self.waits {
            let wait = tokio::time::timeout(OPEN_WAIT, node.acquire_many_owned(held));
            block_on(self.runtime.as_ref()?, wait)?.ok()?.ok()?
        } else {
            node.try_acquire_many_owned(held).ok()?
        };
        Some((run, permit))
    }

    /// The record of `id` and its seal, if it is one of the app's sealed files.
    fn sealed(&self, id: &str) -> Option<(&BlobRecord, &FileSeal)> {
        let record = self.blobs.get(&parse_blob_id(id)?)?;
        Some((record, record.file.as_ref()?))
    }

    fn source(&self) -> SegmentSource {
        SegmentSource {
            node: self.node.clone(),
            runtime: self.runtime.clone(),
            app_id: self.app_id.clone(),
        }
    }
}

/// Where a reader gets segments: this node's store, else the app's other replicas.
struct SegmentSource {
    node: Weak<Node>,
    runtime: Option<Handle>,
    app_id: String,
}

impl SegmentSource {
    fn get(&self, hash: &str) -> Option<Vec<u8>> {
        let node = self.node.upgrade()?;
        if let Some(bytes) = node.local_segment(hash) {
            return Some(bytes);
        }
        let app = node.sys(|s| s.apps.get(&self.app_id).cloned())?;
        let peers = node.other_replicas(&app);
        let fetch = tokio::time::timeout(FETCH_WAIT, node.segment_from(hash, &peers));
        block_on(self.runtime.as_ref()?, fetch)?.ok()?.ok()
    }
}

impl Files for NodeFiles {
    fn open(&self, id: &str) -> Result<Box<dyn FileRead>, FileError> {
        let (raw, seal) = self
            .sealed(id)
            .ok_or_else(|| FileError::Failed(NO_SUCH_FILE.to_owned()))?;
        let held = u32::try_from(sealed_len(raw.size).min(OPEN_READER_BYTES))
            .map_err(|_| FileError::Unavailable)?;
        let permits = self.reserve(held).ok_or(FileError::Unavailable)?;
        let file_key = self.file_key().ok_or(FileError::Unavailable)?;
        let source = self.source();
        Ok(Box::new(SealedReader::new(
            Box::new(move |hash| source.get(hash)),
            raw.size,
            seal,
            &self.app_id,
            &file_key,
            permits,
        )))
    }

    fn info(&self, id: &str) -> Option<FileInfo> {
        let (record, seal) = self.sealed(id)?;
        Some(FileInfo {
            size: record.size,
            sha256: seal.sha256.0.clone(),
        })
    }

    fn uploader(&self, id: &str) -> Option<String> {
        self.sealed(id)?.1.uploader.clone()
    }

    fn publish(&self, id: &str) -> Result<PublishedFile, FileError> {
        let not_an_output = || FileError::Failed(NOT_A_RUN_OUTPUT.to_owned());
        let (record, seal) = self
            .sealed(id)
            .filter(|(_, s)| s.output)
            .ok_or_else(not_an_output)?;
        publishable(record.size)?;
        Ok(PublishedFile {
            source: id.to_owned(),
            id: hex::encode(&seal.sha256.0),
            size: record.size,
            bytes: self.plaintext(id, record.size)?,
        })
    }

    fn create(&self) -> Result<Box<dyn FileWrite>, FileError> {
        let file_key = self.file_key().ok_or(FileError::Unavailable)?;
        Ok(Box::new(SealedWriter {
            writer: Some(SegmentWriter::new()),
            segments: Vec::new(),
            node: self.node.clone(),
            made: Arc::clone(&self.made),
            pending: Arc::clone(&self.pending),
            reserved: 0,
            app_id: self.app_id.clone(),
            file_key,
        }))
    }
}

impl NodeFiles {
    /// The whole plaintext of `id`, opened and proven like any read, held against the node's
    /// pending budget until the entry that publishes it is proposed.
    fn plaintext(&self, id: &str, size: u64) -> Result<Vec<u8>, FileError> {
        let held = usize::try_from(size).map_err(|_| FileError::Unavailable)?;
        if !self.pending.reserve(held) {
            return Err(FileError::Unavailable);
        }
        let mut reader = self.open(id)?;
        let mut bytes = Vec::with_capacity(held);
        loop {
            let chunk = reader.read(MAX_READ_CHUNK as usize).map_err(|e| match e {
                ReadError::Corrupt => FileError::Failed(FILE_CORRUPT.to_owned()),
                ReadError::Unavailable => FileError::Unavailable,
                ReadError::Failed(why) => FileError::Failed(why),
            })?;
            if chunk.is_empty() {
                return Ok(bytes);
            }
            bytes.extend_from_slice(&chunk);
        }
    }
}

/// Whether a file of `size` plaintext bytes may be published: the plain blob it becomes must
/// reach the other nodes in one request.
fn publishable(size: u64) -> Result<(), FileError> {
    if size > MAX_BLOB_BYTES as u64 {
        return Err(FileError::Failed(
            calimero_app_host::FILE_TOO_LARGE.to_owned(),
        ));
    }
    Ok(())
}

/// Seals what the guest writes and stores each segment here as it completes, so a writer holds
/// at most its partial segment in memory, counted against the node's pending budget.
struct SealedWriter {
    writer: Option<SegmentWriter>,
    /// The stored segments' hashes, without bytes.
    segments: Vec<NewSegment>,
    node: Weak<Node>,
    made: Arc<std::sync::Mutex<Vec<String>>>,
    pending: Arc<PendingBytes>,
    /// Bytes of the pending budget this writer holds now.
    reserved: usize,
    app_id: String,
    file_key: Zeroizing<[u8; 32]>,
}

impl SealedWriter {
    fn failed() -> FileError {
        FileError::Failed(FILE_FAILED.to_owned())
    }

    /// Holds `to` bytes of the pending budget: else the run is aborted and retried, as for a file
    /// it cannot read.
    fn hold(&mut self, to: usize) -> Result<(), FileError> {
        if to > self.reserved {
            if !self.pending.reserve(to - self.reserved) {
                return Err(FileError::Unavailable);
            }
        } else {
            self.pending.release(self.reserved - to);
        }
        self.reserved = to;
        Ok(())
    }

    /// Stores a completed segment on this node and keeps its hash.
    fn store(&mut self, segment: Segment) -> Result<(), FileError> {
        let node = self.node.upgrade().ok_or(FileError::Unavailable)?;
        let hash = hex::encode(segment.hash);
        (node.engine.put_segment(&hash, &segment.bytes)).map_err(|_| FileError::Unavailable)?;
        (self.made.lock())
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(hash);
        self.segments.push(NewSegment {
            hash: segment.hash,
            bytes: Vec::new(),
        });
        Ok(())
    }

    fn buffered(&self) -> usize {
        self.writer.as_ref().map_or(0, SegmentWriter::buffered)
    }
}

impl Drop for SealedWriter {
    fn drop(&mut self) {
        self.pending.release(self.reserved);
    }
}

impl FileWrite for SealedWriter {
    fn write(&mut self, bytes: &[u8]) -> Result<(), FileError> {
        self.hold(self.buffered() + bytes.len())?;
        let done = (self.writer.as_mut())
            .ok_or_else(Self::failed)?
            .write(bytes)
            .map_err(|_| Self::failed())?;
        for segment in done {
            self.store(segment)?;
        }
        self.hold(self.buffered())
    }

    fn finish(mut self: Box<Self>) -> Result<NewFile, FileError> {
        self.hold(self.buffered() + TAG_BYTES)?;
        let writer = self.writer.take().ok_or_else(Self::failed)?;
        let data_key = Zeroizing::new(*writer.data_key());
        let sealed = writer.finish().map_err(|_| Self::failed())?;
        self.store(sealed.last)?;
        let wrapped_key =
            wrap_key(&self.file_key, &self.app_id, &data_key).map_err(|_| Self::failed())?;
        Ok(NewFile {
            id: hex::encode(sealed.id),
            size: sealed.size,
            sha256: sealed.sha256.to_vec(),
            wrapped_key,
            segments: std::mem::take(&mut self.segments),
        })
    }
}

/// Gives a segment's bytes by its hex hash, or none when nobody can.
type FetchSegment = Box<dyn Fn(&str) -> Option<Vec<u8>> + Send>;

/// Decrypts a file as the guest reads it, a segment at a time: an opener failure is
/// [`ReadError::Corrupt`], a segment nobody can give [`ReadError::Unavailable`].
struct SealedReader {
    fetch: FetchSegment,
    segments: Vec<String>,
    next: usize,
    /// `None` once the stream is proven, or when the mark's key does not open.
    check: Option<Check>,
    proven: bool,
    pending: Vec<u8>,
    /// Count the file's bytes against the run's and the node's open-bytes budgets until the reader drops.
    _held: (OwnedSemaphorePermit, OwnedSemaphorePermit),
}

struct Check {
    reader: SegmentReader,
    size: u64,
    sha256: [u8; 32],
}

impl SealedReader {
    fn new(
        fetch: FetchSegment,
        size: u64,
        seal: &FileSeal,
        app_id: &str,
        file_key: &[u8; 32],
        held: (OwnedSemaphorePermit, OwnedSemaphorePermit),
    ) -> Self {
        let hashes: Option<Vec<[u8; 32]>> = seal
            .segments
            .iter()
            .map(|s| hex::decode(s).ok()?.try_into().ok())
            .collect();
        let check = unwrap_key(file_key, app_id, &seal.wrapped_key.0)
            .ok()
            .zip(hashes)
            .zip(<[u8; 32]>::try_from(seal.sha256.0.as_slice()).ok())
            .map(|((key, hashes), sha256)| Check {
                reader: SegmentReader::new(&key, hashes),
                size,
                sha256,
            });
        Self {
            fetch,
            segments: seal.segments.clone(),
            next: 0,
            check,
            proven: false,
            pending: Vec::new(),
            _held: held,
        }
    }

    /// Opens the next segment into `pending`, or proves the end.
    fn advance(&mut self) -> Result<(), ReadError> {
        let check = self.check.as_mut().ok_or(ReadError::Corrupt)?;
        if let Some(hash) = self.segments.get(self.next) {
            let bytes = (self.fetch)(hash).ok_or(ReadError::Unavailable)?;
            self.pending = check.reader.push(&bytes).map_err(|_| ReadError::Corrupt)?;
            self.next += 1;
            return Ok(());
        }
        let check = self.check.take().ok_or(ReadError::Corrupt)?;
        self.pending = check
            .reader
            .finish_expecting(check.size, &check.sha256)
            .map_err(|_| ReadError::Corrupt)?;
        self.proven = true;
        Ok(())
    }
}

impl FileRead for SealedReader {
    fn read(&mut self, max: usize) -> Result<Vec<u8>, ReadError> {
        while self.pending.is_empty() && !self.proven {
            self.advance()?;
        }
        let rest = self.pending.split_off(max.min(self.pending.len()));
        Ok(std::mem::replace(&mut self.pending, rest))
    }
}

/// The `members` that pass `check`, asked all at once; one that has not answered by `deadline`
/// is refused.
async fn allowed_by<F, Fut>(
    members: Vec<(NodeId, String)>,
    deadline: Instant,
    check: F,
) -> HashMap<NodeId, String>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = bool>,
{
    let answers = futures_util::future::join_all(members.into_iter().map(|(id, addr)| {
        let asked = check(addr.clone());
        async move {
            let ok = tokio::time::timeout_at(deadline, asked)
                .await
                .unwrap_or(false);
            ok.then_some((id, addr))
        }
    }))
    .await;
    answers.into_iter().flatten().collect()
}

impl Node {
    /// Deletes this node's copy of the segments of `blobs`: files no entry will mark.
    pub(super) fn forget_local(&self, blobs: &[Blob]) {
        for blob in blobs.iter().filter(|b| b.segment) {
            let _gone = self.engine.delete_segment(&blob.id);
        }
    }

    /// Stores a run's new files here and on the app's replicas; `503 FILES_UNAVAILABLE` unless
    /// a majority of the voters hold every one, and then what was stored is removed.
    pub(super) fn persist_files(&self, app_id: &str, blobs: &[Blob]) -> Result<(), ApiError> {
        if blobs.is_empty() {
            return Ok(());
        }
        let Ok(runtime) = Handle::try_current() else {
            return Err(files_unavailable());
        };
        off_workers(|| block_on(&runtime, self.push_files(app_id, blobs, false)))
            .unwrap_or_else(|| Err(files_unavailable()))
    }

    pub(super) async fn push_files(
        &self,
        app_id: &str,
        blobs: &[Blob],
        outputs: bool,
    ) -> Result<(), ApiError> {
        #[cfg(feature = "testing")]
        let _n = self
            .probes
            .file_pushes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match self.push_blobs(app_id, blobs, PUSH_WAIT).await {
            Ok(()) => Ok(()),
            Err(e) => {
                // No entry registers these files, so no other run can need them.
                self.discard_bytes(app_id, stored_of_blobs(blobs), outputs);
                Err(e)
            }
        }
    }

    /// Puts `blobs` on a majority of the app group's voters within `wait`; what was stored stays.
    pub(super) async fn push_blobs(
        &self,
        app_id: &str,
        blobs: &[Blob],
        wait: Duration,
    ) -> Result<(), ApiError> {
        let deadline = Instant::now() + wait;
        let push = async {
            let targets =
                (self.push_targets(app_id, deadline).await).ok_or_else(files_unavailable)?;
            for batch in blobs.chunks(MAX_PUSHING_BLOBS) {
                let pushed = futures_util::future::join_all(
                    batch.iter().map(|b| self.push_blob(b, &targets)),
                )
                .await;
                pushed.into_iter().collect::<Result<Vec<()>, ApiError>>()?;
            }
            Ok::<(), ApiError>(())
        };
        match tokio::time::timeout_at(deadline, push).await {
            Ok(Ok(())) => {
                #[cfg(feature = "testing")]
                self.push_checkpoint().await;
                Ok(())
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err(files_unavailable()),
        }
    }

    /// The app group's other voters and learners, as its own Raft membership names them, but
    /// none whose release may not hold the app's data ([`Node::may_send`]); one whose release
    /// is not learnt by `deadline` is left out.
    async fn push_targets(&self, app_id: &str, deadline: Instant) -> Option<Targets> {
        let status = self.hosted(app_id)?.status();
        let group = self.sys(|s| s.apps.get(app_id).map(|a| a.group))?;
        let members: Vec<(NodeId, String)> = self.sys(|s| {
            (status.voters.iter().chain(&status.learners))
                .filter_map(|id| s.replica_node(id).map(|n| (*id, n.addr.clone())))
                .collect()
        });
        let allowed = allowed_by(members, deadline, |addr| async move {
            self.may_send(&addr, group).await
        })
        .await;
        Some(Targets::of(
            self.config.node_id,
            &status.voters,
            &status.learners,
            |id| allowed.get(&id).cloned(),
        ))
    }

    async fn put_on(
        &self,
        addr: &str,
        hash: &str,
        bytes: axum::body::Bytes,
        segment: bool,
    ) -> eyre::Result<()> {
        if segment {
            return self.put_segment_on(addr, hash, bytes).await;
        }
        let body = BlobPut {
            hash,
            wasm: STANDARD.encode(&bytes),
        };
        self.peer_post(addr, "internal/blob", &body).await.map(drop)
    }

    /// The bytes of `blob`, stored here: a segment a writer stored is read back.
    async fn stored_bytes(&self, blob: &Blob) -> Result<axum::body::Bytes, ApiError> {
        let engine = Arc::clone(&self.engine);
        let guards = Arc::clone(&self.blob_guards);
        let (id, held, segment) = (blob.id.clone(), blob.bytes.clone(), blob.segment);
        let found = tokio::task::spawn_blocking(move || match held {
            None => engine.get_segment(&id),
            Some(held) if segment => engine.put_segment(&id, &held).map(|()| Some(held.to_vec())),
            Some(held) => guards
                .put(&id, || engine.put_blob(&id, &held))
                .map(|()| Some(held.to_vec())),
        })
        .await
        .map_err(|e| ApiError::internal(&e.to_string()))?
        .map_err(|e| ApiError::internal(&e.to_string()))?;
        found
            .map(axum::body::Bytes::from)
            .ok_or_else(files_unavailable)
    }

    /// Stores `blob` here and on `targets`: a segment raw, a plain blob as JSON. Holds the push
    /// budget for the bytes it sends, one body shared by every target.
    async fn push_blob(&self, blob: &Blob, targets: &Targets) -> Result<(), ApiError> {
        let held = blob.bytes.as_ref().map_or(SEGMENT_BYTES, |b| b.len());
        let _budget = Arc::clone(&self.push_bytes)
            .acquire_many_owned(u32::try_from(held).map_err(|_| files_unavailable())?)
            .await
            .map_err(|_| files_unavailable())?;
        #[cfg(feature = "testing")]
        let _was = self.probes.push_low.fetch_min(
            self.push_bytes.available_permits(),
            std::sync::atomic::Ordering::Relaxed,
        );
        let bytes = self.stored_bytes(blob).await?;
        let (id, segment) = (blob.id.as_str(), blob.segment);
        let (voters, _learners) = futures_util::future::join(
            futures_util::future::join_all(
                (targets.voters.iter()).map(|a| self.put_on(a, id, bytes.clone(), segment)),
            ),
            futures_util::future::join_all(
                (targets.learners.iter()).map(|a| self.put_on(a, id, bytes.clone(), segment)),
            ),
        )
        .await;
        if targets.is_majority(voters.iter().filter(|r| r.is_ok()).count()) {
            Ok(())
        } else {
            Err(files_unavailable())
        }
    }

    /// [`Self::delete_files_later`] for segments no entry registers; a compute run's outputs only
    /// after the app's blob records were read through its leader.
    fn discard_bytes(&self, app_id: &str, stored: Stored, outputs: bool) {
        if !outputs {
            self.delete_files_later(app_id, stored);
            return;
        }
        let Some(node) = self.this.lock().ok().and_then(|w| w.upgrade()) else {
            return;
        };
        let app_id = app_id.to_owned();
        drop(tokio::spawn(async move {
            node.delete_unmarked_outputs(&app_id, stored.segments).await;
        }));
    }

    /// Deletes compute-output segments no file of the app lists, judged after a leader barrier and
    /// a strong read; without the barrier they stay.
    pub(super) async fn delete_unmarked_outputs(&self, app_id: &str, segments: Vec<String>) {
        let Some(g) = self.hosted(app_id) else {
            return;
        };
        let barrier = serde_json::to_vec(&ComponentCmd::Barrier).unwrap_or_default();
        if g.write(barrier).await.is_err() {
            return;
        }
        let unmarked = g
            .read_strong(|m, _| {
                segments
                    .iter()
                    .filter(|hash| !m.holds_segment(hash))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .await;
        if let Ok(unmarked) = unmarked {
            let stored = Stored {
                segments: unmarked,
                ..Stored::default()
            };
            self.delete_files(&stored).await;
        }
    }

    /// Whether a component app hosted here lists `hash` as a segment of one of its files; true
    /// when the apps cannot be read, so nothing is deleted on a guess.
    pub(crate) fn holds_segment(&self, hash: &str) -> bool {
        self.apps.read().map_or(true, |apps| {
            apps.values()
                .any(|g| g.read_local(|m, _| m.holds_segment(hash)))
        })
    }

    /// Removes the plain blobs and segments of `stored` here and on the app's other replicas, in
    /// a task of its own: nobody waits for it. A replica that misses it keeps unreachable bytes.
    pub(super) fn delete_files_later(&self, app_id: &str, stored: Stored) {
        let Some(node) = self.this.lock().ok().and_then(|w| w.upgrade()) else {
            return;
        };
        let app_id = app_id.to_owned();
        drop(tokio::spawn(async move {
            let removal = node.delete_files(&stored);
            if tokio::time::timeout(DELETE_WAIT, removal).await.is_err() {
                warn!(app = %app_id, "file bytes not deleted everywhere in time");
            }
        }));
    }

    async fn delete_files(&self, stored: &Stored) {
        for batch in stored.ids.chunks(MAX_DELETE_CONCURRENCY) {
            let _all =
                futures_util::future::join_all(batch.iter().map(|id| self.delete_file(id))).await;
        }
        // Replicas' copies of the segments go with the sweep, after its grace.
        for hash in &stored.segments {
            self.delete_segment(hash);
        }
    }

    async fn delete_file(&self, id: &str) {
        if let Some(raw) = parse_blob_id(id) {
            self.collect_blob(&raw).await;
        }
    }

    fn delete_segment(&self, hash: &str) {
        if self.holds_segment(hash) {
            return;
        }
        if let Err(e) = self.engine.delete_segment(hash) {
            warn!(segment = %hash, error = %e, "file bytes not deleted");
        }
    }
}

impl Node {
    /// Counts `delta` plaintext bytes of files (negative: a release) against the app's payer in the
    /// system log; `429 QUOTA_EXCEEDED` when a charge would pass the quota.
    pub(super) async fn count_file_bytes(&self, app_id: &str, delta: i64) -> Result<(), ApiError> {
        let cmd = SystemCmd::ChargeFileBytes {
            app_id: app_id.to_owned(),
            delta,
        };
        self.sys_write(&cmd).await.map(drop)
    }

    /// Gives back `bytes`: files that were never marked, or that retention collected.
    pub(super) async fn refund_files(&self, app_id: &str, bytes: u64) {
        let Ok(delta) = i64::try_from(bytes).map(|b| -b) else {
            return;
        };
        if bytes == 0 {
            return;
        }
        if let Err(e) = self.count_file_bytes(app_id, delta).await {
            warn!(app = %app_id, error = %e.message, "file bytes not refunded");
        }
    }

    /// [`Self::refund_files`] in a task of its own, for a path that cannot wait.
    fn refund_files_later(&self, app_id: &str, bytes: u64) {
        let Some(node) = self.this.lock().ok().and_then(|w| w.upgrade()) else {
            return;
        };
        let app_id = app_id.to_owned();
        drop(tokio::spawn(async move {
            node.refund_files(&app_id, bytes).await;
        }));
    }

    /// Counts the new files of an entry about to be proposed, from a run's blocking thread, within
    /// [`CHARGE_WAIT`]. A charge that lands after the wait is refunded when its answer arrives.
    pub(super) fn charge_files(&self, app_id: &str, bytes: u64) -> Result<(), ApiError> {
        let node = self.this.lock().ok().and_then(|w| w.upgrade());
        let (Some(node), Ok(delta), Ok(runtime)) =
            (node, i64::try_from(bytes), Handle::try_current())
        else {
            return Err(files_unavailable());
        };
        let (answer, mut counted) = tokio::sync::oneshot::channel();
        let app = app_id.to_owned();
        drop(runtime.spawn(async move {
            let charge = node.count_file_bytes(&app, delta).await;
            if let Err(Ok(())) = answer.send(charge) {
                node.refund_files(&app, bytes).await;
            }
        }));
        let wait = async {
            match tokio::time::timeout(CHARGE_WAIT, &mut counted).await {
                Ok(Ok(charge)) => Some(charge),
                Ok(Err(_)) => None,
                Err(_) => {
                    // Closing first leaves one owner for a charge that lands in the gap.
                    counted.close();
                    if let Ok(Ok(())) = counted.try_recv() {
                        self.refund_files_later(app_id, bytes);
                    }
                    None
                }
            }
        };
        off_workers(|| block_on(&runtime, wait))
            .flatten()
            .unwrap_or_else(|| Err(files_unavailable()))
    }

    /// Undoes what a logged entry that did not commit stored and counted.
    pub(super) fn discard(&self, app_id: &str, mut stored: Stored) {
        drop(stored.pin.take());
        let (charged, outputs) = (stored.charged, stored.outputs);
        if !stored.ids.is_empty() || !stored.segments.is_empty() {
            self.discard_bytes(app_id, stored, outputs);
        }
        if charged > 0 {
            self.refund_files_later(app_id, charged);
        }
    }

    /// Commits the mark of a file whose bytes are stored. When the commit fails, the leader says
    /// whether the mark is there: if so the upload succeeded, if not its bytes and charge go.
    pub(super) async fn commit_mark(
        &self,
        g: &Group<AppMachine>,
        app_id: &str,
        mark: FileMark,
        key: &str,
        uploader: &str,
    ) -> Result<Uploaded, Upload> {
        let error = match self.propose_mark(g, &mark, key, uploader).await {
            Ok(answer) if answer.status == crate::app::Status::Ok => {
                // A record under this key from another upload wins: ours is not marked.
                return match uploaded(&answer) {
                    Some(recorded) if recorded.mark.id != mark.id => {
                        self.undo_upload(app_id, &mark).await;
                        Ok(recorded)
                    }
                    _ => Ok(Uploaded {
                        mark,
                        index: answer.index,
                    }),
                };
            }
            Ok(answer) => {
                self.undo_upload(app_id, &mark).await;
                return Err(Upload::Refused(ApiError::internal(&format!(
                    "the app refused the file: {}",
                    answer.error
                ))));
            }
            Err(e) => e,
        };
        if let WriteError::NotLeader { .. } | WriteError::TooLarge { .. } = error {
            self.undo_upload(app_id, &mark).await;
            return Err(match error {
                WriteError::NotLeader {
                    leader: Some((_, addr)),
                } if !addr.is_empty() => Upload::Elsewhere(Some(addr)),
                e => Upload::Refused(status_of(&e)),
            });
        }
        match self.marked_after_barrier(g, &mark.id).await {
            Some((true, index)) => Ok(Uploaded { mark, index }),
            Some((false, _)) => {
                self.undo_upload(app_id, &mark).await;
                Err(Upload::Refused(status_of(&error)))
            }
            // Unknown: the entry may yet commit, so the bytes and the charge stay.
            None => Err(Upload::Unknown(status_of(&error))),
        }
    }

    /// Whether the app holds file `id`, once the entry that failed cannot commit any more: a
    /// barrier entry committed through this leader commits every entry before it in its log and
    /// ends any other. `None` when no barrier could be committed (this node no longer leads).
    pub(super) async fn marked_after_barrier(
        &self,
        g: &Group<AppMachine>,
        id: &str,
    ) -> Option<(bool, u64)> {
        let raw = parse_hex32(id)?;
        #[cfg(feature = "testing")]
        if self
            .probes
            .barrier_fault
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            return None;
        }
        let barrier = serde_json::to_vec(&ComponentCmd::Barrier).unwrap_or_default();
        let _committed = g.write(barrier).await.ok()?;
        g.read_strong(|m, applied| (m.holds_blob(&raw), applied))
            .await
            .ok()
    }

    pub(super) async fn undo_upload(&self, app_id: &str, mark: &FileMark) {
        self.refund_files(app_id, mark.size).await;
        let stored = Stored {
            segments: mark.segments.clone(),
            ..Stored::default()
        };
        self.delete_files_later(app_id, stored);
    }

    /// Appends the upload's mark to the app's log, or answers the one recorded for `key`.
    async fn propose_mark(
        &self,
        g: &Group<AppMachine>,
        mark: &FileMark,
        key: &str,
        uploader: &str,
    ) -> Result<crate::component::Applied, WriteError> {
        #[cfg(feature = "testing")]
        let fault = self
            .probes
            .commit_fault
            .swap(0, std::sync::atomic::Ordering::Relaxed);
        #[cfg(feature = "testing")]
        if fault == 1 {
            return Err(WriteError::Unavailable("injected before".into()));
        }
        #[cfg(feature = "testing")]
        if fault == 3 {
            // Another upload commits under this key first.
            let mut other = mark.clone();
            other.segments = vec!["ab".repeat(32)];
            other.id = hex::encode(crate::files::file_id(&[[0xab; 32]]));
            let _ = self.append_mark(g, &other, key, uploader).await;
        }
        let proposed = self.append_mark(g, mark, key, uploader).await?;
        #[cfg(feature = "testing")]
        if fault == 2 {
            return Err(WriteError::Unavailable("injected after".into()));
        }
        serde_json::from_slice(&proposed.output).map_err(|e| WriteError::Unavailable(e.to_string()))
    }

    async fn append_mark(
        &self,
        g: &Group<AppMachine>,
        mark: &FileMark,
        key: &str,
        uploader: &str,
    ) -> Result<calimero_log::Proposed, WriteError> {
        g.propose(|m, _| match m.component_replay(&Caller::System, key) {
            Some(prev) => Proposal::Reply(serde_json::to_vec(&prev).unwrap_or_default()),
            None => Proposal::Append(
                serde_json::to_vec(&ComponentCmd::File {
                    mark: mark.clone(),
                    ts_ms: self.stamp(m),
                    key: Some(key.to_owned()),
                    uploader: Some(uploader.to_owned()),
                })
                .unwrap_or_default(),
            ),
        })
        .await
    }
}

/// A segment stored on this node, as a blob to push.
pub(super) fn segment_blob(hash: &[u8; 32]) -> Blob {
    Blob {
        id: hex::encode(hash),
        bytes: None,
        segment: true,
    }
}

/// The segments of `file` as pushable blobs, leaving the hashes in place.
pub(super) fn blobs_of(file: &mut NewFile) -> Vec<Blob> {
    (file.segments.iter_mut())
        .map(|s| Blob {
            id: hex::encode(s.hash),
            bytes: Some(std::mem::take(&mut s.bytes))
                .filter(|b| !b.is_empty())
                .map(Into::into),
            segment: true,
        })
        .collect()
}

/// What `blobs` hold, by name, as a [`Stored`] to delete.
pub(super) fn stored_of_blobs(blobs: &[Blob]) -> Stored {
    let named = |segment| {
        (blobs.iter())
            .filter(|b| b.segment == segment)
            .map(|b| b.id.clone())
            .collect()
    };
    Stored {
        ids: named(false),
        segments: named(true),
        ..Stored::default()
    }
}

/// Whether a peer's uploader is an account id: printable ASCII, at most [`MAX_UPLOADER_BYTES`].
pub(crate) fn valid_uploader(who: &str) -> bool {
    !who.is_empty() && who.len() <= MAX_UPLOADER_BYTES && who.bytes().all(|b| b.is_ascii_graphic())
}

/// Whether the app holds the blob with hex id `id`: a file registered by a final or an upload.
pub(super) fn holds_file(m: &AppMachine, id: &str) -> bool {
    parse_hex32(id).is_some_and(|raw| m.holds_blob(&raw))
}

/// Why an upload did not finish on this node.
pub(super) enum Upload {
    Refused(ApiError),
    /// The commit may yet land: its bytes and charge stay.
    Unknown(ApiError),
    /// This node does not lead the app; the leader's address, if known.
    Elsewhere(Option<String>),
}

/// An upload's answer as a command recorded: the mark it committed and the entry's index.
pub(super) fn uploaded(answer: &crate::component::Applied) -> Option<Uploaded> {
    Some(Uploaded {
        mark: serde_json::from_value(answer.result["mark"].clone()).ok()?,
        index: answer.index,
    })
}

/// Reads a sealed file through the provider a compute run reads with, the only reader a guest
/// has. Opens it `open` times at once and reads the first. Tests only.
#[cfg(feature = "testing")]
impl Node {
    pub fn read_sealed_file(&self, app_id: &str, id: &str, open: usize) -> Result<Vec<u8>, String> {
        let blobs = self
            .hosted(app_id)
            .map(|g| g.read_local(|m, _| Arc::clone(&m.state.component.blobs.records)))
            .unwrap_or_default();
        off_workers(|| {
            let files = NodeFiles::new(self, app_id, blobs, InvocationKind::Action);
            let open_one = || {
                files.open(id).map_err(|e| match e {
                    FileError::Failed(reason) => reason,
                    FileError::Unavailable => calimero_app_host::FILE_UNAVAILABLE.to_owned(),
                })
            };
            let mut readers = (0..open.max(1))
                .map(|_| open_one())
                .collect::<Result<Vec<_>, _>>()?;
            let mut bytes = Vec::new();
            loop {
                let chunk = readers[0].read(1 << 20).map_err(|e| match e {
                    ReadError::Failed(reason) => reason,
                    ReadError::Unavailable => calimero_app_host::FILE_UNAVAILABLE.to_owned(),
                    ReadError::Corrupt => calimero_app_host::FILE_CORRUPT.to_owned(),
                })?;
                if chunk.is_empty() {
                    return Ok(bytes);
                }
                bytes.extend_from_slice(&chunk);
            }
        })
    }
}

impl Node {
    /// Writes `count` files of `len` bytes through the writers a run's files use, all at once in
    /// small pieces, and answers their ids. Tests only.
    #[cfg(feature = "testing")]
    pub fn write_test_files(
        &self,
        app_id: &str,
        count: usize,
        len: usize,
    ) -> Result<Vec<String>, String> {
        let failed = |e: FileError| format!("{e:?}");
        off_workers(|| {
            let files = NodeFiles::new(self, app_id, Blobs::default(), InvocationKind::Call);
            let mut writers = (0..count)
                .map(|_| files.create())
                .collect::<Result<Vec<_>, _>>()
                .map_err(failed)?;
            let piece = vec![7u8; 64 * 1024];
            for _ in 0..len / piece.len() {
                for writer in &mut writers {
                    writer.write(&piece).map_err(failed)?;
                }
            }
            (writers.into_iter())
                .map(|w| w.finish().map(|f| f.id).map_err(failed))
                .collect()
        })
    }

    /// The most bytes pushes to replicas held at once. Tests only.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn push_bytes_peak(&self) -> usize {
        MAX_PUSH_BYTES
            - self
                .probes
                .push_low
                .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The most bytes the new files of runs held at once. Tests only.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn pending_file_bytes_peak(&self) -> usize {
        MAX_PENDING_FILE_BYTES
            - self
                .probes
                .pending_low
                .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Runs the cleanup of compute output segments `hashes` as a losing leader would. Tests only.
    #[cfg(feature = "testing")]
    pub async fn clean_up_outputs(&self, app_id: &str, hashes: Vec<String>) {
        self.delete_unmarked_outputs(app_id, hashes).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::FileMark;
    use crate::files::CHUNK_BYTES;

    #[test]
    fn a_file_over_the_blob_cap_is_too_large_to_publish() {
        let cap = MAX_BLOB_BYTES as u64;
        assert!(publishable(cap).is_ok());
        assert_eq!(
            publishable(cap + 1),
            Err(FileError::Failed(
                calimero_app_host::FILE_TOO_LARGE.to_owned()
            ))
        );
    }

    #[test]
    fn an_uploader_a_peer_names_is_an_account_id_not_free_text() {
        assert!(valid_uploader(
            "vFtvwMfpnAQxoUWzlvp86ysNqE_C8XEG5MjtwRs1oNM"
        ));
        for bad in ["", "a b", "caf\u{e9}", &"a".repeat(MAX_UPLOADER_BYTES + 1)] {
            assert!(!valid_uploader(bad), "{bad:?}");
        }
        assert!(valid_uploader(&"a".repeat(MAX_UPLOADER_BYTES)));
    }

    #[tokio::test]
    async fn a_member_that_does_not_answer_by_the_deadline_is_refused() {
        let members = vec![
            (1, "a".to_owned()),
            (2, "b".to_owned()),
            (3, "c".to_owned()),
        ];
        let deadline = tokio::time::Instant::now() + Duration::from_millis(200);
        let allowed = allowed_by(members, deadline, |addr| async move {
            if addr == "b" {
                std::future::pending::<()>().await;
            }
            addr != "c"
        })
        .await;
        assert_eq!(allowed.keys().copied().collect::<Vec<_>>(), vec![1]);
        assert!(tokio::time::Instant::now() < deadline + Duration::from_millis(500));
    }

    fn addr(id: NodeId) -> Option<String> {
        (id != 9).then(|| format!("http://node{id}/"))
    }

    #[test]
    fn a_majority_is_counted_over_the_voters_and_not_the_nodes_the_directory_lists() {
        // One voter (this node) and two learners: this node alone is the majority.
        let t = Targets::of(1, &[1], &[2, 3], addr);
        assert!(t.voters.is_empty() && t.learners.len() == 2);
        assert!(t.is_majority(0));
        // Three voters: this node and one more.
        let t = Targets::of(1, &[1, 2, 3], &[], addr);
        assert!(!t.is_majority(0));
        assert!(t.is_majority(1));
    }

    #[test]
    fn a_voter_with_no_address_still_counts_in_the_denominator() {
        let t = Targets::of(1, &[1, 2, 9], &[], addr);
        assert_eq!(t.voters, vec!["http://node2/".to_owned()]);
        assert!(t.is_majority(1));
        let t = Targets::of(1, &[1, 8, 9, 7], &[], addr);
        assert!(!t.is_majority(1), "2 of 4 is not a majority");
        assert!(t.is_majority(2));
    }

    #[test]
    fn this_node_counts_only_if_it_is_a_voter() {
        let t = Targets::of(4, &[1, 2, 3], &[4], addr);
        assert!(!t.self_voter);
        assert!(!t.is_majority(1));
        assert!(t.is_majority(2));
    }

    #[test]
    fn two_runs_over_the_budget_do_not_both_hold_their_bytes() {
        let node = Arc::new(Semaphore::new(100));
        let (a, b) = (
            PendingBytes::new(node.clone()),
            PendingBytes::new(node.clone()),
        );
        assert!(a.reserve(60));
        assert!(!b.reserve(60), "a second run took what was not left");
        assert_eq!(node.available_permits(), 40, "a refused run took some");
        // A failed or finished run returns what it held, and the other may then take it.
        drop(a);
        assert_eq!(node.available_permits(), 100);
        assert!(b.reserve(60) && b.reserve(40));
        assert!(!b.reserve(1));
        drop(b);
        assert_eq!(node.available_permits(), 100);
    }

    #[test]
    fn a_reservation_past_u32_is_refused() {
        let b = PendingBytes::new(Arc::new(Semaphore::new(1)));
        assert!(!b.reserve(usize::MAX));
    }

    #[tokio::test]
    async fn blocking_is_refused_on_a_current_thread_runtime() {
        let runtime = Handle::current();
        assert_eq!(block_on(&runtime, async { 1 }), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn blocking_runs_the_future_on_a_blocking_thread_of_a_multi_thread_runtime() {
        let runtime = Handle::current();
        let done = tokio::task::spawn_blocking(move || block_on(&runtime, async { 1 }))
            .await
            .unwrap();
        assert_eq!(done, Some(1));
    }

    /// A sealed file as its segments, and the mark that describes it.
    fn sealed(plain: &[u8]) -> (Vec<Vec<u8>>, FileMark, [u8; 32]) {
        let file_key = [7u8; 32];
        let mut writer = SegmentWriter::new();
        let mut segments: Vec<Vec<u8>> = writer
            .write(plain)
            .unwrap()
            .into_iter()
            .map(|s| s.bytes)
            .collect();
        let data_key = *writer.data_key();
        let done = writer.finish().unwrap();
        segments.push(done.last.bytes);
        let mark = FileMark {
            id: hex::encode(done.id),
            size: done.size,
            sha256: calimero_log::Bytes(done.sha256.to_vec()),
            wrapped_key: calimero_log::Bytes(wrap_key(&file_key, "app", &data_key).unwrap()),
            segments: done.segments.iter().map(hex::encode).collect(),
        };
        (segments, mark, file_key)
    }

    fn read_all(
        segments: Vec<Vec<u8>>,
        mark: &FileMark,
        file_key: &[u8; 32],
    ) -> Result<Vec<u8>, ReadError> {
        let permit = || Arc::new(Semaphore::new(1)).try_acquire_owned().unwrap();
        let seal = FileSeal {
            sha256: mark.sha256.clone(),
            wrapped_key: mark.wrapped_key.clone(),
            segments: mark.segments.clone(),
            output: false,
            uploader: None,
        };
        let held: HashMap<String, Vec<u8>> = mark.segments.iter().cloned().zip(segments).collect();
        let mut reader = SealedReader::new(
            Box::new(move |hash| held.get(hash).cloned()),
            mark.size,
            &seal,
            "app",
            file_key,
            (permit(), permit()),
        );
        let mut out = Vec::new();
        loop {
            let chunk = reader.read(100_000)?;
            if chunk.is_empty() {
                return Ok(out);
            }
            out.extend(chunk);
        }
    }

    #[test]
    fn a_reader_returns_the_file_and_traps_on_any_alteration() {
        let plain: Vec<u8> = (0..crate::files::SEGMENT_CHUNKS * CHUNK_BYTES + 100)
            .map(|i| (i % 251) as u8)
            .collect();
        let (segments, mark, key) = sealed(&plain);
        assert_eq!(segments.len(), 2);
        assert_eq!(read_all(segments.clone(), &mark, &key).unwrap(), plain);

        let mut altered = segments.clone();
        altered[1][5] ^= 1;
        assert_eq!(read_all(altered, &mark, &key), Err(ReadError::Corrupt));

        // A segment nobody can give is unavailable, not corrupt.
        assert_eq!(
            read_all(segments[..1].to_vec(), &mark, &key),
            Err(ReadError::Unavailable)
        );
        // A list that ends early: every segment opens, the stream ends too early.
        let short = FileMark {
            segments: mark.segments[..1].to_vec(),
            ..mark.clone()
        };
        assert_eq!(
            read_all(segments.clone(), &short, &key),
            Err(ReadError::Corrupt)
        );

        let (other, _other_mark, _) = sealed(&plain);
        assert_eq!(read_all(other, &mark, &key), Err(ReadError::Corrupt));

        let wrong_hash = FileMark {
            sha256: calimero_log::Bytes(vec![0; 32]),
            ..mark.clone()
        };
        assert_eq!(
            read_all(segments.clone(), &wrong_hash, &key),
            Err(ReadError::Corrupt)
        );
        assert_eq!(read_all(segments, &mark, &[8; 32]), Err(ReadError::Corrupt));
    }
}
