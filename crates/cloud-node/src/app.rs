//! An app group: the app's state, its idempotency table and recent events.
//!
//! The leader executes a call against the applied state and appends the
//! result as one entry: `{ call, outcome { result, diff, events } }`. Every
//! replica applies the diff; none re-executes.

use calimero_storage::shared_writers::CellWriters;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use calimero_log::{Bytes, Machine};
use calimero_runtime::store::{Key, Storage, Value as StoreValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::types::{now_ms, AppId, Caller, EventRecord};

/// How long an idempotency key is remembered.
pub const IDEMPOTENCY_TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// Keys the node itself gives a call that came without one, so a retried
/// forward replays instead of running twice. They only need to outlive the
/// retries, so they are kept briefly.
pub const NODE_KEY_PREFIX: &str = "node:";
const NODE_KEY_TTL_MS: u64 = 10 * 60 * 1000;
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

/// How deep a cascade of handlers and xcalls may go (v1's `MAX_XCALL_DEPTH`):
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
    /// The applied index the leader executed against.
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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub kv: BTreeMap<Bytes, Bytes>,
    pub bundle: Option<String>,
    pub initialized: bool,
    /// Why `init` failed, if it did (`{ status, error }`).
    #[serde(default)]
    pub init_error: Option<Value>,
    pub last_index: u64,
    idempotency: BTreeMap<String, IdemRecord>,
    pub events: VecDeque<EventRecord>,
    /// Follow-ups the leader has yet to run, by sequence number.
    #[serde(default)]
    pub pending: BTreeMap<u64, Pending>,
    #[serde(default)]
    pub next_pending: u64,
    /// The last tick fired per timer method.
    #[serde(default)]
    pub timer_ticks: BTreeMap<String, u64>,
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
        }
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

    /// Whether `id` (hex) is a blob of this app.
    #[must_use]
    pub fn holds_blob(&self, id: &[u8; 32]) -> bool {
        self.state.kv.contains_key(&Bytes(blob_key(id)))
    }

    /// A previous answer for this idempotency key, if still remembered.
    #[must_use]
    pub fn replay(&self, caller: &Caller, key: &str, now: u64) -> Option<Applied> {
        self.state
            .idempotency
            .get(&Self::idem_key(caller, key))
            .filter(|r| r.expires_ms > now)
            .map(|r| r.applied.clone())
    }

    fn apply_outcome(
        &mut self,
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
                    Some(v) => {
                        let _prev = self.state.kv.insert(w.key.clone(), v.clone());
                    }
                    None => {
                        let _prev = self.state.kv.remove(&w.key);
                    }
                }
            }
            for (seq, (kind, data)) in outcome.events.iter().enumerate() {
                let rec = EventRecord {
                    index,
                    seq: u32::try_from(seq).unwrap_or(u32::MAX),
                    kind: kind.clone(),
                    data: data.clone(),
                };
                self.state.events.push_back(rec.clone());
                if self.state.events.len() > EVENT_RING {
                    let _old = self.state.events.pop_front();
                }
                let _receivers = self.events_tx.send(rec);
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
    fn apply(&mut self, index: u64, command: &[u8]) -> Vec<u8> {
        let applied = match serde_json::from_slice::<AppCmd>(command) {
            Ok(AppCmd::Init {
                bundle,
                base_index,
                outcome,
            }) => {
                let a = self.apply_outcome(index, base_index, &outcome, 0);
                if !a.conflict {
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
                let a = self.apply_outcome(index, base_index, &outcome, 0);
                if !a.conflict && outcome.status == Status::Ok {
                    self.state.bundle = Some(bundle);
                }
                a
            }
            Ok(AppCmd::Blob { id, size }) => {
                if let Ok(raw) = hex::decode(&id) {
                    if let Ok(id) = <[u8; 32]>::try_from(raw.as_slice()) {
                        let _prev = self
                            .state
                            .kv
                            .insert(Bytes(blob_key(&id)), Bytes(size.to_le_bytes().to_vec()));
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
                let a = self.apply_outcome(index, call.base_index, &call.outcome, depth);
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
                if let (Some(key), false) = (&call.idempotency_key, a.conflict) {
                    let _prev = self.state.idempotency.insert(
                        Self::idem_key(&call.caller, key),
                        IdemRecord {
                            applied: a.clone(),
                            expires_ms: call.ts_ms
                                + if key.starts_with(NODE_KEY_PREFIX) {
                                    NODE_KEY_TTL_MS
                                } else {
                                    IDEMPOTENCY_TTL_MS
                                },
                        },
                    );
                    // Forget expired keys, judged by the log's own clock.
                    let now = call.ts_ms;
                    self.state.idempotency.retain(|_, r| r.expires_ms > now);
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
        self.state.last_index = index;
        serde_json::to_vec(&applied).unwrap_or_default()
    }

    fn snapshot(&self) -> Vec<u8> {
        serde_json::to_vec(&self.state).unwrap_or_default()
    }

    fn restore(&mut self, data: &[u8]) -> eyre::Result<()> {
        self.state = serde_json::from_slice(data)?;
        self.touched.clear();
        self.touched_since = u64::MAX;
        Ok(())
    }
}

/// Copy-on-write view of an app's state, recording writes in order.
pub struct Overlay<'a> {
    base: &'a BTreeMap<Bytes, Bytes>,
    writes: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    order: Vec<Vec<u8>>,
    /// Prepended to every key: empty for the app's state, an account's space
    /// for its private storage ([`private_prefix`]).
    prefix: Vec<u8>,
}

/// Where a shared-storage cell's current writer set lives in the app's state.
///
/// v1 recorded a cell's writer rotations as group governance ops. A cloud app
/// has one log already, so the rotation a run asks for is written beside the
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

/// Where an account's private storage lives in the app's state.
///
/// v1 kept "private" data on the member's own node, never synced. A cloud app
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
    pub fn new(base: &'a BTreeMap<Bytes, Bytes>) -> Self {
        Self::with_prefix(base, Vec::new())
    }

    /// An overlay that reads and writes only keys under `prefix`.
    #[must_use]
    pub fn with_prefix(base: &'a BTreeMap<Bytes, Bytes>, prefix: Vec<u8>) -> Self {
        Self {
            base,
            writes: BTreeMap::new(),
            order: Vec::new(),
            prefix,
        }
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
        let key = self.full(key);
        match self.writes.get(&key) {
            Some(v) => v.clone(),
            None => self.base.get(&Bytes(key)).map(|v| v.0.clone()),
        }
    }

    fn set(&mut self, key: Key, value: StoreValue) -> Option<StoreValue> {
        let prev = self.get(&key);
        let key = self.full(&key);
        self.order.push(key.clone());
        let _prev = self.writes.insert(key, Some(value));
        prev
    }

    fn remove(&mut self, key: &Key) -> Option<Vec<u8>> {
        let prev = self.get(key);
        let key = self.full(key);
        self.order.push(key.clone());
        let _prev = self.writes.insert(key, None);
        prev
    }

    fn has(&self, key: &Key) -> bool {
        self.get(key).is_some()
    }

    fn shared_writers(&self, cell: &[u8; 32]) -> Option<CellWriters> {
        let key = writers_key(cell);
        let raw = match self.writes.get(&key) {
            Some(v) => v.clone(),
            None => self.base.get(&Bytes(key)).map(|v| v.0.clone()),
        };
        match raw {
            None => Some(CellWriters::Genesis),
            Some(bytes) => borsh::from_slice(&bytes).ok().map(CellWriters::Rotated),
        }
    }
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
}

/// The storage entities a run's artifact (a borsh `StorageDelta`) touches, and
/// every entry above them, hex: what search re-extracts. Empty for anything
/// that does not decode. (v1: `calimero_context::search::changed_entity_ids`.)
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

    /// Run `method` against `base` as `caller`, with JSON `args`. Never fails:
    /// errors become the outcome's status.
    #[must_use]
    pub fn run(
        module: &calimero_runtime::Module,
        app_id: &str,
        caller: &Caller,
        method: &str,
        args: &Value,
        base: &BTreeMap<Bytes, Bytes>,
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
        base: &BTreeMap<Bytes, Bytes>,
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
        // release-v2 has no devices: a person is their account, so the executor
        // (v1's per-device key, `env::executor_id`) is the account too. Apps that
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
            },
        );
        let mut new_blobs = Vec::new();
        if let Ok(out) = &result {
            if out.returns.is_ok() {
                // Rotations of shared cells' writers take effect with the run's writes.
                for r in &out.shared_rotations {
                    if let Ok(bytes) = borsh::to_vec(&r.new) {
                        let _prev = overlay.set(writers_key(r.cell.as_bytes()), bytes);
                    }
                }
                // So are the blobs it wrote, and they are this app's.
                for (id, bytes) in &out.new_blobs {
                    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                    let _prev = overlay.set(blob_key(id), size.to_le_bytes().to_vec());
                    new_blobs.push((hex::encode(id), bytes.clone()));
                }
            }
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
