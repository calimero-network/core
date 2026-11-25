//! An app group: the app's state, its recent events and, for an app the node
//! no longer runs, the state it left.
//!
//! An app is a `calimero:app` component on wasmtime ([`crate::component`]): the
//! leader runs a call against the applied state and appends the result as one
//! entry; every replica applies the result, and none re-executes.
//!
//! An app created before this release may be an SDK 0.12 core module, which ran
//! on wasmer ([`crate::types::Executor::Wasmer`]). This node has no wasmer: such
//! an app's state still opens, it is listed and it can be deleted, but it runs
//! nothing (`410 UNSUPPORTED_ENGINE`), and an entry of its format applies as a
//! no-op on every replica alike.
//!
//! Ids in these comments cite `docs/specs/app-runtime-v0.14.md` (D decisions in section 2, R
//! requirements in section 8): D17 (actions are durable, logged when started and when done),
//! D25 (xcalls are deduplicated by a per-source watermark), D29 (a stuck xcall is retried for
//! up to an hour, then dead-lettered), R8 (the blob API and its lifecycle).

use std::collections::{BTreeMap, VecDeque};

use calimero_log::{Bytes, GroupState, Machine, StateTx};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::types::{AppId, EventRecord};

/// Idempotency retention's classes and floors (`crate::idempotency`).
pub use crate::idempotency::{IDEMPOTENCY_MIN_KEEP, IDEMPOTENCY_TTL_MS, NODE_KEY_PREFIX};

/// What a node without wasmer answers for an SDK 0.12 app: every call, view and
/// upgrade of one ([`crate::types::Executor::Wasmer`]).
pub const WASMER_GONE: &str = "this app is an SDK 0.12 (wasmer) app, which this node no longer runs; rebuild it with calimero-sdk and create it again";

/// Events kept for resuming subscriptions.
pub const EVENT_RING: usize = 10_000;

/// One storage write.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Write {
    pub key: Bytes,
    /// `None` deletes the key.
    pub value: Option<Bytes>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    #[default]
    Ok,
    /// The app refused the call; `error` holds its error value.
    AppError,
    /// The call trapped (out of gas, panic, unknown method).
    Trap,
}

/// What `apply` answers for a call.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub index: u64,
    pub status: Status,
    pub result: Value,
    pub error: Value,
    pub events: Vec<(String, Value)>,
    /// The entry was built on a state that was no longer current; nothing changed.
    #[serde(default)]
    pub conflict: bool,
}

/// Where an app's rows live in its group's state.
///
/// The app's own keys and the idempotency table are hidden rows (the key on
/// disk is an HMAC; see `calimero_log::store::hide`); the rest are the node's
/// bookkeeping.
pub(crate) mod rows {
    /// An SDK 0.12 app's storage (wasmer apps are gone: nothing writes it).
    #[cfg(test)]
    pub const KV: u8 = b'k';
    /// The [`super::Head`].
    pub const HEAD: &[u8] = b"h";
    /// Event `n`: `e ‖ n`.
    pub const EVENT: u8 = b'e';

    /// A component's storage (`crate::component`): its stored keys.
    pub const CKV: u8 = b'c';
    /// A component's compute runs: one record per run, keyed by run id.
    pub const CRUN: u8 = b'u';
    /// A component's pipeline runs: one record per run, keyed by pipeline run id.
    pub const CPIPE: u8 = b'p';
    /// A component's pipeline step runs: one row per run, keyed `<pipeline run>/<step>/<index>`.
    pub const CITEM: u8 = b'y';
    /// A component's step run proofs: one per step run that ran, keyed by run id.
    pub const CPROOF: u8 = b'v';
    /// The attestation of each job node a pipeline run's steps ran on: one per pipeline run and
    /// job key, keyed [`cjob`].
    pub const CJOB: u8 = b'w';
    /// What each step run's job report signs beyond its proof, keyed by run id. Kept apart from
    /// the proof so a release before this reads its proof rows unchanged.
    pub const CREPORT: u8 = b'g';
    /// A component's pipeline step cache: one entry per cache key.
    pub const CCACHE: u8 = b'x';
    /// A component's idempotency records, by `caller|key`.
    pub const CIDEM: u8 = b'j';
    /// A component's idempotency keys per class in the order they were recorded:
    /// `q ‖ class ‖ n`, for retention (`crate::idempotency`).
    pub const CORDER: u8 = b'q';
    /// A component's blobs (R8, `crate::component::blobs`): its record of each, by SHA-256.
    pub const CBLOB: u8 = b'b';
    /// A component's xcall outbox (D25, `crate::component::xcall`): `o ‖ seq`.
    pub const COUT: u8 = b'o';
    /// A component's dead letters (D29): `d ‖ n`.
    pub const CDEAD: u8 = b'd';
    /// A component's pending actions (D17, `crate::component::action`): `a ‖ id`.
    pub const CACT: u8 = b'a';
    /// A component's own secret values (`crate::component::InstanceSecrets`), one row:
    /// `s ‖ all`. Never in the app's storage: no run reads it, only the node's outbound gate.
    pub const CSECRET: u8 = b's';

    pub fn cjob(pipeline_run: &str, job_key: &str) -> String {
        format!("{pipeline_run}/{job_key}")
    }

    pub fn corder(class: u8, n: u64) -> Vec<u8> {
        let mut k = vec![CORDER, class];
        k.extend_from_slice(&n.to_be_bytes());
        k
    }

    pub fn event(n: u64) -> Vec<u8> {
        let mut k = vec![EVENT];
        k.extend_from_slice(&n.to_be_bytes());
        k
    }
}

/// An app's storage, read from its group's durable state.
#[derive(Clone, Default)]
pub struct KvView {
    state: Option<GroupState>,
}

impl KvView {
    /// The group's state these rows are read from (`None` before `attach`).
    #[must_use]
    pub(crate) const fn group(&self) -> Option<&GroupState> {
        self.state.as_ref()
    }
}

use crate::idempotency::Ring;

/// What an app's state machine keeps besides its storage: small, rewritten
/// with every entry.
///
/// `pending`, `timer_ticks` and `expiry` are an SDK 0.12 app's (its follow-ups,
/// timer ticks and idempotency rings): carried as they were so its state still
/// reads and writes the same, never acted on.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Head {
    bundle: Option<String>,
    initialized: bool,
    #[serde(default)]
    init_error: Option<Value>,
    last_index: u64,
    #[serde(default)]
    pending: BTreeMap<u64, Value>,
    #[serde(default)]
    next_pending: u64,
    #[serde(default)]
    timer_ticks: BTreeMap<String, u64>,
    #[serde(default)]
    events_first: u64,
    #[serde(default)]
    events_next: u64,
    #[serde(default)]
    expiry: [Ring; 2],
    #[serde(default)]
    newest_ts_ms: u64,
    /// The executor the app's `init` entry chose. Absent in a state written
    /// before components existed: wasmer ([`AppState::executor`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    executor: Option<crate::types::Executor>,
    /// A component's host records; absent for an SDK 0.12 app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    component: Option<crate::component::ComponentHead>,
}

/// The bundle and compute component a state's head (its [`rows::HEAD`] row)
/// runs: what a backup of the state is restored onto (`crate::node::restore`).
pub(crate) fn backup_code(head: &[u8]) -> eyre::Result<(Option<String>, Option<String>)> {
    let head: Head = serde_json::from_slice(head)?;
    Ok((head.bundle, head.component.and_then(|c| c.compute_bundle)))
}

#[derive(Clone, Default)]
pub struct AppState {
    /// The app's group state, on disk.
    pub kv: KvView,
    pub bundle: Option<String>,
    pub initialized: bool,
    /// Why `init` failed, if it did (`{ status, error }`).
    pub init_error: Option<Value>,
    pub last_index: u64,
    events_first: u64,
    events_next: u64,
    /// An SDK 0.12 app's rows of [`Head`], kept as they were.
    legacy: Legacy,
    /// The executor the `init` entry recorded; see [`Self::executor`].
    pub executor: Option<crate::types::Executor>,
    /// A component's host records and its storage in memory.
    pub component: crate::component::ComponentState,
}

/// The parts of [`Head`] only an SDK 0.12 app wrote.
#[derive(Clone, Debug, Default)]
struct Legacy {
    pending: BTreeMap<u64, Value>,
    next_pending: u64,
    timer_ticks: BTreeMap<String, u64>,
    expiry: [Ring; 2],
    newest_ts_ms: u64,
}

impl AppState {
    /// The executor the app's entries are applied with: the one its `init`
    /// recorded; wasmer for an app initialized before components existed;
    /// `None` before `init` applied.
    #[must_use]
    pub fn executor(&self) -> Option<crate::types::Executor> {
        self.executor.or_else(|| {
            (self.bundle.is_some() || self.initialized).then_some(crate::types::Executor::Wasmer)
        })
    }

    fn head(&self) -> Head {
        Head {
            bundle: self.bundle.clone(),
            initialized: self.initialized,
            init_error: self.init_error.clone(),
            last_index: self.last_index,
            pending: self.legacy.pending.clone(),
            next_pending: self.legacy.next_pending,
            timer_ticks: self.legacy.timer_ticks.clone(),
            events_first: self.events_first,
            events_next: self.events_next,
            expiry: self.legacy.expiry,
            newest_ts_ms: self.legacy.newest_ts_ms,
            executor: self.executor,
            component: (self.executor == Some(crate::types::Executor::Component))
                .then(|| self.component.head.clone()),
        }
    }

    fn set_head(&mut self, h: Head) {
        self.bundle = h.bundle;
        self.initialized = h.initialized;
        self.init_error = h.init_error;
        self.last_index = h.last_index;
        self.legacy = Legacy {
            pending: h.pending,
            next_pending: h.next_pending,
            timer_ticks: h.timer_ticks,
            expiry: h.expiry,
            newest_ts_ms: h.newest_ts_ms,
        };
        self.events_first = h.events_first;
        self.events_next = h.events_next;
        self.executor = h.executor;
        self.component.head = h.component.unwrap_or_default();
    }
}

/// Entities touched per entry, kept in memory for search to catch up from.
pub const TOUCHED_RING: usize = 4096;

pub struct AppMachine {
    pub app_id: AppId,
    pub state: AppState,
    pub events_tx: broadcast::Sender<EventRecord>,
    /// `(index, record keys)` of recent entries that touched any; empty after a
    /// restart or a snapshot, which makes search rebuild instead.
    pub touched: VecDeque<(u64, Vec<String>)>,
    /// The first index `touched` is complete from.
    pub touched_since: u64,
    /// A component's last applied ticks with their outcome, for tests (feature
    /// `testing`: the Rooms cluster runner checks a tick step's `expect`). Not
    /// part of the state, and not in a production build.
    #[cfg(feature = "testing")]
    pub ticks: crate::component::Ticks,
    /// Idempotency floors and cap (`crate::idempotency`'s; tests scale them down).
    pub(crate) idem_limits: crate::idempotency::Limits,
}

impl AppMachine {
    #[must_use]
    pub fn new(app_id: AppId) -> Self {
        let (events_tx, _) = broadcast::channel(1024);
        Self {
            app_id,
            state: AppState::default(),
            events_tx,
            touched: VecDeque::new(),
            touched_since: u64::MAX,
            #[cfg(feature = "testing")]
            ticks: crate::component::Ticks::new(),
            idem_limits: crate::idempotency::Limits::default(),
        }
    }

    /// Append `rec` to the event ring, dropping the oldest past [`EVENT_RING`],
    /// and hand it to live subscribers.
    pub(crate) fn push_event(&mut self, tx: &mut StateTx<'_>, rec: EventRecord) {
        match serde_json::to_vec(&rec) {
            Ok(v) => tx.put(rows::event(self.state.events_next), v),
            Err(e) => tx.fail(e),
        }
        self.state.events_next += 1;
        while self.state.events_next - self.state.events_first > EVENT_RING as u64 {
            tx.delete(rows::event(self.state.events_first));
            self.state.events_first += 1;
        }
        let _receivers = self.events_tx.send(rec);
    }

    /// The events kept for resuming subscriptions, from entry `from` on.
    #[must_use]
    pub fn events_from(&self, from: u64) -> Vec<EventRecord> {
        let mut out = Vec::new();
        let Some(state) = &self.state.kv.state else {
            return out;
        };
        let res = state.scan(&rows::event(self.state.events_first), |k, v| {
            if k.first() != Some(&rows::EVENT) {
                return Ok(false);
            }
            let rec: EventRecord = serde_json::from_slice(v)?;
            if rec.index >= from {
                out.push(rec);
            }
            Ok(true)
        });
        if let Err(e) = res {
            // An event row's decode error quotes the app's event.
            let named = crate::log_safe::kind(e.as_ref());
            tracing::error!(app = %self.app_id, error_kind = %named, "reading events failed");
        }
        out
    }

    /// Keep what entry `index` touched for search to catch up from (`TOUCHED_RING`
    /// entries): a component's record keys (hex).
    pub(crate) fn note_touched(&mut self, index: u64, touched: Vec<String>) {
        if touched.is_empty() {
            if self.touched_since == u64::MAX {
                self.touched_since = index;
            }
            return;
        }
        if self.touched_since == u64::MAX {
            self.touched_since = index;
        }
        self.touched.push_back((index, touched));
        if self.touched.len() > TOUCHED_RING {
            if let Some((i, _)) = self.touched.pop_front() {
                self.touched_since = i + 1;
            }
        }
    }

    /// The record keys touched by entries after `index`, or `None` when this
    /// replica does not know them all (search then rebuilds).
    #[must_use]
    pub fn touched_after(&self, index: u64) -> Option<Vec<String>> {
        if index + 1 < self.touched_since {
            return None;
        }
        let mut out = Vec::new();
        for (i, ids) in &self.touched {
            if *i > index {
                out.extend(ids.iter().cloned());
            }
        }
        Some(out)
    }

    /// Whether `id` is a blob of this app (a sealed file included). An SDK 0.12
    /// app holds none: it runs nothing that could open one.
    #[must_use]
    pub fn holds_blob(&self, id: &[u8; 32]) -> bool {
        self.is_component() && self.state.component.blobs.records.contains_key(id)
    }

    /// Whether a sealed file of this app lists the segment with hex hash `hash`.
    #[must_use]
    pub fn holds_segment(&self, hash: &str) -> bool {
        self.is_component() && self.state.component.blobs.holds_segment(hash)
    }

    /// The account that uploaded the sealed file `id`; none for a file a call or a run made.
    #[must_use]
    pub fn file_uploader(&self, id: &[u8; 32]) -> Option<String> {
        let record = self.state.component.blobs.records.get(id)?;
        record.file.as_ref()?.uploader.clone()
    }

    /// A component app's blob `id` as a download judges it: whether it is a published
    /// output (served only through a grant) and its revocation count; `None` if not held.
    #[must_use]
    pub fn blob_standing(&self, id: &[u8; 32]) -> Option<(bool, u32)> {
        let record = self.state.component.blobs.records.get(id)?;
        self.is_component()
            .then_some((record.charged, record.epoch))
    }

    fn is_component(&self) -> bool {
        self.state.executor() == Some(crate::types::Executor::Component)
    }

    /// Whether `id` is one of a component app's sealed files, which only its code reads.
    #[must_use]
    pub fn is_sealed_file(&self, id: &[u8; 32]) -> bool {
        self.state
            .component
            .blobs
            .records
            .get(id)
            .is_some_and(|r| r.file.is_some())
    }

    /// A sealed file's plaintext size and seal; `None` for anything else.
    #[must_use]
    pub fn file_record(&self, id: &[u8; 32]) -> Option<(u64, crate::component::blobs::FileSeal)> {
        let record = self.state.component.blobs.records.get(id)?;
        Some((record.size, record.file.clone()?))
    }

    /// The app's blobs and what its storage holds besides. A component's storage
    /// is its own keyspace, in memory; its blobs are host records beside it (R8). An
    /// SDK 0.12 app's is not read: it is reported empty.
    #[must_use]
    pub fn storage_stats(&self) -> StorageStats {
        if !self.is_component() {
            return StorageStats::default();
        }
        let kv = &self.state.component.kv;
        let blobs = &self.state.component.blobs.records;
        StorageStats {
            keys: kv.len() as u64,
            bytes: kv.iter().map(|(k, v)| (k.len() + v.len()) as u64).sum(),
            blobs: blobs
                .iter()
                .filter(|(_, r)| r.file.is_none())
                .map(|(id, r)| (*id, r.size))
                .collect(),
            files: blobs
                .iter()
                .filter_map(|(id, r)| {
                    let seal = r.file.as_ref().filter(|f| !f.log)?;
                    let sha256 = <[u8; 32]>::try_from(seal.sha256.0.as_slice());
                    Some((*id, r.size, sha256.ok()?))
                })
                .collect(),
            blob_bytes: blobs
                .values()
                .map(|r| {
                    r.file
                        .as_ref()
                        .map_or(r.size, |_| crate::files::sealed_len(r.size))
                })
                .sum(),
        }
    }

    /// When the app last committed a write (Unix milliseconds, the entry's
    /// timestamp: [`crate::component::ComponentHead::last_write_ms`]); `None`
    /// before its first, and for an SDK 0.12 app.
    #[must_use]
    pub fn last_write_ms(&self) -> Option<u64> {
        let ms = self.state.component.head.last_write_ms;
        (self.is_component() && ms > 0).then_some(ms)
    }

    /// What applying an entry of an app this node does not run answers: nothing
    /// changes but the app's position in its log, the same on every replica.
    fn unsupported(index: u64) -> Vec<u8> {
        applied_bytes(
            index,
            &Applied {
                index,
                status: Status::AppError,
                result: Value::Null,
                error: Value::String(WASMER_GONE.into()),
                events: Vec::new(),
                conflict: false,
            },
        )
    }
}

/// An applied entry's answer (an [`Applied`] or a component's) as bytes, never empty.
///
/// Both hold JSON values and plain fields, so they serialize; were one ever not to, its
/// caller is answered a fault that says why, logged here with the entry's index, rather
/// than empty bytes it reads as no answer at all, far from the cause.
pub(crate) fn applied_bytes<T: Serialize>(index: u64, answer: &T) -> Vec<u8> {
    serde_json::to_vec(answer).unwrap_or_else(|e| {
        tracing::error!(index, error = %e, "an applied entry's answer does not serialize: answered as a fault");
        // `Value`'s `Display` cannot fail.
        json!({
            "index": index,
            "status": "trap",
            "result": null,
            "error": { "kind": "Fault", "detail": format!("the answer does not serialize: {e}") },
            "events": [],
            "conflict": false,
        })
        .to_string()
        .into_bytes()
    })
}

impl Machine for AppMachine {
    fn attach(&mut self, state: GroupState) {
        self.state.kv.state = Some(state);
    }

    fn apply(&mut self, index: u64, command: &[u8], tx: &mut StateTx<'_>) -> Vec<u8> {
        use crate::types::Executor as Kind;
        // Each entry is applied with the executor the app's state names; before
        // `init`, the entry itself names it. An SDK 0.12 app's entries (its
        // state names wasmer, or, before its `init` applied, the entry is not a
        // component's) change nothing.
        let component = match self.state.executor() {
            Some(Kind::Component) => true,
            Some(Kind::Wasmer) => false,
            None => crate::component::ComponentCmd::is_component(command),
        };
        let out = if component {
            self.apply_component(tx, index, command)
        } else {
            Self::unsupported(index)
        };
        if tx.failed() {
            // The group stops on this entry (`StateTx::fail`): it never applied.
            return out;
        }
        self.state.last_index = index;
        match serde_json::to_vec(&self.state.head()) {
            Ok(h) => tx.put(rows::HEAD.to_vec(), h),
            Err(e) => tx.fail(e),
        }
        out
    }

    fn restore(&mut self, _data: &[u8]) -> eyre::Result<()> {
        // A head that cannot be read is an error, never a fresh app's head.
        let raw = match self.state.kv.state.as_ref() {
            Some(s) => s.try_get(rows::HEAD)?,
            None => None,
        };
        let head = match raw {
            Some(raw) => serde_json::from_slice(&raw)?,
            None => Head::default(),
        };
        self.state.set_head(head);
        // A component's storage, in memory again from its rows.
        self.state.component.kv = crate::component::Kv::default();
        self.state.component.blobs = crate::component::blobs::BlobTable::default();
        if self.is_component() {
            if let Some(state) = self.state.kv.state.clone() {
                self.state.component.load(&state)?;
            }
        }
        // What search knows to catch up from is gone with the old state.
        self.touched.clear();
        self.touched_since = u64::MAX;
        Ok(())
    }
}

/// What an app's storage holds.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStats {
    /// Keys the app wrote, private spaces and host rows included, but no blob marks.
    pub keys: u64,
    /// Their keys' and values' bytes, as the app sees them (before sealing).
    pub bytes: u64,
    #[serde(skip)]
    pub blobs: Vec<([u8; 32], u64)>,
    /// A component's sealed files: id, plaintext size and plaintext SHA-256.
    #[serde(skip)]
    pub files: Vec<([u8; 32], u64, [u8; 32])>,
    pub blob_bytes: u64,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    struct Nowhere;

    impl calimero_log::Transport for Nowhere {
        fn send(
            &self,
            _t: calimero_log::NodeId,
            _a: String,
            _g: calimero_log::GroupId,
            _r: calimero_log::RpcRequest,
        ) -> calimero_log::BoxFuture<Result<calimero_log::RpcResponse, String>> {
            Box::pin(async { Err("no peers".into()) })
        }
    }

    /// An app machine over a fresh store, outside any running group.
    fn machine(dir: &tempfile::TempDir) -> (Arc<calimero_log::Engine>, AppMachine) {
        let engine = calimero_log::Engine::open(
            1,
            dir.path(),
            &[3; 32],
            Arc::new(Nowhere),
            calimero_log::Config::default(),
        )
        .unwrap();
        let mut m = AppMachine::new("app".into());
        m.attach(engine.group_state(9));
        m.restore(&[]).unwrap();
        (engine, m)
    }

    fn apply(m: &mut AppMachine, index: u64, cmd: &Value) -> Applied {
        let state = m.state.kv.state.clone().unwrap();
        let mut tx = StateTx::new(&state);
        let out = m.apply(index, &serde_json::to_vec(cmd).unwrap(), &mut tx);
        tx.commit().unwrap();
        serde_json::from_slice(&out).unwrap()
    }

    /// An SDK 0.12 app's entries, as merod before this release logged them: its
    /// `init`, a call writing a key and an event, an upgrade.
    fn wasmer_entries() -> Vec<Value> {
        let outcome = serde_json::json!({
            "status": "ok",
            "diff": [{ "key": [97], "value": [49] }],
            "events": [["set", "a"]],
        });
        vec![
            serde_json::json!({ "op": "init", "bundle": "abc", "baseIndex": 0, "outcome": outcome }),
            serde_json::json!({ "op": "call", "method": "set", "args": null,
                "caller": { "kind": "account", "account": "acct" }, "idempotencyKey": "k",
                "tsMs": 1000, "baseIndex": 1, "outcome": outcome }),
            serde_json::json!({ "op": "upgrade", "bundle": "def", "baseIndex": 2, "outcome": outcome }),
            serde_json::json!({ "op": "done", "seqs": [0] }),
        ]
    }

    /// A node without wasmer applies an SDK 0.12 app's entries (a replica catching up
    /// on a log an older merod wrote) as no-ops: nothing is written but the app's
    /// position, every replica answers the same, and none panics.
    #[test]
    fn an_sdk_0_12_apps_entries_apply_as_no_ops() {
        let run = || {
            let dir = tempfile::tempdir().unwrap();
            let (engine, mut m) = machine(&dir);
            let answers: Vec<Applied> = wasmer_entries()
                .iter()
                .enumerate()
                .map(|(i, cmd)| apply(&mut m, i as u64 + 1, cmd))
                .collect();
            (engine, dir, m, answers)
        };
        let (_e1, _d1, m, answers) = run();
        for a in &answers {
            assert_eq!(a.status, Status::AppError);
            assert_eq!(a.error, Value::String(WASMER_GONE.into()));
            assert!(!a.conflict);
            assert!(a.events.is_empty());
        }
        assert_eq!(m.state.last_index, 4);
        assert_eq!(m.state.bundle, None, "no wasmer entry took effect");
        assert!(!m.state.initialized);
        assert!(m.events_from(0).is_empty());
        let mut keys = 0;
        m.state
            .kv
            .group()
            .unwrap()
            .scan_hidden(rows::KV, |_k, _v| {
                keys += 1;
                Ok(true)
            })
            .unwrap();
        assert_eq!(keys, 0, "the call's diff was not written");
        // A second replica fed the same entries answers alike.
        let (_e2, _d2, _m2, again) = run();
        assert_eq!(
            serde_json::to_value(&answers).unwrap(),
            serde_json::to_value(&again).unwrap()
        );
    }

    /// An entry that is no JSON at all applies the same way: never a panic.
    #[test]
    fn a_malformed_entry_applies_as_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let (_engine, mut m) = machine(&dir);
        let state = m.state.kv.state.clone().unwrap();
        let mut tx = StateTx::new(&state);
        let out = m.apply(1, b"\xff not json", &mut tx);
        tx.commit().unwrap();
        let a: Applied = serde_json::from_slice(&out).unwrap();
        assert_eq!(a.status, Status::AppError);
        assert_eq!(m.state.last_index, 1);
    }

    /// Regression: an answer that did not serialize was sent as empty bytes, with
    /// nothing logged. It is now a fault that says why, readable as either `Applied`.
    #[test]
    fn an_answer_that_does_not_serialize_is_a_fault_never_empty_bytes() {
        // A map whose keys are not strings is not JSON.
        let unserializable: std::collections::BTreeMap<Vec<u8>, u8> = [(vec![1], 1)].into();
        assert!(serde_json::to_vec(&unserializable).is_err());
        let bytes = applied_bytes(9, &unserializable);
        let a: Applied = serde_json::from_slice(&bytes).unwrap();
        assert_eq!((a.index, a.status), (9, Status::Trap));
        assert_eq!(a.error["kind"], "Fault");
        let c: crate::component::Applied = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(c.status, Status::Trap);
        assert!(!c.conflict && !c.timeout);

        let fine = Applied {
            index: 3,
            status: Status::Ok,
            result: json!({ "x": 1 }),
            error: Value::Null,
            events: Vec::new(),
            conflict: false,
        };
        assert_eq!(applied_bytes(3, &fine), serde_json::to_vec(&fine).unwrap());
    }
}
