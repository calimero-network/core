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
use calimero_log::NodeId;
use serde::Serialize;
use serde_json::json;
use tokio::runtime::{Handle, RuntimeFlavor};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::warn;
use zeroize::Zeroizing;

use crate::component::{FileMark, Marks};
use crate::error::ApiError;
use crate::files::{
    sealed_len, unwrap_key, wrap_key, FileOpener, FileSealer, CHUNK_BYTES, TAG_BYTES,
};
use crate::node::{off_workers, Node, FORWARD_TIMEOUT};

/// The longest a run waits to fetch a file it does not hold from the app's replicas.
const FETCH_WAIT: Duration = Duration::from_secs(5);
/// The longest a call waits for its files to be taken: inside one forward, so the client's
/// idempotency key makes a retry safe.
const PUSH_WAIT: Duration = Duration::from_secs(4);
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

const _: () = assert!(PUSH_WAIT.as_millis() < FORWARD_TIMEOUT.as_millis());

/// A file's ciphertext as the host core hands it over, with the blob id it is stored under.
pub type Blob = (String, Vec<u8>);

/// Knobs and counters for tests (feature `testing`).
#[cfg(feature = "testing")]
pub(crate) struct Probes {
    pub peer_blobs: std::sync::Mutex<crate::node::PeerBlobs>,
    pub file_pushes: std::sync::atomic::AtomicU64,
    pub blob_puts_seen: std::sync::atomic::AtomicU64,
    pub conflict_next_call: std::sync::atomic::AtomicBool,
    pub run_open_cap: std::sync::atomic::AtomicUsize,
}

#[cfg(feature = "testing")]
impl Default for Probes {
    fn default() -> Self {
        Self {
            peer_blobs: std::sync::Mutex::new(crate::node::PeerBlobs::Accept),
            file_pushes: std::sync::atomic::AtomicU64::new(0),
            blob_puts_seen: std::sync::atomic::AtomicU64::new(0),
            conflict_next_call: std::sync::atomic::AtomicBool::new(false),
            run_open_cap: std::sync::atomic::AtomicUsize::new(MAX_RUN_OPEN_BYTES),
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
        off_workers(|| block_on(&runtime, self.push_files(app_id, blobs)))
            .unwrap_or_else(|| Err(files_unavailable()))
    }

    async fn push_files(&self, app_id: &str, blobs: &[Blob]) -> Result<(), ApiError> {
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
                self.delete_files_later(app_id, blobs.iter().map(|(id, _)| id.clone()).collect());
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
        if self.is_code_blob(id) {
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
