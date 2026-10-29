//! An app group: the app's state, its idempotency table and recent events.
//!
//! The leader executes a call against the applied state and appends the
//! result as one entry: `{ call, outcome { result, diff, events } }`. Every
//! replica applies the diff; none re-executes.
//!
//! An app runs on one of two executors ([`crate::types::Executor`]), recorded by
//! its `init` entry: a v1 core module on wasmer (this module's [`AppCmd`] and
//! [`Executor`]), or a `calimero:app` component on wasmtime
//! ([`crate::component`]). Each executor's entries are a format of their own
//! and write a keyspace of their own; a replica applies an entry with the
//! executor the app's state names, and an entry of the other format changes
//! nothing.

use calimero_storage::shared_writers::CellWriters;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use calimero_log::{Bytes, GroupState, Machine, StateTx};
use calimero_runtime::store::{Key, Storage, Value as StoreValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::types::{now_ms, AppId, Caller, EventRecord};

/// Idempotency retention's classes and floors, shared by both executors
/// (`crate::idempotency`, D74).
pub use crate::idempotency::{IDEMPOTENCY_MIN_KEEP, IDEMPOTENCY_TTL_MS, NODE_KEY_PREFIX};
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

/// What executing a call produced.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub status: Status,
    #[serde(default)]
    pub result: Value,
    #[serde(default)]
    pub error: Value,
    #[serde(default)]
    pub diff: Vec<Write>,
    #[serde(default)]
    pub events: Vec<(String, Value)>,
    #[serde(default)]
    pub logs: Vec<String>,
    /// Runs this one asked for: its events' handlers and its `xcall`s. They
    /// are queued in the app's state when the entry applies, and the leader
    /// runs them after (see [`AppState::pending`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub followups: Vec<FollowUp>,
    /// The storage entities the run touched (hex), and every entry above
    /// them: what search re-extracts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub touched: Vec<String>,
    /// Blobs the run wrote, `(hex id, bytes)`. Never logged: the leader stores
    /// the bytes in the cluster's blob store, and `diff` marks them as the
    /// app's ([`blob_key`]).
    #[serde(skip)]
    pub new_blobs: Vec<(String, Vec<u8>)>,
}

impl Outcome {
    /// A run that did not happen, for `why`.
    #[must_use]
    pub fn trap(why: &str) -> Self {
        Self {
            status: Status::Trap,
            error: Value::String(why.to_owned()),
            ..Self::default()
        }
    }
}

/// A run another run asked for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FollowUp {
    /// An event's handler (`app::emit!((event, "handler"))`), with the event's
    /// data as its input. `tee` for a `tee:<method>` handler, which runs as the
    /// app's TEE authority.
    Handler {
        method: String,
        input: Bytes,
        #[serde(default)]
        tee: bool,
    },
    /// A call into another app (`env::xcall`), by its context id (hex).
    XCall {
        context: String,
        method: String,
        input: Bytes,
    },
}

/// A follow-up waiting for the leader to run it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    pub followup: FollowUp,
    /// The entry that asked for it.
    pub from_index: u64,
    /// Hops from the call a person made; see [`MAX_FOLLOWUP_DEPTH`].
    pub depth: u32,
}

/// How deep a cascade of handlers and xcalls may go:
/// a run at this depth still runs, but asks for nothing more.
pub const MAX_FOLLOWUP_DEPTH: u32 = 3;

/// Why the node ran a call itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Trigger {
    /// The pending follow-up `seq`, which the entry consumes.
    FollowUp { seq: u64 },
    /// The `tick`th period of an `#[app::tee(every = "..")]` method.
    Timer { method: String, tick: u64 },
    /// An `xcall` from another app.
    XCall { origin: String, depth: u32 },
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
    pub ts_ms: u64,
    /// The index of the app's last entry when the leader ran the call
    /// (`AppState::last_index`); `apply` refuses it on any other state.
    pub base_index: u64,
    pub outcome: Outcome,
    /// Set when the node ran it itself rather than for a request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<Trigger>,
}

impl CallEntry {
    /// Hops from the call a person made.
    #[must_use]
    pub fn depth(&self, pending: &BTreeMap<u64, Pending>) -> u32 {
        match &self.trigger {
            None | Some(Trigger::Timer { .. }) => 0,
            Some(Trigger::XCall { depth, .. }) => *depth,
            Some(Trigger::FollowUp { seq }) => pending.get(seq).map_or(0, |p| p.depth + 1),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum AppCmd {
    /// The first entry: run `init`.
    Init {
        bundle: String,
        base_index: u64,
        outcome: Outcome,
    },
    Call(Box<CallEntry>),
    /// Switch to a new bundle and run its migration.
    Upgrade {
        bundle: String,
        base_index: u64,
        outcome: Outcome,
    },
    /// An app no longer has blob `id` (hex): its runs cannot open it and the
    /// API answers 404. The bytes go once no other app marks them.
    BlobForget {
        id: String,
    },
    /// A blob uploaded for the app (its bytes are in the blob store already).
    Blob {
        id: String,
        size: u64,
    },
    /// Follow-ups the leader handed to other apps; they are done here.
    Done {
        seqs: Vec<u64>,
    },
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

#[derive(Clone, Debug, Serialize, Deserialize)]
struct IdemRecord {
    applied: Applied,
    expires_ms: u64,
}

/// Where a wasmer app's idempotency records live (D74's rule, `crate::idempotency`).
const IDEM_ROWS: crate::idempotency::Rows = crate::idempotency::Rows {
    tag: rows::IDEM,
    ring: rows::expiry,
};

/// Reads an app's state: the keys its runs see.
pub trait StateRead: Send + Sync {
    fn get(&self, key: &[u8]) -> Option<Vec<u8>>;

    /// An owned handle on the same state, for callbacks that outlive a borrow.
    fn handle(&self) -> Arc<dyn StateRead>;
}

impl StateRead for BTreeMap<Bytes, Bytes> {
    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        BTreeMap::get(self, &Bytes(key.to_vec())).map(|v| v.0.clone())
    }

    fn handle(&self) -> Arc<dyn StateRead> {
        Arc::new(self.clone())
    }
}

/// Where an app's rows live in its group's state.
///
/// The app's own keys and the idempotency table are hidden rows (the key on
/// disk is an HMAC; see `calimero_log::store::hide`); the rest are the node's
/// bookkeeping.
pub(crate) mod rows {
    /// The app's storage: `env::storage_*`, private storage, blobs, writers.
    pub const KV: u8 = b'k';
    /// Idempotency records, by `caller|key`.
    pub const IDEM: u8 = b'i';
    /// The [`super::Head`].
    pub const HEAD: &[u8] = b"h";
    /// Event `n`: `e ‖ n`.
    pub const EVENT: u8 = b'e';
    /// The idempotency keys in the order they expire, per TTL class: `r ‖ class ‖ n`.
    pub const EXPIRY: u8 = b'r';

    /// A component's storage (`crate::component`): its stored keys.
    pub const CKV: u8 = b'c';
    /// A component's files: one mark per file, keyed by blob id.
    pub const CFILE: u8 = b'f';
    /// A component's idempotency records, by `caller|key`.
    pub const CIDEM: u8 = b'j';
    /// A component's idempotency keys per class in the order they were recorded:
    /// `q ‖ class ‖ n` (D74).
    pub const CORDER: u8 = b'q';

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

    pub fn expiry(class: u8, n: u64) -> Vec<u8> {
        let mut k = vec![EXPIRY, class];
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

    /// Every key and value of the app's storage, in no particular order.
    pub fn for_each(&self, mut f: impl FnMut(&[u8], &[u8])) -> eyre::Result<()> {
        let Some(state) = &self.state else {
            return Ok(());
        };
        state.scan_hidden(rows::KV, |k, v| {
            f(k, v);
            Ok(true)
        })
    }
}

impl StateRead for KvView {
    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.state.as_ref()?.get_hidden(rows::KV, key)
    }

    fn handle(&self) -> Arc<dyn StateRead> {
        Arc::new(self.clone())
    }
}

use crate::idempotency::Ring;

/// What an app's state machine keeps besides its storage: small, rewritten
/// with every entry.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Head {
    bundle: Option<String>,
    initialized: bool,
    #[serde(default)]
    init_error: Option<Value>,
    last_index: u64,
    #[serde(default)]
    pending: BTreeMap<u64, Pending>,
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
    /// The newest timestamp of a call applied (D74: what idempotency records
    /// expire by). Absent in a state written before: 0, raised by the next call.
    #[serde(default)]
    newest_ts_ms: u64,
    /// The executor the app's `init` entry chose. Absent in a state written
    /// before components existed: wasmer ([`AppState::executor`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    executor: Option<crate::types::Executor>,
    /// A component's host records; absent for a wasmer app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    component: Option<crate::component::ComponentHead>,
}

#[derive(Clone, Default)]
pub struct AppState {
    /// The app's storage, on disk.
    pub kv: KvView,
    pub bundle: Option<String>,
    pub initialized: bool,
    /// Why `init` failed, if it did (`{ status, error }`).
    pub init_error: Option<Value>,
    pub last_index: u64,
    /// Follow-ups the leader has yet to run, by sequence number.
    pub pending: BTreeMap<u64, Pending>,
    pub next_pending: u64,
    /// The last tick fired per timer method.
    pub timer_ticks: BTreeMap<String, u64>,
    events_first: u64,
    events_next: u64,
    expiry: [Ring; 2],
    /// The newest timestamp of a call applied (D74).
    newest_ts_ms: u64,
    /// The executor the `init` entry recorded; see [`Self::executor`].
    pub executor: Option<crate::types::Executor>,
    /// A component's host records and its storage in memory (empty for wasmer).
    pub component: crate::component::ComponentState,
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
            pending: self.pending.clone(),
            next_pending: self.next_pending,
            timer_ticks: self.timer_ticks.clone(),
            events_first: self.events_first,
            events_next: self.events_next,
            expiry: self.expiry,
            newest_ts_ms: self.newest_ts_ms,
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
        self.pending = h.pending;
        self.next_pending = h.next_pending;
        self.timer_ticks = h.timer_ticks;
        self.events_first = h.events_first;
        self.events_next = h.events_next;
        self.expiry = h.expiry;
        self.newest_ts_ms = h.newest_ts_ms;
        self.executor = h.executor;
        self.component.head = h.component.unwrap_or_default();
    }
}

/// The whole-state JSON the previous storage format kept for an app.
#[derive(Deserialize)]
struct LegacyAppState {
    kv: BTreeMap<Bytes, Bytes>,
    bundle: Option<String>,
    initialized: bool,
    #[serde(default)]
    init_error: Option<Value>,
    last_index: u64,
    #[serde(default)]
    idempotency: BTreeMap<String, IdemRecord>,
    #[serde(default)]
    events: VecDeque<EventRecord>,
    #[serde(default)]
    pending: BTreeMap<u64, Pending>,
    #[serde(default)]
    next_pending: u64,
    #[serde(default)]
    timer_ticks: BTreeMap<String, u64>,
}

/// Entities touched per entry, kept in memory for search to catch up from.
pub const TOUCHED_RING: usize = 4096;

/// The app group's state machine.
pub struct AppMachine {
    pub app_id: AppId,
    pub state: AppState,
    pub events_tx: broadcast::Sender<EventRecord>,
    /// `(index, entities)` of recent entries that touched any; empty after a
    /// restart or a snapshot, which makes search rebuild instead.
    pub touched: VecDeque<(u64, Vec<String>)>,
    /// The first index `touched` is complete from.
    pub touched_since: u64,
    /// A component's last applied ticks with their outcome, for tests (feature
    /// `testing`: the Rooms cluster runner checks a tick step's `expect`). Not
    /// part of the state, and not in a production build.
    #[cfg(feature = "testing")]
    pub ticks: crate::component::Ticks,
    /// Idempotency floors and cap, both executors (D74's; tests scale them down).
    pub(crate) idem_limits: crate::idempotency::Limits,
    /// Where an applied entry's index and the ids of the files it deleted go, so this replica
    /// drops their bytes outside `apply`; not part of the state.
    pub forgotten: Option<tokio::sync::mpsc::UnboundedSender<(u64, Vec<String>)>>,
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
            forgotten: None,
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
            tracing::error!(app = %self.app_id, error = %e, "reading events failed");
        }
        out
    }

    fn idem_key(caller: &Caller, key: &str) -> String {
        match caller {
            Caller::App { app } => format!("app:{app}|{key}"),
            _ => format!("{}|{key}", caller.account().unwrap_or("public")),
        }
    }

    /// The entities touched by entries after `index`, or `None` when this
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

    /// Whether `id` is a blob of this app or, for a component, one of its files.
    #[must_use]
    pub fn holds_blob(&self, id: &[u8; 32]) -> bool {
        self.state.kv.get(&blob_key(id)).is_some()
            || self.state.component.files.contains_key(&hex::encode(id))
    }

    /// The size of one of the app's blobs, if it is one.
    #[must_use]
    pub fn blob_size(&self, id: &[u8; 32]) -> Option<u64> {
        let v = self.state.kv.get(&blob_key(id))?;
        Some(u64::from_le_bytes(v.get(..8)?.try_into().ok()?))
    }

    /// The app's blobs (id, size), and what its storage holds besides:
    /// a scan of its whole state.
    #[must_use]
    pub fn storage_stats(&self) -> StorageStats {
        // A component's storage is its own keyspace, in memory; its files count as blob bytes.
        if self.state.executor() == Some(crate::types::Executor::Component) {
            let kv = &self.state.component.kv;
            return StorageStats {
                keys: kv.len() as u64,
                bytes: kv.iter().map(|(k, v)| (k.len() + v.len()) as u64).sum(),
                blob_bytes: (self.state.component.files.values())
                    .map(|f| crate::files::sealed_len(f.size))
                    .sum(),
                ..StorageStats::default()
            };
        }
        let prefix = blob_key(&[0; 32]);
        let prefix = &prefix[..prefix.len() - 32];
        let mut stats = StorageStats::default();
        let _r = self.state.kv.for_each(|k, v| {
            if let Some(id) = k.strip_prefix(prefix) {
                if let (Ok(id), Some(size)) = (
                    <[u8; 32]>::try_from(id),
                    v.get(..8).and_then(|b| b.try_into().ok()),
                ) {
                    let size = u64::from_le_bytes(size);
                    stats.blob_bytes += size;
                    stats.blobs.push((id, size));
                    return;
                }
            }
            stats.keys += 1;
            stats.bytes += (k.len() + v.len()) as u64;
        });
        stats.blobs.sort();
        stats
    }

    /// A previous answer for this idempotency key, if still remembered. Never
    /// judged by a clock: a record that is kept replays.
    #[must_use]
    pub fn replay(&self, caller: &Caller, key: &str) -> Option<Applied> {
        let raw = self
            .state
            .kv
            .state
            .as_ref()?
            .get_hidden(rows::IDEM, Self::idem_key(caller, key).as_bytes())?;
        serde_json::from_slice::<IdemRecord>(&raw)
            .ok()
            .map(|r| r.applied)
    }

    /// Remember `applied` for an idempotency key until `expires_ms` (D74's rule,
    /// `crate::idempotency`).
    fn remember(
        &mut self,
        tx: &mut StateTx<'_>,
        class: usize,
        key: String,
        applied: Applied,
        expires_ms: u64,
    ) {
        let record = IdemRecord {
            applied,
            expires_ms,
        };
        match serde_json::to_vec(&record) {
            Ok(v) => {
                crate::idempotency::record(tx, &IDEM_ROWS, &mut self.state.expiry, class, &key, &v)
            }
            Err(e) => tx.fail(e),
        }
    }

    /// The conflict a build answers when its module is not the state's.
    /// `loaded` is the bundle the caller compiled its module from, read before
    /// the propose lock: an upgrade (or the init) applied since would have the
    /// old code run against the new state, on a base the apply check accepts.
    /// The caller runs it again (`Node::call`), with the bundle named now.
    #[must_use]
    pub fn stale_module(&self, loaded: Option<&str>) -> Option<Applied> {
        (self.state.bundle.as_deref() != loaded).then(|| Applied {
            index: self.state.last_index,
            status: Status::Trap,
            result: Value::Null,
            error: Value::String("conflict: the app's bundle changed; retry".into()),
            events: Vec::new(),
            conflict: true,
        })
    }

    fn apply_outcome(
        &mut self,
        tx: &mut StateTx<'_>,
        index: u64,
        base_index: u64,
        outcome: &Outcome,
        depth: u32,
    ) -> Applied {
        let conflict = base_index != self.state.last_index;
        if !conflict {
            if depth < MAX_FOLLOWUP_DEPTH {
                for f in &outcome.followups {
                    let seq = self.state.next_pending;
                    self.state.next_pending += 1;
                    let _prev = self.state.pending.insert(
                        seq,
                        Pending {
                            followup: f.clone(),
                            from_index: index,
                            depth,
                        },
                    );
                }
            }
            if !outcome.touched.is_empty() {
                if self.touched_since == u64::MAX {
                    self.touched_since = index;
                }
                self.touched.push_back((index, outcome.touched.clone()));
                if self.touched.len() > TOUCHED_RING {
                    if let Some((i, _)) = self.touched.pop_front() {
                        self.touched_since = i + 1;
                    }
                }
            } else if self.touched_since == u64::MAX {
                self.touched_since = index;
            }
            for w in &outcome.diff {
                match &w.value {
                    Some(v) => tx.put_hidden(rows::KV, &w.key.0, &v.0),
                    None => tx.delete_hidden(rows::KV, &w.key.0),
                }
            }
            for (seq, (kind, data)) in outcome.events.iter().enumerate() {
                self.push_event(
                    tx,
                    EventRecord {
                        index,
                        seq: u32::try_from(seq).unwrap_or(u32::MAX),
                        kind: kind.clone(),
                        data: data.clone(),
                        audience: None,
                        caller: None,
                    },
                );
            }
        }
        Applied {
            index,
            status: if conflict {
                Status::Trap
            } else {
                outcome.status.clone()
            },
            result: if conflict {
                Value::Null
            } else {
                outcome.result.clone()
            },
            error: if conflict {
                Value::String("conflict: retry".into())
            } else {
                outcome.error.clone()
            },
            events: if conflict {
                Vec::new()
            } else {
                outcome.events.clone()
            },
            conflict,
        }
    }
}

impl Machine for AppMachine {
    fn attach(&mut self, state: GroupState) {
        self.state.kv.state = Some(state);
    }

    fn apply(&mut self, index: u64, command: &[u8], tx: &mut StateTx<'_>) -> Vec<u8> {
        use crate::types::Executor as Kind;
        // Each entry is applied with the executor the app's state names; before
        // `init`, the entry itself names it.
        let component = match self.state.executor() {
            Some(Kind::Component) => true,
            Some(Kind::Wasmer) => false,
            None => crate::component::ComponentCmd::is_component(command),
        };
        let out = if component {
            self.apply_component(tx, index, command)
        } else {
            self.apply_wasmer(tx, index, command)
        };
        self.state.last_index = index;
        match serde_json::to_vec(&self.state.head()) {
            Ok(h) => tx.put(rows::HEAD.to_vec(), h),
            Err(e) => tx.fail(e),
        }
        out
    }

    fn restore(&mut self, _data: &[u8]) -> eyre::Result<()> {
        let head = match self.state.kv.state.as_ref().and_then(|s| s.get(rows::HEAD)) {
            Some(raw) => serde_json::from_slice(&raw)?,
            None => Head::default(),
        };
        self.state.set_head(head);
        // A component's storage, in memory again from its rows.
        self.state.component.kv = crate::component::Kv::default();
        self.state.component.files = crate::component::Marks::default();
        if self.state.executor() == Some(crate::types::Executor::Component) {
            if let Some(state) = self.state.kv.state.clone() {
                self.state.component.load(&state)?;
            }
        }
        // What search knows to catch up from is gone with the old state.
        self.touched.clear();
        self.touched_since = u64::MAX;
        Ok(())
    }

    fn import(&mut self, data: &[u8], tx: &mut StateTx<'_>) -> eyre::Result<()> {
        self.import_legacy(data, tx)
    }
}

impl AppMachine {
    /// Apply one entry of an app on wasmer. An entry that is not of that format
    /// (a component's) changes nothing.
    fn apply_wasmer(&mut self, tx: &mut StateTx<'_>, index: u64, command: &[u8]) -> Vec<u8> {
        // The entry's own timestamp, for idempotency expiry (only calls carry one).
        let mut entry_ts_ms = None;
        let applied = match serde_json::from_slice::<AppCmd>(command) {
            Ok(AppCmd::Init {
                bundle,
                base_index,
                outcome,
            }) => {
                let a = self.apply_outcome(tx, index, base_index, &outcome, 0);
                if !a.conflict {
                    self.state.executor = Some(crate::types::Executor::Wasmer);
                    self.state.bundle = Some(bundle);
                    self.state.initialized = outcome.status == Status::Ok;
                    if !self.state.initialized {
                        self.state.init_error = Some(serde_json::json!({
                            "status": outcome.status,
                            "error": outcome.error,
                        }));
                    }
                }
                a
            }
            Ok(AppCmd::Upgrade {
                bundle,
                base_index,
                outcome,
            }) => {
                let a = self.apply_outcome(tx, index, base_index, &outcome, 0);
                if !a.conflict && outcome.status == Status::Ok {
                    self.state.bundle = Some(bundle);
                }
                a
            }
            Ok(AppCmd::BlobForget { id }) => {
                if let Ok(raw) = hex::decode(&id) {
                    if let Ok(id) = <[u8; 32]>::try_from(raw.as_slice()) {
                        tx.delete_hidden(rows::KV, &blob_key(&id));
                    }
                }
                Applied {
                    index,
                    status: Status::Ok,
                    result: Value::Null,
                    error: Value::Null,
                    events: Vec::new(),
                    conflict: false,
                }
            }
            Ok(AppCmd::Blob { id, size }) => {
                if let Ok(raw) = hex::decode(&id) {
                    if let Ok(id) = <[u8; 32]>::try_from(raw.as_slice()) {
                        tx.put_hidden(rows::KV, &blob_key(&id), &size.to_le_bytes());
                    }
                }
                Applied {
                    index,
                    status: Status::Ok,
                    result: Value::Null,
                    error: Value::Null,
                    events: Vec::new(),
                    conflict: false,
                }
            }
            Ok(AppCmd::Done { seqs }) => {
                for seq in seqs {
                    let _done = self.state.pending.remove(&seq);
                }
                Applied {
                    index,
                    status: Status::Ok,
                    result: Value::Null,
                    error: Value::Null,
                    events: Vec::new(),
                    conflict: false,
                }
            }
            Ok(AppCmd::Call(call)) => {
                let depth = call.depth(&self.state.pending);
                let a = self.apply_outcome(tx, index, call.base_index, &call.outcome, depth);
                if !a.conflict {
                    match &call.trigger {
                        Some(Trigger::FollowUp { seq }) => {
                            let _done = self.state.pending.remove(seq);
                        }
                        Some(Trigger::Timer { method, tick }) => {
                            let _prev = self.state.timer_ticks.insert(method.clone(), *tick);
                        }
                        Some(Trigger::XCall { .. }) | None => {}
                    }
                }
                entry_ts_ms = Some(call.ts_ms);
                if !a.conflict {
                    self.state.newest_ts_ms = self.state.newest_ts_ms.max(call.ts_ms);
                }
                if let (Some(key), false) = (&call.idempotency_key, a.conflict) {
                    let id = Self::idem_key(&call.caller, key);
                    // The class (and so the TTL) is the call's key's prefix (D74).
                    let class = crate::idempotency::class_of(key);
                    let ttl = crate::idempotency::ttl_ms(class);
                    self.remember(tx, class, id, a.clone(), call.ts_ms.saturating_add(ttl));
                }
                a
            }
            Err(e) => Applied {
                index,
                status: Status::Trap,
                result: Value::Null,
                error: Value::String(format!("malformed entry: {e}")),
                events: Vec::new(),
                conflict: true,
            },
        };
        // D74 (spec v0.26): what has expired by the earlier of the newest applied
        // call's timestamp and this entry's own goes, and the app keeps at most the cap.
        crate::idempotency::expire(
            tx,
            &IDEM_ROWS,
            &mut self.state.expiry,
            self.idem_limits,
            crate::idempotency::expiry_now(self.state.newest_ts_ms, entry_ts_ms),
            |raw| {
                serde_json::from_slice::<IdemRecord>(raw)
                    .ok()
                    .map(|r| (r.expires_ms, r.applied.index))
            },
        );
        serde_json::to_vec(&applied).unwrap_or_default()
    }

    /// Bring in an app state written by the previous storage format (wasmer apps
    /// only: components came after it).
    fn import_legacy(&mut self, data: &[u8], tx: &mut StateTx<'_>) -> eyre::Result<()> {
        let old: LegacyAppState = serde_json::from_slice(data)?;
        for (k, v) in &old.kv {
            tx.put_hidden(rows::KV, &k.0, &v.0);
        }
        self.state.set_head(Head {
            bundle: old.bundle,
            initialized: old.initialized,
            init_error: old.init_error,
            last_index: old.last_index,
            pending: old.pending,
            next_pending: old.next_pending,
            timer_ticks: old.timer_ticks,
            ..Head::default()
        });
        for rec in old.events {
            tx.put(
                rows::event(self.state.events_next),
                serde_json::to_vec(&rec)?,
            );
            self.state.events_next += 1;
        }
        let mut records: Vec<_> = old.idempotency.into_iter().collect();
        records.sort_by_key(|(_, r)| r.expires_ms);
        for (key, r) in records {
            // The stored key is `caller|key`; the class is the call's key's prefix.
            let class = crate::idempotency::class_of(key.split_once('|').map_or(&*key, |(_, k)| k));
            tx.put_hidden(rows::IDEM, key.as_bytes(), &serde_json::to_vec(&r)?);
            let ring = &mut self.state.expiry[class];
            tx.put(rows::expiry(class as u8, ring.next), key.into_bytes());
            ring.next += 1;
        }
        tx.put(rows::HEAD.to_vec(), serde_json::to_vec(&self.state.head())?);
        Ok(())
    }
}

/// Copy-on-write view of an app's state, recording writes in order.
pub struct Overlay<'a> {
    base: &'a dyn StateRead,
    writes: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    order: Vec<Vec<u8>>,
    /// Prepended to every key: empty for the app's state, an account's space
    /// for its private storage ([`private_prefix`]).
    prefix: Vec<u8>,
    /// Set when app code touched [`RESERVED`]: the run is refused.
    reserved_touched: std::cell::Cell<bool>,
}

/// The host's key space in an app's state: accounts' private spaces, cells'
/// writer sets, the app's blob marks. App code addresses the app's state with
/// raw keys too (`storage_read`/`storage_write`), so the unprefixed overlay
/// answers nothing under this prefix and a run that tries to read or write it
/// is refused; only the host writes there (`Overlay::set_host`).
pub const RESERVED: &[u8] = b"\0calimero.";

/// Where a shared-storage cell's current writer set lives in the app's state.
///
/// An app has one log, so the rotation a run asks for is written beside the
/// cell, in the same entry as the run's other writes, and read back from there.
#[must_use]
pub fn writers_key(cell: &[u8; 32]) -> Vec<u8> {
    let mut k = b"\0calimero.writers\0".to_vec();
    k.extend_from_slice(cell);
    k
}

/// Marks a blob as the app's: written with the run that wrote it (or the
/// upload's entry), so a run opens only its own app's blobs.
#[must_use]
pub fn blob_key(id: &[u8; 32]) -> Vec<u8> {
    let mut k = b"\0calimero.blobs\0".to_vec();
    k.extend_from_slice(id);
    k
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
    pub blob_bytes: u64,
}

/// Where an account's private storage lives in the app's state.
///
/// v1 kept "private" data on the member's own node, never synced. Here an app
/// has no member nodes, so private data is the caller's own space instead:
/// replicated and sealed like the rest of the state, but the host hands a run
/// only its caller's space, so app code can never read another account's.
#[must_use]
pub fn private_prefix(account: &[u8; 32]) -> Vec<u8> {
    let mut p = b"\0calimero.private\0".to_vec();
    p.extend_from_slice(account);
    p.push(0);
    p
}

impl<'a> Overlay<'a> {
    #[must_use]
    pub fn new(base: &'a dyn StateRead) -> Self {
        Self::with_prefix(base, Vec::new())
    }

    /// An overlay that reads and writes only keys under `prefix`.
    #[must_use]
    pub fn with_prefix(base: &'a dyn StateRead, prefix: Vec<u8>) -> Self {
        Self {
            base,
            writes: BTreeMap::new(),
            order: Vec::new(),
            prefix,
            reserved_touched: std::cell::Cell::new(false),
        }
    }

    /// Whether app code reaches `key` in the host's space, which it may not.
    fn forbidden(&self, key: &[u8]) -> bool {
        let hit = self.prefix.is_empty() && key.starts_with(RESERVED);
        if hit {
            self.reserved_touched.set(true);
        }
        hit
    }

    /// Whether app code read or wrote the host's key space ([`RESERVED`]).
    #[must_use]
    pub fn touched_reserved(&self) -> bool {
        self.reserved_touched.get()
    }

    /// A write by the host itself, which may be under [`RESERVED`].
    pub fn set_host(&mut self, key: Vec<u8>, value: Vec<u8>) {
        let key = self.full(&key);
        self.order.push(key.clone());
        let _prev = self.writes.insert(key, Some(value));
    }

    fn full(&self, key: &[u8]) -> Vec<u8> {
        let mut k = self.prefix.clone();
        k.extend_from_slice(key);
        k
    }

    /// The writes, in the order they last happened.
    #[must_use]
    pub fn into_diff(self) -> Vec<Write> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for k in self.order.into_iter().rev() {
            if seen.insert(k.clone()) {
                let v = self.writes.get(&k).cloned().flatten();
                out.push(Write {
                    key: Bytes(k),
                    value: v.map(Bytes),
                });
            }
        }
        out.reverse();
        out
    }
}

impl Storage for Overlay<'_> {
    fn get(&self, key: &Key) -> Option<StoreValue> {
        if self.forbidden(key) {
            return None;
        }
        let key = self.full(key);
        match self.writes.get(&key) {
            Some(v) => v.clone(),
            None => self.base.get(&key),
        }
    }

    fn set(&mut self, key: Key, value: StoreValue) -> Option<StoreValue> {
        if self.forbidden(&key) {
            return None;
        }
        let prev = self.get(&key);
        let key = self.full(&key);
        self.order.push(key.clone());
        let _prev = self.writes.insert(key, Some(value));
        prev
    }

    fn remove(&mut self, key: &Key) -> Option<Vec<u8>> {
        if self.forbidden(key) {
            return None;
        }
        let prev = self.get(key);
        let key = self.full(key);
        self.order.push(key.clone());
        let _prev = self.writes.insert(key, None);
        prev
    }

    fn has(&self, key: &Key) -> bool {
        self.get(key).is_some()
    }

    fn host_get(&self, key: &Key) -> Option<StoreValue> {
        let key = self.full(key);
        match self.writes.get(&key) {
            Some(v) => v.clone(),
            None => self.base.get(&key),
        }
    }

    fn shared_writers(&self, cell: &[u8; 32]) -> Option<CellWriters> {
        let key = writers_key(cell);
        let raw = match self.writes.get(&key) {
            Some(v) => v.clone(),
            None => self.base.get(&key),
        };
        match raw {
            None => Some(CellWriters::Genesis),
            Some(bytes) => borsh::from_slice(&bytes).ok().map(CellWriters::Rotated),
        }
    }
}

/// Why a run's writer-set rotations may not take effect, if they may not: the
/// host-side rules (`shared_rotations.rs`). Each rotation steps from the set in
/// effect (the state's, or at genesis the run's own prior if the cell id commits
/// to it, or this run's previous rotation of the cell), and the run's account
/// must hold `ADMIN` in it (so a `Frozen` cell, which grants none, never rotates).
/// A TEE run or a cross-app call may not rotate.
fn rotation_refusal(
    overlay: &Overlay<'_>,
    rotations: &[calimero_storage::shared_writers::SharedRotation],
    account: &calimero_account::AccountId,
    caller: &Caller,
    env: &RunEnv,
) -> Option<String> {
    use calimero_storage::entities::OpMask;
    use calimero_storage::shared_writers::{CellWriters, Writers};
    if rotations.is_empty() {
        return None;
    }
    if env.tee_trigger {
        return Some("a TEE run may not rotate a cell's writers".into());
    }
    if matches!(caller, Caller::App { .. } | Caller::Public) {
        return Some("only an account's own call may rotate a cell's writers".into());
    }
    let mut stepped: BTreeMap<calimero_storage::address::Id, Writers> = BTreeMap::new();
    for r in rotations {
        let in_effect = match stepped.get(&r.cell) {
            Some(w) => w.clone(),
            None => {
                match calimero_runtime::store::Storage::shared_writers(overlay, r.cell.as_bytes()) {
                    Some(CellWriters::Rotated(w)) => w,
                    Some(CellWriters::Genesis)
                        if calimero_storage::collections::cell_id_binds(r.cell, &r.prior) =>
                    {
                        r.prior.clone()
                    }
                    Some(CellWriters::Genesis) => {
                        return Some(
                            "the rotation's prior set is not the one the cell id commits to".into(),
                        )
                    }
                    None => return Some("the cell's writers could not be read".into()),
                }
            }
        };
        if !in_effect
            .get(account)
            .is_some_and(|mask| mask.contains(OpMask::ADMIN))
        {
            return Some("only a writer holding ADMIN may rotate the cell's writers".into());
        }
        let _prev = stepped.insert(r.cell, r.new.clone());
    }
    None
}

/// Write `state` (borsh of the app's state) as the app's root entry, with the
/// Merkle index updated. The storage
/// layer's callbacks outlive any borrow, so they work over a copy of the state
/// and the writes are carried back into `overlay`.
fn write_root_state(
    overlay: &mut Overlay<'_>,
    base: &dyn StateRead,
    context: [u8; 32],
    executor: [u8; 32],
    state: &[u8],
) -> Result<(), String> {
    use std::cell::RefCell;
    use std::rc::Rc;

    use calimero_prelude::ROOT_STORAGE_ENTRY_ID;
    use calimero_storage::address::Id;
    use calimero_storage::entities::Metadata;
    use calimero_storage::env::{with_runtime_env, RuntimeEnv};
    use calimero_storage::index::Index;
    use calimero_storage::store::{Key as SKey, MainStorage};
    use calimero_storage::Interface;

    // The run's writes over the app's state, read through: never a copy of
    // the whole state.
    let view = Rc::new(RefCell::new(overlay.writes.clone()));
    let base = base.handle();
    let written = Rc::new(RefCell::new(Vec::<(Vec<u8>, Option<Vec<u8>>)>::new()));
    let read = {
        let view = Rc::clone(&view);
        Rc::new(move |k: &SKey| {
            let k = k.to_bytes().to_vec();
            let staged = view.borrow().get(&k).cloned();
            match staged {
                Some(v) => v,
                None => base.get(&k),
            }
        }) as Rc<dyn Fn(&SKey) -> Option<Vec<u8>>>
    };
    let write = {
        let (view, written) = (Rc::clone(&view), Rc::clone(&written));
        Rc::new(move |k: SKey, v: &[u8]| {
            let k = k.to_bytes().to_vec();
            let _prev = view.borrow_mut().insert(k.clone(), Some(v.to_vec()));
            written.borrow_mut().push((k, Some(v.to_vec())));
            true
        }) as Rc<dyn Fn(SKey, &[u8]) -> bool>
    };
    let remove = {
        let (view, written) = (Rc::clone(&view), Rc::clone(&written));
        Rc::new(move |k: &SKey| {
            let k = k.to_bytes().to_vec();
            let _prev = view.borrow_mut().insert(k.clone(), None);
            written.borrow_mut().push((k, None));
            true
        }) as Rc<dyn Fn(&SKey) -> bool>
    };
    let env = RuntimeEnv::new(read, write, remove, context, executor, executor);
    let root = Id::new(ROOT_STORAGE_ENTRY_ID);
    let mut entry = state.to_vec();
    entry.extend_from_slice(&ROOT_STORAGE_ENTRY_ID);
    with_runtime_env(env, || -> Result<(), String> {
        // Stamped after whatever the root entry holds now, so it wins.
        let at = Index::<MainStorage>::get_index(root)
            .map_err(|e| e.to_string())?
            .map_or(u64::MAX / 2, |i| {
                i.metadata
                    .updated_at()
                    .max(i.metadata.created_at())
                    .saturating_add(1)
            });
        let result = Interface::<MainStorage>::write_migrated_root_state(
            root,
            &entry,
            Metadata::new(at, at),
        )
        .map(drop)
        .map_err(|e| e.to_string());
        calimero_storage::delta::clear_pending_delta();
        result
    })?;
    for (k, v) in written.take() {
        let _prev = match v {
            Some(v) => overlay.set(k, v),
            None => overlay.remove(&k),
        };
    }
    Ok(())
}

/// What the node gives one run besides its call.
#[derive(Clone, Debug, Default)]
pub struct RunEnv {
    /// The caller's organization standing ([`caller_org`]), as `env::caller_org` reads it.
    pub org: Option<Vec<u8>>,
    /// The app's blobs.
    pub blobs: Option<Arc<dyn calimero_runtime::logic::BlobHost>>,
    /// Search over the app's own indexes (views only).
    pub search: Option<Arc<dyn calimero_runtime::logic::SearchHost>>,
    /// The app that dispatched this run with `xcall` (its context id).
    pub xcall_origin: Option<[u8; 32]>,
    /// The node fired it itself, as the app's TEE authority.
    pub tee_trigger: bool,
    /// The keys `TeeSecret` and the other sealing host functions use.
    pub sealing: calimero_runtime::logic::SealingContext,
    /// Outbound HTTPS (`http_request`): only a writing run's, and only under
    /// what its app declares and its owner approved.
    pub outbound: Option<Arc<dyn calimero_runtime::logic::OutboundHost>>,
}

/// The storage entities a run's artifact (a borsh `StorageDelta`) touches, and
/// every entry above them, hex: what search re-extracts. Empty for anything
/// that does not decode.
#[must_use]
pub fn changed_entities(artifact: &[u8]) -> Vec<String> {
    use calimero_storage::action::Action;
    use calimero_storage::delta::StorageDelta;
    let actions = match borsh::from_slice::<StorageDelta>(artifact) {
        Ok(StorageDelta::Actions(actions) | StorageDelta::CausalActions { actions, .. }) => actions,
        _ => return Vec::new(),
    };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for action in &actions {
        let ancestors = match action {
            Action::Add { ancestors, .. } | Action::Update { ancestors, .. } => {
                ancestors.as_slice()
            }
            Action::DeleteRef { .. } => &[],
        };
        let ids = core::iter::once(action.id()).chain(ancestors.iter().map(|c| c.id()));
        for id in ids {
            let id: [u8; 32] = id.into();
            if seen.insert(id) {
                out.push(hex::encode(id));
            }
        }
    }
    out
}

/// Compiles and runs app bundles.
pub struct Executor {
    modules: Mutex<HashMap<String, Arc<calimero_runtime::Module>>>,
}

impl Default for Executor {
    fn default() -> Self {
        Self {
            modules: Mutex::new(HashMap::new()),
        }
    }
}

fn json_or_string(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(bytes).into_owned()))
}

impl Executor {
    /// The compiled module for `hash`, compiling `wasm` on first use.
    pub fn module(&self, hash: &str, wasm: &[u8]) -> eyre::Result<Arc<calimero_runtime::Module>> {
        if let Some(m) = self
            .modules
            .lock()
            .map_err(|e| eyre::eyre!("{e}"))?
            .get(hash)
        {
            return Ok(Arc::clone(m));
        }
        let module = calimero_runtime::Engine::default()
            .compile(wasm)
            .map_err(|e| eyre::eyre!("compile: {e:?}"))?;
        let module = Arc::new(module);
        let _prev = self
            .modules
            .lock()
            .map_err(|e| eyre::eyre!("{e}"))?
            .insert(hash.to_owned(), Arc::clone(&module));
        Ok(module)
    }

    /// Run a migration export: the export reads the old state and returns the
    /// new root state (borsh), the app's `__calimero_migration_check`, if it
    /// exports one, must pass over it, and the host writes it as the app's root
    /// entry. Never fails: errors become the outcome's status. A module without
    /// the export upgrades code-only.
    #[must_use]
    pub fn migrate(
        module: &calimero_runtime::Module,
        app_id: &str,
        caller: &Caller,
        method: &str,
        base: &dyn StateRead,
        env: &RunEnv,
    ) -> Outcome {
        use calimero_runtime::errors::{FunctionCallError, MethodResolutionError};
        let mut overlay = Overlay::new(base);
        let context = calimero_identity::sha256(app_id.as_bytes());
        let account = calimero_account::AccountId::from(caller.account_bytes());
        let cloud = || calimero_runtime::CloudRun {
            caller_org: env.org.clone(),
            blobs: env.blobs.clone(),
            ..Default::default()
        };
        let out = match module.run_cloud(
            context.into(),
            account,
            caller.account_bytes().into(),
            method,
            &[],
            &mut overlay,
            None,
            cloud(),
        ) {
            Ok(out) => out,
            Err(e) => return Outcome::trap(&format!("{e:?}")),
        };
        let state = match out.returns {
            Ok(Some(state)) => state,
            Ok(None) => {
                return Outcome {
                    status: Status::AppError,
                    error: Value::String(format!("{method} returned no state")),
                    logs: out.logs,
                    ..Outcome::default()
                }
            }
            Err(FunctionCallError::MethodResolutionError(
                MethodResolutionError::MethodNotFound { .. },
            )) => return Outcome::default(),
            Err(e) => {
                return Outcome {
                    status: Status::AppError,
                    error: Value::String(format!("migration failed: {e:?}")),
                    logs: out.logs,
                    ..Outcome::default()
                }
            }
        };
        // The check reads the migrated state; whatever it writes is dropped.
        if module.exports_function("__calimero_migration_check") {
            let input =
                borsh::to_vec(&(state.clone(), out.migration_witness.clone())).unwrap_or_default();
            let (writes, order) = (overlay.writes.clone(), overlay.order.clone());
            let verdict = module.run_cloud(
                context.into(),
                account,
                caller.account_bytes().into(),
                "__calimero_migration_check",
                &input,
                &mut overlay,
                None,
                cloud(),
            );
            overlay.writes = writes;
            overlay.order = order;
            // A check that trapped or answered anything but `true` refuses.
            let passed = matches!(
                &verdict,
                Ok(v) if matches!(&v.returns, Ok(Some(bytes)) if borsh::from_slice::<bool>(bytes).is_ok_and(|ok| ok))
            );
            if !passed {
                return Outcome {
                    status: Status::AppError,
                    error: Value::String("the migration check refused the migrated state".into()),
                    logs: out.logs,
                    ..Outcome::default()
                };
            }
        }
        if overlay.touched_reserved() {
            return Outcome {
                status: Status::AppError,
                error: Value::String("the migration addressed the host's key space".into()),
                logs: out.logs,
                ..Outcome::default()
            };
        }
        if let Err(e) =
            write_root_state(&mut overlay, base, context, caller.account_bytes(), &state)
        {
            return Outcome::trap(&format!("writing the migrated state: {e}"));
        }
        let events = out
            .events
            .iter()
            .map(|e| (e.kind.clone(), json_or_string(&e.data)))
            .collect();
        Outcome {
            status: Status::Ok,
            diff: overlay.into_diff(),
            events,
            logs: out.logs,
            ..Outcome::default()
        }
    }

    /// Run `method` against `base` as `caller`, with JSON `args`. Never fails:
    /// errors become the outcome's status.
    #[must_use]
    pub fn run(
        module: &calimero_runtime::Module,
        app_id: &str,
        caller: &Caller,
        method: &str,
        args: &Value,
        base: &dyn StateRead,
        env: &RunEnv,
    ) -> Outcome {
        let input = if args.is_null() {
            Vec::new()
        } else {
            serde_json::to_vec(args).unwrap_or_default()
        };
        Self::run_bytes(module, app_id, caller, method, &input, base, env).0
    }

    /// [`Self::run`] with raw input; also answers the raw return value.
    #[must_use]
    pub fn run_bytes(
        module: &calimero_runtime::Module,
        app_id: &str,
        caller: &Caller,
        method: &str,
        input: &[u8],
        base: &dyn StateRead,
        env: &RunEnv,
    ) -> (Outcome, Option<Vec<u8>>) {
        let mut overlay = Overlay::new(base);
        let context = calimero_identity::sha256(app_id.as_bytes());
        // A run the node fired as the app's TEE (a timer, a `tee:` handler)
        // authors as the TEE authority, the one writer `TeeOnly` state admits.
        let account = if env.tee_trigger {
            calimero_account::AccountId::TEE_AUTHORITY
        } else {
            calimero_account::AccountId::from(caller.account_bytes())
        };
        // There are no device ids: a person is their account, so the executor
        // (`env::executor_id`, a per-device key in v1) is the account too. Apps that
        // key players or authors by executor then tell people apart as before.
        // The caller's private storage (`env::private_storage_*`), when there
        // is a caller to own it.
        let mut private = caller
            .account()
            .map(|_| Overlay::with_prefix(base, private_prefix(&caller.account_bytes())));
        let result = module.run_cloud(
            context.into(),
            account,
            caller.account_bytes().into(),
            method,
            input,
            &mut overlay,
            private
                .as_mut()
                .map(|p| p as &mut dyn calimero_runtime::store::Storage),
            calimero_runtime::CloudRun {
                caller_org: env.org.clone(),
                search: env.search.clone(),
                blobs: env.blobs.clone(),
                xcall_origin: env.xcall_origin.map(Into::into),
                tee_trigger: env.tee_trigger,
                sealing: env.sealing.clone(),
                outbound: env.outbound.clone(),
            },
        );
        let mut new_blobs = Vec::new();
        let mut refused: Option<String> = None;
        if let Ok(out) = &result {
            if out.returns.is_ok() {
                // Rotations of shared cells' writers take effect with the run's writes.
                match rotation_refusal(&overlay, &out.shared_rotations, &account, caller, env) {
                    Some(why) => refused = Some(why),
                    None => {
                        for r in &out.shared_rotations {
                            if let Ok(bytes) = borsh::to_vec(&r.new) {
                                overlay.set_host(writers_key(r.cell.as_bytes()), bytes);
                            }
                        }
                    }
                }
                // So are the blobs it wrote, and they are this app's.
                for (id, bytes) in &out.new_blobs {
                    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                    overlay.set_host(blob_key(id), size.to_le_bytes().to_vec());
                    new_blobs.push((hex::encode(id), bytes.clone()));
                }
            }
        }
        if overlay.touched_reserved() {
            refused = Some(format!(
                "the run addressed the host's key space ({}*); app code may not",
                String::from_utf8_lossy(RESERVED).trim_start_matches('\0')
            ));
        }
        if let Some(why) = refused {
            return (
                Outcome {
                    status: Status::AppError,
                    error: Value::String(why),
                    logs: result.map(|o| o.logs).unwrap_or_default(),
                    ..Outcome::default()
                },
                None,
            );
        }
        let mut diff = overlay.into_diff();
        if let Some(p) = private {
            diff.extend(p.into_diff());
        }
        match result {
            Ok(out) => {
                let events: Vec<(String, Value)> = out
                    .events
                    .iter()
                    .map(|e| (e.kind.clone(), json_or_string(&e.data)))
                    .collect();
                let mut followups: Vec<FollowUp> = out
                    .events
                    .iter()
                    .filter_map(|e| {
                        let handler = e.handler.as_deref()?;
                        Some(match handler.strip_prefix("tee:") {
                            Some(method) => FollowUp::Handler {
                                method: method.to_owned(),
                                input: Bytes(e.data.clone()),
                                tee: true,
                            },
                            None => FollowUp::Handler {
                                method: handler.to_owned(),
                                input: Bytes(e.data.clone()),
                                tee: false,
                            },
                        })
                    })
                    .collect();
                followups.extend(out.xcalls.iter().map(|x| FollowUp::XCall {
                    context: hex::encode(x.context_id),
                    method: x.function.clone(),
                    input: Bytes(x.params.clone()),
                }));
                match out.returns {
                    Ok(ret) => (
                        Outcome {
                            status: Status::Ok,
                            result: ret.as_deref().map_or(Value::Null, json_or_string),
                            error: Value::Null,
                            diff,
                            events,
                            logs: out.logs,
                            followups,
                            touched: changed_entities(&out.artifact),
                            new_blobs,
                        },
                        ret,
                    ),
                    Err(calimero_runtime::errors::FunctionCallError::ExecutionError(payload)) => (
                        Outcome {
                            status: Status::AppError,
                            error: json_or_string(&payload),
                            logs: out.logs,
                            ..Outcome::default()
                        },
                        None,
                    ),
                    Err(e) => (
                        Outcome {
                            status: Status::Trap,
                            error: Value::String(format!("{e:?}")),
                            logs: out.logs,
                            ..Outcome::default()
                        },
                        None,
                    ),
                }
            }
            Err(e) => (Outcome::trap(&format!("{e:?}")), None),
        }
    }
}

/// Milliseconds now, for entries.
#[must_use]
pub fn stamp() -> u64 {
    now_ms()
}

/// The caller's standing in the app's organization, encoded for
/// `env::caller_org` as `org\nrole\nteam\n...`; `None` when the app has no
/// organization or the caller is not in it.
#[must_use]
pub fn caller_org(
    state: &crate::system::SystemState,
    app_id: &str,
    caller: &Caller,
) -> Option<Vec<u8>> {
    let app = state.apps.get(app_id)?;
    let org_id = app.org.as_ref()?;
    let org = state.orgs.get(org_id)?;
    let account = caller.account()?;
    let role = match org.role(account)? {
        crate::types::OrgRole::Guest => "guest",
        crate::types::OrgRole::Member => "member",
        crate::types::OrgRole::Admin => "admin",
        crate::types::OrgRole::Owner => "owner",
    };
    let mut out = format!("{org_id}\n{role}");
    for team in org.teams.keys().filter(|t| org.in_team(account, t)) {
        out.push('\n');
        out.push_str(team);
    }
    Some(out.into_bytes())
}

#[cfg(test)]
mod tests {
    use calimero_runtime::store::Storage;

    use super::*;

    #[test]
    fn private_storage_is_the_callers_own_space() {
        let alice = [1_u8; 32];
        let bob = [2_u8; 32];
        let base = BTreeMap::new();
        let mut a = Overlay::with_prefix(&base, private_prefix(&alice));
        let _prev = a.set(b"note".to_vec(), b"alice's".to_vec());
        let diff = a.into_diff();
        // The write lands under Alice's prefix, never under the bare key.
        assert_eq!(diff.len(), 1);
        assert!(diff[0].key.0.starts_with(&private_prefix(&alice)));

        let mut state = BTreeMap::new();
        for w in diff {
            let _prev = state.insert(w.key, w.value.unwrap());
        }
        let main = Overlay::new(&state);
        let as_bob = Overlay::with_prefix(&state, private_prefix(&bob));
        let as_alice = Overlay::with_prefix(&state, private_prefix(&alice));
        assert!(!main.has(&b"note".to_vec()));
        assert!(!as_bob.has(&b"note".to_vec()));
        assert_eq!(as_alice.get(&b"note".to_vec()), Some(b"alice's".to_vec()));
    }

    /// Xil's review, issue 1: app code reading or writing raw keys could reach
    /// another account's private space (and the host's writer sets and blob
    /// marks), because they sat in the same map under a prefix.
    #[test]
    fn app_code_cannot_reach_the_hosts_key_space() {
        let alice = [1_u8; 32];
        let mut state = BTreeMap::new();
        let mut key = private_prefix(&alice);
        key.extend_from_slice(b"note");
        let _prev = state.insert(Bytes(key.clone()), Bytes(b"alice's".to_vec()));

        // Bob's run, through the main overlay, asks for Alice's key verbatim.
        let mut bob = Overlay::new(&state);
        assert_eq!(bob.get(&key), None, "read refused");
        assert_eq!(bob.set(key.clone(), b"bob's".to_vec()), None);
        let _ = bob.remove(&key);
        assert!(bob.touched_reserved(), "the run is refused");
        assert!(bob.into_diff().is_empty(), "nothing lands");

        // The same for a forged writer set and a forged blob mark.
        for host_key in [writers_key(&[7; 32]), blob_key(&[9; 32])] {
            let mut o = Overlay::new(&state);
            let _ = o.set(host_key, vec![0]);
            assert!(o.touched_reserved());
            assert!(o.into_diff().is_empty());
        }

        // Ordinary keys are untouched, and the host still writes its own space.
        let mut o = Overlay::new(&state);
        let _ = o.set(b"plain".to_vec(), b"v".to_vec());
        o.set_host(blob_key(&[9; 32]), vec![1]);
        assert!(!o.touched_reserved());
        assert_eq!(o.into_diff().len(), 2);
    }

    /// Xil's review, issue 8: a rotation needs `ADMIN` in the set in effect, and
    /// at genesis a prior the cell id commits to.
    #[test]
    fn rotations_follow_v1s_host_rules() {
        use calimero_storage::address::Id;
        use calimero_storage::collections::cell_id;
        use calimero_storage::entities::OpMask;
        use calimero_storage::shared_writers::{SharedRotation, Writers};

        let admin = calimero_account::AccountId::from([1_u8; 32]);
        let writer = calimero_account::AccountId::from([2_u8; 32]);
        let genesis: Writers = [
            (admin, OpMask::ADMIN.union(OpMask::WRITE)),
            (writer, OpMask::WRITE),
        ]
        .into_iter()
        .collect();
        let cell = cell_id(Id::new([3; 32]), &genesis);
        let new: Writers = [(admin, OpMask::ADMIN.union(OpMask::WRITE))]
            .into_iter()
            .collect();
        let rotation = |prior: &Writers| SharedRotation {
            cell,
            prior: prior.clone(),
            new: new.clone(),
        };
        let base = BTreeMap::new();
        let o = Overlay::new(&base);
        let as_account = |_: &calimero_account::AccountId| Caller::Account {
            account: "acct".into(),
        };
        let env = RunEnv::default();

        assert_eq!(
            rotation_refusal(&o, &[rotation(&genesis)], &admin, &as_account(&admin), &env),
            None
        );
        assert!(
            rotation_refusal(
                &o,
                &[rotation(&genesis)],
                &writer,
                &as_account(&writer),
                &env
            )
            .is_some(),
            "a writer without ADMIN"
        );
        let forged: Writers = [(writer, OpMask::ADMIN)].into_iter().collect();
        assert!(
            rotation_refusal(
                &o,
                &[rotation(&forged)],
                &writer,
                &as_account(&writer),
                &env
            )
            .is_some(),
            "a prior the cell id does not commit to"
        );
        let tee = RunEnv {
            tee_trigger: true,
            ..RunEnv::default()
        };
        assert!(
            rotation_refusal(&o, &[rotation(&genesis)], &admin, &as_account(&admin), &tee)
                .is_some()
        );
        let app = Caller::App {
            app: "other".into(),
        };
        assert!(rotation_refusal(&o, &[rotation(&genesis)], &admin, &app, &env).is_some());
    }

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
    fn machine(dir: &tempfile::TempDir) -> (std::sync::Arc<calimero_log::Engine>, AppMachine) {
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

    fn apply(m: &mut AppMachine, index: u64, cmd: &AppCmd) -> Applied {
        let state = m.state.kv.state.clone().unwrap();
        let mut tx = StateTx::new(&state);
        let out = m.apply(index, &serde_json::to_vec(cmd).unwrap(), &mut tx);
        tx.commit().unwrap();
        serde_json::from_slice(&out).unwrap()
    }

    fn call(
        m: &AppMachine,
        key: &str,
        ts_ms: u64,
        writes: &[(&str, &str)],
        events: usize,
    ) -> AppCmd {
        AppCmd::Call(Box::new(CallEntry {
            method: "set".into(),
            args: Value::Null,
            caller: Caller::Account {
                account: "acct".into(),
            },
            idempotency_key: Some(key.into()),
            consent: None,
            ts_ms,
            base_index: m.state.last_index,
            outcome: Outcome {
                diff: writes
                    .iter()
                    .map(|(k, v)| Write {
                        key: Bytes(k.as_bytes().to_vec()),
                        value: Some(Bytes(v.as_bytes().to_vec())),
                    })
                    .collect(),
                events: (0..events)
                    .map(|i| ("e".to_owned(), Value::from(i)))
                    .collect(),
                ..Outcome::default()
            },
            trigger: None,
        }))
    }

    /// `Node::call_local` compiles a call's module from the bundle it reads
    /// before the propose lock. An upgrade that applies in between leaves the
    /// base check satisfied (the build reads the upgraded state), so only the
    /// build can tell that the module is the old one: the old code's result
    /// must not commit.
    #[test]
    fn a_call_built_with_a_module_an_upgrade_replaced_never_commits() {
        let dir = tempfile::tempdir().unwrap();
        let (_engine, mut m) = machine(&dir);
        let init = AppCmd::Init {
            bundle: "old".into(),
            base_index: 0,
            outcome: Outcome::default(),
        };
        assert!(!apply(&mut m, 1, &init).conflict);
        // The call's module is chosen, outside the lock.
        let loaded = m.state.bundle.clone();
        // An upgrade applies before the call's build.
        let upgrade = AppCmd::Upgrade {
            bundle: "new".into(),
            base_index: m.state.last_index,
            outcome: Outcome::default(),
        };
        assert!(!apply(&mut m, 2, &upgrade).conflict);
        // The build, as `call_local`'s: refused, or the old module's run on
        // the state the upgrade left.
        match m.stale_module(loaded.as_deref()) {
            Some(refused) => assert!(refused.conflict, "{refused:?}"),
            None => {
                let cmd = call(&m, "k1", 1_000, &[("a", "old code")], 0);
                let _applied = apply(&mut m, 3, &cmd);
            }
        }
        assert_eq!(
            m.state.kv.get(b"a"),
            None,
            "the old module's diff applied to the upgraded state"
        );
        // Built with the module the state names, the call runs.
        assert!(m.stale_module(Some("new")).is_none());
    }

    /// The app's state is the store's: written by entries, read back by a
    /// machine that holds none of it in memory.
    #[test]
    fn the_state_lives_in_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let (engine, mut m) = machine(&dir);
        let a = {
            let cmd = call(&m, "k1", 1_000, &[("a", "1"), ("b", "2")], 2);
            apply(&mut m, 1, &cmd)
        };
        assert_eq!(a.status, Status::Ok);
        assert_eq!(m.state.kv.get(b"a"), Some(b"1".to_vec()));
        // A second machine over the same store sees the same state.
        let mut again = AppMachine::new("app".into());
        again.attach(engine.group_state(9));
        again.restore(&[]).unwrap();
        assert_eq!(again.state.last_index, 1);
        assert_eq!(again.state.kv.get(b"b"), Some(b"2".to_vec()));
        assert_eq!(again.events_from(0).len(), 2);
        let caller = Caller::Account {
            account: "acct".into(),
        };
        assert!(again.replay(&caller, "k1").is_some());
        // The app's keys are not on disk in the clear.
        let mut seen = Vec::new();
        again
            .state
            .kv
            .for_each(|k, _v| seen.push(k.to_vec()))
            .unwrap();
        seen.sort();
        assert_eq!(seen, vec![b"a".to_vec(), b"b".to_vec()]);
    }

    /// Idempotency keys are forgotten once expired, without a scan of the
    /// whole table: the oldest go first.
    #[test]
    fn expired_idempotency_keys_are_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let (_engine, mut m) = machine(&dir);
        // Time alone decides here; the kept minimum is tested below.
        m.idem_limits.keep = [0, 0];
        let caller = Caller::Account {
            account: "acct".into(),
        };
        let _a = {
            let cmd = call(&m, "old", 1_000, &[], 0);
            apply(&mut m, 1, &cmd)
        };
        let _a = {
            let cmd = call(&m, "node:retry", 1_000, &[], 0);
            apply(&mut m, 2, &cmd)
        };
        // Ten minutes on, the node's own key is gone, the client's is not.
        let later = 1_000 + 11 * 60 * 1000;
        let _a = {
            let cmd = call(&m, "mid", later, &[], 0);
            apply(&mut m, 3, &cmd)
        };
        assert!(m.replay(&caller, "node:retry").is_none());
        assert!(m.replay(&caller, "old").is_some());
        // A day on, so is the client's; the newest stays.
        let day = 1_000 + IDEMPOTENCY_TTL_MS + 1;
        let _a = {
            let cmd = call(&m, "new", day, &[], 0);
            apply(&mut m, 4, &cmd)
        };
        assert!(m.replay(&caller, "old").is_none());
        assert!(m.replay(&caller, "new").is_some());
        assert!(m.replay(&caller, "mid").is_some());
        // Each ring dropped exactly what expired: "old", and the node's key.
        assert_eq!(m.state.expiry[0].first, 1);
        assert_eq!(m.state.expiry[1].first, 1);
    }

    /// D74 on wasmer: a caller's own key is still honoured at 23 hours of entry time
    /// and gone past 24; a `node:` key (a D70 client's `node:c-…`) past 10 minutes;
    /// judged by the newest applied call's timestamp, never a node's clock.
    #[test]
    fn idempotency_records_are_kept_by_time_class_on_wasmer() {
        const MIN: u64 = 60 * 1000;
        const HOUR: u64 = 60 * MIN;
        let dir = tempfile::tempdir().unwrap();
        let (_engine, mut m) = machine(&dir);
        m.idem_limits.keep = [0, 0];
        let caller = Caller::Account {
            account: "acct".into(),
        };
        let t0 = 1_000;
        for (index, key, ts) in [
            (1, "mine", t0),
            (2, "node:c-retry", t0),
            (3, "x1", t0 + 9 * MIN),
        ] {
            let cmd = call(&m, key, ts, &[], 0);
            let _a = apply(&mut m, index, &cmd);
        }
        assert!(
            m.replay(&caller, "node:c-retry").is_some(),
            "gone before 10 minutes"
        );
        let cmd = call(&m, "x2", t0 + 11 * MIN, &[], 0);
        let _a = apply(&mut m, 4, &cmd);
        assert!(
            m.replay(&caller, "node:c-retry").is_none(),
            "outlived 10 minutes"
        );
        let cmd = call(&m, "x3", t0 + 23 * HOUR, &[], 0);
        let _a = apply(&mut m, 5, &cmd);
        assert!(
            m.replay(&caller, "mine").is_some(),
            "dropped before 24 hours"
        );
        let cmd = call(&m, "x4", t0 + 24 * HOUR + 1, &[], 0);
        let _a = apply(&mut m, 6, &cmd);
        assert!(m.replay(&caller, "mine").is_none(), "outlived 24 hours");
        assert!(m.replay(&caller, "x4").is_some());
    }

    /// D74's cap on wasmer (scaled down to 3 through `Limits::cap`; the real one is
    /// 100,000 per app), spec v0.27: past it `node:` records go first, then the
    /// oldest caller record.
    #[test]
    fn the_idempotency_cap_drops_node_keys_first_then_the_oldest_on_wasmer() {
        let dir = tempfile::tempdir().unwrap();
        let (_engine, mut m) = machine(&dir);
        m.idem_limits = crate::idempotency::Limits {
            keep: [0, 0],
            cap: 3,
            ..crate::idempotency::Limits::default()
        };
        let caller = Caller::Account {
            account: "acct".into(),
        };
        let keys = ["k1", "node:k2", "k3", "node:k4", "k5"];
        for (n, key) in keys.iter().enumerate() {
            let cmd = call(&m, key, 1_000, &[], 0);
            let _a = apply(&mut m, n as u64 + 1, &cmd);
        }
        let kept: Vec<&str> = keys
            .into_iter()
            .filter(|k| m.replay(&caller, k).is_some())
            .collect();
        assert_eq!(kept, vec!["k1", "k3", "k5"]);
        // No `node:` record left: the oldest caller record goes.
        let cmd = call(&m, "k6", 1_000, &[], 0);
        let _a = apply(&mut m, 6, &cmd);
        let kept: Vec<&str> = ["k1", "k3", "k5", "k6"]
            .into_iter()
            .filter(|k| m.replay(&caller, k).is_some())
            .collect();
        assert_eq!(kept, vec!["k3", "k5", "k6"]);
    }

    /// D74 (v0.27) on wasmer: a burst of keyless calls (`node:` keys) past the cap
    /// never evicts a caller's own record, even when the caller class is above its
    /// floor: `node:` records go first.
    #[test]
    fn a_burst_of_node_keys_does_not_evict_callers_records_above_their_floor_on_wasmer() {
        let dir = tempfile::tempdir().unwrap();
        let (_engine, mut m) = machine(&dir);
        m.idem_limits = crate::idempotency::Limits {
            keep: [2, 1],
            cap: 6,
            ..crate::idempotency::Limits::default()
        };
        let caller = Caller::Account {
            account: "acct".into(),
        };
        let mine = ["c0", "c1", "c2", "c3"];
        for (n, key) in mine.iter().enumerate() {
            let cmd = call(&m, key, 1_000 + n as u64, &[], 0);
            let _a = apply(&mut m, n as u64 + 1, &cmd);
        }
        for n in 0..20u64 {
            let cmd = call(&m, &format!("node:c-{n}"), 2_000 + n, &[], 0);
            let _a = apply(&mut m, n + 5, &cmd);
        }
        for key in mine {
            assert!(
                m.replay(&caller, key).is_some(),
                "{key}: a caller's record was evicted by node: keys"
            );
        }
    }

    /// D74 (v0.26) on wasmer: the cap never takes a class below its floor; a burst
    /// of `node:` keys past the cap leaves a caller's own record to answer its retry.
    #[test]
    fn a_burst_of_node_keys_does_not_evict_a_callers_record_below_its_floor_on_wasmer() {
        let dir = tempfile::tempdir().unwrap();
        let (_engine, mut m) = machine(&dir);
        m.idem_limits = crate::idempotency::Limits {
            keep: [2, 1],
            cap: 5,
            ..crate::idempotency::Limits::default()
        };
        let caller = Caller::Account {
            account: "acct".into(),
        };
        let cmd = call(&m, "mine", 1_000, &[], 0);
        let _a = apply(&mut m, 1, &cmd);
        for n in 0..20u64 {
            let cmd = call(&m, &format!("node:c-{n}"), 1_000 + n, &[], 0);
            let _a = apply(&mut m, n + 2, &cmd);
        }
        assert!(
            m.replay(&caller, "mine").is_some(),
            "a caller's record below its floor was evicted"
        );
    }

    /// D74 (v0.26) on wasmer: one entry stamped 25 h ahead by a skewed leader can't
    /// expire what is written after it (expiry is against the earlier of the newest
    /// applied entry's timestamp and the applying entry's own).
    #[test]
    fn one_skewed_entry_does_not_expire_the_records_written_after_it_on_wasmer() {
        const MIN: u64 = 60 * 1000;
        const HOUR: u64 = 60 * MIN;
        let dir = tempfile::tempdir().unwrap();
        let (_engine, mut m) = machine(&dir);
        m.idem_limits.keep = [0, 0];
        let caller = Caller::Account {
            account: "acct".into(),
        };
        let t0 = 1_000_000;
        // The skewed entry carries no key: nothing of its own sits in the rings.
        let mut skewed = call(&m, "unused", t0 + 25 * HOUR, &[], 0);
        if let AppCmd::Call(c) = &mut skewed {
            c.idempotency_key = None;
        }
        let _a = apply(&mut m, 1, &skewed);
        for (index, key, ts) in [
            (2, "after", t0 + MIN),
            (3, "later", t0 + 2 * MIN),
            (4, "x", t0 + 3 * MIN),
        ] {
            let cmd = call(&m, key, ts, &[], 0);
            let _a = apply(&mut m, index, &cmd);
        }
        assert!(
            m.replay(&caller, "after").is_some(),
            "a record written after the skew expired"
        );
        assert!(m.replay(&caller, "later").is_some());
    }

    /// D74 (v0.26), the upgrade from a release that kept up to 1,000,000 records per
    /// class: the first entries under the new cap drop the excess a bounded number per
    /// entry (scaled down: 60 records kept under the old limits, then a cap of 10 and a
    /// step of 7), never more per entry, and the rest carries over until the app is
    /// under the cap.
    #[test]
    fn records_over_the_new_cap_drain_a_bounded_number_per_entry() {
        let dir = tempfile::tempdir().unwrap();
        let (_engine, mut m) = machine(&dir);
        m.idem_limits = crate::idempotency::Limits {
            keep: [0, 0],
            cap: 1_000_000,
            step: u64::MAX,
        };
        for n in 0..60u64 {
            let cmd = call(&m, &format!("k{n}"), 1_000 + n, &[], 0);
            let _a = apply(&mut m, n + 1, &cmd);
        }
        let held = |m: &AppMachine| m.state.expiry.iter().map(|r| r.next - r.first).sum::<u64>();
        assert_eq!(held(&m), 60);
        // The new release: a cap of 10, at most 7 dropped per entry.
        m.idem_limits = crate::idempotency::Limits {
            keep: [0, 0],
            cap: 10,
            step: 7,
        };
        let mut index = 61;
        let mut entries = 0;
        while held(&m) > 10 && entries < 50 {
            let before = held(&m);
            let mut cmd = call(&m, "unused", 2_000, &[], 0);
            if let AppCmd::Call(c) = &mut cmd {
                c.idempotency_key = None;
            }
            let _a = apply(&mut m, index, &cmd);
            assert!(
                before - held(&m) <= 7,
                "one entry dropped {} records",
                before - held(&m)
            );
            index += 1;
            entries += 1;
        }
        assert_eq!(held(&m), 10, "the excess never drained");
        assert!(entries >= 7, "drained in {entries} entries");
        // The newest stay.
        let caller = Caller::Account {
            account: "acct".into(),
        };
        assert!(m.replay(&caller, "k59").is_some());
        assert!(m.replay(&caller, "k0").is_none());
    }

    /// D74 (v0.26) on wasmer: a key's class is decided by its prefix; a caller's own
    /// key that merely contains `node:` keeps its 24 hours.
    #[test]
    fn a_callers_key_containing_node_is_kept_24_hours_on_wasmer() {
        const MIN: u64 = 60 * 1000;
        let dir = tempfile::tempdir().unwrap();
        let (_engine, mut m) = machine(&dir);
        m.idem_limits.keep = [0, 0];
        let caller = Caller::Account {
            account: "acct".into(),
        };
        let cmd = call(&m, "sync-node:42", 1_000, &[], 0);
        let _a = apply(&mut m, 1, &cmd);
        let cmd = call(&m, "x", 1_000 + 11 * MIN, &[], 0);
        let _a = apply(&mut m, 2, &cmd);
        assert!(
            m.replay(&caller, "sync-node:42").is_some(),
            "a caller's key got the node class"
        );
    }

    /// Wasmer retention is a function of the applied entries alone: two replicas
    /// fed the same entries keep the same records.
    #[test]
    fn replicas_drop_the_same_records_on_wasmer() {
        const MIN: u64 = 60 * 1000;
        let caller = Caller::Account {
            account: "acct".into(),
        };
        let keys = ["a", "node:b", "c", "node:d", "e", "f"];
        let feed = |dir: &tempfile::TempDir| {
            let (engine, mut m) = machine(dir);
            m.idem_limits = crate::idempotency::Limits {
                keep: [0, 0],
                cap: 4,
                ..crate::idempotency::Limits::default()
            };
            for (n, key) in keys.iter().enumerate() {
                let cmd = call(&m, key, 1_000 + n as u64 * 4 * MIN, &[], 0);
                let _a = apply(&mut m, n as u64 + 1, &cmd);
            }
            let kept: Vec<bool> = keys
                .iter()
                .map(|k| m.replay(&caller, k).is_some())
                .collect();
            (engine, m.state.expiry, kept)
        };
        let (d1, d2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (_e1, rings1, kept1) = feed(&d1);
        let (_e2, rings2, kept2) = feed(&d2);
        assert_eq!(rings1, rings2);
        assert_eq!(kept1, kept2);
        assert!(kept1.iter().any(|k| !k), "nothing expired: {kept1:?}");
    }

    /// Xil's review, issue 10: a leader whose clock jumped 25 hours ahead
    /// forgot an acknowledged call, and its retry ran again.
    #[test]
    fn a_skewed_clock_does_not_forget_acknowledged_calls() {
        let dir = tempfile::tempdir().unwrap();
        let (_engine, mut m) = machine(&dir);
        let caller = Caller::Account {
            account: "acct".into(),
        };
        let _a = {
            let cmd = call(&m, "pay", 1_000, &[], 0);
            apply(&mut m, 1, &cmd)
        };
        let skewed = 1_000 + 25 * 60 * 60 * 1000;
        let _a = {
            let cmd = call(&m, "other", skewed, &[], 0);
            apply(&mut m, 2, &cmd)
        };
        assert!(m.replay(&caller, "pay").is_some(), "the retry replays");
    }

    /// Events past the ring are dropped from the store, oldest first.
    #[test]
    fn the_event_ring_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let (_engine, mut m) = machine(&dir);
        let _a = {
            let cmd = call(&m, "k", 1, &[], EVENT_RING + 5);
            apply(&mut m, 1, &cmd)
        };
        let events = m.events_from(0);
        assert_eq!(events.len(), EVENT_RING);
        assert_eq!(events[0].seq, 5);
        assert_eq!(m.events_from(2).len(), 0);
    }

    /// The previous storage format kept an app as one JSON document; a node
    /// migrating from it brings every part in.
    #[test]
    fn a_legacy_app_state_is_imported() {
        let dir = tempfile::tempdir().unwrap();
        let (engine, mut m) = machine(&dir);
        let legacy = serde_json::json!({
            "kv": { "YQ==": "MQ==" },
            "bundle": "abc",
            "initialized": true,
            "init_error": null,
            "last_index": 41,
            "idempotency": {
                "acct|k": {
                    "applied": { "index": 40, "status": "ok", "result": 7, "error": null, "events": [] },
                    "expires_ms": 9_000_000
                }
            },
            "events": [{ "index": 40, "seq": 0, "kind": "set", "data": "a" }],
            "pending": {},
            "next_pending": 3,
            "timer_ticks": { "tick": 5 }
        });
        let state = engine.group_state(9);
        let mut tx = StateTx::new(&state);
        m.import(&serde_json::to_vec(&legacy).unwrap(), &mut tx)
            .unwrap();
        tx.commit().unwrap();
        let mut back = AppMachine::new("app".into());
        back.attach(engine.group_state(9));
        back.restore(&[]).unwrap();
        assert_eq!(back.state.kv.get(b"a"), Some(b"1".to_vec()));
        assert_eq!(back.state.bundle.as_deref(), Some("abc"));
        assert!(back.state.initialized);
        assert_eq!(back.state.last_index, 41);
        assert_eq!(back.state.next_pending, 3);
        assert_eq!(back.state.timer_ticks.get("tick"), Some(&5));
        assert_eq!(back.events_from(0).len(), 1);
        let caller = Caller::Account {
            account: "acct".into(),
        };
        assert_eq!(
            back.replay(&caller, "k").map(|a| a.result),
            Some(Value::from(7))
        );
    }

    #[test]
    fn a_cells_writers_come_from_the_apps_own_state() {
        use calimero_storage::shared_writers::CellWriters;

        let cell = [7_u8; 32];
        let base = BTreeMap::new();
        let fresh = Overlay::new(&base);
        assert_eq!(fresh.shared_writers(&cell), Some(CellWriters::Genesis));

        // A stored set (as a run's rotation writes it) is what the next run reads.
        let mut state = BTreeMap::new();
        let writers = calimero_storage::shared_writers::Writers::default();
        let _prev = state.insert(
            Bytes(writers_key(&cell)),
            Bytes(borsh::to_vec(&writers).unwrap()),
        );
        let later = Overlay::new(&state);
        assert_eq!(
            later.shared_writers(&cell),
            Some(CellWriters::Rotated(writers))
        );
    }
}
