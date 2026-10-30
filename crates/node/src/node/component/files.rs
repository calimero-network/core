//! The files of a component app on the node: the provider a run reads and writes them through,
//! and their bytes on the app's replicas (pushed before the entry that marks them is proposed).

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Weak};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use calimero_app_host::{
    FileError, FileInfo, FileRead, FileWrite, Files, InvocationKind, NewFile, ReadError,
    FILE_FAILED, NO_SUCH_FILE,
};
use calimero_log::{Group, NodeId, Proposal, WriteError};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::runtime::{Handle, RuntimeFlavor};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::warn;
use zeroize::Zeroizing;

use crate::app::{AppMachine, Applied};
use crate::component::{ComponentCmd, FileMark, Marks};
use crate::error::ApiError;
use crate::files::{
    sealed_len, unwrap_key, wrap_key, FileOpener, FileSealer, CHUNK_BYTES, TAG_BYTES,
};
use crate::node::{off_workers, status_of, CallRequest, Node, FORWARD_TIMEOUT, MAX_BLOB_BYTES};
use crate::system::SystemCmd;
use crate::types::{AppRecord, Caller};

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
/// The bytes being pushed to replicas at once, across all calls of the node.
pub(in crate::node) const MAX_PUSH_BYTES: usize = 512 << 20;
/// Files whose bytes a removal deletes at once.
const MAX_DELETE_CONCURRENCY: usize = 8;
/// The code of a call whose files did not reach a majority of the replicas; the leader's
/// forward loop does not retry it, the client does, with its key.
pub const FILES_UNAVAILABLE: &str = "FILES_UNAVAILABLE";
/// The code of an answer that names the app's leader in `details.leaderUrl`.
const NOT_LEADER: &str = "NOT_LEADER";

const _: () =
    assert!(PUSH_WAIT.as_millis() + CHARGE_WAIT.as_millis() < FORWARD_TIMEOUT.as_millis());

/// A file's ciphertext as the host core hands it over, with the blob id it is stored under.
pub type Blob = (String, Vec<u8>);

/// What a logged entry stored and counted against the quota: undone if it does not commit.
#[derive(Default)]
pub(super) struct Stored {
    /// The ids of the files stored for the entry.
    pub ids: Vec<String>,
    /// Plaintext bytes charged for them.
    pub charged: u64,
    /// The ids are a compute run's outputs: content hashes of what a job node made, which
    /// another leader may have stored under the same ids, so they are only deleted once the
    /// app's marks are known ([`Node::delete_unmarked_outputs`]).
    pub outputs: bool,
}

/// The node's own command that forgets a file, run on the app's leader
/// ([`HOST_METHODS`](crate::node::HOST_METHODS)).
pub(crate) const FILE_FORGET: &str = "__file_forget";

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
    pub blob_puts_seen: std::sync::atomic::AtomicU64,
    pub conflict_next_call: std::sync::atomic::AtomicBool,
    /// While set, every run final this node logs conflicts.
    pub conflict_finals: std::sync::atomic::AtomicBool,
    pub run_open_cap: std::sync::atomic::AtomicUsize,
    /// Fails the next mark commit: 1 before it is proposed, 2 after it applied.
    pub commit_fault: std::sync::atomic::AtomicU8,
    /// The waits of the leader's run driving, shortened by tests.
    pub limits: std::sync::Mutex<crate::node::ComputeLimits>,
    /// While set, this node's system reads fail.
    pub system_outage: std::sync::atomic::AtomicBool,
}

#[cfg(feature = "testing")]
impl Default for Probes {
    fn default() -> Self {
        Self {
            peer_blobs: std::sync::Mutex::new(crate::node::PeerBlobs::Accept),
            file_pushes: std::sync::atomic::AtomicU64::new(0),
            blob_puts_seen: std::sync::atomic::AtomicU64::new(0),
            conflict_next_call: std::sync::atomic::AtomicBool::new(false),
            conflict_finals: std::sync::atomic::AtomicBool::new(false),
            run_open_cap: std::sync::atomic::AtomicUsize::new(MAX_RUN_OPEN_BYTES),
            commit_fault: std::sync::atomic::AtomicU8::new(0),
            limits: std::sync::Mutex::new(crate::node::ComputeLimits::default()),
            system_outage: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

/// Runs `fut` to completion from a run's blocking thread. A current-thread runtime runs the
/// run in place on its only thread, where blocking would panic: refused with `None`.
fn block_on<T>(runtime: &Handle, fut: impl Future<Output = T>) -> Option<T> {
    (runtime.runtime_flavor() != RuntimeFlavor::CurrentThread).then(|| runtime.block_on(fut))
}

fn files_unavailable() -> ApiError {
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
    marks: Marks,
    node_open: Arc<Semaphore>,
    run_open: Arc<Semaphore>,
    /// Logged runs wait a little for the node-wide budget; views do not.
    waits: bool,
}

impl NodeFiles {
    pub(super) fn new(node: &Node, app_id: &str, marks: Marks, kind: InvocationKind) -> Self {
        Self {
            node: node.this.lock().map(|w| w.clone()).unwrap_or_default(),
            runtime: Handle::try_current().ok(),
            app_id: app_id.to_owned(),
            marks,
            node_open: Arc::clone(&node.open_file_bytes),
            run_open: Arc::new(Semaphore::new(node.run_open_cap())),
            waits: !matches!(
                kind,
                InvocationKind::View | InvocationKind::Action | InvocationKind::Migrate
            ),
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

    /// The ciphertext of `id`: this node's if it is the file, else the app's other replicas'.
    fn ciphertext(&self, id: &str) -> Option<Vec<u8>> {
        let node = self.node.upgrade()?;
        match node.engine.get_blob(id) {
            Ok(Some(bytes)) if hex::encode(calimero_identity::sha256(&bytes)) == id => {
                return Some(bytes)
            }
            Ok(Some(_)) => {
                warn!(file = %id, "a stored file is not its id; fetching it again");
                let _dropped = node.engine.delete_blob(id);
            }
            _ => {}
        }
        let app = node.sys(|s| s.apps.get(&self.app_id).cloned())?;
        let peers = node.other_replicas(&app);
        let fetch = tokio::time::timeout(FETCH_WAIT, node.fetch_blob_from(id, &peers));
        block_on(self.runtime.as_ref()?, fetch)?.ok()?.ok()
    }
}

impl Files for NodeFiles {
    fn open(&self, id: &str) -> Result<Box<dyn FileRead>, FileError> {
        let mark = self
            .marks
            .get(id)
            .ok_or_else(|| FileError::Failed(NO_SUCH_FILE.to_owned()))?;
        let held = u32::try_from(sealed_len(mark.size)).map_err(|_| FileError::Unavailable)?;
        let permits = self.reserve(held).ok_or(FileError::Unavailable)?;
        let ciphertext = self.ciphertext(id).ok_or(FileError::Unavailable)?;
        let file_key = self.file_key().ok_or(FileError::Unavailable)?;
        Ok(Box::new(SealedReader::new(
            ciphertext,
            mark,
            &self.app_id,
            &file_key,
            permits,
        )))
    }

    fn info(&self, id: &str) -> Option<FileInfo> {
        let mark = self.marks.get(id)?;
        Some(FileInfo {
            size: mark.size,
            sha256: mark.sha256.0.clone(),
        })
    }

    fn create(&self) -> Result<Box<dyn FileWrite>, FileError> {
        let file_key = self.file_key().ok_or(FileError::Unavailable)?;
        Ok(Box::new(SealedWriter {
            sealer: Some(FileSealer::new()),
            ciphertext: Vec::new(),
            app_id: self.app_id.clone(),
            file_key,
        }))
    }
}

/// Seals what the guest writes into memory; the node stores it when the run is logged.
struct SealedWriter {
    sealer: Option<FileSealer>,
    ciphertext: Vec<u8>,
    app_id: String,
    file_key: Zeroizing<[u8; 32]>,
}

impl FileWrite for SealedWriter {
    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        let sealer = self.sealer.as_mut().ok_or_else(|| FILE_FAILED.to_owned())?;
        let sealed = sealer.write(bytes).map_err(|_| FILE_FAILED.to_owned())?;
        self.ciphertext.extend_from_slice(&sealed);
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<NewFile, String> {
        let sealer = self.sealer.take().ok_or_else(|| FILE_FAILED.to_owned())?;
        let data_key = Zeroizing::new(*sealer.data_key());
        let sealed = sealer.finish().map_err(|_| FILE_FAILED.to_owned())?;
        self.ciphertext.extend_from_slice(&sealed.tail);
        let wrapped_key = wrap_key(&self.file_key, &self.app_id, &data_key)
            .map_err(|_| FILE_FAILED.to_owned())?;
        Ok(NewFile {
            id: hex::encode(calimero_identity::sha256(&self.ciphertext)),
            size: sealed.size,
            sha256: sealed.sha256.to_vec(),
            wrapped_key,
            ciphertext: std::mem::take(&mut self.ciphertext),
        })
    }
}

/// Decrypts a file as the guest reads it, one chunk at a time. Any failure of the
/// opener, including the one that ends the stream, is [`ReadError::Corrupt`].
struct SealedReader {
    ciphertext: Vec<u8>,
    at: usize,
    /// `None` once the stream is proven, or when the mark's key does not open.
    check: Option<Check>,
    proven: bool,
    pending: Vec<u8>,
    /// Count the ciphertext against the run's and the node's open-bytes budgets until the reader drops.
    _held: (OwnedSemaphorePermit, OwnedSemaphorePermit),
}

struct Check {
    opener: FileOpener,
    size: u64,
    sha256: [u8; 32],
}

impl SealedReader {
    fn new(
        ciphertext: Vec<u8>,
        mark: &FileMark,
        app_id: &str,
        file_key: &[u8; 32],
        held: (OwnedSemaphorePermit, OwnedSemaphorePermit),
    ) -> Self {
        let check = unwrap_key(file_key, app_id, &mark.wrapped_key.0)
            .ok()
            .zip(<[u8; 32]>::try_from(mark.sha256.0.as_slice()).ok())
            .map(|(key, sha256)| Check {
                opener: FileOpener::new(&key),
                size: mark.size,
                sha256,
            });
        Self {
            ciphertext,
            at: 0,
            check,
            proven: false,
            pending: Vec::new(),
            _held: held,
        }
    }

    /// Opens the next piece of the stream into `pending`, or proves the end.
    fn advance(&mut self) -> Result<(), ReadError> {
        let check = self.check.as_mut().ok_or(ReadError::Corrupt)?;
        if let Some(rest) = self.ciphertext.get(self.at..).filter(|r| !r.is_empty()) {
            let piece = &rest[..rest.len().min(CHUNK_BYTES + TAG_BYTES)];
            self.pending = check.opener.push(piece).map_err(|_| ReadError::Corrupt)?;
            self.at += piece.len();
            return Ok(());
        }
        let check = self.check.take().ok_or(ReadError::Corrupt)?;
        self.pending = check
            .opener
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

impl Node {
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
        let targets = (self.push_targets(app_id).await).ok_or_else(files_unavailable)?;
        let held: usize = blobs.iter().map(|(_, bytes)| bytes.len()).sum();
        let held = u32::try_from(held).map_err(|_| files_unavailable())?;
        let push = async {
            let _budget = Arc::clone(&self.push_bytes)
                .acquire_many_owned(held)
                .await
                .map_err(|_| files_unavailable())?;
            let pushed = futures_util::future::join_all(
                blobs
                    .iter()
                    .map(|(id, bytes)| self.push_blob(id, bytes, &targets)),
            )
            .await;
            pushed.into_iter().collect::<Result<Vec<()>, ApiError>>()
        };
        match tokio::time::timeout(PUSH_WAIT, push).await {
            Ok(Ok(_)) => Ok(()),
            _ => {
                // No mark names these files, so no other run can need them.
                self.discard_bytes(
                    app_id,
                    blobs.iter().map(|(id, _)| id.clone()).collect(),
                    outputs,
                );
                Err(files_unavailable())
            }
        }
    }

    /// The app group's other voters and learners, as its own Raft membership names them, but
    /// none whose release may not hold the app's data ([`Node::may_send`]).
    async fn push_targets(&self, app_id: &str) -> Option<Targets> {
        let status = self.hosted(app_id)?.status();
        let group = self.sys(|s| s.apps.get(app_id).map(|a| a.group))?;
        let members: Vec<(NodeId, String)> = self.sys(|s| {
            (status.voters.iter().chain(&status.learners))
                .filter_map(|id| s.nodes.get(id).map(|n| (*id, n.addr.clone())))
                .collect()
        });
        let mut allowed = HashMap::new();
        for (id, addr) in members {
            if self.may_send(&addr, group).await {
                let _prev = allowed.insert(id, addr);
            }
        }
        Some(Targets::of(
            self.config.node_id,
            &status.voters,
            &status.learners,
            |id| allowed.get(&id).cloned(),
        ))
    }

    async fn push_blob(&self, id: &str, bytes: &[u8], targets: &Targets) -> Result<(), ApiError> {
        self.engine
            .put_blob(id, bytes)
            .map_err(|e| ApiError::internal(&e.to_string()))?;
        let body = BlobPut {
            hash: id,
            wasm: STANDARD.encode(bytes),
        };
        let (voters, _learners) = futures_util::future::join(
            futures_util::future::join_all(
                targets
                    .voters
                    .iter()
                    .map(|addr| self.peer_post(addr, "internal/blob", &body)),
            ),
            futures_util::future::join_all(
                targets
                    .learners
                    .iter()
                    .map(|addr| self.peer_post(addr, "internal/blob", &body)),
            ),
        )
        .await;
        if targets.is_majority(voters.iter().filter(|r| r.is_ok()).count()) {
            Ok(())
        } else {
            Err(files_unavailable())
        }
    }

    /// [`Self::delete_files_later`] for ids no mark names; a compute run's outputs only after the
    /// app's marks were read through its leader.
    fn discard_bytes(&self, app_id: &str, ids: Vec<String>, outputs: bool) {
        if !outputs {
            self.delete_files_later(app_id, ids);
            return;
        }
        let Some(node) = self.this.lock().ok().and_then(|w| w.upgrade()) else {
            return;
        };
        let app_id = app_id.to_owned();
        drop(tokio::spawn(async move {
            node.delete_unmarked_outputs(&app_id, ids).await;
        }));
    }

    /// Deletes the ids of compute outputs that no mark names, judged after a barrier committed as
    /// the app's leader and a strong read: until then a final of another leader may be about to
    /// mark them. Without the barrier (not the leader, no quorum) the bytes stay: an over-count
    /// and some orphaned bytes, never a mark without its bytes.
    pub(super) async fn delete_unmarked_outputs(&self, app_id: &str, ids: Vec<String>) {
        let Some(g) = self.hosted(app_id) else {
            return;
        };
        let barrier = serde_json::to_vec(&ComponentCmd::Barrier).unwrap_or_default();
        if g.write(barrier).await.is_err() {
            return;
        }
        let unmarked = g
            .read_strong(|m, _| {
                ids.iter()
                    .filter(|id| !m.state.component.files.contains_key(*id))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .await;
        if let Ok(unmarked) = unmarked {
            self.delete_files(app_id, &unmarked).await;
        }
    }

    /// Whether a component app hosted here marks `id` as one of its files.
    pub(crate) fn marks_file(&self, id: &str) -> bool {
        self.apps.read().is_ok_and(|apps| {
            apps.values()
                .any(|g| g.read_local(|m, _| m.state.component.files.contains_key(id)))
        })
    }

    /// Removes the bytes of `ids` on this node and the app's other replicas, in a task of
    /// its own: nobody waits for it. A replica that misses it keeps unreachable bytes.
    pub(super) fn delete_files_later(&self, app_id: &str, ids: Vec<String>) {
        let Some(node) = self.this.lock().ok().and_then(|w| w.upgrade()) else {
            return;
        };
        let app_id = app_id.to_owned();
        drop(tokio::spawn(async move {
            let removal = node.delete_files(&app_id, &ids);
            if tokio::time::timeout(DELETE_WAIT, removal).await.is_err() {
                warn!(app = %app_id, "file bytes not deleted everywhere in time");
            }
        }));
    }

    async fn delete_files(&self, app_id: &str, ids: &[String]) {
        // The nodes a push went to: the group's voters and learners.
        let peers: Vec<String> = self
            .push_targets(app_id)
            .await
            .map(|t| [t.voters, t.learners].concat())
            .unwrap_or_default();
        for batch in ids.chunks(MAX_DELETE_CONCURRENCY) {
            let _all =
                futures_util::future::join_all(batch.iter().map(|id| self.delete_file(id, &peers)))
                    .await;
        }
    }

    async fn delete_file(&self, id: &str, peers: &[String]) {
        if self.is_code_blob(id) || self.marks_file(id) {
            return;
        }
        if let Err(e) = self.engine.delete_blob(id) {
            warn!(file = %id, error = %e, "file bytes not deleted");
        }
        let body = json!({ "hash": id });
        let _sent = futures_util::future::join_all(
            peers
                .iter()
                .map(|addr| self.peer_post(addr, "internal/blob/delete", &body)),
        )
        .await;
    }
}

impl Node {
    /// Counts `delta` plaintext bytes of files (negative: a refund) against the app's payer in the
    /// system log; `429 QUOTA_EXCEEDED` when a charge would pass the quota.
    pub(super) async fn count_file_bytes(&self, app_id: &str, delta: i64) -> Result<(), ApiError> {
        let cmd = SystemCmd::ChargeFileBytes {
            app_id: app_id.to_owned(),
            delta,
        };
        self.sys_write(&cmd).await.map(drop)
    }

    /// Gives back `bytes` after the entry that held them was removed or deleted them.
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
    pub(super) fn discard(&self, app_id: &str, stored: Stored) {
        if !stored.ids.is_empty() {
            self.discard_bytes(app_id, stored.ids, stored.outputs);
        }
        if stored.charged > 0 {
            self.refund_files_later(app_id, stored.charged);
        }
    }

    /// Seals `bytes` as a file of the app: a fresh data key, wrapped under the app's file key.
    fn seal_upload(&self, app_id: &str, bytes: &[u8]) -> Result<NewFile, ApiError> {
        let sealed = NodeFiles::new(self, app_id, Marks::default(), InvocationKind::Call)
            .create()
            .and_then(|mut writer| {
                writer.write(bytes).map_err(FileError::Failed)?;
                writer.finish().map_err(FileError::Failed)
            });
        sealed.map_err(|_| ApiError::internal("the file could not be sealed"))
    }

    /// Stores an uploaded file for a component app. This node seals and stores nothing unless it
    /// leads the app: the body goes to the leader, which seals it, counts it against the quota,
    /// puts it on a majority of its group's voters and commits the mark. The key is per call, so
    /// only a forward that is retried replays a committed upload.
    pub(crate) async fn upload_file(
        &self,
        app: &AppRecord,
        bytes: &[u8],
    ) -> Result<Uploaded, ApiError> {
        if bytes.len() > MAX_BLOB_BYTES {
            return Err(crate::node::blob_too_large());
        }
        let key = format!(
            "{}{}",
            crate::app::NODE_KEY_PREFIX,
            calimero_identity::b64url(&calimero_identity::random::<16>())
        );
        let mut last = ApiError::unavailable("no replica of the app answered");
        let mut leader: Option<String> = None;
        for _attempt in 0..30 {
            let done = if let Some(addr) = leader.take() {
                self.forward_upload(&addr, &app.app_id, &key, bytes).await
            } else if let Some(g) = self.replica_group(app) {
                match self.upload_local(&g, &app.app_id, &key, bytes).await {
                    Ok(uploaded) => Ok(uploaded),
                    Err(Upload::Refused(e)) => Err(e),
                    Err(Upload::Elsewhere(addr)) => {
                        leader = addr;
                        continue;
                    }
                }
            } else if let Some(addr) = self.other_replica(app) {
                self.forward_upload(&addr, &app.app_id, &key, bytes).await
            } else {
                Err(last.clone())
            };
            match done {
                Err(e) if e.code == NOT_LEADER => {
                    leader = e.details["leaderUrl"].as_str().map(str::to_owned);
                    if leader.is_some() {
                        continue;
                    }
                    last = e;
                }
                Err(e) if e.code == "UNAVAILABLE" => last = e,
                done => return done,
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        Err(last)
    }

    /// [`Self::upload_file`] as a peer asks it, one hop: this node hosts the app, and if it is
    /// not the leader it names it (`421`) instead of passing the body on again.
    pub(crate) async fn upload_here(
        &self,
        app_id: &str,
        key: &str,
        bytes: &[u8],
    ) -> Result<Uploaded, ApiError> {
        if bytes.len() > MAX_BLOB_BYTES {
            return Err(crate::node::blob_too_large());
        }
        let g = self
            .hosted(app_id)
            .ok_or_else(|| ApiError::unavailable("app not hosted here"))?;
        match self.upload_local(&g, app_id, key, bytes).await {
            Ok(uploaded) => Ok(uploaded),
            Err(Upload::Refused(e)) => Err(e),
            Err(Upload::Elsewhere(Some(addr))) => {
                Err(
                    ApiError::new(421, NOT_LEADER, "send this to the app group's leader")
                        .with_details(json!({ "leaderUrl": addr })),
                )
            }
            Err(Upload::Elsewhere(None)) => Err(ApiError::unavailable("the app has no leader yet")),
        }
    }

    async fn forward_upload(
        &self,
        addr: &str,
        app_id: &str,
        key: &str,
        bytes: &[u8],
    ) -> Result<Uploaded, ApiError> {
        let body = json!({
            "app": app_id,
            "key": key,
            "bytes": calimero_log::Bytes(bytes.to_vec()),
        });
        let answer = self
            .peer_post(addr, "internal/file-upload", &body)
            .await
            .map_err(|e| {
                e.downcast::<ApiError>()
                    .unwrap_or_else(|e| ApiError::unavailable(&e.to_string()))
            })?;
        serde_json::from_value(answer).map_err(|e| ApiError::internal(&e.to_string()))
    }

    /// The upload on the app's leader; [`Upload::Elsewhere`] when this node is not it.
    async fn upload_local(
        &self,
        g: &Group<AppMachine>,
        app_id: &str,
        key: &str,
        bytes: &[u8],
    ) -> Result<Uploaded, Upload> {
        let status = g.status();
        if !status.is_leader {
            let leader = status
                .leader
                .and_then(|id| self.sys(|s| s.nodes.get(&id).map(|n| n.addr.clone())));
            return Err(Upload::Elsewhere(leader));
        }
        // A retry of an upload that committed is its first answer.
        if let Some(prev) = g.read_local(|m, _| m.component_replay(&Caller::System, key)) {
            return uploaded(&prev)
                .ok_or_else(|| Upload::Refused(ApiError::internal("bad record")));
        }
        let mut file = off_workers(|| self.seal_upload(app_id, bytes)).map_err(Upload::Refused)?;
        let size = file.size;
        let delta = i64::try_from(size)
            .map_err(|_| Upload::Refused(ApiError::internal("file too large")))?;
        self.count_file_bytes(app_id, delta)
            .await
            .map_err(Upload::Refused)?;
        let blob = (file.id.clone(), std::mem::take(&mut file.ciphertext));
        if let Err(e) = self.push_files(app_id, &[blob], false).await {
            self.refund_files(app_id, size).await;
            return Err(Upload::Refused(e));
        }
        self.commit_mark(g, app_id, FileMark::from(file), key).await
    }

    /// Commits the mark of a file whose bytes are stored. When the commit fails, the leader says
    /// whether the mark is there: if so the upload succeeded, if not its bytes and charge go.
    async fn commit_mark(
        &self,
        g: &Group<AppMachine>,
        app_id: &str,
        mark: FileMark,
        key: &str,
    ) -> Result<Uploaded, Upload> {
        let cmd = ComponentCmd::File {
            mark: mark.clone(),
            key: Some(key.to_owned()),
        };
        let bytes = serde_json::to_vec(&cmd).unwrap_or_default();
        let error = match self.propose_command(g, Some(key), bytes).await {
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
            None => Err(Upload::Refused(status_of(&error))),
        }
    }

    /// Whether `id` is marked, once the entry that failed cannot commit any more: a barrier
    /// entry committed through this leader commits every entry before it in its log and ends
    /// any other. `None` when no barrier could be committed (this node no longer leads).
    async fn marked_after_barrier(&self, g: &Group<AppMachine>, id: &str) -> Option<(bool, u64)> {
        let barrier = serde_json::to_vec(&ComponentCmd::Barrier).unwrap_or_default();
        let _committed = g.write(barrier).await.ok()?;
        g.read_strong(|m, applied| (m.state.component.files.contains_key(id), applied))
            .await
            .ok()
    }

    async fn undo_upload(&self, app_id: &str, mark: &FileMark) {
        self.refund_files(app_id, mark.size).await;
        self.delete_files_later(app_id, vec![mark.id.clone()]);
    }

    /// Appends a node command to the app's log, or answers its recorded answer for `key`.
    async fn propose_command(
        &self,
        g: &Group<AppMachine>,
        key: Option<&str>,
        entry: Vec<u8>,
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
            if let Ok(ComponentCmd::File { mut mark, key }) = serde_json::from_slice(&entry) {
                mark.id = "ab".repeat(32);
                let other = ComponentCmd::File { mark, key };
                let _ = g
                    .write(serde_json::to_vec(&other).unwrap_or_default())
                    .await;
            }
        }
        let proposed = g
            .propose(|m, _| {
                let prev = key.and_then(|k| m.component_replay(&Caller::System, k));
                match prev {
                    Some(prev) => Proposal::Reply(serde_json::to_vec(&prev).unwrap_or_default()),
                    None => Proposal::Append(entry.clone()),
                }
            })
            .await?;
        #[cfg(feature = "testing")]
        if fault == 2 {
            return Err(WriteError::Unavailable("injected after".into()));
        }
        serde_json::from_slice(&proposed.output).map_err(|e| WriteError::Unavailable(e.to_string()))
    }

    /// Forgets one of the app's files through its leader, refunds the bytes the entry really
    /// removed, and answers its index; `None` when the app has no such file. Each replica
    /// drops the bytes as it applies.
    pub(crate) async fn forget_file(
        &self,
        app: &AppRecord,
        caller: Caller,
        id: &str,
    ) -> Result<Option<u64>, ApiError> {
        let answer = self
            .call(&CallRequest::host(
                app.app_id.clone(),
                FILE_FORGET,
                json!({ "id": id }),
                caller,
            ))
            .await?;
        if answer.status != crate::app::Status::Ok {
            return Ok(None);
        }
        // A forget that fails with an unknown outcome refunds nothing: an over-count, never a loss.
        self.refund_files(&app.app_id, answer.result["size"].as_u64().unwrap_or(0))
            .await;
        Ok(Some(answer.index))
    }

    /// The forget command, on the app's leader: one entry in its log.
    pub(in crate::node) async fn file_local(
        &self,
        g: &Group<AppMachine>,
        req: &CallRequest,
    ) -> Result<Applied, WriteError> {
        let cmd = ComponentCmd::FileForget {
            id: req.args["id"].as_str().unwrap_or_default().to_owned(),
            key: req.idempotency_key.clone(),
        };
        let entry = serde_json::to_vec(&cmd).unwrap_or_default();
        let answer = self
            .propose_command(g, req.idempotency_key.as_deref(), entry)
            .await?;
        Ok(super::to_wasmer_applied(answer))
    }
}

/// Why an upload did not finish on this node.
enum Upload {
    Refused(ApiError),
    /// This node does not lead the app; the leader's address, if known.
    Elsewhere(Option<String>),
}

/// An upload's answer as a command recorded: the mark it committed and the entry's index.
fn uploaded(answer: &crate::component::Applied) -> Option<Uploaded> {
    Some(Uploaded {
        mark: serde_json::from_value(answer.result["mark"].clone()).ok()?,
        index: answer.index,
    })
}

impl Node {
    /// Runs the cleanup of compute outputs `ids` as a losing leader would. Tests only.
    #[cfg(feature = "testing")]
    pub async fn clean_up_outputs(&self, app_id: &str, ids: Vec<String>) {
        self.delete_unmarked_outputs(app_id, ids).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// A sealed file and the mark that describes it.
    fn sealed(plain: &[u8]) -> (Vec<u8>, FileMark, [u8; 32]) {
        let file_key = [7u8; 32];
        let mut sealer = FileSealer::new();
        let mut ciphertext = sealer.write(plain).unwrap();
        let data_key = *sealer.data_key();
        let done = sealer.finish().unwrap();
        ciphertext.extend_from_slice(&done.tail);
        let mark = FileMark {
            id: String::new(),
            size: done.size,
            sha256: calimero_log::Bytes(done.sha256.to_vec()),
            wrapped_key: calimero_log::Bytes(wrap_key(&file_key, "app", &data_key).unwrap()),
        };
        (ciphertext, mark, file_key)
    }

    fn read_all(
        ciphertext: Vec<u8>,
        mark: &FileMark,
        file_key: &[u8; 32],
    ) -> Result<Vec<u8>, ReadError> {
        let permit = || Arc::new(Semaphore::new(1)).try_acquire_owned().unwrap();
        let mut reader = SealedReader::new(ciphertext, mark, "app", file_key, (permit(), permit()));
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
        let plain: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let (ciphertext, mark, key) = sealed(&plain);
        assert_eq!(read_all(ciphertext.clone(), &mark, &key).unwrap(), plain);

        let mut altered = ciphertext.clone();
        altered[ciphertext.len() / 2] ^= 1;
        assert_eq!(read_all(altered, &mark, &key), Err(ReadError::Corrupt));

        // Cut on a chunk boundary: every chunk opens, the stream ends too early.
        let cut = ciphertext[..CHUNK_BYTES + TAG_BYTES].to_vec();
        assert_eq!(read_all(cut, &mark, &key), Err(ReadError::Corrupt));

        let (other, _other_mark, _) = sealed(&plain);
        assert_eq!(read_all(other, &mark, &key), Err(ReadError::Corrupt));

        let wrong_hash = FileMark {
            sha256: calimero_log::Bytes(vec![0; 32]),
            ..mark.clone()
        };
        assert_eq!(
            read_all(ciphertext.clone(), &wrong_hash, &key),
            Err(ReadError::Corrupt)
        );
        assert_eq!(
            read_all(ciphertext, &mark, &[8; 32]),
            Err(ReadError::Corrupt)
        );
    }
}
