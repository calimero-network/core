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
//! applies a component's diff to a wasmer app's state or the other way round.
//!
//! Storage (D46): the component's keys are hidden rows of the group's state (sealed,
//! keyed by an HMAC, as a wasmer app's are), and a replica keeps them in memory too,
//! ordered, as an `Arc<BTreeMap>`: a run gets the current version as its
//! `StateView` (ordered `range` scans), and `apply` copies it only while a run still
//! holds the previous one. The map is rebuilt from the rows on a restart and after a
//! snapshot install. Host records (`last_id`, `timers`, `install_ts_ms`,
//! `last_ts_ms`, idempotency) are never keys the app sees (D39).

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Weak};

use calimero_app_host as host;
use calimero_log::{Bytes, GroupState, StateTx};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::{rows, AppMachine, Status, Write};
use crate::node::Node;
use crate::types::{AppId, Audience, Caller, EventRecord};

/// Idempotency records are kept for this many log entries of the app (D16).
pub const IDEMPOTENCY_ENTRIES: u64 = 100_000;

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
}

/// The name a trap kind has on the wire (the spec's names).
#[must_use]
pub fn trap_name(kind: host::TrapKind) -> &'static str {
    match kind {
        host::TrapKind::OutOfFuel => "OutOfFuel",
        host::TrapKind::Timeout => "Timeout",
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
        let mut outcome = match out.status {
            host::Status::Ok => Self::ok(out.result, out.diff, out.events),
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
        outcome
    }

    fn ok(result: Vec<u8>, diff: Vec<host::Write>, events: Vec<host::Event>) -> Self {
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
        }
    }

    /// Whether the run hit its wall-clock deadline (or found no instance): never
    /// logged, the idempotency key not spent (D23).
    #[must_use]
    pub fn is_timeout(&self) -> bool {
        self.status == Status::Trap && self.error["kind"] == trap_name(host::TrapKind::Timeout)
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
    Upgrade { bundle: String, base_index: u64 },
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
    /// The run timed out: nothing was logged (D23); answered `503 TIMEOUT`.
    #[serde(default)]
    pub timeout: bool,
}

impl Applied {
    /// An answer for a run that was not logged.
    #[must_use]
    pub fn unlogged(index: u64, outcome: Outcome) -> Self {
        Self {
            index,
            timeout: outcome.is_timeout(),
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

#[derive(Clone, Debug, Serialize, Deserialize)]
struct IdemRecord {
    applied: Applied,
}

/// An app's stored keys and values, shared between the machine and the runs reading it.
pub type Kv = Arc<BTreeMap<Vec<u8>, Vec<u8>>>;

/// The ring of a component's idempotency keys, oldest first (`rows::corder`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ring {
    pub first: u64,
    pub next: u64,
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
    /// The highest `(id_term, id_seq)` any logged run carried, conflicted ones
    /// included (D52, Ruling 29): a node leading the same term again after a
    /// restart continues after it instead of reusing an id.
    #[serde(default)]
    pub last_id: (u64, u64),
    /// The timestamp of the applied `init` entry: timer slots are aligned to it (D34).
    #[serde(default)]
    pub install_ts_ms: u64,
    /// Each timer's last applied slot (D34).
    #[serde(default)]
    pub timers: BTreeMap<String, u64>,
    /// The idempotency keys in the order they were recorded.
    #[serde(default)]
    pub idempotency: Ring,
}

/// A component app's state on a replica: its host records, and its storage in
/// memory (the rows on disk are the durable copy).
#[derive(Clone, Debug, Default)]
pub struct ComponentState {
    pub head: ComponentHead,
    /// The app's data, stored keys (generation prefix included). Shared with
    /// running calls and views as their `StateView` (D46).
    pub kv: Kv,
}

impl ComponentState {
    /// Read the app's storage rows into memory (after a restart or a snapshot install).
    pub fn load(&mut self, state: &GroupState) -> eyre::Result<()> {
        let mut kv = BTreeMap::new();
        state.scan_hidden(rows::CKV, |k, v| {
            let _prev = kv.insert(k.to_vec(), v.to_vec());
            Ok(true)
        })?;
        self.kv = Arc::new(kv);
        Ok(())
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
        caller: Option<&str>,
    ) -> Applied {
        if base_index != self.state.last_index {
            return Applied::conflict(index, "conflict: retry");
        }
        let c = &mut self.state.component;
        c.head.last_ts_ms = c.head.last_ts_ms.max(ts_ms);
        if outcome.status == Status::Ok {
            if !outcome.diff.is_empty() {
                let kv = Arc::make_mut(&mut c.kv);
                for w in &outcome.diff {
                    match &w.value {
                        Some(v) => {
                            tx.put_hidden(rows::CKV, &w.key.0, &v.0);
                            let _prev = kv.insert(w.key.0.clone(), v.0.clone());
                        }
                        None => {
                            tx.delete_hidden(rows::CKV, &w.key.0);
                            let _prev = kv.remove(&w.key.0);
                        }
                    }
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

    fn remember_component(
        &mut self,
        tx: &mut StateTx<'_>,
        index: u64,
        caller: &Caller,
        key: &str,
        applied: &Applied,
    ) {
        let id = idem_key(caller, key);
        let record = IdemRecord {
            applied: applied.clone(),
        };
        match serde_json::to_vec(&record) {
            Ok(v) => tx.put_hidden(rows::CIDEM, id.as_bytes(), &v),
            Err(e) => tx.fail(e),
        }
        let ring = &mut self.state.component.head.idempotency;
        match serde_json::to_vec(&(index, id)) {
            Ok(v) => tx.put(rows::corder(ring.next), v),
            Err(e) => tx.fail(e),
        }
        ring.next += 1;
    }

    /// Forget records older than the last [`IDEMPOTENCY_ENTRIES`] entries.
    fn expire_component(&mut self, tx: &mut StateTx<'_>, index: u64) {
        let ring = &mut self.state.component.head.idempotency;
        while ring.first < ring.next {
            let slot = rows::corder(ring.first);
            let Some((at, key)) = tx
                .get(&slot)
                .and_then(|raw| serde_json::from_slice::<(u64, String)>(&raw).ok())
            else {
                tx.delete(slot);
                ring.first += 1;
                continue;
            };
            if at + IDEMPOTENCY_ENTRIES > index {
                break;
            }
            // A key reused later has a newer record; keep that one.
            let current = tx
                .get_hidden(rows::CIDEM, key.as_bytes())
                .and_then(|r| serde_json::from_slice::<IdemRecord>(&r).ok())
                .is_some_and(|r| r.applied.index == at);
            if current {
                tx.delete_hidden(rows::CIDEM, key.as_bytes());
            }
            tx.delete(slot);
            ring.first += 1;
        }
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
            None,
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

    /// Apply one entry of a component app. An entry that is not a component's
    /// (a wasmer app's format) changes nothing.
    pub(crate) fn apply_component(
        &mut self,
        tx: &mut StateTx<'_>,
        index: u64,
        command: &[u8],
    ) -> Vec<u8> {
        let cmd = serde_json::from_slice::<ComponentCmd>(command);
        match &cmd {
            Ok(ComponentCmd::Init {
                id_term, id_seq, ..
            }) => self.note_id(*id_term, *id_seq),
            Ok(ComponentCmd::Call(call)) => self.note_id(call.id_term, call.id_seq),
            Ok(ComponentCmd::Tick(tick)) => self.note_id(tick.id_term, tick.id_seq),
            _ => {}
        }
        let applied = match cmd {
            Ok(ComponentCmd::Init {
                bundle,
                base_index,
                ts_ms,
                outcome,
                ..
            }) => {
                if self.state.bundle.is_some() {
                    Applied::conflict(index, "already initialized")
                } else {
                    let a =
                        self.apply_component_outcome(tx, index, base_index, ts_ms, &outcome, None);
                    if !a.conflict {
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
            Ok(ComponentCmd::Upgrade { bundle, base_index }) => {
                let a = self.apply_component_outcome(
                    tx,
                    index,
                    base_index,
                    0,
                    &Outcome::default(),
                    None,
                );
                if !a.conflict {
                    self.state.bundle = Some(bundle);
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
                    call.caller.account(),
                );
                if let (Some(key), false) = (&call.idempotency_key, a.conflict) {
                    self.remember_component(tx, index, &call.caller, key, &a);
                }
                a
            }
            Ok(ComponentCmd::Tick(tick)) => self.apply_tick(tx, index, &tick),
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
        self.expire_component(tx, index);
        serde_json::to_vec(&applied).unwrap_or_default()
    }
}

/// The ticks a replica applied, newest last (feature `testing`).
#[cfg(feature = "testing")]
pub type Ticks = VecDeque<TickApplied>;

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

    fn orgs_of(&self, account: &str) -> Vec<(String, host::OrgRole)> {
        let Some(node) = self.node.upgrade() else {
            return Vec::new();
        };
        node.sys(|s| {
            s.orgs
                .iter()
                .filter_map(|(id, o)| o.role(account).map(|r| (id.clone(), host_role(r))))
                .collect()
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

    #[test]
    fn idempotency_records_are_kept_for_a_number_of_entries() {
        let mut r = Rig::new();
        let caller = Caller::Account {
            account: "a".into(),
        };
        let _i = r.apply(1, &init(0));
        let _a = r.apply(2, &call(1, Some("k"), Vec::new()));
        assert!(r.machine.component_replay(&caller, "k").is_some());
        // Entries without keys: the record lives until IDEMPOTENCY_ENTRIES later.
        r.machine.state.last_index = IDEMPOTENCY_ENTRIES + 1;
        let _a = r.apply(
            IDEMPOTENCY_ENTRIES + 1,
            &call(IDEMPOTENCY_ENTRIES + 1, None, Vec::new()),
        );
        assert!(r.machine.component_replay(&caller, "k").is_some());
        let _a = r.apply(
            IDEMPOTENCY_ENTRIES + 2,
            &call(IDEMPOTENCY_ENTRIES + 1, None, Vec::new()),
        );
        assert!(r.machine.component_replay(&caller, "k").is_none());
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
    fn a_wasmer_entry_never_applies_to_a_component_app() {
        let mut r = Rig::new();
        let _i = r.apply(1, &init(0));
        let v1 = serde_json::to_vec(&crate::app::AppCmd::BlobForget { id: "00".into() }).unwrap();
        let out = r.apply(2, &v1);
        assert!(out.conflict, "a wasmer entry applied to a component app");
    }

    #[test]
    fn only_a_component_init_decides_the_executor() {
        assert!(ComponentCmd::is_init(&init(0)));
        assert!(!ComponentCmd::is_init(&call(0, None, Vec::new())));
        let v1 = serde_json::to_vec(&crate::app::AppCmd::Init {
            bundle: "b".into(),
            base_index: 0,
            outcome: crate::app::Outcome::default(),
        })
        .unwrap();
        assert!(!ComponentCmd::is_init(&v1));
    }
}
