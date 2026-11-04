//! The component executor's side of an app group: what a `calimero:app` component's
//! entries are, and how a replica applies them.
//!
//! An app runs on one executor, chosen once from its bundle's WASM when the app is
//! created ([`crate::types::Executor`]) and recorded by its `init` entry in the
//! group's state. A component (the v2 SDK) runs on wasmtime through the host core
//! (`calimero-app-host`); the leader appends `{ call, outcome { status, result,
//! error, diff, events, logs } }` and every replica applies the diff, never
//! re-executing. The entries here ([`ComponentCmd`]) are a format of their own,
//! applied to a keyspace of their own (`app::rows::CKV`), so a replica never
//! applies a component's diff to an SDK 0.12 app's state (which no longer runs,
//! [`crate::app::WASMER_GONE`]) or the other way round.
//!
//! Storage (D46): the component's keys are hidden rows of the group's state (sealed,
//! keyed by an HMAC), and a replica keeps them in memory too,
//! ordered, as an `Arc<BTreeMap>`: a run gets the current version as its
//! `StateView` (ordered `range` scans), and `apply` copies it only while a run still
//! holds the previous one. The map is rebuilt from the rows on a restart and after a
//! snapshot install. Host records (`last_id`, `timers`, `install_ts_ms`,
//! `last_ts_ms`, idempotency, blob records) are never keys the app sees (D39).

pub mod action;
pub mod blobs;
mod runs;
pub mod search;
pub mod xcall;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Weak};

use calimero_app_host as host;
use calimero_log::{Bytes, GroupState, StateTx};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::{rows, AppMachine, Status, Write};
use crate::component::action::{ActionOut, ActionRecord, ActionsHead};
use crate::component::blobs::{ref_deltas, BlobTable, MAX_COLLECT_QUEUE};
use crate::component::xcall::{XcallFrom, XcallHead, XcallOut};
use crate::idempotency;
use crate::node::Node;
use crate::types::{AppId, Audience, Caller, EventRecord};

pub use runs::{
    Attempt, AttemptEntry, ComputeStart, FailedReason, Final, FinalEntry, FinalOutcome, NodeRuns,
    RunCaps, RunInputRef, RunRecord, RunState, DEADLINE_GRACE, FINISHED_RUNS_KEPT, MAX_ATTEMPTS,
};

/// An event a run emitted, with its audience (R9).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventOut {
    pub kind: String,
    pub data: Value,
    pub audience: Audience,
}

/// What running an invocation produced, as the log stores it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub status: Status,
    #[serde(default)]
    pub result: Value,
    /// `AppError`: `{ code, fields }`; `Trap`: `{ kind, detail }`; else `null`.
    #[serde(default)]
    pub error: Value,
    #[serde(default)]
    pub diff: Vec<Write>,
    #[serde(default)]
    pub events: Vec<EventOut>,
    /// The owner-only app log (spec section 5); kept whatever the status.
    #[serde(default)]
    pub logs: Vec<String>,
    /// The xcalls the run sent (D25); empty unless `Ok`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub xcalls: Vec<XcallOut>,
    /// The actions the run started (D17); empty unless `Ok`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<ActionOut>,
    /// The run's outbound requests and what came back (D76), kept whatever the status:
    /// the entry is their record, so followers and replays never fetch again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub http: Vec<HttpRecord>,
    /// The files the run finished: their marks only, the ciphertext never enters the log.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FileMark>,
    /// The compute runs the run started; empty unless `Ok`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compute_starts: Vec<ComputeStart>,
    /// The active runs the run asked to cancel; empty unless `Ok`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compute_cancels: Vec<String>,
}

/// One outbound request of a logged run and its response (D76), as the entry records it:
/// the request as the app made it (secret placeholders, never secrets), the response as
/// the outbound gate handed it back (secrets redacted); bodies cut to
/// `calimero_app_host::MAX_RECORDED_BODY_BYTES`, with their full lengths.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpRecord {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: calimero_log::Bytes,
    pub body_len: usize,
    /// The response's status; `None` when there was none (`error` says why).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default)]
    pub response_headers: Vec<(String, String)>,
    #[serde(default)]
    pub response_body: calimero_log::Bytes,
    #[serde(default)]
    pub response_body_len: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl From<host::HttpExchange> for HttpRecord {
    fn from(x: host::HttpExchange) -> Self {
        let (status, response_headers, response_body, error) = match x.response {
            Ok(r) => (Some(r.status), r.headers, r.body, None),
            Err(e) => (None, Vec::new(), Vec::new(), Some(e)),
        };
        Self {
            method: x.request.method,
            url: x.request.url,
            headers: x.request.headers,
            body: calimero_log::Bytes(x.request.body),
            body_len: x.request_body_len,
            status,
            response_headers,
            response_body: calimero_log::Bytes(response_body),
            response_body_len: x.response_body_len,
            error,
        }
    }
}

/// What a run finished of a file, as the log carries it (ciphertext lives outside the log, by
/// `id`); applying it records the file as a blob of the app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMark {
    /// Hex SHA-256 of the ciphertext: the blob's name.
    pub id: String,
    /// Plaintext bytes.
    pub size: u64,
    /// SHA-256 of the plaintext.
    pub sha256: Bytes,
    /// The data key, wrapped under the app's file key.
    pub wrapped_key: Bytes,
}

/// The name a trap kind has on the wire (the spec's names).
#[must_use]
pub fn trap_name(kind: host::TrapKind) -> &'static str {
    match kind {
        host::TrapKind::OutOfFuel => "OutOfFuel",
        host::TrapKind::Timeout => "Timeout",
        host::TrapKind::Cancelled => "Cancelled",
        host::TrapKind::Unavailable => "Unavailable",
        host::TrapKind::OutOfMemory => "OutOfMemory",
        host::TrapKind::StackOverflow => "StackOverflow",
        host::TrapKind::Fault => "Fault",
        host::TrapKind::HostDenied => "HostDenied",
    }
}

impl Outcome {
    /// A trap of `kind`: no diff, no events.
    #[must_use]
    pub fn trap(kind: host::TrapKind, detail: &str) -> Self {
        Self {
            status: Status::Trap,
            error: json!({ "kind": trap_name(kind), "detail": detail }),
            ..Self::default()
        }
    }

    /// The engine's outcome in the log's shape. Any status but `Ok` drops the diff
    /// and the events (the host core already did); a result or event that is not
    /// JSON is a `Fault`, and nothing of it is applied.
    #[must_use]
    pub fn from_host(out: host::Outcome) -> Self {
        let logs = out.logs;
        let http = out.http.into_iter().map(HttpRecord::from).collect();
        let files = out.new_files;
        let (starts, cancels) = (out.compute_starts, out.compute_cancels);
        let mut outcome = match out.status {
            host::Status::Ok => Self::ok(out.result, out.diff, out.events, out.xcalls, out.actions),
            host::Status::AppError { code, message } => Self {
                status: Status::AppError,
                error: json!({
                    "code": code,
                    "fields": serde_json::from_str::<Value>(&message).unwrap_or(Value::String(message)),
                }),
                ..Self::default()
            },
            host::Status::Trap { kind, detail } => Self::trap(kind, &detail),
        };
        outcome.logs = logs;
        outcome.http = http;
        if outcome.status == Status::Ok {
            outcome.files = files.into_iter().map(FileMark::from).collect();
            outcome.compute_starts = starts.into_iter().map(ComputeStart::from).collect();
            outcome.compute_cancels = cancels;
        }
        outcome
    }

    fn ok(
        result: Vec<u8>,
        diff: Vec<host::Write>,
        events: Vec<host::Event>,
        xcalls: Vec<host::Xcall>,
        actions: Vec<host::ActionStart>,
    ) -> Self {
        let result = if result.is_empty() {
            Value::Null
        } else {
            match serde_json::from_slice(&result) {
                Ok(v) => v,
                Err(_) => return Self::trap(host::TrapKind::Fault, "result is not JSON"),
            }
        };
        let mut out = Vec::with_capacity(events.len());
        for e in events {
            let Ok(data) = serde_json::from_slice(&e.data) else {
                return Self::trap(host::TrapKind::Fault, "event data is not JSON");
            };
            out.push(EventOut {
                kind: e.name,
                data,
                audience: e.audience.into(),
            });
        }
        let mut sent = Vec::with_capacity(xcalls.len());
        for x in xcalls {
            let args = if x.args.is_empty() {
                Value::Null
            } else {
                let Ok(args) = serde_json::from_slice(&x.args) else {
                    return Self::trap(host::TrapKind::Fault, "xcall args are not JSON");
                };
                args
            };
            sent.push(XcallOut {
                app: x.app,
                method: x.method,
                args,
            });
        }
        let mut started = Vec::with_capacity(actions.len());
        for a in actions {
            let args = if a.args.is_empty() {
                Value::Null
            } else {
                let Ok(args) = serde_json::from_slice(&a.args) else {
                    return Self::trap(host::TrapKind::Fault, "action args are not JSON");
                };
                args
            };
            started.push(ActionOut {
                id: a.id,
                method: a.method,
                args,
            });
        }
        Self {
            status: Status::Ok,
            result,
            error: Value::Null,
            diff: diff
                .into_iter()
                .map(|w| Write {
                    key: Bytes(w.key),
                    value: w.value.map(Bytes),
                })
                .collect(),
            events: out,
            logs: Vec::new(),
            xcalls: sent,
            actions: started,
            http: Vec::new(),
            files: Vec::new(),
            compute_starts: Vec::new(),
            compute_cancels: Vec::new(),
        }
    }

    /// Whether the run hit its wall-clock deadline or the node could not serve it (no
    /// instance, memory or file bytes): never logged, the idempotency key not spent (D23).
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        self.status == Status::Trap
            && [host::TrapKind::Timeout, host::TrapKind::Unavailable]
                .into_iter()
                .any(|kind| self.error["kind"] == trap_name(kind))
    }
}

impl From<host::NewFile> for FileMark {
    fn from(file: host::NewFile) -> Self {
        Self {
            id: file.id,
            size: file.size,
            sha256: Bytes(file.sha256),
            wrapped_key: Bytes(file.wrapped_key),
        }
    }
}

/// A call as recorded in the log.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallEntry {
    pub method: String,
    pub args: Value,
    pub caller: Caller,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    /// The passkey assertion that approved this call, for confirm methods.
    #[serde(default)]
    pub consent: Option<Value>,
    /// The entry timestamp the run saw (D16, D19).
    pub ts_ms: u64,
    /// The index of the app's last entry when the leader ran it
    /// (`AppState::last_index`); `apply` refuses it on any other state.
    pub base_index: u64,
    /// The leader's term and per-term execution counter the run's unique ids
    /// were made from (D52).
    pub id_term: u64,
    pub id_seq: u64,
    pub outcome: Outcome,
    /// A delivered xcall (D25): which message of which source app it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xcall: Option<XcallFrom>,
}

/// A timer tick as recorded in the log (D34).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TickEntry {
    /// The timer method's ABI name.
    pub timer: String,
    /// The slot: `install_ts_ms + k · interval`.
    pub scheduled_ms: u64,
    /// The timer's last applied slot when the leader ran the tick.
    #[serde(default)]
    pub previous_ms: Option<u64>,
    /// The entry timestamp (app clock, D16), at or after the slot.
    pub ts_ms: u64,
    pub base_index: u64,
    /// The ids the tick ran with (D52), as for a call.
    pub id_term: u64,
    pub id_seq: u64,
    pub outcome: Outcome,
}

/// A component app's entries. Their `op` names are not a wasmer app's
/// (`app::AppCmd`), so neither executor's entries decode as the other's.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum ComponentCmd {
    /// The first entry: `init` ran. It also records the executor.
    #[serde(rename = "componentInit")]
    Init {
        bundle: String,
        /// The compute component shipped with `bundle`, if any.
        #[serde(default)]
        compute_bundle: Option<String>,
        base_index: u64,
        ts_ms: u64,
        /// The ids `init` ran with (D52), as for a call.
        id_term: u64,
        id_seq: u64,
        outcome: Box<Outcome>,
    },
    #[serde(rename = "componentCall")]
    Call(Box<CallEntry>),
    /// A timer tick (D34), run by the leader as `System` for the slot
    /// `scheduled_ms`. Applied only if that slot is later than the timer's last
    /// applied one; then recorded whatever its status (a failed tick counts as
    /// applied). A tick that timed out is never proposed.
    #[serde(rename = "componentTick")]
    Tick(Box<TickEntry>),
    /// Switch to a new bundle, code only (migrations are rebuilds, D38, not in
    /// this milestone). The bundle is a component too: an upgrade across
    /// executors is refused before it is proposed.
    #[serde(rename = "componentUpgrade")]
    Upgrade {
        bundle: String,
        /// The compute component shipped with `bundle`, if any.
        #[serde(default)]
        compute_bundle: Option<String>,
        base_index: u64,
    },
    /// A blob uploaded for the app (R8; its bytes are in the blob store already): the
    /// app holds it, idle until a value refers to it ([`blobs`]).
    #[serde(rename = "componentBlob")]
    Blob {
        /// Its SHA-256, hex.
        id: String,
        size: u64,
        mime: String,
        /// The entry timestamp (D16): its idle time starts here.
        ts_ms: u64,
    },
    /// The app's owner deleted a blob: the app no longer holds it, whatever refers to it.
    #[serde(rename = "componentBlobForget")]
    BlobForget { id: String },
    /// An uploaded file (its ciphertext is on the replicas already): the app holds it, idle
    /// until a value refers to it ([`blobs`]). The node's own command, built by the upload route.
    #[serde(rename = "componentFile")]
    File {
        mark: FileMark,
        /// The entry timestamp (D16): its idle time starts here.
        ts_ms: u64,
        /// The request's idempotency key: a retry of a committed command replays its answer.
        #[serde(default)]
        key: Option<String>,
    },
    /// Applies as nothing: committed through the leader to settle which earlier entries stand.
    #[serde(rename = "componentBarrier")]
    Barrier,
    /// An action's attempt timed out (`retry`, D22), or the action ended: done, or failed
    /// with `failed`'s reason.
    #[serde(rename = "componentActionEnd")]
    ActionEnd {
        id: String,
        #[serde(default)]
        failed: Option<String>,
        #[serde(default)]
        retry: bool,
        ts_ms: u64,
    },
    /// The leader finished with the outbox's head message (D25, D29): delivered, or
    /// dead-lettered with `dead`'s reason.
    #[serde(rename = "componentXcallDone")]
    XcallDone {
        seq: u64,
        #[serde(default)]
        dead: Option<String>,
        ts_ms: u64,
    },
    /// The leader collected the bytes of blobs the app forgot (idle past their time):
    /// they leave the queue.
    #[serde(rename = "componentBlobsCollected")]
    BlobsCollected { ids: Vec<String> },
    /// The leader assigned a run to a job node. Applies to an active run whatever else was
    /// logged since (see [`AttemptEntry`]).
    #[serde(rename = "computeAttempt")]
    ComputeAttempt(Box<AttemptEntry>),
    /// A run's end, with the outcome of its result method. The first valid one wins.
    #[serde(rename = "computeFinal")]
    ComputeFinal(Box<FinalEntry>),
}

impl ComponentCmd {
    /// Whether `command` is a component's `init` entry, which is what decides an
    /// app group's executor before anything else applied.
    #[must_use]
    pub fn is_init(command: &[u8]) -> bool {
        #[derive(Deserialize)]
        struct Op<'a> {
            #[serde(borrow)]
            op: &'a str,
        }
        serde_json::from_slice::<Op<'_>>(command).is_ok_and(|o| o.op == "componentInit")
    }

    /// Whether `command` is any component entry: before the group's state names an
    /// executor, a component's entry (its `init`, or an upgrade taken while its
    /// init had not applied) is what names it.
    #[must_use]
    pub fn is_component(command: &[u8]) -> bool {
        #[derive(Deserialize)]
        struct Op<'a> {
            #[serde(borrow)]
            op: &'a str,
        }
        serde_json::from_slice::<Op<'_>>(command).is_ok_and(|o| o.op.starts_with("component"))
    }
}

/// What `apply` answers for a component's entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub index: u64,
    pub status: Status,
    pub result: Value,
    pub error: Value,
    pub events: Vec<EventOut>,
    /// The entry was built on a state that was no longer current; nothing changed.
    #[serde(default)]
    pub conflict: bool,
    /// The run is retryable (wall clock or node unavailable): nothing was logged (D23); answered
    /// `503 TIMEOUT`.
    #[serde(default)]
    pub timeout: bool,
}

impl Applied {
    /// An answer for a run that was not logged.
    #[must_use]
    pub fn unlogged(index: u64, outcome: Outcome) -> Self {
        Self {
            index,
            timeout: outcome.is_retryable(),
            status: outcome.status,
            result: outcome.result,
            error: outcome.error,
            events: Vec::new(),
            conflict: false,
        }
    }

    fn conflict(index: u64, detail: &str) -> Self {
        Self {
            index,
            status: Status::Trap,
            result: Value::Null,
            error: json!({ "kind": "Conflict", "detail": detail }),
            events: Vec::new(),
            conflict: true,
            timeout: false,
        }
    }
}

/// A component call's idempotency record: its answer, and when it expires (D74).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdemRecord {
    applied: Applied,
    expires_ms: u64,
}

/// Where a component's idempotency records live (D74's rule, `crate::idempotency`).
const IDEM_ROWS: idempotency::Rows = idempotency::Rows {
    tag: rows::CIDEM,
    ring: rows::corder,
};

/// `(expires_ms, index)` of a stored component record.
fn stamps(raw: &[u8]) -> Option<(u64, u64)> {
    serde_json::from_slice::<IdemRecord>(raw)
        .ok()
        .map(|r| (r.expires_ms, r.applied.index))
}

/// An app's stored keys and values, shared between the machine and the runs reading it.
pub type Kv = Arc<BTreeMap<Vec<u8>, Vec<u8>>>;

/// Whether `n` is zero: a field serde leaves out then.
const fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// A component's host records (D39), kept in the group's `Head`, so a snapshot
/// carries them like every other row.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentHead {
    /// The newest entry timestamp applied (D16): the next entry is stamped
    /// after it, and views see it (D19).
    #[serde(default)]
    pub last_ts_ms: u64,
    /// The timestamp (D16) of the newest entry that wrote the app's data: a
    /// call that applied with a diff or files, or an uploaded blob or file
    /// the app took. The leader stamps it into the entry, so every replica
    /// keeps the same one. Zero before the first write, and in a state
    /// written before it was kept, which then reads and writes as it did.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub last_write_ms: u64,
    /// The highest `(id_term, id_seq)` any logged run carried, conflicted ones
    /// included (D52, Ruling 29): a node leading the same term again after a
    /// restart continues after it instead of reusing an id.
    #[serde(default)]
    pub last_id: (u64, u64),
    /// The timestamp of the applied `init` entry: timer slots are aligned to it (D34).
    #[serde(default)]
    pub install_ts_ms: u64,
    /// The compute component of the release the group's bundle ships in, set with `bundle` by
    /// the same entry, so the pair never mixes releases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compute_bundle: Option<String>,
    /// Each timer's last applied slot (D34).
    #[serde(default)]
    pub timers: BTreeMap<String, u64>,
    /// The idempotency keys of each class (caller, `node:`) in the order they were
    /// recorded (D74).
    #[serde(default)]
    pub idempotency: [idempotency::Ring; 2],
    /// Blobs the app forgot (idle past their time, R8) whose bytes the leader has yet to
    /// collect, hex; at most [`MAX_COLLECT_QUEUE`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collect: Vec<String>,
    /// The plaintext bytes of the files among `collect`, which the quota releases once the
    /// leader's `BlobsCollected` entry applies.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub collect_files: BTreeMap<String, u64>,
    /// The xcall outbox, dead letters and the watermarks of the apps that send here.
    #[serde(default)]
    pub xcall: XcallHead,
    /// The actions started and not ended (D17).
    #[serde(default)]
    pub actions: ActionsHead,
    /// The ids of the runs that are queued or running (at most `MAX_ACTIVE_RUNS`).
    #[serde(default)]
    pub active_runs: BTreeSet<String>,
    /// The last finished runs, oldest first; their records are kept until they leave it.
    #[serde(default)]
    pub finished_runs: VecDeque<String>,
}

impl ComponentHead {
    /// An entry stamped `ts_ms` wrote the app's data.
    pub fn note_write(&mut self, ts_ms: u64) {
        self.last_write_ms = self.last_write_ms.max(ts_ms);
    }
}

/// A component app's state on a replica: its host records, and its storage in
/// memory (the rows on disk are the durable copy).
#[derive(Clone, Debug, Default)]
pub struct ComponentState {
    pub head: ComponentHead,
    /// The app's data, stored keys (generation prefix included). Shared with
    /// running calls and views as their `StateView` (D46).
    pub kv: Kv,
    /// The blobs the app holds (R8), host records.
    pub blobs: BlobTable,
}

impl ComponentState {
    /// Read the app's storage and blob rows into memory (after a restart or a snapshot install).
    pub fn load(&mut self, state: &GroupState) -> eyre::Result<()> {
        let mut kv = BTreeMap::new();
        state.scan_hidden(rows::CKV, |k, v| {
            let _prev = kv.insert(k.to_vec(), v.to_vec());
            Ok(true)
        })?;
        self.kv = Arc::new(kv);
        self.blobs.load(state)
    }

    /// Forget the blobs idle past their time at `now_ms` and queue their bytes for the
    /// leader to collect.
    fn expire_blobs(&mut self, tx: &mut StateTx<'_>, now_ms: u64) {
        for (id, file_size) in self.blobs.expire(tx, now_ms) {
            let id = hex::encode(id);
            if let Some(size) = file_size {
                let _prev = self.head.collect_files.insert(id.clone(), size);
            }
            self.head.collect.push(id);
        }
        let over = self.head.collect.len().saturating_sub(MAX_COLLECT_QUEUE);
        for dropped in self.head.collect.drain(..over) {
            let _gone = self.head.collect_files.remove(&dropped);
        }
    }
}

/// How many applied ticks `AppMachine::ticks` keeps (feature `testing`).
#[cfg(feature = "testing")]
pub const TICK_RING: usize = 64;

/// One applied tick: its timer, slot and outcome (feature `testing`).
#[cfg(feature = "testing")]
#[derive(Clone, Debug)]
pub struct TickApplied {
    pub timer: String,
    pub scheduled_ms: u64,
    pub applied: Applied,
}

fn idem_key(caller: &Caller, key: &str) -> String {
    match caller {
        Caller::App { app } => format!("app:{app}|{key}"),
        Caller::System => format!("system|{key}"),
        _ => format!("{}|{key}", caller.account().unwrap_or("public")),
    }
}

impl AppMachine {
    /// The bundle a component's calls and views run, once its `init` applied (it
    /// succeeded, or failed for good). `None` while init is pending, even when an
    /// upgrade taken before it already named a bundle.
    #[must_use]
    pub fn component_bundle(&self) -> Option<String> {
        (self.state.initialized || self.state.init_error.is_some())
            .then(|| self.state.bundle.clone())
            .flatten()
    }

    /// A component's previous answer for this idempotency key, if still remembered.
    #[must_use]
    pub fn component_replay(&self, caller: &Caller, key: &str) -> Option<Applied> {
        let raw = self
            .state
            .kv
            .group()?
            .get_hidden(rows::CIDEM, idem_key(caller, key).as_bytes())?;
        serde_json::from_slice::<IdemRecord>(&raw)
            .ok()
            .map(|r| r.applied)
    }

    /// The state a component's run reads (D46): the current version, shared.
    #[must_use]
    pub fn view(&self) -> Arc<dyn host::StateView> {
        Arc::clone(&self.state.component.kv) as Arc<dyn host::StateView>
    }

    /// The blobs a component's run sees (R8): the current records, shared.
    #[must_use]
    pub fn blob_view(&self) -> blobs::Blobs {
        Arc::clone(&self.state.component.blobs.records)
    }

    /// Pending action `id`, if it has not ended.
    #[must_use]
    pub fn action_record(&self, id: &str) -> Option<ActionRecord> {
        let raw = self
            .state
            .kv
            .group()?
            .get_hidden(rows::CACT, &action::action_key(id))?;
        serde_json::from_slice(&raw).ok()
    }

    /// The pending actions (D17), by id.
    #[must_use]
    pub fn pending_actions(&self) -> Vec<ActionRecord> {
        self.state
            .component
            .head
            .actions
            .pending
            .iter()
            .filter_map(|id| self.action_record(id))
            .collect()
    }

    /// An action failed for good (D22): an `ActionFailed` event to whoever started it.
    fn action_failed(
        &mut self,
        tx: &mut StateTx<'_>,
        index: u64,
        starter: &Caller,
        id: &str,
        method: &str,
        why: &str,
    ) {
        let Some(account) = starter.account() else {
            return;
        };
        self.push_event(
            tx,
            EventRecord {
                index,
                // After any event of the entry's own run.
                seq: u32::MAX,
                kind: "ActionFailed".to_owned(),
                data: json!({ "actionId": id, "method": method, "reason": why }),
                audience: Some(crate::types::Audience::Accounts {
                    accounts: vec![account.to_owned()],
                }),
                caller: Some(account.to_owned()),
            },
        );
    }

    /// The outbox's message `seq`, if it is still there.
    #[must_use]
    pub fn outgoing(&self, seq: u64) -> Option<xcall::Outgoing> {
        let raw = self
            .state
            .kv
            .group()?
            .get_hidden(rows::COUT, &xcall::outbox_key(seq))?;
        serde_json::from_slice(&raw).ok()
    }

    /// The app's dead letters, oldest first (D29).
    #[must_use]
    pub fn dead_letters(&self) -> Vec<xcall::DeadLetter> {
        let h = &self.state.component.head.xcall;
        let Some(group) = self.state.kv.group() else {
            return Vec::new();
        };
        (h.dead_first..h.dead_next)
            .filter_map(|n| group.get_hidden(rows::CDEAD, &xcall::dead_key(n)))
            .filter_map(|raw| serde_json::from_slice(&raw).ok())
            .collect()
    }

    /// Whether `slot` of `timer` was already applied.
    #[must_use]
    pub fn tick_applied(&self, timer: &str, slot: u64) -> bool {
        self.state
            .component
            .head
            .timers
            .get(timer)
            .is_some_and(|last| *last >= slot)
    }

    fn apply_component_outcome(
        &mut self,
        tx: &mut StateTx<'_>,
        index: u64,
        base_index: u64,
        ts_ms: u64,
        outcome: &Outcome,
        starter: &Caller,
    ) -> Applied {
        let caller = starter.account();
        if base_index != self.state.last_index {
            return Applied::conflict(index, "conflict: retry");
        }
        let mut touched = Vec::new();
        let c = &mut self.state.component;
        c.head.last_ts_ms = c.head.last_ts_ms.max(ts_ms);
        if outcome.status == Status::Ok {
            // Files first, so the references this entry's diff stores to them are counted.
            for mark in &outcome.files {
                c.blobs.add_file(tx, mark, ts_ms);
            }
            if !outcome.diff.is_empty() {
                // References to blobs move with the values that hold them (R8).
                let mut deltas = BTreeMap::new();
                let kv = Arc::make_mut(&mut c.kv);
                for w in &outcome.diff {
                    // Search catches up from the records an entry touched.
                    if search::record_table(&w.key.0).is_some() {
                        touched.push(hex::encode(&w.key.0));
                    }
                    match &w.value {
                        Some(v) => {
                            tx.put_hidden(rows::CKV, &w.key.0, &v.0);
                            let prev = kv.insert(w.key.0.clone(), v.0.clone());
                            ref_deltas(&mut deltas, prev.as_deref(), Some(&v.0));
                        }
                        None => {
                            tx.delete_hidden(rows::CKV, &w.key.0);
                            let prev = kv.remove(&w.key.0);
                            ref_deltas(&mut deltas, prev.as_deref(), None);
                        }
                    }
                }
                deltas.retain(|_, d| *d != 0);
                c.blobs.count(tx, &deltas, ts_ms);
            }
            self.apply_run_changes(tx, ts_ms, starter, outcome);
            self.note_touched(index, touched);
            if !outcome.xcalls.is_empty() {
                self.state
                    .component
                    .head
                    .xcall
                    .send(tx, &outcome.xcalls, ts_ms);
            }
            if !outcome.actions.is_empty() {
                let refused =
                    self.state
                        .component
                        .head
                        .actions
                        .start(tx, &outcome.actions, starter, ts_ms);
                for a in refused {
                    self.action_failed(
                        tx,
                        index,
                        starter,
                        &a.id,
                        &a.method,
                        "too many pending actions",
                    );
                }
            }
            for (seq, e) in outcome.events.iter().enumerate() {
                self.push_event(
                    tx,
                    EventRecord {
                        index,
                        seq: u32::try_from(seq).unwrap_or(u32::MAX),
                        kind: e.kind.clone(),
                        data: e.data.clone(),
                        audience: Some(e.audience.clone()),
                        caller: caller.map(str::to_owned),
                    },
                );
            }
        }
        Applied {
            index,
            status: outcome.status.clone(),
            result: outcome.result.clone(),
            error: outcome.error.clone(),
            events: if outcome.status == Status::Ok {
                outcome.events.clone()
            } else {
                Vec::new()
            },
            conflict: false,
            timeout: false,
        }
    }

    /// Remember a call's answer under its idempotency key until its class's time
    /// (D74) is past its entry's timestamp.
    fn remember_component(
        &mut self,
        tx: &mut StateTx<'_>,
        ts_ms: u64,
        caller: &Caller,
        key: &str,
        applied: &Applied,
    ) {
        let id = idem_key(caller, key);
        // The class (and so the TTL) is the call's key's prefix (D74, spec v0.26).
        let class = idempotency::class_of(key);
        let record = IdemRecord {
            applied: applied.clone(),
            expires_ms: ts_ms.saturating_add(idempotency::ttl_ms(class)),
        };
        match serde_json::to_vec(&record) {
            Ok(v) => idempotency::record(
                tx,
                &IDEM_ROWS,
                &mut self.state.component.head.idempotency,
                class,
                &id,
                &v,
            ),
            Err(e) => tx.fail(e),
        }
    }

    /// Forget what D74 lets go, against the earlier of the newest applied entry's
    /// timestamp and this entry's own (`idempotency::expiry_now`).
    fn expire_component(&mut self, tx: &mut StateTx<'_>, entry_ts_ms: Option<u64>) {
        let now = idempotency::expiry_now(self.state.component.head.last_ts_ms, entry_ts_ms);
        idempotency::expire(
            tx,
            &IDEM_ROWS,
            &mut self.state.component.head.idempotency,
            self.idem_limits,
            now,
            stamps,
        );
    }

    fn note_id(&mut self, term: u64, seq: u64) {
        let head = &mut self.state.component.head;
        head.last_id = head.last_id.max((term, seq));
    }

    /// A tick for a slot at or before the timer's last applied one changes
    /// nothing (two leaders around a failover proposed it); any other is applied
    /// like a call and its slot recorded, whatever its status (D34).
    fn apply_tick(&mut self, tx: &mut StateTx<'_>, index: u64, tick: &TickEntry) -> Applied {
        if self.tick_applied(&tick.timer, tick.scheduled_ms) {
            return Applied::conflict(index, "slot already applied");
        }
        let a = self.apply_component_outcome(
            tx,
            index,
            tick.base_index,
            tick.ts_ms,
            &tick.outcome,
            &Caller::System,
        );
        if !a.conflict {
            let _prev = self
                .state
                .component
                .head
                .timers
                .insert(tick.timer.clone(), tick.scheduled_ms);
            #[cfg(feature = "testing")]
            {
                self.ticks.push_back(TickApplied {
                    timer: tick.timer.clone(),
                    scheduled_ms: tick.scheduled_ms,
                    applied: a.clone(),
                });
                if self.ticks.len() > TICK_RING {
                    let _old = self.ticks.pop_front();
                }
            }
        }
        a
    }

    /// Records a node command's answer under its request's key, as a call's is; the first
    /// answer under a key stays.
    fn remember_command(
        &mut self,
        tx: &mut StateTx<'_>,
        ts_ms: u64,
        key: Option<&str>,
        applied: &Applied,
    ) {
        let known = key.is_some_and(|k| self.component_replay(&Caller::System, k).is_some());
        if let (Some(key), false) = (key, known) {
            self.remember_component(tx, ts_ms, &Caller::System, key, applied);
        }
    }

    /// Apply one entry of a component app. An entry that is not a component's
    /// (an SDK 0.12 app's format) changes nothing.
    pub(crate) fn apply_component(
        &mut self,
        tx: &mut StateTx<'_>,
        index: u64,
        command: &[u8],
    ) -> Vec<u8> {
        let cmd = serde_json::from_slice::<ComponentCmd>(command);
        let entry_ts_ms = match &cmd {
            Ok(ComponentCmd::Init { ts_ms, .. }) => Some(*ts_ms),
            Ok(ComponentCmd::Call(call)) => Some(call.ts_ms),
            Ok(ComponentCmd::Tick(tick)) => Some(tick.ts_ms),
            Ok(ComponentCmd::ComputeFinal(done)) => Some(done.ts_ms),
            Ok(
                ComponentCmd::Blob { ts_ms, .. }
                | ComponentCmd::File { ts_ms, .. }
                | ComponentCmd::XcallDone { ts_ms, .. }
                | ComponentCmd::ActionEnd { ts_ms, .. },
            ) => Some(*ts_ms),
            Ok(
                ComponentCmd::Upgrade { .. }
                | ComponentCmd::BlobForget { .. }
                | ComponentCmd::BlobsCollected { .. }
                | ComponentCmd::ComputeAttempt(_)
                | ComponentCmd::Barrier,
            )
            | Err(_) => None,
        };
        match &cmd {
            Ok(ComponentCmd::Init {
                id_term, id_seq, ..
            }) => self.note_id(*id_term, *id_seq),
            Ok(ComponentCmd::Call(call)) => self.note_id(call.id_term, call.id_seq),
            Ok(ComponentCmd::Tick(tick)) => self.note_id(tick.id_term, tick.id_seq),
            Ok(ComponentCmd::ComputeFinal(done)) => self.note_id(done.id_term, done.id_seq),
            _ => {}
        }
        let applied = match cmd {
            Ok(ComponentCmd::Init {
                bundle,
                compute_bundle,
                base_index,
                ts_ms,
                outcome,
                ..
            }) => {
                // Init applies once: it succeeded (`initialized`) or failed for good
                // (`init_error`). An upgrade taken before it gives the app a bundle,
                // not an init.
                if self.state.initialized || self.state.init_error.is_some() {
                    Applied::conflict(index, "already initialized")
                } else {
                    // The runs init starts name its bundle and compute component.
                    let before = self.state.bundle.replace(bundle.clone());
                    let before_compute = std::mem::replace(
                        &mut self.state.component.head.compute_bundle,
                        compute_bundle.clone(),
                    );
                    let a = self.apply_component_outcome(
                        tx,
                        index,
                        base_index,
                        ts_ms,
                        &outcome,
                        &Caller::System,
                    );
                    if a.conflict {
                        self.state.bundle = before;
                        self.state.component.head.compute_bundle = before_compute;
                    } else {
                        self.state.executor = Some(crate::types::Executor::Component);
                        self.state.bundle = Some(bundle);
                        self.state.component.head.install_ts_ms = ts_ms;
                        self.state.initialized = outcome.status == Status::Ok;
                        if !self.state.initialized {
                            self.state.init_error = Some(json!({
                                "status": outcome.status,
                                "error": outcome.error,
                            }));
                        }
                    }
                    a
                }
            }
            Ok(ComponentCmd::Upgrade {
                bundle,
                compute_bundle,
                base_index,
            }) => {
                let a = self.apply_component_outcome(
                    tx,
                    index,
                    base_index,
                    0,
                    &Outcome::default(),
                    &Caller::System,
                );
                if !a.conflict {
                    // Taken before `init` applied: the entry names the executor.
                    self.state.executor = Some(crate::types::Executor::Component);
                    self.state.bundle = Some(bundle);
                    self.state.component.head.compute_bundle = compute_bundle;
                }
                a
            }
            Ok(ComponentCmd::Call(call)) => {
                let a = self.apply_component_outcome(
                    tx,
                    index,
                    call.base_index,
                    call.ts_ms,
                    &call.outcome,
                    &call.caller,
                );
                let wrote = !call.outcome.diff.is_empty() || !call.outcome.files.is_empty();
                if !a.conflict && a.status == Status::Ok && wrote {
                    self.state.component.head.note_write(call.ts_ms);
                }
                if let (Some(key), false) = (&call.idempotency_key, a.conflict) {
                    self.remember_component(tx, call.ts_ms, &call.caller, key, &a);
                }
                // Whatever its status, a delivered xcall was answered (D29).
                if let (Some(from), false) = (&call.xcall, a.conflict) {
                    self.state.component.head.xcall.delivered(from);
                }
                a
            }
            Ok(ComponentCmd::Tick(tick)) => self.apply_tick(tx, index, &tick),
            Ok(ComponentCmd::Blob {
                id,
                size,
                mime,
                ts_ms,
            }) => {
                let c = &mut self.state.component;
                c.head.last_ts_ms = c.head.last_ts_ms.max(ts_ms);
                let held =
                    parse_hex32(&id).is_some_and(|raw| c.blobs.upload(tx, raw, size, &mime, ts_ms));
                if held {
                    c.head.note_write(ts_ms);
                }
                host_answer(index, held, "the app holds too many blobs")
            }
            Ok(ComponentCmd::File { mark, ts_ms, key }) => {
                let c = &mut self.state.component;
                c.head.last_ts_ms = c.head.last_ts_ms.max(ts_ms);
                let raw = parse_hex32(&mark.id).filter(|id| !c.blobs.records.contains_key(id));
                let a = if raw.is_some() {
                    c.blobs.add_file(tx, &mark, ts_ms);
                    c.head.note_write(ts_ms);
                    Applied {
                        result: json!({ "mark": mark }),
                        ..host_answer(index, true, "")
                    }
                } else {
                    host_answer(index, false, "file exists")
                };
                self.remember_command(tx, ts_ms, key.as_deref(), &a);
                a
            }
            Ok(ComponentCmd::Barrier) => host_answer(index, true, ""),
            Ok(ComponentCmd::ComputeAttempt(attempt)) => {
                self.apply_compute_attempt(tx, index, &attempt)
            }
            Ok(ComponentCmd::ComputeFinal(done)) => self.apply_compute_final(tx, index, &done),
            Ok(ComponentCmd::BlobForget { id }) => {
                let c = &mut self.state.component;
                let held = parse_hex32(&id).is_some_and(|raw| c.blobs.forget_blob(tx, &raw));
                host_answer(index, held, "no such blob in this app")
            }
            Ok(ComponentCmd::ActionEnd {
                id,
                failed,
                retry,
                ts_ms,
            }) => {
                let c = &mut self.state.component;
                c.head.last_ts_ms = c.head.last_ts_ms.max(ts_ms);
                match self.action_record(&id) {
                    None => host_answer(index, false, "no such pending action"),
                    Some(record) if retry => {
                        self.state.component.head.actions.retried(tx, record);
                        host_answer(index, true, "")
                    }
                    Some(record) => {
                        let _was = self.state.component.head.actions.ended(tx, &id);
                        if let Some(why) = &failed {
                            self.action_failed(tx, index, &record.caller, &id, &record.method, why);
                        }
                        host_answer(index, true, "")
                    }
                }
            }
            Ok(ComponentCmd::XcallDone { seq, dead, ts_ms }) => {
                let head = self.outgoing(seq);
                let c = &mut self.state.component;
                c.head.last_ts_ms = c.head.last_ts_ms.max(ts_ms);
                let done = c.head.xcall.done(tx, head, seq, dead.as_deref(), ts_ms);
                host_answer(index, done, "not the outbox's head")
            }
            Ok(ComponentCmd::BlobsCollected { ids }) => {
                let head = &mut self.state.component.head;
                head.collect.retain(|id| !ids.contains(id));
                let freed: u64 = ids
                    .iter()
                    .filter_map(|id| head.collect_files.remove(id))
                    .sum();
                Applied {
                    result: json!({ "freed": freed }),
                    ..host_answer(index, true, "")
                }
            }
            Err(e) => Applied {
                index,
                status: Status::Trap,
                result: Value::Null,
                error: json!({ "kind": "Fault", "detail": format!("not a component's entry: {e}") }),
                events: Vec::new(),
                conflict: true,
                timeout: false,
            },
        };
        self.expire_component(tx, entry_ts_ms);
        if let Some(now) =
            idempotency::expiry_now(self.state.component.head.last_ts_ms, entry_ts_ms)
        {
            self.state.component.expire_blobs(tx, now);
        }
        serde_json::to_vec(&applied).unwrap_or_default()
    }
}

/// A blob id as the host commands carry it (hex).
fn parse_hex32(id: &str) -> Option<[u8; 32]> {
    hex::decode(id)
        .ok()
        .and_then(|raw| <[u8; 32]>::try_from(raw.as_slice()).ok())
}

/// The answer to a host command (an upload, a delete, a collection): `Ok`, or an
/// `AppError` with `why`.
pub(crate) fn host_answer(index: u64, ok: bool, why: &str) -> Applied {
    Applied {
        index,
        status: if ok { Status::Ok } else { Status::AppError },
        result: Value::Null,
        error: if ok {
            Value::Null
        } else {
            Value::String(why.to_owned())
        },
        events: Vec::new(),
        conflict: false,
        timeout: false,
    }
}

/// The ticks a replica applied, newest last (feature `testing`).
#[cfg(feature = "testing")]
pub type Ticks = std::collections::VecDeque<TickApplied>;

/// What the host core asks the node about identity, answered from this node's
/// replica of the system state at the moment it asks (the run is the leader's,
/// and only its outcome is logged).
pub struct SysDirectory {
    node: Weak<Node>,
    app_id: AppId,
}

impl SysDirectory {
    #[must_use]
    pub fn new(node: Weak<Node>, app_id: AppId) -> Self {
        Self { node, app_id }
    }

    fn with<R: Default>(
        &self,
        f: impl FnOnce(&crate::system::SystemState, &crate::types::AppRecord) -> R,
    ) -> R {
        let Some(node) = self.node.upgrade() else {
            return R::default();
        };
        node.sys(|s| {
            s.apps
                .get(&self.app_id)
                .map(|a| f(s, a))
                .unwrap_or_default()
        })
    }

    fn with_org<R: Default>(&self, f: impl FnOnce(&crate::types::Org) -> R) -> R {
        self.with(|s, a| {
            a.org
                .as_ref()
                .and_then(|o| s.orgs.get(o))
                .map(f)
                .unwrap_or_default()
        })
    }
}

fn host_role(r: crate::types::OrgRole) -> host::OrgRole {
    match r {
        crate::types::OrgRole::Guest => host::OrgRole::Guest,
        crate::types::OrgRole::Member => host::OrgRole::Member,
        crate::types::OrgRole::Admin => host::OrgRole::Admin,
        crate::types::OrgRole::Owner => host::OrgRole::Owner,
    }
}

impl host::Directory for SysDirectory {
    fn org(&self) -> Option<String> {
        self.with(|_, a| a.org.clone())
    }

    fn app_owner(&self) -> String {
        self.with(|_, a| a.owner.clone())
    }

    fn org_role(&self, account: &str) -> Option<host::OrgRole> {
        self.with_org(|o| o.role(account).map(host_role))
    }

    fn in_team(&self, account: &str, team: &str) -> bool {
        self.with_org(|o| o.in_team(account, team))
    }

    fn team_exists(&self, team: &str) -> bool {
        self.with_org(|o| o.teams.contains_key(team))
    }

    /// The app's own organization only (D75, spec v0.26): for `account`, at most the
    /// app's org with the account's role in it; a personal app learns nothing about
    /// its callers' organizations. A component runs code the platform doesn't vet,
    /// so it never maps its callers' memberships in other tenants.
    fn orgs_of(&self, account: &str) -> Vec<(String, host::OrgRole)> {
        self.with(|s, a| {
            a.org
                .as_ref()
                .and_then(|id| {
                    s.orgs
                        .get(id)
                        .and_then(|o| o.role(account))
                        .map(|r| vec![(id.clone(), host_role(r))])
                })
                .unwrap_or_default()
        })
    }
}

/// The ABI JSON a component embeds (its top-level `calimero:abi` section), as the
/// build wrote it; `None` when it carries none.
#[must_use]
pub fn embedded_abi(component: &[u8]) -> Option<Value> {
    for payload in wasmparser::Parser::new(0).parse_all(component) {
        match payload {
            Ok(wasmparser::Payload::CustomSection(section)) if section.name() == "calimero:abi" => {
                return serde_json::from_slice(section.data()).ok();
            }
            // A nested module's or component's sections are not the component's own.
            Ok(
                wasmparser::Payload::ModuleSection { .. }
                | wasmparser::Payload::ComponentSection { .. },
            ) => {}
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    mod runs;
    use crate::component::blobs::BLOB_IDLE_MS;

    #[test]
    fn a_file_never_outgrows_the_blob_cap() {
        assert!(host::MAX_FILE_BYTES <= crate::node::MAX_BLOB_BYTES as u64);
    }

    /// D76: a logged run's outbound exchange is part of the outcome its entry carries,
    /// whatever the status, and reads back from the entry's bytes as it was recorded.
    #[test]
    fn a_runs_outbound_exchange_is_recorded_in_its_entry() {
        let request = host::HttpRequest {
            method: "POST".into(),
            url: "https://challenges.cloudflare.com/turnstile/v0/siteverify".into(),
            headers: vec![("Content-Type".into(), "x".into())],
            body: b"secret={{secret:TURNSTILE_SECRET}}&response=t".to_vec(),
        };
        let response = Ok(host::HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: br#"{"success":true}"#.to_vec(),
        });
        let out = Outcome::from_host(host::Outcome {
            status: host::Status::AppError {
                code: "No".into(),
                message: "{}".into(),
            },
            result: Vec::new(),
            diff: Vec::new(),
            events: Vec::new(),
            xcalls: Vec::new(),
            actions: Vec::new(),
            logs: Vec::new(),
            http: vec![host::HttpExchange::record(&request, &response)],
            new_files: Vec::new(),
            compute_starts: Vec::new(),
            compute_cancels: Vec::new(),
            usage: host::Usage::default(),
        });
        let entry: Outcome = serde_json::from_slice(&serde_json::to_vec(&out).unwrap()).unwrap();
        assert_eq!(entry.http.len(), 1);
        let x = &entry.http[0];
        assert_eq!(x.status, Some(200));
        assert_eq!(x.response_body.0, br#"{"success":true}"#);
        // The request as the app made it: the placeholder, never the secret.
        assert!(String::from_utf8_lossy(&x.body.0).contains("{{secret:TURNSTILE_SECRET}}"));
        // An outcome without one keeps its old bytes (no `http` key).
        let plain = serde_json::to_value(Outcome::default()).unwrap();
        assert!(plain.get("http").is_none());
    }

    #[test]
    fn the_outcome_of_a_run_carries_its_new_files_as_marks_and_only_when_ok() {
        let run = |status| host::Outcome {
            status,
            result: Vec::new(),
            diff: Vec::new(),
            events: Vec::new(),
            xcalls: Vec::new(),
            actions: Vec::new(),
            logs: Vec::new(),
            http: Vec::new(),
            compute_starts: Vec::new(),
            compute_cancels: Vec::new(),
            new_files: vec![host::NewFile {
                id: fid(1),
                size: 5,
                sha256: vec![1; 32],
                wrapped_key: vec![2; 60],
                ciphertext: vec![3; 21],
            }],
            usage: host::Usage::default(),
        };
        let ok = Outcome::from_host(run(host::Status::Ok));
        assert_eq!(ok.files, [mark(1)]);
        // The ciphertext is not in what the log carries.
        assert!(!serde_json::to_string(&ok).unwrap().contains("ciphertext"));
        let trapped = Outcome::from_host(run(host::Status::Trap {
            kind: host::TrapKind::Fault,
            detail: String::new(),
        }));
        assert!(trapped.files.is_empty());
    }

    #[test]
    fn only_wall_clock_and_node_unavailability_are_retryable() {
        use host::TrapKind::*;
        let wire: Vec<_> = [
            OutOfFuel,
            Timeout,
            Cancelled,
            Unavailable,
            OutOfMemory,
            StackOverflow,
            Fault,
            HostDenied,
        ]
        .into_iter()
        .map(|kind| (trap_name(kind), Outcome::trap(kind, "d").is_retryable()))
        .collect();
        assert_eq!(
            wire,
            [
                ("OutOfFuel", false),
                ("Timeout", true),
                ("Cancelled", false),
                ("Unavailable", true),
                ("OutOfMemory", false),
                ("StackOverflow", false),
                ("Fault", false),
                ("HostDenied", false),
            ]
        );
        assert!(!Outcome::default().is_retryable());
    }

    /// A machine on a real, empty group state (the log's store, in a temp dir).
    struct Rig {
        _dir: tempfile::TempDir,
        engine: Arc<calimero_log::Engine>,
        machine: AppMachine,
    }

    struct NoTransport;
    impl calimero_log::Transport for NoTransport {
        fn send(
            &self,
            _target: calimero_log::NodeId,
            _addr: String,
            _group: calimero_log::GroupId,
            _request: calimero_log::RpcRequest,
        ) -> calimero_log::BoxFuture<Result<calimero_log::RpcResponse, String>> {
            Box::pin(async { Err("no peers".to_owned()) })
        }
    }

    impl Rig {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let engine = calimero_log::Engine::open(
                1,
                dir.path(),
                &[7; 32],
                Arc::new(NoTransport),
                calimero_log::Config::default(),
            )
            .unwrap();
            let mut machine = AppMachine::new("app".into());
            calimero_log::Machine::attach(&mut machine, engine.group_state(5));
            Self {
                _dir: dir,
                engine,
                machine,
            }
        }

        fn apply(&mut self, index: u64, cmd: &[u8]) -> Applied {
            let state = self.engine.group_state(5);
            let mut tx = StateTx::new(&state);
            let out = calimero_log::Machine::apply(&mut self.machine, index, cmd, &mut tx);
            tx.commit().unwrap();
            serde_json::from_slice(&out).unwrap()
        }
    }

    fn init(base: u64) -> Vec<u8> {
        serde_json::to_vec(&ComponentCmd::Init {
            bundle: "b".into(),
            compute_bundle: None,
            base_index: base,
            ts_ms: 1,
            id_term: 1,
            id_seq: 1,
            outcome: Box::default(),
        })
        .unwrap()
    }

    fn call(index_base: u64, key: Option<&str>, diff: Vec<Write>) -> Vec<u8> {
        serde_json::to_vec(&ComponentCmd::Call(Box::new(CallEntry {
            method: "m".into(),
            args: Value::Null,
            caller: Caller::Account {
                account: "a".into(),
            },
            idempotency_key: key.map(str::to_owned),
            consent: None,
            ts_ms: 1,
            base_index: index_base,
            id_term: 1,
            id_seq: 1,
            outcome: Outcome {
                diff,
                ..Outcome::default()
            },
            xcall: None,
        })))
        .unwrap()
    }

    fn tick(base: u64, slot: u64, status: Status, diff: Vec<Write>) -> Vec<u8> {
        serde_json::to_vec(&ComponentCmd::Tick(Box::new(TickEntry {
            timer: "beat".into(),
            scheduled_ms: slot,
            previous_ms: None,
            ts_ms: slot,
            base_index: base,
            id_term: 1,
            id_seq: slot,
            outcome: Outcome {
                status,
                diff,
                ..Outcome::default()
            },
        })))
        .unwrap()
    }

    fn w(k: &str, v: &str) -> Write {
        Write {
            key: Bytes(k.as_bytes().to_vec()),
            value: Some(Bytes(v.as_bytes().to_vec())),
        }
    }

    /// The id of a file in these tests: hex of 32 bytes, all `n`.
    fn fid(n: u8) -> String {
        hex::encode([n; 32])
    }

    fn mark(n: u8) -> FileMark {
        FileMark {
            id: fid(n),
            size: 5,
            sha256: Bytes(vec![1; 32]),
            wrapped_key: Bytes(vec![2; 60]),
        }
    }

    /// A stored value that is a `BlobRef` to file `n`.
    fn blob_ref(n: u8) -> Vec<u8> {
        let mut out = Vec::new();
        ciborium::into_writer(
            &ciborium::Value::Tag(
                calimero_app_host::BLOB_REF_TAG,
                Box::new(ciborium::Value::Bytes(vec![n; 32])),
            ),
            &mut out,
        )
        .unwrap();
        out
    }

    fn store_ref(key: &str, n: u8) -> Write {
        Write {
            key: Bytes(key.as_bytes().to_vec()),
            value: Some(Bytes(blob_ref(n))),
        }
    }

    fn remove(key: &str) -> Write {
        Write {
            key: Bytes(key.as_bytes().to_vec()),
            value: None,
        }
    }

    fn file_call(
        base: u64,
        ts_ms: u64,
        status: Status,
        files: Vec<FileMark>,
        diff: Vec<Write>,
    ) -> Vec<u8> {
        let mut cmd: ComponentCmd = serde_json::from_slice(&call(base, None, diff)).unwrap();
        if let ComponentCmd::Call(c) = &mut cmd {
            c.ts_ms = ts_ms;
            c.outcome.status = status;
            c.outcome.files = files;
        }
        serde_json::to_vec(&cmd).unwrap()
    }

    fn held(r: &Rig) -> Vec<String> {
        let records = &r.machine.state.component.blobs.records;
        records.keys().map(hex::encode).collect()
    }

    fn queued(r: &Rig) -> Vec<String> {
        r.machine.state.component.head.collect.clone()
    }

    #[test]
    fn an_ok_entry_records_its_files_and_a_failed_one_does_not() {
        let mut r = Rig::new();
        let _i = r.apply(1, &init(0));
        let _a = r.apply(
            2,
            &file_call(1, 1, Status::Ok, vec![mark(1), mark(2)], vec![]),
        );
        assert_eq!(held(&r), [fid(1), fid(2)]);
        let _a = r.apply(3, &file_call(2, 1, Status::AppError, vec![mark(3)], vec![]));
        assert_eq!(held(&r), [fid(1), fid(2)], "a failed run recorded a file");
        // Built on a state that is no longer current: nothing applied.
        let stale = file_call(1, 1, Status::Ok, vec![mark(4)], vec![]);
        assert!(r.apply(4, &stale).conflict);
        assert_eq!(held(&r), [fid(1), fid(2)]);
    }

    #[test]
    fn a_file_stored_as_a_blob_ref_survives_and_one_not_stored_is_forgotten() {
        let mut r = Rig::new();
        let _i = r.apply(1, &init(0));
        // One entry makes both files and stores a reference to the first.
        let made = file_call(
            1,
            1,
            Status::Ok,
            vec![mark(1), mark(2)],
            vec![store_ref("k", 1)],
        );
        let _a = r.apply(2, &made);
        let records = &r.machine.state.component.blobs.records;
        assert_eq!(
            records[&[1; 32]].refs, 1,
            "counted in the entry that made it"
        );
        // Just inside the idle time nothing is forgotten.
        let _a = r.apply(3, &file_call(2, BLOB_IDLE_MS, Status::Ok, vec![], vec![]));
        assert_eq!(held(&r), [fid(1), fid(2)]);
        assert!(queued(&r).is_empty());
        // Past it the unreferenced file goes and is queued for collection; the stored one stays.
        let _a = r.apply(
            4,
            &file_call(3, BLOB_IDLE_MS + 2, Status::Ok, vec![], vec![]),
        );
        assert_eq!(held(&r), [fid(1)]);
        assert_eq!(queued(&r), [fid(2)]);
    }

    fn file_cmd(n: u8, ts_ms: u64, key: Option<&str>) -> Vec<u8> {
        serde_json::to_vec(&ComponentCmd::File {
            mark: mark(n),
            ts_ms,
            key: key.map(str::to_owned),
        })
        .unwrap()
    }

    fn collected(ids: &[String]) -> Vec<u8> {
        serde_json::to_vec(&ComponentCmd::BlobsCollected { ids: ids.to_vec() }).unwrap()
    }

    #[test]
    fn an_uploaded_file_is_held_idle_and_a_second_under_its_id_is_refused() {
        let mut r = Rig::new();
        let _i = r.apply(1, &init(0));
        let up = r.apply(2, &file_cmd(1, 5, Some("node:k")));
        assert_eq!(up.status, Status::Ok);
        assert_eq!(up.result["mark"]["id"], json!(fid(1)));
        assert_eq!(held(&r), [fid(1)]);
        let record = &r.machine.state.component.blobs.records[&[1; 32]];
        assert!((record.file.is_some(), record.refs, record.idle_since_ms) == (true, 0, 5));
        // Its answer is what a retry under the key gets back.
        let kept = r
            .machine
            .component_replay(&Caller::System, "node:k")
            .unwrap();
        assert_eq!(kept.result["mark"]["id"], json!(fid(1)));
        assert_eq!(
            r.apply(3, &file_cmd(1, 6, Some("node:k2"))).status,
            Status::AppError
        );
    }

    #[test]
    fn collecting_files_answers_the_bytes_they_held_once() {
        let mut r = Rig::new();
        let _i = r.apply(1, &init(0));
        let _a = r.apply(2, &file_cmd(1, 1, None));
        let _a = r.apply(3, &file_cmd(2, 1, None));
        let _a = r.apply(
            4,
            &file_call(3, BLOB_IDLE_MS + 2, Status::Ok, vec![], vec![]),
        );
        let ids = queued(&r);
        assert_eq!(ids, [fid(1), fid(2)]);
        let freed = |a: Applied| a.result["freed"].as_u64().unwrap();
        assert_eq!(freed(r.apply(5, &collected(&ids))), 10);
        assert!(queued(&r).is_empty());
        assert_eq!(
            freed(r.apply(6, &collected(&ids))),
            0,
            "a repeat frees nothing"
        );
    }

    #[test]
    fn a_collected_blob_that_is_not_a_file_frees_no_quota() {
        let mut r = Rig::new();
        let _i = r.apply(1, &init(0));
        let _a = r.apply(2, &file_cmd(1, 1, None));
        let _a = r.apply(
            3,
            &file_call(2, BLOB_IDLE_MS + 2, Status::Ok, vec![], vec![]),
        );
        // A plain blob queued for collection is no file's quota.
        r.machine.state.component.head.collect.push(fid(9));
        let a = r.apply(4, &collected(&[fid(9)]));
        assert_eq!(a.result["freed"], json!(0));
        assert_eq!(r.machine.state.component.head.collect_files.len(), 1);
    }

    #[test]
    fn a_file_whose_last_reference_goes_is_forgotten_after_the_idle_time() {
        let mut r = Rig::new();
        let _i = r.apply(1, &init(0));
        let _a = r.apply(
            2,
            &file_call(
                1,
                1,
                Status::Ok,
                vec![mark(1)],
                vec![store_ref("a", 1), store_ref("b", 1)],
            ),
        );
        let t = 10 * BLOB_IDLE_MS;
        let _a = r.apply(3, &file_call(2, t, Status::Ok, vec![], vec![remove("a")]));
        let _a = r.apply(
            4,
            &file_call(3, t + BLOB_IDLE_MS + 1, Status::Ok, vec![], vec![]),
        );
        assert_eq!(held(&r), [fid(1)], "one reference is left");
        let _a = r.apply(
            5,
            &file_call(4, t + 1, Status::Ok, vec![], vec![remove("b")]),
        );
        let _a = r.apply(
            6,
            &file_call(5, t + BLOB_IDLE_MS, Status::Ok, vec![], vec![]),
        );
        assert_eq!(
            held(&r),
            [fid(1)],
            "idle only since the last reference went"
        );
        let later = t + 1 + BLOB_IDLE_MS + 1;
        let _a = r.apply(7, &file_call(6, later, Status::Ok, vec![], vec![]));
        assert!(held(&r).is_empty());
        assert_eq!(queued(&r), [fid(1)]);
    }

    #[test]
    fn the_stored_files_count_as_blob_bytes_with_their_tags_and_stay_out_of_the_blob_list() {
        let mut r = Rig::new();
        let _i = r.apply(1, &init(0));
        let _a = r.apply(
            2,
            &file_call(1, 1, Status::Ok, vec![mark(1), mark(2)], vec![]),
        );
        let stats = r.machine.storage_stats();
        assert_eq!(stats.blob_bytes, 2 * (5 + 16));
        assert!(stats.blobs.is_empty());
    }

    #[test]
    fn a_file_comes_back_with_its_seal_after_a_restore() {
        let mut r = Rig::new();
        let _i = r.apply(1, &init(0));
        let _a = r.apply(
            2,
            &file_call(1, 1, Status::Ok, vec![mark(1)], vec![store_ref("k", 1)]),
        );
        calimero_log::Machine::restore(&mut r.machine, &[]).unwrap();
        let record = &r.machine.state.component.blobs.records[&[1; 32]];
        let seal = record.file.as_ref().expect("the seal is kept");
        assert_eq!(seal.sha256, mark(1).sha256);
        assert_eq!(seal.wrapped_key, mark(1).wrapped_key);
        assert_eq!((record.size, record.refs), (5, 1));
    }

    #[test]
    fn an_upload_or_an_owner_delete_cannot_touch_a_sealed_file() {
        let mut r = Rig::new();
        let _i = r.apply(1, &init(0));
        let _a = r.apply(2, &file_call(1, 1, Status::Ok, vec![mark(1)], vec![]));
        let upload = serde_json::to_vec(&ComponentCmd::Blob {
            id: fid(1),
            size: 9,
            mime: "text/plain".into(),
            ts_ms: 2,
        })
        .unwrap();
        assert_eq!(
            r.apply(3, &upload).status,
            Status::AppError,
            "an upload claimed a file id"
        );
        let forget = serde_json::to_vec(&ComponentCmd::BlobForget { id: fid(1) }).unwrap();
        assert_eq!(
            r.apply(4, &forget).status,
            Status::AppError,
            "an owner delete took a file"
        );
        let record = &r.machine.state.component.blobs.records[&[1; 32]];
        assert_eq!((record.size, record.mime.as_str()), (5, ""));
        assert!(record.file.is_some());
    }

    /// A call stamped `ts_ms` on `base`, which ended with `status` and `diff`.
    fn call_ending(base: u64, ts_ms: u64, status: Status, diff: Vec<Write>) -> Vec<u8> {
        let ComponentCmd::Call(mut entry) =
            serde_json::from_slice(&call(base, None, diff)).unwrap()
        else {
            panic!("not a call");
        };
        entry.ts_ms = ts_ms;
        entry.outcome.status = status;
        serde_json::to_vec(&ComponentCmd::Call(entry)).unwrap()
    }

    /// The last write is the timestamp of the newest entry that wrote the app's
    /// data: a call that applied with a diff, an uploaded blob. `init`, a call that
    /// wrote nothing, one the app refused, a conflict and a timer tick leave it.
    /// It is kept in the head, so a restart reads it back.
    #[test]
    fn the_last_write_is_the_newest_writing_entrys_timestamp() {
        let mut r = Rig::new();
        assert!(!r.apply(1, &init(0)).conflict);
        assert_eq!(r.machine.last_write_ms(), None, "no write yet");
        assert!(
            !r.apply(2, &call_ending(1, 100, Status::Ok, vec![w("k", "v")]))
                .conflict
        );
        assert_eq!(r.machine.last_write_ms(), Some(100));
        assert!(
            !r.apply(3, &call_ending(2, 200, Status::Ok, vec![]))
                .conflict
        );
        assert!(
            !r.apply(4, &call_ending(3, 300, Status::AppError, vec![w("k", "x")]))
                .conflict
        );
        assert!(
            r.apply(5, &call_ending(1, 400, Status::Ok, vec![w("k", "y")]))
                .conflict
        );
        assert!(
            !r.apply(6, &tick(5, 500, Status::Ok, vec![w("t", "1")]))
                .conflict
        );
        assert_eq!(r.machine.last_write_ms(), Some(100), "none of those wrote");
        let blob = serde_json::to_vec(&ComponentCmd::Blob {
            id: fid(9),
            size: 3,
            mime: "text/plain".into(),
            ts_ms: 600,
        })
        .unwrap();
        assert_eq!(r.apply(7, &blob).status, Status::Ok);
        assert_eq!(r.machine.last_write_ms(), Some(600), "an upload writes");

        let mut again = AppMachine::new("app".into());
        calimero_log::Machine::attach(&mut again, r.engine.group_state(5));
        calimero_log::Machine::restore(&mut again, &[]).unwrap();
        assert_eq!(again.last_write_ms(), Some(600), "read back from the head");
    }

    /// A head written before the last write was kept reads with none and writes
    /// back the same bytes; one that has it reads back as written.
    #[test]
    fn a_head_from_before_the_last_write_reads_and_writes_as_it_did() {
        let before = serde_json::to_value(ComponentHead::default()).unwrap();
        assert!(before.get("lastWriteMs").is_none(), "{before}");
        let read: ComponentHead = serde_json::from_value(before.clone()).unwrap();
        assert_eq!(read.last_write_ms, 0);
        assert_eq!(serde_json::to_value(&read).unwrap(), before);

        let mut head = ComponentHead::default();
        head.note_write(42);
        head.note_write(7);
        let written = serde_json::to_value(&head).unwrap();
        assert_eq!(written["lastWriteMs"], json!(42), "never moves back");
        let back: ComponentHead = serde_json::from_value(written).unwrap();
        assert_eq!(back, head);
    }

    #[test]
    fn an_entry_logged_before_files_existed_still_decodes_and_applies() {
        let old = br#"{"op":"componentCall","method":"m","args":null,
            "caller":{"kind":"account","account":"a"},"tsMs":1,"baseIndex":1,"idTerm":1,"idSeq":1,
            "outcome":{"status":"ok","result":null,"error":null,"diff":[],"events":[],"logs":[]}}"#;
        let ComponentCmd::Call(entry) = serde_json::from_slice(old).unwrap() else {
            panic!("not a call");
        };
        assert!(entry.outcome.files.is_empty());
        let mut r = Rig::new();
        let _i = r.apply(1, &init(0));
        assert!(!r.apply(2, old).conflict);
    }

    #[test]
    fn a_tick_for_a_slot_already_applied_changes_nothing() {
        let mut r = Rig::new();
        assert!(!r.apply(1, &init(0)).conflict);
        assert!(
            !r.apply(2, &tick(1, 2000, Status::Ok, vec![w("k", "v")]))
                .conflict
        );
        assert_eq!(r.machine.state.component.head.timers["beat"], 2000);
        // The same slot again (a second leader's proposal), and an older one.
        let _gone = Arc::make_mut(&mut r.machine.state.component.kv).remove(b"k".as_slice());
        assert!(
            r.apply(3, &tick(2, 2000, Status::Ok, vec![w("k", "v")]))
                .conflict
        );
        assert!(
            r.apply(4, &tick(3, 1000, Status::Ok, vec![w("k", "v")]))
                .conflict
        );
        assert!(
            r.machine.view().get(b"k").is_none(),
            "an applied slot ran again"
        );
        assert_eq!(r.machine.state.component.head.timers["beat"], 2000);
    }

    #[test]
    fn a_failed_tick_counts_as_applied_and_a_conflicted_one_does_not() {
        let mut r = Rig::new();
        let _i = r.apply(1, &init(0));
        let failed = r.apply(2, &tick(1, 1000, Status::AppError, Vec::new()));
        assert_eq!(failed.status, Status::AppError);
        assert_eq!(
            r.machine.state.component.head.timers["beat"], 1000,
            "a failed tick counts as applied"
        );
        // Built on a state that is no longer current: not applied, the slot stays due.
        assert!(r.apply(3, &tick(1, 2000, Status::Ok, Vec::new())).conflict);
        assert_eq!(r.machine.state.component.head.timers["beat"], 1000);
        #[cfg(feature = "testing")]
        {
            let kept: Vec<_> = r
                .machine
                .ticks
                .iter()
                .map(|t| (t.timer.as_str(), t.scheduled_ms, t.applied.status.clone()))
                .collect();
            assert_eq!(kept, vec![("beat", 1000, Status::AppError)]);
        }
    }

    #[test]
    fn the_highest_id_is_kept_even_from_a_conflicted_entry() {
        let mut r = Rig::new();
        let _i = r.apply(1, &init(0));
        let mut cmd: ComponentCmd = serde_json::from_slice(&call(1, None, Vec::new())).unwrap();
        if let ComponentCmd::Call(c) = &mut cmd {
            c.id_term = 5;
            c.id_seq = 9;
            c.base_index = 7; // not the app's last entry: a conflict
        }
        assert!(r.apply(2, &serde_json::to_vec(&cmd).unwrap()).conflict);
        assert_eq!(r.machine.state.component.head.last_id, (5, 9));
        let _a = r.apply(3, &call(2, None, Vec::new()));
        assert_eq!(
            r.machine.state.component.head.last_id,
            (5, 9),
            "a lower id does not lower it"
        );
    }

    #[test]
    fn a_view_keeps_its_version_while_apply_moves_on() {
        let mut r = Rig::new();
        let _i = r.apply(1, &init(0));
        let _a = r.apply(2, &call(1, None, vec![w("k", "1")]));
        let held = r.machine.view();
        let _a = r.apply(3, &call(2, None, vec![w("k", "2")]));
        assert_eq!(held.get(b"k"), Some(b"1".to_vec()));
        assert_eq!(r.machine.view().get(b"k"), Some(b"2".to_vec()));
    }

    fn with_ts(raw: &[u8], ts_ms: u64) -> Vec<u8> {
        let mut cmd: ComponentCmd = serde_json::from_slice(raw).unwrap();
        if let ComponentCmd::Call(c) = &mut cmd {
            c.ts_ms = ts_ms;
        }
        serde_json::to_vec(&cmd).unwrap()
    }

    /// A keyed call at `ts_ms` on base `base`, as account `a`.
    fn call_at(base: u64, key: &str, ts_ms: u64) -> Vec<u8> {
        with_ts(&call(base, Some(key), Vec::new()), ts_ms)
    }

    fn honoured(r: &Rig, key: &str) -> bool {
        r.machine
            .component_replay(
                &Caller::Account {
                    account: "a".into(),
                },
                key,
            )
            .is_some()
    }

    /// D74 on a component: a `node:` key is honoured for 10 minutes of entry time, a
    /// caller's own key for 24 hours, judged by the newest applied entry's timestamp
    /// (with the per-class floors at zero).
    #[test]
    fn idempotency_records_are_kept_by_time_class() {
        const MIN: u64 = 60 * 1000;
        const HOUR: u64 = 60 * MIN;
        let mut r = Rig::new();
        r.machine.idem_limits.keep = [0, 0];
        let t0 = 1_000;
        let _i = r.apply(1, &init(0));
        let _a = r.apply(2, &call_at(1, "mine", t0));
        let _a = r.apply(3, &call_at(2, "node:c-retry", t0));
        assert!(honoured(&r, "mine") && honoured(&r, "node:c-retry"));
        // Nine minutes on: both still honoured.
        let _a = r.apply(4, &call_at(3, "x1", t0 + 9 * MIN));
        assert!(honoured(&r, "node:c-retry"));
        // Eleven minutes on, by an entry without a key: the node key is gone.
        let _a = r.apply(5, &with_ts(&call(4, None, Vec::new()), t0 + 11 * MIN));
        assert!(
            !honoured(&r, "node:c-retry"),
            "a node key outlived 10 minutes"
        );
        assert!(honoured(&r, "mine"));
        // 23 hours on: the caller's key is still honoured; past 24 hours, gone.
        let _a = r.apply(6, &call_at(5, "x2", t0 + 23 * HOUR));
        assert!(honoured(&r, "mine"), "a caller key dropped before 24 hours");
        let _a = r.apply(7, &call_at(6, "x3", t0 + 24 * HOUR + 1));
        assert!(!honoured(&r, "mine"), "a caller key outlived 24 hours");
        assert!(honoured(&r, "x3"));
    }

    /// D74's cap (scaled down here to 3 records through `Limits::cap`; the real one is
    /// 100,000), spec v0.27: past it `node:` records go first, then the oldest
    /// caller record.
    #[test]
    fn the_idempotency_cap_drops_node_keys_first_then_the_oldest() {
        let mut r = Rig::new();
        r.machine.idem_limits = crate::idempotency::Limits {
            keep: [0, 0],
            cap: 3,
            ..crate::idempotency::Limits::default()
        };
        let _i = r.apply(1, &init(0));
        for (n, key) in ["k1", "node:k2", "k3", "node:k4", "k5"].iter().enumerate() {
            let index = n as u64 + 2;
            let _a = r.apply(index, &call_at(index - 1, key, 1_000));
        }
        let kept: Vec<&str> = ["k1", "node:k2", "k3", "node:k4", "k5"]
            .into_iter()
            .filter(|k| honoured(&r, k))
            .collect();
        assert_eq!(kept, vec!["k1", "k3", "k5"]);
        // No `node:` record left: the oldest caller record goes.
        let _a = r.apply(7, &call_at(6, "k6", 1_000));
        let kept: Vec<&str> = ["k1", "k3", "k5", "k6"]
            .into_iter()
            .filter(|k| honoured(&r, k))
            .collect();
        assert_eq!(kept, vec!["k3", "k5", "k6"]);
    }

    /// A rolling upgrade lowered the cap while an app held more records than the new
    /// one: the first entries under the new cap drop the excess a bounded number per
    /// entry (scaled down: 60 records kept under the old limits, then a cap of 10 and a
    /// step of 7), never more per entry, and the rest carries over until the app is
    /// under the cap. The newest stay.
    #[test]
    fn records_over_the_new_cap_drain_a_bounded_number_per_entry() {
        let mut r = Rig::new();
        r.machine.idem_limits = crate::idempotency::Limits {
            keep: [0, 0],
            cap: 1_000_000,
            step: u64::MAX,
        };
        let _i = r.apply(1, &init(0));
        for n in 0..60u64 {
            let index = n + 2;
            let _a = r.apply(index, &call_at(index - 1, &format!("k{n}"), 1_000 + n));
        }
        let held = |r: &Rig| {
            let rings = r.machine.state.component.head.idempotency;
            rings.iter().map(|g| g.next - g.first).sum::<u64>()
        };
        assert_eq!(held(&r), 60);
        // The new release: a cap of 10, at most 7 dropped per entry.
        r.machine.idem_limits = crate::idempotency::Limits {
            keep: [0, 0],
            cap: 10,
            step: 7,
        };
        let mut index = 62;
        let mut entries = 0;
        while held(&r) > 10 && entries < 50 {
            let before = held(&r);
            let _a = r.apply(index, &with_ts(&call(index - 1, None, Vec::new()), 2_000));
            assert!(
                before - held(&r) <= 7,
                "one entry dropped {} records",
                before - held(&r)
            );
            index += 1;
            entries += 1;
        }
        assert_eq!(held(&r), 10, "the excess never drained");
        assert!(entries >= 7, "drained in {entries} entries");
        assert!(honoured(&r, "k59"));
        assert!(!honoured(&r, "k0"));
    }

    /// D74 (v0.27): a burst of keyless calls (`node:` keys) past the cap never
    /// evicts a caller's own record, even when the caller class is above its floor:
    /// `node:` records go first (scaled: cap 6, floors 2 caller / 1 node, 4 caller
    /// records, then 20 `node:` keys).
    #[test]
    fn a_burst_of_node_keys_does_not_evict_callers_records_above_their_floor() {
        let mut r = Rig::new();
        r.machine.idem_limits = crate::idempotency::Limits {
            keep: [2, 1],
            cap: 6,
            ..crate::idempotency::Limits::default()
        };
        let _i = r.apply(1, &init(0));
        let mine = ["c0", "c1", "c2", "c3"];
        for (n, key) in mine.iter().enumerate() {
            let index = n as u64 + 2;
            let _a = r.apply(index, &call_at(index - 1, key, 1_000 + n as u64));
        }
        for n in 0..20u64 {
            let index = n + 6;
            let _a = r.apply(
                index,
                &call_at(index - 1, &format!("node:c-{n}"), 2_000 + n),
            );
        }
        for key in mine {
            assert!(
                honoured(&r, key),
                "{key}: a caller's record was evicted by node: keys"
            );
        }
    }

    /// D74 (v0.26): the cap never takes a class below its floor. A burst of keyless
    /// calls (`node:` keys) past the cap (scaled to 5, floors to 2 caller / 1 node)
    /// leaves a caller's own record, inside its 24 h, to answer its retry.
    #[test]
    fn a_burst_of_node_keys_does_not_evict_a_callers_record_below_its_floor() {
        let mut r = Rig::new();
        r.machine.idem_limits = crate::idempotency::Limits {
            keep: [2, 1],
            cap: 5,
            ..crate::idempotency::Limits::default()
        };
        let _i = r.apply(1, &init(0));
        let _a = r.apply(2, &call_at(1, "mine", 1_000));
        for n in 0..20u64 {
            let index = n + 3;
            let _a = r.apply(
                index,
                &call_at(index - 1, &format!("node:c-{n}"), 1_000 + n),
            );
        }
        assert!(
            honoured(&r, "mine"),
            "a caller's record below its floor was evicted"
        );
        let rings = r.machine.state.component.head.idempotency;
        assert!(rings[0].next - rings[0].first >= 1 && rings[1].next - rings[1].first >= 1);
    }

    /// D74 (v0.26): a record expires against the earlier of the newest applied entry's
    /// timestamp and the applying entry's own, so one entry stamped 25 h ahead by a
    /// skewed leader can't expire what is written after it.
    #[test]
    fn one_skewed_entry_does_not_expire_the_records_written_after_it() {
        const MIN: u64 = 60 * 1000;
        const HOUR: u64 = 60 * MIN;
        let mut r = Rig::new();
        r.machine.idem_limits.keep = [0, 0];
        let t0 = 1_000_000;
        let _i = r.apply(1, &init(0));
        // The skewed entry carries no key: nothing of its own sits in the rings.
        let _a = r.apply(2, &with_ts(&call(1, None, Vec::new()), t0 + 25 * HOUR));
        let _a = r.apply(3, &call_at(2, "after", t0 + MIN));
        let _a = r.apply(4, &call_at(3, "later", t0 + 2 * MIN));
        let _a = r.apply(5, &call_at(4, "x", t0 + 3 * MIN));
        assert!(
            honoured(&r, "after"),
            "a record written after the skew expired"
        );
        assert!(honoured(&r, "later"));
    }

    /// D74 (v0.26): a key's class is decided by its prefix. A caller's own key that
    /// merely contains `node:` (`sync-node:42`) keeps its 24 hours.
    #[test]
    fn a_callers_key_containing_node_is_kept_24_hours() {
        const MIN: u64 = 60 * 1000;
        let mut r = Rig::new();
        r.machine.idem_limits.keep = [0, 0];
        let _i = r.apply(1, &init(0));
        let _a = r.apply(2, &call_at(1, "sync-node:42", 1_000));
        let _a = r.apply(3, &call_at(2, "x", 1_000 + 11 * MIN));
        assert!(
            honoured(&r, "sync-node:42"),
            "a caller's key got the node class"
        );
    }

    /// Retention is a function of the applied entries alone: two replicas fed the
    /// same entries keep the same records (no node clock is read).
    #[test]
    fn replicas_drop_the_same_records() {
        const MIN: u64 = 60 * 1000;
        let feed = |r: &mut Rig| {
            r.machine.idem_limits = crate::idempotency::Limits {
                keep: [0, 0],
                cap: 4,
                ..crate::idempotency::Limits::default()
            };
            let _i = r.apply(1, &init(0));
            let keys = ["a", "node:b", "c", "node:d", "e", "f"];
            for (n, key) in keys.iter().enumerate() {
                let index = n as u64 + 2;
                let _a = r.apply(index, &call_at(index - 1, key, 1_000 + n as u64 * 4 * MIN));
            }
            keys.iter()
                .map(|k| (*k, honoured(r, k)))
                .collect::<Vec<_>>()
        };
        let (mut one, mut two) = (Rig::new(), Rig::new());
        let a = feed(&mut one);
        std::thread::sleep(std::time::Duration::from_millis(20));
        let b = feed(&mut two);
        assert_eq!(a, b);
        assert_eq!(
            one.machine.state.component.head.idempotency,
            two.machine.state.component.head.idempotency
        );
        assert!(a.iter().any(|(_, kept)| !kept), "nothing expired: {a:?}");
    }

    #[test]
    fn the_storage_and_host_records_come_back_from_disk() {
        let mut r = Rig::new();
        let _i = r.apply(1, &init(0));
        let _a = r.apply(2, &call(1, Some("k"), vec![w("a", "1"), w("b", "2")]));
        let _t = r.apply(3, &tick(2, 5000, Status::Ok, Vec::new()));
        // A replica that starts again (or installed a snapshot) reads its rows.
        let mut fresh = AppMachine::new("app".into());
        calimero_log::Machine::attach(&mut fresh, r.engine.group_state(5));
        calimero_log::Machine::restore(&mut fresh, &[]).unwrap();
        assert_eq!(
            fresh.state.executor,
            Some(crate::types::Executor::Component)
        );
        assert_eq!(fresh.state.component.head, r.machine.state.component.head);
        assert_eq!(*fresh.state.component.kv, *r.machine.state.component.kv);
        assert!(fresh
            .component_replay(
                &Caller::Account {
                    account: "a".into()
                },
                "k"
            )
            .is_some());
    }

    #[test]
    fn the_compute_component_changes_with_the_bundle_in_one_entry() {
        let upgrade = |bundle: &str, compute: Option<&str>, base: u64| {
            serde_json::to_vec(&ComponentCmd::Upgrade {
                bundle: bundle.into(),
                compute_bundle: compute.map(str::to_owned),
                base_index: base,
            })
            .unwrap()
        };
        let mut rig = Rig::new();
        let init = serde_json::to_vec(&ComponentCmd::Init {
            bundle: "b1".into(),
            compute_bundle: Some("c1".into()),
            base_index: 0,
            ts_ms: 1,
            id_term: 1,
            id_seq: 1,
            outcome: Box::default(),
        })
        .unwrap();
        let _ = rig.apply(1, &init);
        let pair = |rig: &Rig| {
            let s = &rig.machine.state;
            (s.bundle.clone(), s.component.head.compute_bundle.clone())
        };
        assert_eq!(pair(&rig), (Some("b1".into()), Some("c1".into())));
        let _ = rig.apply(2, &upgrade("b2", None, 1));
        assert_eq!(pair(&rig), (Some("b2".into()), None));
        let _ = rig.apply(3, &upgrade("b3", Some("c3"), 2));
        assert_eq!(pair(&rig), (Some("b3".into()), Some("c3".into())));
        // An entry logged before the field existed names no compute component.
        let old = json!({ "op": "componentUpgrade", "bundle": "b4", "base_index": 3 });
        let _ = rig.apply(4, &serde_json::to_vec(&old).unwrap());
        assert_eq!(pair(&rig), (Some("b4".into()), None));
    }

    #[test]
    fn a_wasmer_entry_never_applies_to_a_component_app() {
        let mut r = Rig::new();
        let _i = r.apply(1, &init(0));
        // As merod before this release logged an SDK 0.12 app's blob removal.
        let v1 = serde_json::to_vec(&json!({ "op": "blobForget", "id": "00" })).unwrap();
        let out = r.apply(2, &v1);
        assert!(out.conflict, "a wasmer entry applied to a component app");
    }

    #[test]
    fn only_a_component_init_decides_the_executor() {
        assert!(ComponentCmd::is_init(&init(0)));
        assert!(!ComponentCmd::is_init(&call(0, None, Vec::new())));
        // As merod before this release logged an SDK 0.12 app's `init`.
        let v1 = serde_json::to_vec(&json!({
            "op": "init", "bundle": "b", "baseIndex": 0, "outcome": { "status": "ok" },
        }))
        .unwrap();
        assert!(!ComponentCmd::is_init(&v1));
    }
}
