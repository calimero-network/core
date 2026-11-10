//! The node's component executor: running `calimero:app` components (the v2 SDK)
//! on wasmtime through the host core, for the apps whose bundle is a component.
//!
//! Everything an app does besides running its code (routing, the API, the system
//! group, sessions, packages, presence, receipts, upgrades' authority) is the
//! node's. What is here: preparing components
//! (single-flight per bundle), the ABI gate, unique ids (D52), app time (D16,
//! D19), timers (D34), and the calls, views, `init` and code-only upgrades of a
//! component app.
//!
//! Where Sandi's next phases (pipelining, batching, per-key concurrency) will
//! touch this path: every builder below takes `base_index` from
//! `AppState::last_index` (the app's last entry) and every `apply` compares it
//! with the same field (`AppMachine::apply_component_outcome`); a `base_seq` per
//! key or per batch replaces both ends at once. `Node::next_id` runs inside the
//! builder, one id pair per run, so a batch keeps one pair per run it carries.

mod compute;
mod files;
mod uploads;

#[cfg(feature = "testing")]
pub use self::compute::ComputeLimits;
#[cfg(not(feature = "testing"))]
pub(super) use self::compute::ComputeLimits;
pub(super) use self::compute::ComputeState;
pub use self::compute::JOB_POLL;
pub(crate) use self::compute::RUN_VIEW;
#[cfg(feature = "testing")]
pub(super) use self::files::Probes;
pub(crate) use self::files::{valid_uploader, Uploaded};
pub(super) use self::files::{MAX_OPEN_FILE_BYTES, MAX_PENDING_FILE_BYTES, MAX_PUSH_BYTES};
pub(crate) use self::uploads::{UploadOp, Uploads};

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use calimero_app_host::{
    Budget, Engine as _, HostCore, Invocation, InvocationKind, MethodKind, Prepared, Runs,
    TimerCtx, TrapKind,
};
use calimero_log::{Group, GroupId, Proposal, WriteError};
use serde_json::{json, Value};
use tracing::debug;

use self::files::{blobs_of, stored_of_blobs, Blob, NodeFiles, PendingBytes, Stored};
use super::{status_of, CallRequest, Node, ViewAnswer, ViewRequest};
use crate::app::{AppMachine, Status};
use crate::component::blobs::{grant_key, Blobs, RunBlobs, COLLECT_BATCH};
use crate::component::xcall::{backoff_ms, Outgoing, XcallFrom, DELIVER_BATCH, XCALL_WINDOW_MS};
use crate::component::{
    utc_day, Applied, CallEntry, ComponentCmd, DayUsage, Outcome, SysDirectory, TickEntry,
    UsageBook,
};
use crate::error::ApiError;
use crate::metrics::CallPhase;
use crate::types::{now_ms, AppRecord, Caller, Executor};
use crate::usage::{Kind as RunKind, Meter};

/// App time (D16, D19): this host's clock plus a shared offset. It stamps a
/// component's entries and timer slots, nothing else; sessions, tokens and
/// challenges use `types::now_ms`/`now_secs`, and Raft timers use `Instant`.
#[derive(Clone, Debug, Default)]
pub struct Clock {
    offset_ms: Arc<AtomicI64>,
}

impl Clock {
    #[must_use]
    pub fn new(offset_ms: i64) -> Self {
        Self {
            offset_ms: Arc::new(AtomicI64::new(offset_ms)),
        }
    }

    /// Milliseconds since the epoch, on app time.
    #[must_use]
    pub fn now_ms(&self) -> u64 {
        let host = i64::try_from(now_ms()).unwrap_or(i64::MAX);
        let at = host.saturating_add(self.offset_ms.load(Ordering::Relaxed));
        u64::try_from(at).unwrap_or(0)
    }

    /// Moves app time by `ms` (negative: back).
    pub fn advance(&self, ms: i64) {
        let _prev = self.offset_ms.fetch_add(ms, Ordering::Relaxed);
    }
}

/// The error of a call whose caller no longer reaches the app (answered `404`).
pub const NOT_REACHED: &str = "no such app";

/// The code of a component the engine refuses.
pub const BAD_BUNDLE: &str = "BAD_BUNDLE";
/// The code of an upgrade refused because an active compute run still needs a result method the target lacks.
pub const RUN_NEEDS_RESULT_METHOD: &str = "RunNeedsResultMethod";

/// The largest call or `init` arguments a component runs (Ruling 30): well under
/// `MAX_ENTRY_BYTES`, so the entry that logs a refused run always fits.
pub const MAX_ARGS_BYTES: usize = 512 * 1024;

/// How often an app group's leader looks for due timer ticks (D34).
pub const TIMER_POLL: Duration = Duration::from_millis(100);

/// How often a node asks every peer, at once, what the apps it replicates used today,
/// for the fuel cap (D76, D78). The cap reads that answer and never asks a peer
/// during a call, so it is approximate by at most this long: a developer can run past
/// its cap for one interval. 5 s keeps that overshoot a small part of a daily cap
/// and costs each node one small request per peer per interval, however many apps
/// there are. The refresh runs only while a cap is set.
pub const USAGE_REFRESH: Duration = Duration::from_secs(5);

/// What each peer's replicas said its apps used on `day` ([`Node::refresh_usage`]).
/// A peer that does not answer keeps its last figures; a new day starts empty.
#[derive(Debug, Default)]
pub struct PeerUsage {
    day: u64,
    peers: HashMap<u64, HashMap<String, DayUsage>>,
}

impl PeerUsage {
    /// The largest figure any peer reported for `app` on `day` (replicas that lag
    /// report less; usage only grows within a day).
    fn of(&self, app: &str, day: u64) -> DayUsage {
        if self.day != day {
            return DayUsage::default();
        }
        self.peers
            .values()
            .filter_map(|apps| apps.get(app))
            .copied()
            .max_by_key(|u| u.fuel)
            .unwrap_or_default()
    }
}

/// `413 ARGS_TOO_LARGE`: arguments over [`MAX_ARGS_BYTES`], refused before running.
#[must_use]
pub fn args_too_large() -> ApiError {
    ApiError::new(
        413,
        "ARGS_TOO_LARGE",
        &format!("arguments are limited to {MAX_ARGS_BYTES} bytes"),
    )
}

/// `503 TIMEOUT`: the run hit its deadline; not logged, the key not spent (D23, D36).
#[must_use]
pub fn timeout_error() -> ApiError {
    ApiError::new(
        503,
        "TIMEOUT",
        "the run hit its deadline; retry with the same key",
    )
    .with_details(json!({ "retryAfter": 1 }))
}

/// A refusal the host makes for the app, before running it (`422 APP_ERROR`).
#[must_use]
pub fn app_refusal(code: &str) -> ApiError {
    ApiError::new(422, "APP_ERROR", "the app refused the call")
        .with_details(json!({ "code": code, "fields": {} }))
}

/// A run whose entry the log refused as too large: a node bug (the host-denied
/// replacement always fits), never retried.
fn entry_refused(e: &WriteError) -> ApiError {
    ApiError::internal(&format!("the log refused the entry: {e}"))
}

/// A method's JSON arguments as the guest takes them (`null`: none).
#[must_use]
pub fn args_bytes(args: &Value) -> Vec<u8> {
    if args.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(args).unwrap_or_default()
    }
}

/// Why a run on this replica did not happen: the log's answer, or a refusal the
/// replica made before running (its own checks, against its own state). Both
/// executors' local calls answer with it.
pub(super) enum LocalError {
    Write(WriteError),
    Refused(ApiError),
}

impl std::fmt::Display for LocalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Write(e) => e.fmt(f),
            Self::Refused(e) => e.fmt(f),
        }
    }
}

impl From<WriteError> for LocalError {
    fn from(e: WriteError) -> Self {
        Self::Write(e)
    }
}

/// The key the compiled-component cache authenticates its artifacts with (S8): HMAC of
/// K_r over a fixed label, so it changes with the storage key and nothing else.
#[must_use]
pub fn component_cache_key(storage_key: &[u8; 32]) -> [u8; 32] {
    crate::backup::tag(storage_key, "calimero.components.v1")
}

/// The latest slot of a timer with interval `every`, installed at `install_ms`,
/// that is due at `now_ms`: `install_ms + k · every` for the largest `k ≥ 1` not
/// after `now_ms` (D34). Missed slots collapse into this one.
#[must_use]
pub fn due_slot(install_ms: u64, every: Duration, now_ms: u64) -> Option<u64> {
    let every = u64::try_from(every.as_millis()).ok().filter(|e| *e > 0)?;
    let k = now_ms.checked_sub(install_ms)? / every;
    (k >= 1).then(|| install_ms + k * every)
}

/// The ABI checks of [`Node::abi_gate`] against `prepared`, the bundle that runs.
fn abi_check(
    prepared: &Prepared,
    method: &str,
    caller: &Caller,
    view: bool,
    consented: bool,
    from_action: bool,
) -> Result<(), ApiError> {
    if super::reserved_method(method) {
        return Err(app_refusal("UnknownMethod"));
    }
    let Some(m) = prepared.abi.methods.get(method) else {
        return Err(app_refusal("UnknownMethod"));
    };
    // A call of an action starts it (D17); an action never runs as a view, nor is it
    // started by an action, an xcall or the system.
    let starts_action = m.kind == MethodKind::Action
        && !view
        && !from_action
        && matches!(caller, Caller::Account { .. } | Caller::ApiKey { .. });
    if (!matches!(m.kind, MethodKind::Call | MethodKind::View) && !starts_action)
        || m.compute_result
    {
        return Err(app_refusal("WrongKind"));
    }
    // `allow(action)`: only the app's own actions call it, never a client directly.
    if m.action_only && !from_action {
        return Err(app_refusal("Forbidden"));
    }
    if view && m.confirm {
        return Err(app_refusal("ConfirmNotAView"));
    }
    if view && m.kind != MethodKind::View {
        return Err(app_refusal("NotAView"));
    }
    match caller {
        Caller::Public if !m.public => {
            Err(ApiError::unauthenticated("sign in to call this method"))
        }
        Caller::System if !m.allow_system => Err(app_refusal("Forbidden")),
        // A delivered xcall reaches only a method that allows its source (D13).
        Caller::App { app } if !m.allows_app(app) => Err(app_refusal("Forbidden")),
        _ if m.confirm && !view && !consented => Err(app_refusal("ConfirmRequired")),
        _ => Ok(()),
    }
}

/// The code of a final entry that is over the log's cap even without its result method's outcome.
pub const FINAL_TOO_LARGE: &str = "FINAL_TOO_LARGE";

/// `FINAL_TOO_LARGE`: nothing can be logged for the run's end; the leader ends it `Failed{tooLarge}`.
#[must_use]
pub fn final_too_large() -> ApiError {
    ApiError::new(
        500,
        FINAL_TOO_LARGE,
        "the run's final entry is over the log's cap",
    )
}

/// The entry as bytes, and whether it was replaced: one over `MAX_ENTRY_BYTES` becomes the same
/// call trapped `HostDenied` (D50); a final loses only its result method's outcome.
fn entry_bytes(cmd: &mut ComponentCmd) -> (Vec<u8>, bool) {
    let bytes = serde_json::to_vec(&*cmd).unwrap_or_default();
    if bytes.len() <= calimero_log::MAX_ENTRY_BYTES {
        return (bytes, false);
    }
    const DENIED: &str = "entry too large";
    match cmd {
        ComponentCmd::Call(entry) => entry.outcome = entry.outcome.denied(DENIED),
        ComponentCmd::Tick(entry) => entry.outcome = entry.outcome.denied(DENIED),
        ComponentCmd::Init { outcome, .. } => **outcome = outcome.denied(DENIED),
        ComponentCmd::ComputeFinal(entry) => entry.result = entry.result.denied(DENIED),
        ComponentCmd::Upgrade { .. }
        | ComponentCmd::ComputeAttempt(_)
        | ComponentCmd::Blob { .. }
        | ComponentCmd::BlobForget { .. }
        | ComponentCmd::BlobRevoke { .. }
        | ComponentCmd::BlobsCollected { .. }
        | ComponentCmd::File { .. }
        | ComponentCmd::Barrier
        | ComponentCmd::XcallDone { .. }
        | ComponentCmd::ActionEnd { .. } => {}
    }
    (serde_json::to_vec(&*cmd).unwrap_or_default(), true)
}

/// Whether an applied entry recorded none of the blobs it stored: it conflicted, or apply
/// refused it (an entry's outcome that carries blobs is `Ok`).
fn recorded_nothing(a: &Applied) -> bool {
    a.conflict || a.status != Status::Ok
}

/// The outcome of `cmd` whose files its entry stores and charges: a final's outputs were stored
/// and charged by the leader before it took the proposal lock, so only its result method's count.
fn stored_outcome(cmd: &ComponentCmd) -> Option<&Outcome> {
    match cmd {
        ComponentCmd::ComputeFinal(entry) => Some(&entry.result),
        ComponentCmd::Call(entry) => Some(&entry.outcome),
        ComponentCmd::Tick(entry) => Some(&entry.outcome),
        ComponentCmd::Init { outcome, .. } => Some(outcome),
        _ => None,
    }
}

/// The ids `cmd` marks as stored blobs: its new files and the plain blobs it publishes.
fn marked_ids(cmd: &ComponentCmd) -> Vec<&str> {
    stored_outcome(cmd).map_or_else(Vec::new, |o| {
        let files = o.files.iter().map(|f| f.id.as_str());
        files
            .chain(o.published.iter().map(|p| p.id.as_str()))
            .collect()
    })
}

/// The segment hashes of the files `cmd` marks.
fn marked_segments(cmd: &ComponentCmd) -> Vec<&str> {
    stored_outcome(cmd).map_or_else(Vec::new, |o| {
        (o.files.iter())
            .flat_map(|f| f.segments.iter().map(String::as_str))
            .collect()
    })
}

/// The blobs of `all` that are not in `kept`.
fn unmarked(kept: &[Blob], all: &[Blob]) -> Vec<Blob> {
    (all.iter())
        .filter(|b| !kept.iter().any(|k| k.segment == b.segment && k.id == b.id))
        .cloned()
        .collect()
}

/// The plaintext bytes `cmd` charges: its new files, and the published blobs among `blobs`
/// (the app did not hold them yet), once each; none when they overflow a `u64`.
fn new_file_bytes(cmd: &ComponentCmd, blobs: &[Blob]) -> Option<u64> {
    let Some(outcome) = stored_outcome(cmd) else {
        return Some(0);
    };
    let mut charged = BTreeSet::new();
    let published = outcome
        .published
        .iter()
        .filter(|p| blobs.iter().any(|b| !b.segment && b.id == p.id) && charged.insert(&p.id))
        .map(|p| p.size);
    (outcome.files.iter().map(|f| f.size).chain(published)).try_fold(0u64, u64::checked_add)
}

/// The blobs of `blobs` that `cmd` marks: a replaced entry marks fewer than the run made.
fn marked_blobs(cmd: &ComponentCmd, blobs: &[Blob]) -> Vec<Blob> {
    let marked = marked_ids(cmd);
    let segments = marked_segments(cmd);
    blobs
        .iter()
        .filter(|b| {
            if b.segment {
                segments.contains(&b.id.as_str())
            } else {
                marked.contains(&b.id.as_str())
            }
        })
        .cloned()
        .collect()
}

/// Whether a call's outcome changes anything a read-only member may not: keys, events,
/// files, and the compute runs it starts or cancels (they spend the org's run quota).
fn writes(outcome: &Outcome) -> bool {
    !outcome.diff.is_empty()
        || !outcome.events.is_empty()
        || !outcome.files.is_empty()
        || !outcome.published.is_empty()
        || moves_runs(outcome)
}

/// Whether a call's outcome starts or cancels a compute run.
fn moves_runs(outcome: &Outcome) -> bool {
    !outcome.compute_starts.is_empty() || !outcome.compute_cancels.is_empty()
}

/// A finished run: its outcome, and the ciphertext of the files it made, which stays
/// out of the outcome (the log carries marks only).
struct Ran {
    outcome: Outcome,
    blobs: Vec<Blob>,
    /// Keeps the node's budget for `blobs` until the run's entry is proposed or refused.
    _pending: Option<Arc<PendingBytes>>,
}

impl Node {
    /// App time: what a component's entry stamps and timer slots read (D16, D19, D34).
    #[must_use]
    pub fn app_clock(&self) -> Clock {
        self.clock.clone()
    }

    /// Wakes the compute driver: a logged call started or cancelled a run.
    fn kick_runs(&self) {
        #[cfg(feature = "testing")]
        let _n = self.probes.run_kicks.fetch_add(1, Ordering::Relaxed);
        self.runs_kick.notify_one();
    }

    /// How many times a logged call woke the compute driver. Tests only.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn run_kicks(&self) -> u64 {
        self.probes.run_kicks.load(Ordering::Relaxed)
    }

    /// Moves this node's app time by `ms` (negative: back). Tests only.
    #[cfg(feature = "testing")]
    pub fn advance_clock(&self, ms: i64) {
        self.clock.advance(ms);
    }

    /// The key `app`'s file data keys are wrapped under. Tests only.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn app_file_key(&self, app: &str) -> [u8; 32] {
        crate::backup::file_key(&self.cluster_key, app)
    }

    /// Makes this node answer peers' blob requests as `mode` says. Tests only.
    #[cfg(feature = "testing")]
    pub fn set_peer_blobs(&self, mode: super::PeerBlobs) {
        *self
            .probes
            .peer_blobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = mode;
    }

    #[cfg(feature = "testing")]
    pub(crate) fn peer_blobs_mode(&self) -> super::PeerBlobs {
        *self
            .probes
            .peer_blobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Counts a blob put a peer sent here. Tests only.
    #[cfg(feature = "testing")]
    pub(crate) fn note_blob_put(&self) {
        let _n = self.probes.blob_puts_seen.fetch_add(1, Ordering::Relaxed);
    }

    /// How many times this node pushed new files to its replicas. Tests only.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn file_pushes(&self) -> u64 {
        self.probes.file_pushes.load(Ordering::Relaxed)
    }

    /// Fails the next upload's mark commit on this node: `1` before it is proposed, `2` after
    /// it applied. Tests only.
    #[cfg(feature = "testing")]
    pub fn fail_next_mark_commit(&self, mode: u8) {
        self.probes.commit_fault.store(mode, Ordering::Relaxed);
    }

    /// Fails the next barrier a failed mark commit asks for here. Tests only.
    #[cfg(feature = "testing")]
    pub fn fail_next_barrier(&self) {
        self.probes.barrier_fault.store(true, Ordering::Relaxed);
    }

    /// Fails the next segment an upload session stores here. Tests only.
    #[cfg(feature = "testing")]
    pub fn fail_next_segment_store(&self) {
        self.probes
            .segment_store_fault
            .store(true, Ordering::Relaxed);
    }

    /// Lets an upload session hold `n` unpushed segments (default `UPLOAD_WINDOW`). Tests only.
    #[cfg(feature = "testing")]
    pub fn set_upload_window(&self, n: usize) {
        self.probes.upload_window.store(n, Ordering::Relaxed);
    }

    /// How many blob puts peers sent this node. Tests only.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn blob_puts_seen(&self) -> u64 {
        self.probes.blob_puts_seen.load(Ordering::Relaxed)
    }

    /// Makes the next call or run final this node logs conflict with the state it was built on. Tests only.
    #[cfg(feature = "testing")]
    pub fn conflict_next_call(&self) {
        self.probes
            .conflict_next_call
            .store(true, Ordering::Relaxed);
    }

    /// Makes the next call this node logs name a source that is no run's output in its publish marks. Tests only.
    #[cfg(feature = "testing")]
    pub fn forge_next_publish(&self) {
        self.probes
            .forge_next_publish
            .store(true, Ordering::Relaxed);
    }

    /// Holds the next push of files or of a plain blob on this node once their bytes are stored, until the second
    /// notify is notified; the first is notified on arrival. Tests only.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn hold_next_file_push(&self) -> files::Hold {
        files::set_hold(&self.probes.push_hold)
    }

    /// Holds the next blob deletion on this node after its checks, before it deletes. Tests only.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn hold_next_deletion(&self) -> files::Hold {
        files::set_hold(&self.probes.deletion_hold)
    }

    /// Stops at the hold a test set for a push of bytes.
    #[cfg(feature = "testing")]
    pub(crate) async fn push_checkpoint(&self) {
        files::checkpoint(&self.probes.push_hold).await;
    }

    /// Stops at the hold a test set for a blob deletion. Tests only.
    #[cfg(feature = "testing")]
    pub(crate) async fn deletion_checkpoint(&self) {
        files::checkpoint(&self.probes.deletion_hold).await;
    }

    /// Leaves only `leave` places for transfers to job nodes, node-wide. Tests only.
    #[cfg(feature = "testing")]
    pub fn limit_job_transfers(&self, leave: usize) {
        let held = self.job_ops.available_permits().saturating_sub(leave);
        let _forgotten = self.job_ops.forget_permits(held);
    }

    /// Leaves only `leave` places for control requests to job nodes, node-wide. Tests only.
    #[cfg(feature = "testing")]
    pub fn limit_job_control(&self, leave: usize) {
        let held = self.job_control.available_permits().saturating_sub(leave);
        let _forgotten = self.job_control.forget_permits(held);
    }

    /// Makes every run final this node logs conflict while `on`. Tests only.
    #[cfg(feature = "testing")]
    pub fn conflict_finals(&self, on: bool) {
        self.probes.conflict_finals.store(on, Ordering::Relaxed);
    }

    /// Leaves only `leave` bytes for open file readers to take, node-wide. Tests only.
    #[cfg(feature = "testing")]
    pub fn limit_open_file_bytes(&self, leave: usize) {
        let held = self
            .open_file_bytes
            .available_permits()
            .saturating_sub(leave);
        let _forgotten = self.open_file_bytes.forget_permits(held);
    }

    /// Leaves only `leave` bytes for the pushes of new files. Tests only.
    #[cfg(feature = "testing")]
    pub fn limit_push_bytes(&self, leave: usize) {
        let held = self.push_bytes.available_permits().saturating_sub(leave);
        let _forgotten = self.push_bytes.forget_permits(held);
    }

    /// Leaves only `leave` bytes for the new files runs hold before their entries. Tests only.
    #[cfg(feature = "testing")]
    pub fn limit_pending_file_bytes(&self, leave: usize) {
        let held = (self.pending_file_bytes.available_permits()).saturating_sub(leave);
        let _forgotten = self.pending_file_bytes.forget_permits(held);
    }

    /// The bytes of new files runs may still take. Tests only.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn pending_file_bytes_left(&self) -> usize {
        self.pending_file_bytes.available_permits()
    }

    /// Caps the bytes the readers of one run hold at `cap` (new runs). Tests only.
    #[cfg(feature = "testing")]
    pub fn limit_run_open_bytes(&self, cap: usize) {
        self.probes.run_open_cap.store(cap, Ordering::Relaxed);
    }

    /// Replaces the waits that end a run the leader gets no answer for. Tests only.
    #[cfg(feature = "testing")]
    pub fn set_compute_limits(&self, limits: ComputeLimits) {
        *self
            .probes
            .limits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = limits;
    }

    /// Makes this node's system reads fail as if the system group had no leader. Tests only.
    #[cfg(feature = "testing")]
    pub fn set_system_outage(&self, on: bool) {
        self.probes.system_outage.store(on, Ordering::Relaxed);
    }

    /// The waits that end a run the leader gets no answer for.
    pub(super) fn limits(&self) -> ComputeLimits {
        #[cfg(feature = "testing")]
        return *self
            .probes
            .limits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        #[cfg(not(feature = "testing"))]
        ComputeLimits::default()
    }

    /// The bytes the readers of one run may hold.
    fn run_open_cap(&self) -> usize {
        #[cfg(feature = "testing")]
        return self.probes.run_open_cap.load(Ordering::Relaxed);
        #[cfg(not(feature = "testing"))]
        files::MAX_RUN_OPEN_BYTES
    }

    /// The engine every component app runs on.
    #[must_use]
    pub fn app_engine(&self) -> &calimero_app_engine_wasmtime::WasmtimeEngine {
        &self.wasm
    }

    /// The engine a release's compute components are prepared on.
    #[must_use]
    pub fn compute_engine(&self) -> &calimero_app_engine_wasmtime::WasmtimeEngine {
        &self.compute_wasm
    }

    /// The executor `bundle` runs on, from its WASM (the bytes of its package's
    /// service, or of the raw upload).
    pub async fn executor_of(&self, bundle: &str) -> Result<Executor, ApiError> {
        Ok(Executor::of(&self.bundle_wasm(bundle).await?))
    }

    /// The prepared component of `bundle`, preparing it on first use. A component
    /// the engine refuses is `422 BAD_BUNDLE`; a bundle not reachable yet is `503`.
    ///
    /// Single-flight per bundle (Ruling 44): concurrent first uses wait for one
    /// prepare and then read its result, instead of each compiling the component.
    pub async fn prepared_for(&self, bundle: &str) -> Result<Prepared, ApiError> {
        if let Some(p) = self.cached_prepared(bundle) {
            return Ok(p);
        }
        let flight = {
            let mut preparing = self
                .preparing
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(preparing.entry(bundle.to_owned()).or_default())
        };
        let _one_at_a_time = flight.lock().await;
        let result = match self.cached_prepared(bundle) {
            Some(p) => Ok(p),
            None => self.prepare_now(bundle).await,
        };
        // The last holder of the flight drops its entry (the map's reference and this
        // one are all that is left); a failed prepare lets the next waiter try again.
        if let Ok(mut preparing) = self.preparing.lock() {
            if preparing
                .get(bundle)
                .is_some_and(|f| Arc::strong_count(f) <= 2)
            {
                let _done = preparing.remove(bundle);
            }
        }
        result
    }

    fn cached_prepared(&self, bundle: &str) -> Option<Prepared> {
        self.prepared
            .lock()
            .ok()
            .and_then(|m| m.get(bundle).cloned())
    }

    /// Prepares `bundle` and keeps the result ([`Self::prepared_for`]).
    async fn prepare_now(&self, bundle: &str) -> Result<Prepared, ApiError> {
        let component = self.bundle_wasm(bundle).await?;
        self.usage.learn(bundle, &component);
        let prepared = self.prepare_bytes(bundle, component).await?;
        // Components no app uses any more (an upgrade's old bundle, a deleted app)
        // are dropped as a new one comes in, so `prepared` stays bounded by the live
        // apps. Dropping one only costs a prepare from the disk cache if it returns.
        let mut live = self.live_bundles();
        let _new = live.insert(bundle.to_owned());
        if let Ok(mut m) = self.prepared.lock() {
            m.retain(|hash, _| live.contains(hash));
            let _prev = m.insert(bundle.to_owned(), prepared.clone());
        }
        Ok(prepared)
    }

    /// Run the engine's `prepare` on `component` (validation, ABI, compilation),
    /// off the async workers. `422 BAD_BUNDLE` when the engine refuses it.
    pub async fn prepare_bytes(
        &self,
        hash: &str,
        component: Vec<u8>,
    ) -> Result<Prepared, ApiError> {
        #[cfg(feature = "testing")]
        let _n = self.prepares.fetch_add(1, Ordering::Relaxed);
        let (wasm, hash) = (Arc::clone(&self.wasm), hash.to_owned());
        tokio::task::spawn_blocking(move || wasm.prepare(&hash, &component))
            .await
            .map_err(|e| ApiError::internal(&e.to_string()))?
            .map_err(|e| ApiError::new(422, BAD_BUNDLE, &e.to_string()))
    }

    /// The bundles some app uses: the directory's records and what the app groups
    /// hosted here run (an upgrade switches the group before the directory).
    fn live_bundles(&self) -> HashSet<String> {
        let mut live: HashSet<String> =
            self.sys(|s| s.apps.values().map(|a| a.bundle.clone()).collect());
        let groups: Vec<Arc<Group<AppMachine>>> = self
            .apps
            .read()
            .map(|a| a.values().cloned().collect())
            .unwrap_or_default();
        live.extend(
            groups
                .iter()
                .filter_map(|g| g.read_local(|m, _| m.state.bundle.clone())),
        );
        live
    }

    /// How many times this node ran the engine's `prepare`. Tests only.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn prepares(&self) -> u64 {
        self.prepares.load(Ordering::Relaxed)
    }

    fn budget(&self) -> Budget {
        self.config.budget.unwrap_or_else(Budget::call)
    }

    /// The key download grants of every app are MACed with (R8).
    #[must_use]
    pub fn blob_grant_key(&self) -> [u8; 32] {
        grant_key(self.cluster_key())
    }

    /// Run one invocation of `app_id` against `state` and its blobs at `applied`.
    fn run_component(
        &self,
        prepared: &Prepared,
        app_id: &str,
        state: (Arc<dyn calimero_app_host::StateView>, Blobs),
        applied: u64,
        invocation: Invocation,
        extras: RunExtras,
    ) -> Ran {
        self.run_with(prepared, app_id, state, applied, invocation, extras, None)
    }

    /// [`Self::run_component`], with the app's search index for a view (spec section 7).
    /// `extras` says what else the run reaches: other apps synchronously through
    /// [`NodeXcall`] (with the run's own key), and the app's outbound gate (D76).
    #[allow(clippy::too_many_arguments)]
    fn run_with(
        &self,
        prepared: &Prepared,
        app_id: &str,
        (state, blobs): (Arc<dyn calimero_app_host::StateView>, Blobs),
        applied: u64,
        invocation: Invocation,
        extras: RunExtras,
        search: Option<Arc<dyn calimero_app_host::SearchHost>>,
    ) -> Ran {
        let http = extras.outbound.then(|| {
            let meter = self.meter(app_id, &prepared.hash);
            Arc::new(TimedHttp {
                inner: Arc::new(NodeHttp(self.outbound_for(
                    app_id,
                    &prepared.hash,
                    self.usage.outbound(&meter.app, &meter.billed),
                ))),
                timeout: CALL_FETCH_TIMEOUT,
            })
        });
        let xcalls = extras.key.and_then(|key| {
            let node = self.this.lock().ok()?.upgrade()?;
            let handle = tokio::runtime::Handle::try_current().ok()?;
            Some(Arc::new(NodeXcall {
                node,
                source: app_id.to_owned(),
                key,
                handle,
            }) as Arc<dyn calimero_app_host::XcallHost>)
        });
        let this = self.this.lock().map(|w| w.clone()).unwrap_or_default();
        let directory = Arc::new(SysDirectory::new(this, app_id.to_owned()));
        let held = Arc::clone(&blobs);
        let files = NodeFiles::new(self, app_id, Arc::clone(&blobs), invocation.kind);
        let pending = files.pending();
        let files = Arc::new(files);
        let blobs = Arc::new(RunBlobs {
            app_id: app_id.to_owned(),
            records: blobs,
            key: self.blob_grant_key(),
        });
        let owner = self
            .sys(|s| s.apps.get(app_id).map(|a| a.owner.clone()))
            .unwrap_or_default();
        match HostCore::new(app_id, state, applied, invocation, directory) {
            Ok(host) => {
                let host = host.with_blobs(blobs).with_files(files.clone());
                let host = match extras.runs {
                    Some(runs) => host
                        .with_compute(prepared.abi.clone(), runs)
                        .with_compute_component(extras.compute_component),
                    None => host,
                };
                let host = match search {
                    Some(search) => host.with_search(search),
                    None => host,
                };
                let host = match xcalls {
                    Some(xcalls) => host.with_xcall_host(xcalls),
                    None => host,
                };
                let host = match http {
                    Some(http) => host.with_http(http),
                    None => host,
                };
                let mut out = self.wasm.invoke(prepared, host, self.budget());
                let mut blobs: Vec<Blob> = out.new_files.iter_mut().flat_map(blobs_of).collect();
                // A blob the app already holds (or that another publish of this run made) is
                // neither stored nor charged again.
                for p in &mut out.published {
                    let known = calimero_app_host::parse_blob_id(&p.id)
                        .is_some_and(|id| held.contains_key(&id))
                        || blobs.iter().any(|b| !b.segment && b.id == p.id);
                    if !known {
                        blobs.push(Blob::plain(p.id.clone(), std::mem::take(&mut p.bytes)));
                    }
                }
                files.forget_made(&blobs);
                let mut outcome = Outcome::from_host(out);
                if !outcome.compute_starts.is_empty() {
                    outcome.name_owner(&owner);
                }
                Ran {
                    outcome,
                    blobs,
                    _pending: Some(pending),
                }
            }
            Err(e) => Ran {
                outcome: Outcome::trap(TrapKind::Fault, &e.to_string()),
                blobs: Vec::new(),
                _pending: None,
            },
        }
    }

    /// [`Self::run_component`] for a run that becomes a log entry (call, tick,
    /// `init`), which runs inside `Group::propose_led`, on the machine's read lock,
    /// for up to its deadline: off the async workers ([`Node::metered`], which
    /// adds what it took to `meter`'s app).
    #[allow(clippy::too_many_arguments)]
    fn run_logged(
        &self,
        meter: &Meter,
        prepared: &Prepared,
        app_id: &str,
        state: (Arc<dyn calimero_app_host::StateView>, Blobs),
        applied: u64,
        invocation: Invocation,
        extras: RunExtras,
    ) -> Ran {
        self.metered(meter, invocation.kind, || {
            self.run_component(prepared, app_id, state, applied, invocation, extras)
        })
    }

    /// The entry that logs `cmd`, once the files it marks are on a majority of the voters and
    /// counted against the quota, and what was stored for it. A replaced entry marks none.
    fn logged_entry(
        &self,
        app_id: &str,
        #[cfg_attr(not(feature = "testing"), allow(unused_mut))] mut cmd: ComponentCmd,
        blobs: &[Blob],
    ) -> Result<(Vec<u8>, Stored), ApiError> {
        #[cfg(feature = "testing")]
        if matches!(cmd, ComponentCmd::Call(_) | ComponentCmd::ComputeFinal(_))
            && self
                .probes
                .conflict_next_call
                .swap(false, Ordering::Relaxed)
        {
            match &mut cmd {
                ComponentCmd::Call(call) => call.base_index += 1,
                ComponentCmd::ComputeFinal(entry) => entry.base_index += 1,
                _ => {}
            }
        }
        #[cfg(feature = "testing")]
        if let ComponentCmd::Call(call) = &mut cmd {
            if self
                .probes
                .forge_next_publish
                .swap(false, Ordering::Relaxed)
            {
                for mark in &mut call.outcome.published {
                    mark.source = "00".repeat(32);
                }
            }
        }
        #[cfg(feature = "testing")]
        if let ComponentCmd::ComputeFinal(entry) = &mut cmd {
            if self.probes.conflict_finals.load(Ordering::Relaxed) {
                entry.base_index += 1;
            }
        }
        let (bytes, replaced) = entry_bytes(&mut cmd);
        if matches!(cmd, ComponentCmd::ComputeFinal(_))
            && bytes.len() > calimero_log::MAX_ENTRY_BYTES
        {
            return Err(final_too_large());
        }
        let charged = new_file_bytes(&cmd, blobs)
            .ok_or_else(|| ApiError::internal("the sizes of the new files overflow"))?;
        if replaced && charged == 0 {
            self.forget_local(blobs);
            return Ok((bytes, Stored::default()));
        }
        let kept;
        let blobs = if replaced {
            kept = marked_blobs(&cmd, blobs);
            self.forget_local(&unmarked(&kept, blobs));
            &kept
        } else {
            blobs
        };
        // Counted first: a refused charge sends no bytes anywhere.
        if charged > 0 {
            if let Err(e) = self.charge_files(app_id, charged) {
                self.forget_local(blobs);
                return Err(e);
            }
        }
        let plain: Vec<String> = (blobs.iter())
            .filter(|b| !b.segment)
            .map(|b| b.id.clone())
            .collect();
        let pin = self.blob_guards.pin(plain);
        if let Err(e) = self.persist_files(app_id, blobs) {
            self.discard(
                app_id,
                Stored {
                    charged,
                    ..Stored::default()
                },
            );
            return Err(e);
        }
        let segments: Vec<String> = (blobs.iter())
            .filter(|b| b.segment)
            .map(|b| b.id.clone())
            .collect();
        let stored = Stored {
            charged,
            pin: Some(pin),
            _pin: Some(self.pin_segments(&segments)),
            ..stored_of_blobs(blobs)
        };
        Ok((bytes, stored))
    }

    /// [`Node::discard`] after a failed proposal, unless the entry may still commit
    /// (a new leader can append it): only `NotLeader` and `TooLarge` mean it never will.
    fn discard_failed(&self, app_id: &str, stored: Stored, error: &WriteError) {
        if matches!(
            error,
            WriteError::NotLeader { .. } | WriteError::TooLarge { .. }
        ) {
            self.discard(app_id, stored);
        }
    }

    /// The next `(id_term, id_seq)` for a run this node leads in `term` (D52,
    /// Ruling 29). `term` comes from the leadership check that admitted the
    /// proposal (`calimero_log::Group::propose_led`, no round of its own);
    /// `logged` is the highest pair the app's applied state carries, which, after
    /// `propose_led`'s once-per-term catch-up, includes every entry this node
    /// appended in `term`. The counter continues from the larger of this node's
    /// memory and the log, so a node that leads the same term again after a
    /// restart never reissues an id.
    ///
    /// A poisoned lock is recovered, not answered with a fixed pair (which every later
    /// run would share): the map holds plain counters, each changed by one assignment.
    fn next_id(&self, group: GroupId, term: u64, logged: (u64, u64)) -> (u64, u64) {
        let mut ids = self
            .id_seqs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = ids.entry(group).or_insert((term, 0));
        if entry.0 != term {
            *entry = (term, 0);
        }
        if logged.0 == term {
            entry.1 = entry.1.max(logged.1);
        }
        entry.1 += 1;
        (term, entry.1)
    }

    /// The timestamp of a component's next entry (D16): never before the last one applied.
    fn stamp(&self, m: &AppMachine) -> u64 {
        (m.state.component.head.last_ts_ms + 1).max(self.clock.now_ms())
    }

    /// `init` of a component app, on this node, the app group's leader.
    pub(super) async fn init_component(
        &self,
        g: &Group<AppMachine>,
        app: &AppRecord,
    ) -> Result<crate::app::Applied, ApiError> {
        app.require_app_component()?;
        // A component the engine refuses fails `init` for good (reported as
        // INIT_FAILED); anything else is retried by `reconcile`.
        // The bundle the group names if an upgrade gave it one before init applied
        // (its first bundle's init timed out), else the app record's.
        let (bundle, compute_bundle) = g
            .read_local(|m, _| {
                let head = &m.state.component.head;
                m.state
                    .bundle
                    .clone()
                    .map(|b| (b, head.compute_bundle.clone()))
            })
            .unwrap_or_else(|| (app.bundle.clone(), app.compute_bundle.clone()));
        let prepared = match self.prepared_for(&bundle).await {
            Ok(p) => Ok(p),
            Err(e) if e.code == BAD_BUNDLE => Err(format!("component refused: {}", e.message)),
            Err(e) => return Err(e),
        };
        let owner = Caller::Account {
            account: app.owner.clone(),
        };
        let args = args_bytes(&app.init_args);
        if args.len() > MAX_ARGS_BYTES {
            return Err(args_too_large());
        }
        let mut timed_out = false;
        let mut not_stored = None;
        let mut persisted = Stored::default();
        let meter = self.meter(&app.app_id, &bundle);
        let p = g
            .propose_led(|m, applied, leadership| {
                not_stored = None;
                if m.state.initialized || m.state.init_error.is_some() {
                    return Proposal::Reply(Vec::new());
                }
                let ts_ms = self.stamp(m);
                let (id_term, id_seq) =
                    self.next_id(g.id(), leadership.term, m.state.component.head.last_id);
                let ran = match &prepared {
                    Ok(prepared) => self.run_logged(
                        &meter,
                        prepared,
                        &app.app_id,
                        (m.view(), m.blob_view()),
                        applied,
                        Invocation {
                            kind: InvocationKind::Init,
                            method: "init".into(),
                            args: args.clone(),
                            caller: owner.to_host(),
                            entry_ts_ms: ts_ms,
                            id_term,
                            id_seq,
                            seed: calimero_identity::random::<32>(),
                            timer: None,
                            compute: None,
                        },
                        RunExtras {
                            runs: Some(m.runs_provider(None)),
                            compute_component: compute_bundle.is_some(),
                            ..RunExtras::default()
                        },
                    ),
                    Err(why) => Ran {
                        outcome: Outcome::trap(TrapKind::Fault, why),
                        blobs: Vec::new(),
                        _pending: None,
                    },
                };
                let outcome = ran.outcome;
                if outcome.is_retryable() {
                    // Not logged (D23): reconcile runs init again.
                    timed_out = true;
                    return Proposal::Reply(Vec::new());
                }
                timed_out = false;
                let cmd = ComponentCmd::Init {
                    bundle: bundle.clone(),
                    compute_bundle: compute_bundle.clone(),
                    base_index: m.state.last_index,
                    ts_ms,
                    id_term,
                    id_seq,
                    outcome: Box::new(outcome),
                };
                match self.logged_entry(&app.app_id, cmd, &ran.blobs) {
                    Ok((entry, stored)) => {
                        persisted = stored;
                        Proposal::Append(entry)
                    }
                    Err(e) => {
                        not_stored = Some(e);
                        Proposal::Reply(Vec::new())
                    }
                }
            })
            .await
            .map_err(|e| {
                self.discard_failed(&app.app_id, std::mem::take(&mut persisted), &e);
                status_of(&e)
            })?;
        if timed_out {
            return Err(timeout_error());
        }
        if let Some(e) = not_stored {
            return Err(e);
        }
        if !p.appended {
            return Err(ApiError::conflict("already initialized"));
        }
        let a: Applied =
            serde_json::from_slice(&p.output).map_err(|e| ApiError::internal(&e.to_string()))?;
        if recorded_nothing(&a) {
            self.discard(&app.app_id, persisted);
        }
        let a = to_host_applied(a);
        self.count_conflict(&a);
        if prepared.is_ok() && !a.conflict {
            self.usage.called(&meter, RunKind::Mutation);
        }
        Ok(a)
    }

    /// One round of timer ticks (D34), every [`TIMER_POLL`]: for each component
    /// app group this node leads, a task proposes the due ticks, unless the
    /// previous round for that app is still running.
    pub fn tick_timers(self: &Arc<Self>) {
        let led = self.led_component_groups();
        for (group, g) in led {
            let Ok(mut ticking) = self.ticking.lock() else {
                return;
            };
            if !ticking.insert(group) {
                continue;
            }
            drop(ticking);
            let node = Arc::clone(self);
            drop(tokio::spawn(async move {
                node.tick_app(&g).await;
                if let Ok(mut t) = node.ticking.lock() {
                    let _was = t.remove(&group);
                }
            }));
        }
    }

    /// The component app groups this node leads.
    pub(super) fn led_component_groups(&self) -> Vec<(GroupId, Arc<Group<AppMachine>>)> {
        self.apps
            .read()
            .map(|a| {
                a.iter()
                    .filter(|(_, g)| g.status().is_leader)
                    .filter(|(_, g)| {
                        g.read_local(|m, _| m.state.executor() == Some(Executor::Component))
                    })
                    .map(|(id, g)| (*id, Arc::clone(g)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Propose the latest due slot of each of the app's timers not yet applied
    /// (one catch-up tick for a gap). Nothing runs before the app is initialized.
    async fn tick_app(&self, g: &Arc<Group<AppMachine>>) {
        let id = g.read_local(|m, _| m.app_id.clone());
        if self.sys(|s| s.apps.get(&id).is_none_or(|a| !a.components.is_app())) {
            return;
        }
        let Some((app_id, bundle, install, applied)) = g.read_local(|m, _| {
            let bundle = m.state.bundle.clone().filter(|_| m.state.initialized)?;
            Some((
                m.app_id.clone(),
                bundle,
                m.state.component.head.install_ts_ms,
                m.state.component.head.timers.clone(),
            ))
        }) else {
            return;
        };
        let collect = g.read_local(|m, _| m.state.component.head.collect.clone());
        if !collect.is_empty() {
            self.collect_component_blobs(g, collect).await;
        }
        let Ok(prepared) = self.prepared_for(&bundle).await else {
            return;
        };
        self.deliver_xcalls(g, &app_id, &prepared).await;
        self.run_actions(g, &app_id, &prepared);
        let now = self.clock.now_ms();
        for (timer, method) in &prepared.abi.methods {
            if method.kind != MethodKind::Timer {
                continue;
            }
            let Some(slot) = method
                .every
                .and_then(|every| due_slot(install, every, now))
                .filter(|slot| applied.get(timer).is_none_or(|last| last < slot))
            else {
                continue;
            };
            if let Err(e) = self.tick(g, &app_id, &prepared, timer, slot).await {
                debug!(app = %app_id, timer, slot, error = %e, "tick not proposed");
            }
        }
    }

    /// Run `timer` for `slot` on this node, the app group's leader, and log it.
    /// A tick that times out is not logged (D23): the next poll proposes the
    /// same slot again, unless a later one is due by then.
    async fn tick(
        &self,
        g: &Group<AppMachine>,
        app_id: &str,
        prepared: &Prepared,
        timer: &str,
        slot: u64,
    ) -> Result<(), WriteError> {
        let meter = self.meter(app_id, &prepared.hash);
        let mut persisted = Stored::default();
        let p = g
            .propose_led(|m, applied, leadership| {
                // An upgrade applied since the round read the ABI: the next poll
                // reads the new one.
                if m.tick_applied(timer, slot)
                    || m.state.bundle.as_deref() != Some(prepared.hash.as_str())
                {
                    return Proposal::Reply(Vec::new());
                }
                let previous_ms = m.state.component.head.timers.get(timer).copied();
                let ts_ms = self.stamp(m);
                let (id_term, id_seq) =
                    self.next_id(g.id(), leadership.term, m.state.component.head.last_id);
                let ran = self.run_logged(
                    &meter,
                    prepared,
                    app_id,
                    (m.view(), m.blob_view()),
                    applied,
                    Invocation {
                        kind: InvocationKind::Timer,
                        method: timer.to_owned(),
                        args: Vec::new(),
                        caller: calimero_app_host::Caller::System,
                        entry_ts_ms: ts_ms,
                        id_term,
                        id_seq,
                        seed: calimero_identity::random::<32>(),
                        timer: Some(TimerCtx {
                            scheduled_ms: slot,
                            previous_ms,
                        }),
                        compute: None,
                    },
                    RunExtras {
                        key: Some(format!("tick:{timer}:{slot}")),
                        outbound: true,
                        runs: Some(m.runs_provider(None)),
                        compute_component: m.state.component.head.compute_bundle.is_some(),
                    },
                );
                let outcome = ran.outcome;
                if outcome.is_retryable() {
                    debug!(app = %app_id, timer, slot, "tick hit its wall clock or the node was unavailable; proposed again");
                    return Proposal::Reply(Vec::new());
                }
                let cmd = ComponentCmd::Tick(Box::new(TickEntry {
                    timer: timer.to_owned(),
                    scheduled_ms: slot,
                    previous_ms,
                    ts_ms,
                    base_index: m.state.last_index,
                    id_term,
                    id_seq,
                    outcome,
                }));
                match self.logged_entry(app_id, cmd, &ran.blobs) {
                    Ok((entry, stored)) => {
                        persisted = stored;
                        Proposal::Append(entry)
                    }
                    Err(e) => {
                        debug!(app = %app_id, timer, slot, error = %e, "tick files not stored; proposed again");
                        Proposal::Reply(Vec::new())
                    }
                }
            })
            .await
            .inspect_err(|e| self.discard_failed(app_id, std::mem::take(&mut persisted), e))?;
        // A tick that was retryable (wall clock or node unavailable) was not logged and runs again.
        if p.appended {
            self.usage.called(&meter, RunKind::FollowUp);
            let applied: Applied = serde_json::from_slice(&p.output)
                .map_err(|e| WriteError::Unavailable(e.to_string()))?;
            if recorded_nothing(&applied) {
                self.discard(app_id, persisted);
            }
        }
        Ok(())
    }

    /// Run a component's call on this node, which must be the app group's leader.
    async fn call_local_component(
        &self,
        g: &Group<AppMachine>,
        req: &CallRequest,
    ) -> Result<Applied, LocalError> {
        let bundle = g
            .read_local(|m, _| m.component_bundle())
            .ok_or_else(|| WriteError::Unavailable("the app is not initialized yet".into()))?;
        let prepared = self
            .prepared_for(&bundle)
            .await
            .map_err(|e| WriteError::Unavailable(e.message))?;
        // The caller's standing (org role, teams, read-only rows) is read from the
        // system state below: it includes every change committed before the call,
        // wherever it was committed. A client's call carries the index of the
        // round its gate made (`CallRequest::sys_index`), which covers that
        // (`Node::sys_fresh_from`); a timer, delivery or action runs a round.
        let sys_index = match req.sys_index {
            Some(i) => {
                let _caught_up = self.sys_fresh_from(Some(i)).await;
                Some(i)
            }
            None => self.sys_fresh_index().await.0,
        };
        let app = self.sys(|s| s.apps.get(&req.app_id).cloned());
        if let (Some(app), Some(account)) = (&app, req.caller.account()) {
            if !self.sys(|s| s.may_reach(app, Some(account))) {
                return Ok(Applied::unlogged(
                    g.applied(),
                    Outcome {
                        status: Status::AppError,
                        error: Value::String(NOT_REACHED.into()),
                        ..Outcome::default()
                    },
                ));
            }
        }
        // A read-only member's call runs, but one that would write keys or emit
        // events is refused (v1 discarded such writes; a client is told).
        let read_only = match &app {
            Some(app) => crate::api::read_only_in(self, app, req.caller.account(), sys_index).await,
            None => false,
        };
        // A developer over its daily fuel (D76, D78) is refused before a client's call,
        // a delivered xcall or a synchronous xcall runs; a retry of a call that
        // committed still replays, below. An action's own calls are not refused: the
        // action was started by a call that was checked, as an event handler's run is.
        let over_quota = match &app {
            Some(app) if req.action.is_none() => self.fuel_quota_refusal(app),
            _ => None,
        };
        // A delivered xcall only within one organization (or one owner's personal apps),
        // judged again here on this replica's system state.
        let source = req
            .xcall
            .as_ref()
            .map(|from| from.source.as_str())
            .or(req.sync_from.as_deref());
        if let Some(source) = source {
            if !self.sys(|s| same_boundary(s, source, &req.app_id)) {
                return Err(LocalError::Refused(app_refusal("Forbidden")));
            }
        }
        let kind = if source.is_some() {
            InvocationKind::XcallDeliver
        } else {
            InvocationKind::Call
        };
        let args = args_bytes(&req.args);
        let mut refused = None;
        let meter = self.meter(&req.app_id, &bundle);
        let mut answered = false;
        let mut persisted = Stored::default();
        // The run that built the entry, and when the entry was proposed: a logged
        // call's two phases (`calimero_call_phase_duration_seconds`).
        let mut ran_for = None;
        let mut proposed_at = None;
        let mut moved_runs = false;
        let p = g
            .propose_led(|m, applied, leadership| {
                refused = None;
                answered = false;
                moved_runs = false;
                ran_for = None;
                proposed_at = None;
                // A retry of a call that committed is its recorded answer, whatever
                // the bundle runs now (D70): the replay comes before the ABI check.
                if let Some(key) = &req.idempotency_key {
                    if let Some(prev) = m.component_replay(&req.caller, key) {
                        return Proposal::Reply(serde_json::to_vec(&prev).unwrap_or_default());
                    }
                }
                // A message applied here already answers as delivered (D25).
                if let Some(from) = &req.xcall {
                    if m.state.component.head.xcall.seen(from) {
                        let seen = Applied::unlogged(
                            applied,
                            Outcome {
                                status: Status::Ok,
                                ..Outcome::default()
                            },
                        );
                        return Proposal::Reply(serde_json::to_vec(&seen).unwrap_or_default());
                    }
                }
                if let Some(e) = &over_quota {
                    refused = Some(e.clone());
                    return Proposal::Reply(Vec::new());
                }
                // An upgrade applied since the checks: run it again on the new bundle.
                if m.state.bundle.as_deref() != Some(bundle.as_str()) {
                    let again = Applied {
                        conflict: true,
                        ..Applied::unlogged(applied, Outcome::default())
                    };
                    return Proposal::Reply(serde_json::to_vec(&again).unwrap_or_default());
                }
                // The ABI checks again, against the bundle this replica runs (Ruling 40).
                if let Err(e) = abi_check(
                    &prepared,
                    &req.method,
                    &req.caller,
                    false,
                    req.consent.is_some(),
                    req.action.is_some(),
                ) {
                    refused = Some(e);
                    return Proposal::Reply(Vec::new());
                }
                // A read-only member reaches views only: any other method is refused
                // before it runs, so no run burns the developer's fuel uncounted.
                let view = prepared
                    .abi
                    .methods
                    .get(&req.method)
                    .is_some_and(|m| m.kind == MethodKind::View);
                if read_only && !view {
                    return Proposal::Reply(
                        serde_json::to_vec(&read_only_refusal(applied)).unwrap_or_default(),
                    );
                }
                let ts_ms = self.stamp(m);
                let (id_term, id_seq) =
                    self.next_id(g.id(), leadership.term, m.state.component.head.last_id);
                let starts_action = prepared
                    .abi
                    .methods
                    .get(&req.method)
                    .is_some_and(|m| m.kind == MethodKind::Action);
                let ran = if starts_action {
                    // Started, not run here: logged as started, run by the runner (D17).
                    let id = format!("a.{id_term}.{id_seq}.0");
                    Ran {
                        outcome: Outcome {
                            status: Status::Ok,
                            result: json!({ "actionId": id }),
                            actions: vec![crate::component::action::ActionOut {
                                id,
                                method: req.method.clone(),
                                args: req.args.clone(),
                            }],
                            ..Outcome::default()
                        },
                        blobs: Vec::new(),
                        _pending: None,
                    }
                } else {
                    let started = Instant::now();
                    let ran = self.run_logged(
                        &meter,
                        &prepared,
                        &req.app_id,
                        (m.view(), m.blob_view()),
                        applied,
                        Invocation {
                            kind,
                            method: req.method.clone(),
                            args: args.clone(),
                            caller: req.caller.to_host(),
                            entry_ts_ms: ts_ms,
                            id_term,
                            id_seq,
                            seed: calimero_identity::random::<32>(),
                            timer: None,
                            compute: None,
                        },
                        RunExtras {
                            // The caller is part of the key: two callers' calls under the
                            // same key are two calls of the target.
                            key: Some(format!(
                                "call:{}:{}",
                                serde_json::to_string(&req.caller).unwrap_or_default(),
                                req.idempotency_key.as_deref().unwrap_or_default()
                            )),
                            // A read-only member's run reaches no outbound host.
                            outbound: !read_only,
                            runs: Some(m.runs_provider(None)),
                            compute_component: m.state.component.head.compute_bundle.is_some(),
                        },
                    );
                    ran_for = Some(started.elapsed());
                    ran
                };
                let outcome = ran.outcome;
                // A retryable run (wall clock or node unavailable) is not logged and does not spend the
                // key (D23).
                if outcome.is_retryable() {
                    let a = Applied::unlogged(applied, outcome);
                    return Proposal::Reply(serde_json::to_vec(&a).unwrap_or_default());
                }
                answered = true;
                moved_runs = moves_runs(&outcome);
                // A view reached through a call (an API node that read an older ABI)
                // still may not write for a read-only member. Events and files are
                // logged side effects too (Ruling 41): an emit-only run is refused like
                // one that writes keys.
                if read_only && writes(&outcome) {
                    return Proposal::Reply(
                        serde_json::to_vec(&read_only_refusal(applied)).unwrap_or_default(),
                    );
                }
                let cmd = ComponentCmd::Call(Box::new(CallEntry {
                    method: req.method.clone(),
                    args: req.args.clone(),
                    caller: req.caller.clone(),
                    idempotency_key: req.idempotency_key.clone(),
                    consent: req.consent.clone(),
                    ts_ms,
                    base_index: m.state.last_index,
                    id_term,
                    id_seq,
                    outcome,
                    xcall: req.xcall.clone(),
                }));
                // The files are on a majority of the replicas before the entry that marks
                // them is proposed; if not, nothing is logged and the key is not spent.
                match self.logged_entry(&req.app_id, cmd, &ran.blobs) {
                    Ok((entry, stored)) => {
                        persisted = stored;
                        proposed_at = Some(Instant::now());
                        Proposal::Append(entry)
                    }
                    Err(e) => {
                        answered = false;
                        refused = Some(e);
                        Proposal::Reply(Vec::new())
                    }
                }
            })
            .await
            .inspect_err(|e| self.discard_failed(&req.app_id, std::mem::take(&mut persisted), e))?;
        if let Some(e) = refused {
            return Err(LocalError::Refused(e));
        }
        if p.appended {
            if let Some(took) = ran_for {
                crate::metrics::call_phase(CallPhase::Run, took);
            }
            if let Some(at) = proposed_at {
                crate::metrics::call_phase(CallPhase::Commit, at.elapsed());
            }
        }
        let a: Applied = serde_json::from_slice(&p.output)
            .map_err(|e| LocalError::Write(WriteError::Unavailable(e.to_string())))?;
        if p.appended && moved_runs {
            self.kick_runs();
        }
        if p.appended && a.conflict {
            let _prev = self
                .conflicts
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if p.appended && recorded_nothing(&a) {
            self.discard(&req.app_id, persisted);
        }
        if answered && !a.conflict {
            self.usage.called(&meter, RunKind::Mutation);
        }
        Ok(a)
    }

    /// `app`'s usage book (Q20), from a replica: this node's if it is one of the app's
    /// replicas, else one that is. An app recorded before components (SDK 0.12, no
    /// longer run) has none.
    pub async fn app_usage(&self, app: &AppRecord) -> Result<UsageBook, ApiError> {
        if app.executor != Executor::Component {
            return Ok(UsageBook::default());
        }
        if let Some(g) = self.replica_group(app) {
            return Ok(g.read_local(|m, _| m.state.component.head.usage.clone()));
        }
        let body = json!({ "app": app.app_id });
        for id in &app.replicas {
            let Some(addr) = self.sys(|s| s.nodes.get(id).map(|n| n.addr.clone())) else {
                continue;
            };
            if let Ok(v) = self.peer_forward(&addr, "internal/usage", &body).await {
                if let Ok(book) = serde_json::from_value::<UsageBook>(v["usage"].clone()) {
                    return Ok(book);
                }
            }
        }
        Err(ApiError::unavailable("no replica answered"))
    }

    /// What each component app this node replicates used on `day`, by app id: what a
    /// peer asks for in [`Self::refresh_usage`].
    #[must_use]
    pub fn replicas_usage_on(&self, day: u64) -> HashMap<String, DayUsage> {
        let apps: Vec<AppRecord> = self.sys(|s| {
            s.apps
                .values()
                .filter(|a| a.executor == Executor::Component)
                .cloned()
                .collect()
        });
        apps.iter()
            .filter_map(|a| {
                let g = self.replica_group(a)?;
                let used = g.read_local(|m, _| m.state.component.head.usage.on(day));
                Some((a.app_id.clone(), used))
            })
            .collect()
    }

    /// One round of [`USAGE_REFRESH`]: ask every peer at once what the apps it
    /// replicates used today, and keep the answers. A peer that does not answer
    /// within the interval keeps its last figures. Nothing is asked while no fuel
    /// cap is set.
    pub async fn refresh_usage(&self) {
        if self.sys(|s| s.quotas.max_fuel_per_developer_per_day) == 0 {
            return;
        }
        let day = utc_day(self.clock.now_ms());
        let peers: Vec<(u64, String)> = self.sys(|s| {
            s.nodes
                .values()
                .filter(|n| n.id != self.config.node_id && !s.removed_nodes.contains(&n.id))
                .map(|n| (n.id, n.addr.clone()))
                .collect()
        });
        let body = json!({ "day": day });
        let answers = futures_util::future::join_all(peers.into_iter().map(|(id, addr)| {
            let body = &body;
            async move {
                let asked = self.peer_post(&addr, "internal/usage/today", body);
                let answer = match tokio::time::timeout(USAGE_REFRESH, asked).await {
                    Ok(Ok(v)) => serde_json::from_value::<HashMap<String, DayUsage>>(v).ok(),
                    Ok(Err(e)) => {
                        debug!(peer = id, error = %e, "usage not refreshed; keeping the last");
                        None
                    }
                    Err(_) => {
                        debug!(peer = id, "usage not refreshed in time; keeping the last");
                        None
                    }
                };
                (id, answer)
            }
        }))
        .await;
        let Ok(mut known) = self.peer_usage.lock() else {
            return;
        };
        if known.day != day {
            *known = PeerUsage {
                day,
                peers: HashMap::new(),
            };
        }
        for (id, answer) in answers {
            if let Some(apps) = answer {
                let _prev = known.peers.insert(id, apps);
            }
        }
    }

    /// What `developer`'s component apps used today (UTC, on app time), for the API:
    /// each app's book read from a replica, all apps at once (Q20, D78). The developer
    /// is `usage::developer`'s (#4839), the identity usage is billed to: the owner of
    /// the deployment an app was created from (an organization's id, or
    /// `account:{id}`), else the app's organization or `account:{owner}`
    /// (`usage::account_payer`). An app no replica answers for counts as nothing.
    pub async fn developer_usage_today(&self, developer: &str) -> DayUsage {
        let today = utc_day(self.clock.now_ms());
        let apps = self.developer_apps(developer);
        let days = futures_util::future::join_all(apps.iter().map(|app| async move {
            match self.app_usage(app).await {
                Ok(book) => book.on(today),
                Err(e) => {
                    debug!(app = %app.app_id, error = %e, "usage not known; counted as none");
                    DayUsage::default()
                }
            }
        }))
        .await;
        let mut total = DayUsage::default();
        for day in &days {
            total.add(day);
        }
        total
    }

    /// The component apps `developer` is billed for (`usage::Billed::of`).
    fn developer_apps(&self, developer: &str) -> Vec<AppRecord> {
        self.sys(|s| {
            s.apps
                .values()
                .filter(|a| {
                    a.executor == Executor::Component
                        && crate::usage::Billed::of(s, a).developer == developer
                })
                .cloned()
                .collect()
        })
    }

    /// `developer`'s fuel today as the cap sees it, without asking any peer: the apps
    /// this node replicates read from the replica, the others from the last
    /// [`Self::refresh_usage`] (so up to [`USAGE_REFRESH`] old).
    fn developer_fuel_cached(&self, developer: &str) -> u64 {
        let today = utc_day(self.clock.now_ms());
        let apps = self.developer_apps(developer);
        let known = self.peer_usage.lock().ok();
        apps.iter()
            .map(|app| match self.replica_group(app) {
                Some(g) => g.read_local(|m, _| m.state.component.head.usage.on(today).fuel),
                None => known.as_ref().map_or(0, |k| k.of(&app.app_id, today).fuel),
            })
            .fold(0, u64::saturating_add)
    }

    /// The refusal of a new call to `app` when its developer (D78: the identity usage
    /// is billed to, `usage::developer`) is at or over `max_fuel_per_developer_per_day`
    /// (D76; 0: no limit, nothing is read). Reads no peer
    /// ([`Self::developer_fuel_cached`]). Only a client's call or an xcall is refused:
    /// `init`, timer ticks, event handlers and an action's own calls still run, and
    /// they still count toward the developer's day.
    pub(super) fn fuel_quota_refusal(&self, app: &AppRecord) -> Option<ApiError> {
        let limit = self.sys(|s| s.quotas.max_fuel_per_developer_per_day);
        if limit == 0 {
            return None;
        }
        let developer = self.sys(|s| crate::usage::Billed::of(s, app).developer);
        let used = self.developer_fuel_cached(&developer);
        (used >= limit).then(|| {
            super::rejected(&format!(
                "{}: fuel per developer per day ({used} of {limit})",
                crate::system::QUOTA_PREFIX
            ))
        })
    }

    /// The ABI checks the host makes before invoking a component (S5, D13), from
    /// the bundle's own ABI, default-deny (Ruling 31): a method the ABI does not
    /// name is `UnknownMethod`; `init` and timers are never reached this way
    /// (`WrongKind`); a `confirm` method never runs as a view (`ConfirmNotAView`)
    /// and a call to one needs a consent; only views run as views (`NotAView`);
    /// anonymous callers reach only `public` methods (`401 UNAUTHENTICATED`) and
    /// the System caller only `allowSystem` ones (`Forbidden`).
    ///
    /// The ABI is the one of the bundle the app group runs (Ruling 40,
    /// [`Self::running_bundle`]), not the directory's record of it, which an
    /// upgrade writes second. The replica that runs the call or view makes the same
    /// checks again against exactly the bundle it runs.
    pub async fn abi_gate(
        &self,
        app: &AppRecord,
        method: &str,
        caller: &Caller,
        view: bool,
        consented: bool,
    ) -> Result<(), ApiError> {
        let prepared = self.prepared_for(&self.running_bundle(app)).await?;
        abi_check(&prepared, method, caller, view, consented, false)
    }

    /// The bundle `app` runs: the app group's own record on this replica, else (this
    /// node does not host the app, or it is not initialized here yet) the directory's.
    /// Only a hint off the replica that runs: that one checks against its own state.
    #[must_use]
    pub fn running_bundle(&self, app: &AppRecord) -> String {
        self.app_group(app.group)
            .and_then(|g| g.read_local(|m, _| m.state.bundle.clone()))
            .unwrap_or_else(|| app.bundle.clone())
    }

    /// Run a component app's call wherever the app's leader is.
    pub async fn call_component(&self, req: &CallRequest) -> Result<Applied, ApiError> {
        // The loop below retries forwards that timed out and calls that raced a
        // leader change; a call that did commit must replay, not run again.
        // A call that came with a key may be a retry of one that committed, which
        // replays whatever the bundle runs now (D70); only the replica that runs it
        // can tell, after the replay, so the ABI is checked here only for a call
        // that came without one.
        let came_keyed = req.idempotency_key.is_some();
        let keyed;
        let req = if req.idempotency_key.is_some() {
            req
        } else {
            let mut r = req.clone();
            r.idempotency_key = Some(format!(
                "{}{}",
                crate::app::NODE_KEY_PREFIX,
                calimero_identity::b64url(&calimero_identity::random::<16>())
            ));
            keyed = r;
            &keyed
        };
        if args_bytes(&req.args).len() > MAX_ARGS_BYTES {
            return Err(args_too_large());
        }
        let app = self.app(&req.app_id).await?;
        app.require_app_component()?;
        if !came_keyed {
            self.abi_gate(&app, &req.method, &req.caller, false, req.consent.is_some())
                .await?;
        }
        let body = serde_json::to_value(req).map_err(|e| ApiError::internal(&e.to_string()))?;
        for _attempt in 0..30 {
            // A conflict (the diff was built on state a leader change replaced)
            // applied nothing and recorded no idempotency key: run it again.
            let target = if let Some(g) = self.replica_group(&app) {
                match self.call_local_component(&g, req).await {
                    Ok(a) if a.conflict => None,
                    Ok(a) => return Ok(a),
                    Err(LocalError::Refused(e)) => return Err(e),
                    Err(LocalError::Write(e @ WriteError::TooLarge { .. })) => {
                        return Err(entry_refused(&e))
                    }
                    Err(LocalError::Write(WriteError::NotLeader {
                        leader: Some((id, addr)),
                    })) if id != self.config.node_id && !addr.is_empty() => Some(addr),
                    Err(LocalError::Write(e)) => {
                        debug!(error = %e, "call not yet possible here");
                        None
                    }
                }
            } else {
                self.other_replica(&app)
            };
            if let Some(addr) = target {
                match self.peer_forward(&addr, "internal/call", &body).await {
                    Ok(v) => {
                        let a: Applied = serde_json::from_value(v)
                            .map_err(|e| ApiError::internal(&e.to_string()))?;
                        if !a.conflict {
                            return Ok(a);
                        }
                    }
                    Err(e) => match e.downcast::<ApiError>() {
                        Ok(api) if api.code != "UNAVAILABLE" => return Err(api),
                        _ => {}
                    },
                }
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        Err(ApiError::unavailable("the app has no leader"))
    }

    /// Answer an internal call of a component app (from a peer): run it here,
    /// without forwarding further.
    pub(super) async fn call_here_component(
        &self,
        app: &AppRecord,
        req: &CallRequest,
    ) -> Result<Applied, ApiError> {
        if args_bytes(&req.args).len() > MAX_ARGS_BYTES {
            return Err(args_too_large());
        }
        // The ABI is checked by `call_local_component`, after the replay (D70).
        let g = self
            .app_group(app.group)
            .ok_or_else(|| ApiError::unavailable("app not hosted here"))?;
        match self.call_local_component(&g, req).await {
            Ok(a) => Ok(a),
            Err(LocalError::Refused(e)) => Err(e),
            Err(LocalError::Write(WriteError::NotLeader {
                leader: Some((id, addr)),
            })) if id != self.config.node_id && !addr.is_empty() => {
                let body =
                    serde_json::to_value(req).map_err(|e| ApiError::internal(&e.to_string()))?;
                let v = self
                    .peer_forward(&addr, "internal/call", &body)
                    .await
                    .map_err(|e| {
                        e.downcast::<ApiError>()
                            .unwrap_or_else(|e| ApiError::unavailable(&e.to_string()))
                    })?;
                serde_json::from_value(v).map_err(|e| ApiError::internal(&e.to_string()))
            }
            Err(LocalError::Write(e @ WriteError::TooLarge { .. })) => Err(entry_refused(&e)),
            Err(LocalError::Write(e)) => Err(status_of(&e)),
        }
    }

    /// A component's view on this replica: it holds its version of the state (D46)
    /// and runs outside the machine's lock; it sees the last applied entry's time
    /// (D19), and runs the bundle that state belongs to, whose ABI it is checked
    /// against (Ruling 40).
    pub(super) async fn view_local_component(
        &self,
        g: &Group<AppMachine>,
        req: &ViewRequest,
    ) -> Result<ViewAnswer, ApiError> {
        if req.method == RUN_VIEW {
            return self.run_view(g, req);
        }
        let bundle = g
            .read_local(|m, _| m.component_bundle())
            .ok_or_else(|| ApiError::unavailable("the app is not initialized yet"))?;
        let prepared = self.prepared_for(&bundle).await?;
        abi_check(&prepared, &req.method, &req.caller, true, false, false)?;
        // The state, its blobs and the search index as of one applied index; an upgrade
        // applied since the bundle was read runs on its own bundle next time.
        let (bundle_now, state, applied, ts_ms, search) = g.read_local(|m, applied| {
            (
                m.component_bundle(),
                (m.view(), m.blob_view()),
                applied,
                m.state.component.head.last_ts_ms,
                self.component_search
                    .for_view(&bundle, &prepared.abi.search, m, applied),
            )
        });
        if bundle_now.as_deref() != Some(bundle.as_str()) {
            return Err(ApiError::unavailable("the app was upgraded; retry"));
        }
        let node = self
            .this
            .lock()
            .ok()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| ApiError::unavailable("the node is shutting down"))?;
        let (app_id, invocation) = (
            req.app_id.clone(),
            Invocation {
                kind: InvocationKind::View,
                method: req.method.clone(),
                args: args_bytes(&req.args),
                caller: req.caller.to_host(),
                entry_ts_ms: ts_ms,
                id_term: 0,
                id_seq: 0,
                seed: [0; 32],
                timer: None,
                compute: None,
            },
        );
        let meter = self.meter(&req.app_id, &bundle);
        // A view may run up to its deadline: off the async workers.
        let (out, run) = tokio::task::spawn_blocking(move || {
            crate::usage::measure(|| {
                node.run_with(
                    &prepared,
                    &app_id,
                    state,
                    applied,
                    invocation,
                    RunExtras {
                        key: Some("view".to_owned()),
                        outbound: false,
                        runs: None,
                        compute_component: false,
                    },
                    search,
                )
                .outcome
            })
        })
        .await
        .map_err(|e| ApiError::internal(&e.to_string()))?;
        self.usage.ran(&meter, run);
        crate::metrics::wasm_run(InvocationKind::View, run.wall);
        if !out.is_retryable() {
            self.usage.called(&meter, RunKind::View);
        }
        Ok(ViewAnswer {
            index: applied,
            timeout: out.is_retryable(),
            status: out.status,
            result: out.result,
            error: out.error,
        })
    }

    /// A component's blob command (R8) on its leader: `__blob { id, size, mime }` (an upload),
    /// `__blob_forget` or `__blob_revoke` (`{ id }`, the owner's), built by the API after judging who may.
    pub(super) async fn component_blob_cmd(
        &self,
        g: &Group<AppMachine>,
        req: &CallRequest,
    ) -> Result<crate::app::Applied, WriteError> {
        let id = req.args["id"].as_str().unwrap_or_default().to_owned();
        let p = g
            .propose_led(|m, _, _| {
                let cmd = match req.method.as_str() {
                    "__blob" => ComponentCmd::Blob {
                        id: id.clone(),
                        size: req.args["size"].as_u64().unwrap_or_default(),
                        mime: req.args["mime"].as_str().unwrap_or_default().to_owned(),
                        ts_ms: self.stamp(m),
                    },
                    "__blob_revoke" => ComponentCmd::BlobRevoke { id: id.clone() },
                    _ => ComponentCmd::BlobForget { id: id.clone() },
                };
                Proposal::Append(serde_json::to_vec(&cmd).unwrap_or_default())
            })
            .await?;
        let a: Applied = serde_json::from_slice(&p.output)
            .map_err(|e| WriteError::Unavailable(e.to_string()))?;
        Ok(to_host_applied(a))
    }

    /// Start the app's pending actions this node is not running yet, as its leader, in the
    /// node's worker pool (D17): each runs off the async workers and logs its end.
    fn run_actions(&self, g: &Arc<Group<AppMachine>>, app_id: &str, prepared: &Prepared) {
        let Some(node) = self.this.lock().ok().and_then(|w| w.upgrade()) else {
            return;
        };
        let pending = g.read_local(|m, _| m.pending_actions());
        let now = self.clock.now_ms();
        for record in pending {
            let key = (app_id.to_owned(), record.id.clone());
            let waiting = self
                .action_retry
                .lock()
                .ok()
                .and_then(|r| r.get(&key).copied());
            if waiting.is_some_and(|at| now < at) {
                continue;
            }
            let started = self
                .actions_running
                .lock()
                .is_ok_and(|mut running| running.insert(key.clone()));
            if !started {
                continue;
            }
            let Ok(permit) = Arc::clone(&self.action_slots).try_acquire_owned() else {
                if let Ok(mut running) = self.actions_running.lock() {
                    let _was = running.remove(&key);
                }
                return;
            };
            let (node, g, prepared) = (Arc::clone(&node), Arc::clone(g), prepared.clone());
            drop(tokio::spawn(async move {
                node.run_action(&g, &prepared, record).await;
                drop(permit);
                if let Ok(mut running) = node.actions_running.lock() {
                    let _was = running.remove(&key);
                }
            }));
        }
    }

    /// One attempt at an action (D12, D22), then its end logged: done; an attempt that
    /// timed out (retried later, `MAX_ATTEMPTS` in all); or failed, which also runs the
    /// app's `onActionFailed`.
    async fn run_action(
        self: &Arc<Self>,
        g: &Group<AppMachine>,
        prepared: &Prepared,
        record: crate::component::action::ActionRecord,
    ) {
        use crate::component::action::{reason, retry_after_ms, MAX_ATTEMPTS};
        let app_id = g.read_local(|m, _| m.app_id.clone());
        let is_action = prepared
            .abi
            .methods
            .get(&record.method)
            .is_some_and(|m| m.kind == MethodKind::Action);
        let (failed, retry) = if is_action {
            let out = self.attempt_action(&app_id, prepared, &record).await;
            match out.status {
                Status::Ok => (None, false),
                _ if out.is_retryable() && record.attempts + 1 < MAX_ATTEMPTS => (None, true),
                _ if out.is_retryable() => (Some(format!("timed out {MAX_ATTEMPTS} times")), false),
                _ => (Some(reason(&out.error.to_string())), false),
            }
        } else {
            (Some("method removed".to_owned()), false)
        };
        let id = record.id.clone();
        let ended = g
            .propose_led(|m, _, _| {
                // Ended already (another leader logged it): nothing to add.
                if m.action_record(&id).is_none() {
                    return Proposal::Reply(Vec::new());
                }
                Proposal::Append(
                    serde_json::to_vec(&ComponentCmd::ActionEnd {
                        id: id.clone(),
                        failed: failed.clone(),
                        retry,
                        ts_ms: self.stamp(m),
                    })
                    .unwrap_or_default(),
                )
            })
            .await;
        let key = (app_id.clone(), id.clone());
        if let Ok(mut r) = self.action_retry.lock() {
            if retry {
                let _prev = r.insert(key, self.clock.now_ms() + retry_after_ms(record.attempts));
            } else {
                let _was = r.remove(&key);
            }
        }
        if let (Ok(p), Some(why)) = (ended, failed) {
            if p.appended {
                self.on_action_failed(&app_id, prepared, &record, &why)
                    .await;
            }
        }
    }

    /// Run `record` once on the engine, off the async workers, as `kind = action` with
    /// the starter's caller, storage off, views and calls through this node and outbound
    /// HTTP through the gate (S9).
    async fn attempt_action(
        self: &Arc<Self>,
        app_id: &str,
        prepared: &Prepared,
        record: &crate::component::action::ActionRecord,
    ) -> Outcome {
        let this = self.this.lock().map(|w| w.clone()).unwrap_or_default();
        let directory = Arc::new(SysDirectory::new(this, app_id.to_owned()));
        let runner = Arc::new(NodeAction {
            node: Arc::clone(self),
            app_id: app_id.to_owned(),
            caller: record.caller.clone(),
            key: record.id.clone(),
            handle: tokio::runtime::Handle::current(),
        });
        let xcalls = Arc::new(NodeXcall {
            node: Arc::clone(self),
            source: app_id.to_owned(),
            key: format!("action:{}", record.id),
            handle: tokio::runtime::Handle::current(),
        });
        let meter = self.meter(app_id, &prepared.hash);
        let http = Arc::new(NodeHttp(self.outbound_for(
            app_id,
            &prepared.hash,
            self.usage.outbound(&meter.app, &meter.billed),
        )));
        let invocation = Invocation {
            kind: InvocationKind::Action,
            method: record.method.clone(),
            args: args_bytes(&record.args),
            caller: record.caller.to_host(),
            entry_ts_ms: record.started_ms,
            id_term: 0,
            id_seq: 0,
            seed: [0; 32],
            timer: None,
            compute: None,
        };
        let budget = Budget {
            wall_clock: Budget::action().wall_clock,
            ..self.budget()
        };
        let (wasm, prepared, app) = (Arc::clone(&self.wasm), prepared.clone(), app_id.to_owned());
        let ran = tokio::task::spawn_blocking(move || {
            crate::usage::measure(|| {
                match HostCore::new(
                    &app,
                    Arc::new(std::collections::BTreeMap::new()),
                    0,
                    invocation,
                    directory,
                ) {
                    Ok(host) => Outcome::from_host(
                        wasm.invoke(
                            &prepared,
                            host.with_action(runner)
                                .with_http(http)
                                .with_xcall_host(xcalls),
                            budget,
                        ),
                    ),
                    Err(e) => Outcome::trap(TrapKind::Fault, &e.to_string()),
                }
            })
        })
        .await;
        match ran {
            Ok((out, run)) => {
                self.usage.ran(&meter, run);
                crate::metrics::wasm_run(InvocationKind::Action, run.wall);
                out
            }
            Err(e) => Outcome::trap(TrapKind::Fault, &e.to_string()),
        }
    }

    /// The app's `onActionFailed`, if its ABI has a call by that name: told which action
    /// failed and why, as the action's caller (D22). Best effort, once.
    async fn on_action_failed(
        &self,
        app_id: &str,
        prepared: &Prepared,
        record: &crate::component::action::ActionRecord,
        why: &str,
    ) {
        const HANDLER: &str = "onActionFailed";
        if !prepared
            .abi
            .methods
            .get(HANDLER)
            .is_some_and(|m| m.kind == MethodKind::Call)
        {
            return;
        }
        let req = CallRequest {
            idempotency_key: Some(format!(
                "{}action-failed-{}",
                crate::app::NODE_KEY_PREFIX,
                record.id
            )),
            ..CallRequest::new(
                app_id.to_owned(),
                HANDLER.to_owned(),
                json!({ "actionId": record.id, "method": record.method, "reason": why }),
                record.caller.clone(),
            )
        };
        if let Err(e) = self.call_component(&req).await {
            debug!(app = %app_id, action = %record.id, error = %e.message, "onActionFailed did not run");
        }
    }

    /// Deliver the head of `source`'s outbox, in order, at most [`DELIVER_BATCH`] messages
    /// a round (D25, D29). A message that cannot be delivered yet waits its backoff and
    /// blocks the ones after it; past [`XCALL_WINDOW_MS`] it is dead-lettered.
    async fn deliver_xcalls(&self, g: &Group<AppMachine>, source: &str, prepared: &Prepared) {
        for _ in 0..DELIVER_BATCH {
            let Some(msg) = g.read_local(|m, _| {
                let h = &m.state.component.head.xcall;
                (h.first < h.next).then(|| m.outgoing(h.first)).flatten()
            }) else {
                return;
            };
            let key = (source.to_owned(), msg.seq);
            let now = self.clock.now_ms();
            let waiting = self
                .xcall_retry
                .lock()
                .ok()
                .and_then(|r| r.get(&key).copied());
            if waiting.is_some_and(|(_, next_at)| now < next_at) {
                return;
            }
            let dead = match self.deliver_one(source, &msg).await {
                Ok(dead) => dead,
                Err(why) if now >= msg.sent_ms.saturating_add(XCALL_WINDOW_MS) => {
                    Some(format!("not delivered within an hour: {why}"))
                }
                Err(why) => {
                    let attempts = waiting.map_or(0, |(a, _)| a) + 1;
                    debug!(app = %source, seq = msg.seq, error = %why, "xcall retried later");
                    if let Ok(mut r) = self.xcall_retry.lock() {
                        let _prev = r.insert(key, (attempts, now + backoff_ms(attempts)));
                    }
                    return;
                }
            };
            let seq = msg.seq;
            let reason = dead.clone();
            let done = g
                .propose_led(|m, _, _| {
                    Proposal::Append(
                        serde_json::to_vec(&ComponentCmd::XcallDone {
                            seq,
                            dead: reason.clone(),
                            ts_ms: self.stamp(m),
                        })
                        .unwrap_or_default(),
                    )
                })
                .await;
            if let Ok(mut r) = self.xcall_retry.lock() {
                let _was = r.remove(&key);
            }
            if done.is_err() {
                return;
            }
            if let Some(reason) = dead {
                self.on_xcall_failed(source, prepared, &msg, &reason).await;
            }
        }
    }

    /// One delivery attempt: `Ok(None)` the target answered (whatever it answered),
    /// `Ok(Some(reason))` it never can (dead letter), `Err` try again later.
    async fn deliver_one(&self, source: &str, msg: &Outgoing) -> Result<Option<String>, String> {
        let target = self.sys(|s| s.apps.get(&msg.app).cloned());
        let Some(target) = target else {
            return Ok(Some("the target app does not exist".into()));
        };
        if !self.sys(|s| same_boundary(s, source, &target.app_id)) {
            return Ok(Some("the apps are not in the same organization".into()));
        }
        if target.executor != Executor::Component {
            return Ok(Some("the target app is not a component".into()));
        }
        let req = CallRequest {
            idempotency_key: Some(format!(
                "{}xcall-{source}-{}",
                crate::app::NODE_KEY_PREFIX,
                msg.seq
            )),
            xcall: Some(XcallFrom {
                source: source.to_owned(),
                seq: msg.seq,
            }),
            ..CallRequest::new(
                target.app_id.clone(),
                msg.method.clone(),
                msg.args.clone(),
                Caller::App {
                    app: source.to_owned(),
                },
            )
        };
        match self.call_component(&req).await {
            Ok(a) if a.timeout => Err("the target timed out".into()),
            Ok(_) => Ok(None),
            Err(e) if e.status >= 500 => Err(e.message),
            // A refusal (an unknown method, a method that does not allow the source) is
            // the target's final answer, like an app error (D29).
            Err(_) => Ok(None),
        }
    }

    /// The source's `onXcallFailed`, if its ABI has one the system may call: told which
    /// message was dead-lettered and why (D29). Best effort, once.
    async fn on_xcall_failed(
        &self,
        source: &str,
        prepared: &Prepared,
        msg: &Outgoing,
        reason: &str,
    ) {
        const HANDLER: &str = "onXcallFailed";
        if !prepared
            .abi
            .methods
            .get(HANDLER)
            .is_some_and(|m| m.allow_system && m.kind == MethodKind::Call)
        {
            return;
        }
        let req = CallRequest {
            idempotency_key: Some(format!(
                "{}xcall-failed-{}",
                crate::app::NODE_KEY_PREFIX,
                msg.seq
            )),
            ..CallRequest::new(
                source.to_owned(),
                HANDLER.to_owned(),
                json!({
                    "app": msg.app,
                    "method": msg.method,
                    "reason": reason,
                }),
                Caller::System,
            )
        };
        if let Err(e) = self.call_component(&req).await {
            debug!(app = %source, seq = msg.seq, error = %e.message, "onXcallFailed did not run");
        }
    }

    /// The content type a component app's blob was uploaded with, from this node's replica
    /// (`None` when it does not host the app or the app does not hold the blob).
    #[must_use]
    pub fn blob_mime(&self, app: &AppRecord, id: &[u8; 32]) -> Option<String> {
        self.app_group(app.group)?.read_local(|m, _| {
            m.state
                .component
                .blobs
                .records
                .get(id)
                .map(|r| r.mime.clone())
        })
    }

    /// Collect the bytes of blobs the app forgot (idle past their time, R8), at most
    /// [`COLLECT_BATCH`] a round, and log that they are done. `collect_blob` keeps the bytes
    /// another app still holds.
    async fn collect_component_blobs(&self, g: &Group<AppMachine>, queued: Vec<String>) {
        let ids: Vec<String> = queued.into_iter().take(COLLECT_BATCH).collect();
        for id in &ids {
            if let Ok(raw) = hex::decode(id) {
                if let Ok(raw) = <[u8; 32]>::try_from(raw.as_slice()) {
                    self.collect_blob(&raw).await;
                }
            }
        }
        let cmd = ComponentCmd::BlobsCollected { ids };
        let logged = g
            .propose_led(|_, _, _| Proposal::Append(serde_json::to_vec(&cmd).unwrap_or_default()))
            .await;
        match logged {
            // Released by the entry that really took the files off the queue, so a repeat frees nothing.
            Ok(p) => {
                let freed = serde_json::from_slice::<Applied>(&p.output)
                    .ok()
                    .and_then(|a| a.result["freed"].as_u64());
                let app_id = g.read_local(|m, _| m.app_id.clone());
                self.refund_files(&app_id, freed.unwrap_or(0)).await;
            }
            Err(e) => debug!(error = %e, "collected blobs not logged; collected again next round"),
        }
    }

    /// Switch a component app to another component, code only, on this node, the
    /// app group's leader. A named migration is refused: migrations are rebuilds
    /// (D38), not in this milestone. Who may upgrade was judged by the API.
    pub(super) async fn upgrade_component(
        &self,
        g: &Group<AppMachine>,
        bundle: &str,
        compute_bundle: Option<&str>,
        migrate: Option<&str>,
    ) -> Result<crate::app::Applied, WriteError> {
        let refuse = |code: &str, message: &str| crate::app::Applied {
            index: g.applied(),
            status: Status::AppError,
            result: Value::Null,
            error: json!({ "code": code, "fields": { "message": message } }),
            events: Vec::new(),
            conflict: false,
        };
        if migrate.is_some() {
            return Ok(refuse(
                "MigrationsNotSupported",
                "migrations are rebuilds (D38), not in this milestone",
            ));
        }
        let target = match self.prepared_for(bundle).await {
            Ok(target) => target,
            Err(e) if e.code == BAD_BUNDLE => return Ok(refuse(BAD_BUNDLE, &e.message)),
            Err(e) => return Err(WriteError::Unavailable(e.message)),
        };
        let p = g
            .propose_led(|m, _, _| {
                let is_result = |name: &String| {
                    target
                        .abi
                        .methods
                        .get(name)
                        .is_some_and(|method| method.compute_result)
                };
                if let Some(lost) = m.active_result_methods().iter().find(|r| !is_result(r)) {
                    let mut refused = crate::component::host_answer(m.state.last_index, false, "");
                    refused.error = json!({
                        "code": RUN_NEEDS_RESULT_METHOD,
                        "fields": { "message": format!(
                            "an active compute run will call {lost}, which the target release does not declare as a result method"
                        ) },
                    });
                    return Proposal::Reply(serde_json::to_vec(&refused).unwrap_or_default());
                }
                let mut cmd = ComponentCmd::Upgrade {
                    bundle: bundle.to_owned(),
                    compute_bundle: compute_bundle.map(str::to_owned),
                    base_index: m.state.last_index,
                };
                let (entry, _replaced) = entry_bytes(&mut cmd);
                Proposal::Append(entry)
            })
            .await?;
        let a: Applied = serde_json::from_slice(&p.output)
            .map_err(|e| WriteError::Unavailable(e.to_string()))?;
        let a = to_host_applied(a);
        self.count_conflict(&a);
        Ok(a)
    }
}

/// The answer to a read-only member's call that is refused (`403 FORBIDDEN`): not
/// logged, nothing applied.
fn read_only_refusal(applied: u64) -> Applied {
    Applied::unlogged(
        applied,
        Outcome {
            status: Status::AppError,
            error: Value::String(super::READ_ONLY.into()),
            ..Outcome::default()
        },
    )
}

/// What an action reaches through the node (D12): views and calls of its own app, as
/// its caller, run on the async runtime while the action's thread waits.
struct NodeAction {
    node: Arc<Node>,
    app_id: String,
    caller: Caller,
    key: String,
    handle: tokio::runtime::Handle,
}

impl NodeAction {
    /// Run `fut` on the runtime and wait for it on this (blocking) thread.
    fn wait<T: Send + 'static>(
        &self,
        fut: impl std::future::Future<Output = T> + Send + 'static,
    ) -> Option<T> {
        let (tx, rx) = std::sync::mpsc::channel();
        drop(self.handle.spawn(async move {
            let _sent = tx.send(fut.await);
        }));
        rx.recv().ok()
    }
}

/// A run's answer as `action.view`/`action.call` return it: the JSON result, or the
/// error as JSON text.
fn answer_bytes(status: &Status, result: &Value, error: &Value) -> Result<Vec<u8>, String> {
    if *status == Status::Ok {
        Ok(serde_json::to_vec(result).unwrap_or_default())
    } else {
        Err(error.to_string())
    }
}

impl calimero_app_host::ActionHost for NodeAction {
    fn key(&self) -> String {
        self.key.clone()
    }

    fn view(&self, method: &str, args: &[u8]) -> Result<Vec<u8>, String> {
        let args = parse_args(args)?;
        let req = ViewRequest {
            app_id: self.app_id.clone(),
            method: method.to_owned(),
            args,
            caller: self.caller.clone(),
            strong: true,
            min_index: 0,
        };
        let node = Arc::clone(&self.node);
        match self.wait(async move { node.view(&req).await }) {
            Some(Ok(a)) if a.timeout => Err("the view timed out".into()),
            Some(Ok(a)) => answer_bytes(&a.status, &a.result, &a.error),
            Some(Err(e)) => Err(e.message),
            None => Err("the node is shutting down".into()),
        }
    }

    fn call(&self, method: &str, args: &[u8], key: &str) -> Result<Vec<u8>, String> {
        let args = parse_args(args)?;
        let req = CallRequest {
            idempotency_key: Some(key.to_owned()),
            action: Some(self.key.clone()),
            ..CallRequest::new(
                self.app_id.clone(),
                method.to_owned(),
                args,
                self.caller.clone(),
            )
        };
        let node = Arc::clone(&self.node);
        match self.wait(async move { node.call_component(&req).await }) {
            Some(Ok(a)) if a.timeout => Err("the call timed out".into()),
            Some(Ok(a)) => answer_bytes(&a.status, &a.result, &a.error),
            Some(Err(e)) => Err(e.message),
            None => Err("the node is shutting down".into()),
        }
    }
}

/// What a component run reaches besides its own state, by the kind of run.
#[derive(Clone, Default)]
struct RunExtras {
    /// The run's own key (a call's caller and idempotency key, a tick's slot, a view's
    /// placeholder): with one, the run reaches other apps synchronously ([`NodeXcall`]).
    key: Option<String>,
    /// The app's outbound gate (D76): calls (but a read-only member's), timers and
    /// deliveries; never views or `init`.
    outbound: bool,
    /// The app's active runs: with them, a logged run starts and cancels compute runs.
    runs: Option<Arc<dyn Runs>>,
    /// Whether the app's group runs a release that ships a compute component.
    compute_component: bool,
}

/// How long a call, timer or delivery waits for one outbound response (D76): within the
/// 2 s call budget (`calimero_app_host::Budget::call`), so a slow host costs the run an
/// error it handles, not the log a stalled entry.
pub const CALL_FETCH_TIMEOUT: Duration = Duration::from_millis(1_500);

/// [`NodeHttp`] for a logged run: the gate's answer, or a timeout after `timeout`. The
/// request may still complete in the background; the run has moved on, and its entry
/// records the timeout.
struct TimedHttp {
    inner: Arc<NodeHttp>,
    timeout: Duration,
}

impl calimero_app_host::HttpHost for TimedHttp {
    fn send(
        &self,
        request: &calimero_app_host::HttpRequest,
    ) -> Result<calimero_app_host::HttpResponse, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        let (inner, request) = (Arc::clone(&self.inner), request.clone());
        let _detached = std::thread::Builder::new()
            .name("call-fetch".into())
            .spawn(move || {
                let _sent = tx.send(calimero_app_host::HttpHost::send(&*inner, &request));
            })
            .map_err(|e| e.to_string())?;
        rx.recv_timeout(self.timeout)
            .unwrap_or_else(|_| Err(format!("no response within {:?}", self.timeout)))
    }
}

/// How long a run waits for another app's synchronous answer. Past it the run is told the
/// call timed out; a call may still commit in the target, and the run's retry replays it.
pub const SYNC_XCALL_WAIT: Duration = Duration::from_secs(3);

/// What a component run reaches in other apps (`xcall.call`, `xcall.view`): views and
/// calls of a component app in the same organization (or of the same owner), as
/// `Caller::App(source)`, run on the async runtime while the run's thread waits at most
/// [`SYNC_XCALL_WAIT`]. The target runs a call as a delivery, so it reaches no further.
///
/// Two apps calling each other at the same moment each hold their own call while they
/// wait for the other's: both wait out [`SYNC_XCALL_WAIT`] and are told it timed out.
struct NodeXcall {
    node: Arc<Node>,
    source: String,
    key: String,
    handle: tokio::runtime::Handle,
}

/// The node's refusal of a synchronous xcall, for the guest: the target's ABI refusal as
/// the app error it is (`{code, fields}`), anything else as `{message, error}`.
fn refusal_text(e: &ApiError) -> String {
    if e.details.get("code").is_some() {
        return e.details.to_string();
    }
    json!({ "message": e.message, "error": e.code }).to_string()
}

/// A refusal or failure for the guest, as the JSON text `xcall.*` returns.
fn xcall_failure(message: &str) -> String {
    json!({ "message": message }).to_string()
}

impl NodeXcall {
    /// The target's canonical id, if this run may reach it: another component app, in the
    /// caller's boundary (`same_boundary`).
    fn target(&self, app: &str) -> Result<String, String> {
        let target = self.node.canonical_app_id(app);
        if target == self.source {
            return Err(xcall_failure(calimero_app_host::XCALL_SYNC_SELF));
        }
        let ok = self.node.sys(|s| {
            s.apps
                .get(&target)
                .is_some_and(|a| a.executor == Executor::Component)
                && same_boundary(s, &self.source, &target)
        });
        if !ok {
            // One answer for missing, foreign and wasmer apps: nothing is revealed.
            return Err(xcall_failure("no such app in this organization"));
        }
        Ok(target)
    }

    /// Run `fut` on the runtime and wait for it on this (blocking) thread.
    fn wait<T: Send + 'static>(
        &self,
        fut: impl std::future::Future<Output = T> + Send + 'static,
    ) -> Result<T, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        drop(self.handle.spawn(async move {
            let _sent = tx.send(fut.await);
        }));
        rx.recv_timeout(SYNC_XCALL_WAIT).map_err(|e| match e {
            std::sync::mpsc::RecvTimeoutError::Timeout => xcall_failure("the xcall timed out"),
            std::sync::mpsc::RecvTimeoutError::Disconnected => {
                xcall_failure("the node is shutting down")
            }
        })
    }
}

impl calimero_app_host::XcallHost for NodeXcall {
    fn key(&self) -> String {
        self.key.clone()
    }

    fn call(&self, app: &str, method: &str, args: &[u8], key: &str) -> Result<Vec<u8>, String> {
        let target = self.target(app)?;
        let args = parse_args(args).map_err(|e| xcall_failure(&e))?;
        let req = CallRequest {
            idempotency_key: Some(key.to_owned()),
            sync_from: Some(self.source.clone()),
            ..CallRequest::new(
                target,
                method.to_owned(),
                args,
                Caller::App {
                    app: self.source.clone(),
                },
            )
        };
        let node = Arc::clone(&self.node);
        match self.wait(async move { node.call_component(&req).await })? {
            Ok(a) if a.timeout => Err(xcall_failure("the call timed out")),
            Ok(a) if a.conflict => Err(xcall_failure("the call raced a leader change")),
            Ok(a) => answer_bytes(&a.status, &a.result, &a.error),
            Err(e) => Err(refusal_text(&e)),
        }
    }

    fn view(&self, app: &str, method: &str, args: &[u8]) -> Result<Vec<u8>, String> {
        let target = self.target(app)?;
        let args = parse_args(args).map_err(|e| xcall_failure(&e))?;
        let req = ViewRequest {
            app_id: target,
            method: method.to_owned(),
            args,
            caller: Caller::App {
                app: self.source.clone(),
            },
            strong: true,
            min_index: 0,
        };
        let node = Arc::clone(&self.node);
        match self.wait(async move { node.view(&req).await })? {
            Ok(a) if a.timeout => Err(xcall_failure("the view timed out")),
            Ok(a) => answer_bytes(&a.status, &a.result, &a.error),
            Err(e) => Err(refusal_text(&e)),
        }
    }
}

fn parse_args(args: &[u8]) -> Result<Value, String> {
    if args.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(args).map_err(|e| format!("arguments are not JSON: {e}"))
}

/// An action's outbound HTTP through the node's gate (S9, R14): the signed release's
/// declared prefixes, the owner's approval, the operator's hosts, secrets filled in.
struct NodeHttp(Option<Arc<dyn calimero_outbound::OutboundHost>>);

impl calimero_app_host::HttpHost for NodeHttp {
    fn send(
        &self,
        request: &calimero_app_host::HttpRequest,
    ) -> Result<calimero_app_host::HttpResponse, String> {
        use calimero_primitives::outbound::{OutboundMethod, OutboundRequest, OutboundResponse};
        let Some(gate) = &self.0 else {
            return Err("this app declares no outbound hosts in a signed release".into());
        };
        let method = match request.method.as_str() {
            "GET" => OutboundMethod::Get,
            "POST" => OutboundMethod::Post,
            "PUT" => OutboundMethod::Put,
            "PATCH" => OutboundMethod::Patch,
            "DELETE" => OutboundMethod::Delete,
            other => return Err(format!("method {other} is not allowed")),
        };
        let out = OutboundRequest {
            method,
            url: request.url.clone(),
            headers: request.headers.clone(),
            body: request.body.clone(),
        };
        let bytes = borsh::to_vec(&out).map_err(|e| e.to_string())?;
        let answer = gate.send(&bytes)?;
        let response: OutboundResponse = borsh::from_slice(&answer).map_err(|e| e.to_string())?;
        Ok(calimero_app_host::HttpResponse {
            status: response.status,
            headers: Vec::new(),
            body: response.body,
        })
    }
}

/// Whether an xcall from `source` may reach `target`: both in one organization, or both
/// personal apps of one owner.
fn same_boundary(s: &crate::system::SystemState, source: &str, target: &str) -> bool {
    let (Some(a), Some(b)) = (s.apps.get(source), s.apps.get(target)) else {
        return false;
    };
    match (&a.org, &b.org) {
        (Some(x), Some(y)) => x == y,
        (None, None) => a.owner == b.owner,
        _ => false,
    }
}

/// A component's answer in the shape the node's own commands (`init`, upgrades)
/// answer with: their callers read the status, the error and the conflict flag.
fn to_host_applied(a: Applied) -> crate::app::Applied {
    crate::app::Applied {
        index: a.index,
        status: a.status,
        result: a.result,
        error: a.error,
        events: Vec::new(),
        conflict: a.conflict,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_that_starts_or_cancels_a_run_is_a_write() {
        assert!(!writes(&Outcome::default()));
        let mut start = Outcome::default();
        start.compute_starts.push(crate::component::ComputeStart {
            run: "r".into(),
            method: "digest".into(),
            args: calimero_log::Bytes(Vec::new()),
            inputs: Vec::new(),
            caps: crate::component::RunCaps {
                fuel: 1,
                memory_mib: 1,
                wall_secs: 1,
            },
            result_method: "onDone".into(),
            owner: String::new(),
        });
        assert!(writes(&start));
        let cancel = Outcome {
            compute_cancels: vec!["r".into()],
            ..Outcome::default()
        };
        assert!(writes(&cancel));
    }

    #[test]
    fn a_final_is_charged_for_its_result_methods_files_only() {
        use crate::component::{FileMark, FinalEntry, FinalOutcome};
        use crate::job_wire::ReportUsage;
        let file = |size| FileMark {
            id: "ab".repeat(32),
            size,
            sha256: calimero_log::Bytes(vec![1; 32]),
            wrapped_key: calimero_log::Bytes(vec![2; 60]),
            segments: vec!["cd".repeat(32)],
        };
        let entry = FinalEntry {
            run: "r".into(),
            attempt: 1,
            outcome: FinalOutcome::Done,
            report: None,
            signature: None,
            outputs: vec![file(10), file(20)],
            usage: ReportUsage::default(),
            result: Outcome {
                files: vec![file(4)],
                ..Outcome::default()
            },
            base_index: 0,
            ts_ms: 0,
            id_term: 0,
            id_seq: 0,
        };
        assert_eq!(
            new_file_bytes(&ComponentCmd::ComputeFinal(Box::new(entry)), &[]),
            Some(4),
            "the outputs were charged before the entry"
        );
        let attempt = ComponentCmd::Barrier;
        assert_eq!(new_file_bytes(&attempt, &[]), Some(0));
    }

    fn big_final(result_bytes: usize, outputs: usize) -> ComponentCmd {
        use crate::component::{FileMark, FinalEntry, FinalOutcome};
        use crate::job_wire::ReportUsage;
        let file = |i: usize| FileMark {
            id: format!("{i:064x}"),
            size: 10,
            sha256: calimero_log::Bytes(vec![1; 32]),
            wrapped_key: calimero_log::Bytes(vec![2; 60]),
            segments: vec!["cd".repeat(32)],
        };
        ComponentCmd::ComputeFinal(Box::new(FinalEntry {
            run: "r".into(),
            attempt: 1,
            outcome: FinalOutcome::Done,
            report: None,
            signature: None,
            outputs: (0..outputs).map(file).collect(),
            usage: ReportUsage::default(),
            result: Outcome {
                diff: vec![crate::app::Write {
                    key: calimero_log::Bytes(b"k".to_vec()),
                    value: Some(calimero_log::Bytes(vec![0; result_bytes])),
                }],
                files: vec![file(9_999)],
                ..Outcome::default()
            },
            base_index: 0,
            ts_ms: 0,
            id_term: 0,
            id_seq: 0,
        }))
    }

    #[test]
    fn a_published_blob_is_charged_once_and_only_when_the_run_made_its_bytes() {
        let publish = |id: &str, source: &str| crate::component::PublishedMark {
            source: source.to_owned(),
            id: id.to_owned(),
            size: 7,
        };
        let cmd = ComponentCmd::Call(Box::new(crate::component::CallEntry {
            outcome: Outcome {
                published: vec![
                    publish("aa", "s1"),
                    publish("aa", "s2"),
                    publish("bb", "s3"),
                ],
                ..Outcome::default()
            },
            ..serde_json::from_value(json!({
                "method": "m", "args": null,
                "caller": { "kind": "account", "account": "a" }, "tsMs": 0, "baseIndex": 0,
                "idTerm": 0, "idSeq": 0, "outcome": { "status": "ok" }
            }))
            .unwrap()
        }));
        let blobs: Vec<Blob> = vec![Blob::plain("aa".into(), vec![1])];
        assert_eq!(new_file_bytes(&cmd, &blobs), Some(7), "bb was already held");
        assert_eq!(marked_blobs(&cmd, &blobs).len(), 1);
    }

    #[test]
    fn an_oversized_final_loses_only_its_result_methods_outcome() {
        let mut cmd = big_final(2 * calimero_log::MAX_ENTRY_BYTES, 3);
        assert_eq!(new_file_bytes(&cmd, &[]), Some(10), "the result's file");
        let (bytes, replaced) = entry_bytes(&mut cmd);
        assert!(replaced && bytes.len() <= calimero_log::MAX_ENTRY_BYTES);
        let ComponentCmd::ComputeFinal(entry) = &cmd else {
            panic!("not a final");
        };
        assert_eq!(entry.outputs.len(), 3, "the outputs stay");
        assert!(entry.result.diff.is_empty() && entry.result.files.is_empty());
        assert_eq!(
            new_file_bytes(&cmd, &[]),
            Some(0),
            "nothing is left to charge"
        );
        let blobs: Vec<Blob> = [0usize, 9_999]
            .iter()
            .map(|i| Blob::plain(format!("{i:064x}"), vec![1]))
            .collect();
        assert!(
            marked_blobs(&cmd, &blobs).is_empty(),
            "no blob is left to store"
        );
    }

    #[test]
    fn file_sizes_that_overflow_refuse_the_entry() {
        let mut cmd = big_final(10, 2);
        if let ComponentCmd::ComputeFinal(entry) = &mut cmd {
            let mut second = entry.result.files[0].clone();
            entry.result.files[0].size = u64::MAX;
            second.size = 1;
            entry.result.files.push(second);
        }
        assert_eq!(new_file_bytes(&cmd, &[]), None);
    }

    #[test]
    fn the_component_cache_key_matches_its_known_answer() {
        assert_eq!(
            hex::encode(component_cache_key(&[7; 32])),
            "76c96b3cbf04b23fd1e753c6b064af2c6825aca228d1be3897621b9d4714e8af"
        );
    }

    #[test]
    fn the_due_slot_is_the_latest_one_aligned_to_the_install_time() {
        let every = Duration::from_secs(1);
        assert_eq!(due_slot(10_000, every, 9_000), None, "before install");
        assert_eq!(due_slot(10_000, every, 10_999), None, "no slot yet");
        assert_eq!(due_slot(10_000, every, 11_000), Some(11_000));
        assert_eq!(due_slot(10_000, every, 11_999), Some(11_000));
        // Five missed slots collapse into the latest one.
        assert_eq!(due_slot(10_000, every, 16_500), Some(16_000));
        assert_eq!(due_slot(10_000, Duration::ZERO, 16_500), None);
    }

    #[test]
    fn compute_and_compute_result_methods_are_refused_to_every_caller() {
        let abi = calimero_app_host::Abi::from_json(&json!({
            "abiVersion": 1,
            "methods": {
                "digest": { "kind": "compute", "compute": { "fuel": 1, "result": "onDone" } },
                "onDone": { "kind": "call", "computeResult": true, "allowSystem": true },
                "plain": { "kind": "call", "allowSystem": true, "public": true },
            }
        }))
        .unwrap();
        let prepared = Prepared {
            hash: String::new(),
            abi,
            inner: std::sync::Arc::new(()),
        };
        let account = Caller::Account {
            account: "alice".to_owned(),
        };
        for caller in [account, Caller::System, Caller::Public] {
            for (view, from_action) in [(false, false), (true, false), (false, true)] {
                for method in ["digest", "onDone"] {
                    let refused =
                        abi_check(&prepared, method, &caller, view, true, from_action).unwrap_err();
                    assert_eq!(refused.details["code"], "WrongKind", "{method} {caller:?}");
                }
            }
        }
        assert!(abi_check(&prepared, "plain", &Caller::System, false, true, false).is_ok());
    }
}
