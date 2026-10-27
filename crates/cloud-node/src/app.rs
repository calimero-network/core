//! An app group: the app's state, its idempotency table and recent events.
//!
//! The leader runs a call on the engine (`calimero:app`, wasmtime) against the
//! applied state and appends the result as one entry: `{ call, outcome { result,
//! diff, events } }`. Every replica applies the diff; none re-executes.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Weak};

use calimero_app_host as host;
use calimero_log::{Bytes, Machine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::node::Node;
use crate::types::{AppId, Audience, Caller, EventRecord};

/// Keys the node itself gives a call that came without one, so a retried
/// forward replays instead of running twice.
pub const NODE_KEY_PREFIX: &str = "node:";
/// Events kept for resuming subscriptions.
pub const EVENT_RING: usize = 10_000;
/// Idempotency records are kept for this many log entries of the app (D16).
pub const IDEMPOTENCY_ENTRIES: u64 = 100_000;

/// One storage write; `key` is the stored key, generation prefix included (D39).
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
    /// The app refused the call; `error` is `{ code, fields }`.
    AppError,
    /// The run trapped; `error` is `{ kind, detail }` (spec section 5).
    Trap,
}

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
    /// The applied index the leader executed against.
    pub base_index: u64,
    /// The leader's term and per-term execution counter the run's unique ids
    /// were made from (D52).
    pub id_term: u64,
    pub id_seq: u64,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum AppCmd {
    /// The first entry: `init` ran.
    Init {
        bundle: String,
        base_index: u64,
        ts_ms: u64,
        /// The ids `init` ran with (D52), as for a call.
        id_term: u64,
        id_seq: u64,
        outcome: Box<Outcome>,
    },
    Call(Box<CallEntry>),
    /// Switch to a new bundle, code only (migrations are rebuilds, D38, not in
    /// this milestone).
    Upgrade {
        bundle: String,
        base_index: u64,
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct IdemRecord {
    applied: Applied,
}

/// An app's stored keys and values, shared between the machine and the runs reading it.
pub type Kv = Arc<BTreeMap<Vec<u8>, Vec<u8>>>;

/// The app's key-value state in the snapshot: base64 keys and values.
mod kv_serde {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use calimero_log::Bytes;
    use serde::{Deserialize, Deserializer, Serializer};

    use super::Kv;

    pub fn serialize<S: Serializer>(kv: &Kv, s: S) -> Result<S::Ok, S::Error> {
        s.collect_map(kv.iter().map(|(k, v)| (Bytes(k.clone()), Bytes(v.clone()))))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Kv, D::Error> {
        let m = BTreeMap::<Bytes, Bytes>::deserialize(d)?;
        Ok(Arc::new(m.into_iter().map(|(k, v)| (k.0, v.0)).collect()))
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    /// The app's data, stored keys (generation prefix included). Shared with
    /// running calls and views as their `StateView` (D46); `apply` copies it only
    /// while a run still holds the previous version (`Arc::make_mut`).
    #[serde(with = "kv_serde", default)]
    pub kv: Kv,
    pub bundle: Option<String>,
    pub initialized: bool,
    /// Why `init` failed, if it did (`{ status, error }`).
    #[serde(default)]
    pub init_error: Option<Value>,
    pub last_index: u64,
    /// The newest entry timestamp applied (D16): the next entry is stamped
    /// after it, and views see it (D19).
    #[serde(default)]
    pub last_ts_ms: u64,
    /// The highest `(id_term, id_seq)` any logged run carried, conflicted ones
    /// included (D52, Ruling 29): a node leading the same term again after a
    /// restart continues after it instead of reusing an id.
    #[serde(default)]
    pub last_id: (u64, u64),
    /// Host records, never in `kv` (D39).
    #[serde(default)]
    idempotency: BTreeMap<String, IdemRecord>,
    /// `(index, key)` of each record, oldest first, for retention by entries.
    #[serde(default)]
    idempotency_order: VecDeque<(u64, String)>,
    pub events: VecDeque<EventRecord>,
}

/// The app group's state machine.
pub struct AppMachine {
    pub app_id: AppId,
    pub state: AppState,
    pub events_tx: broadcast::Sender<EventRecord>,
}

impl AppMachine {
    #[must_use]
    pub fn new(app_id: AppId) -> Self {
        let (events_tx, _) = broadcast::channel(1024);
        Self {
            app_id,
            state: AppState::default(),
            events_tx,
        }
    }

    fn idem_key(caller: &Caller, key: &str) -> String {
        match caller {
            Caller::App { app } => format!("app:{app}|{key}"),
            Caller::System => format!("system|{key}"),
            _ => format!("{}|{key}", caller.account().unwrap_or("public")),
        }
    }

    /// A previous answer for this idempotency key, if still remembered.
    #[must_use]
    pub fn replay(&self, caller: &Caller, key: &str) -> Option<Applied> {
        self.state
            .idempotency
            .get(&Self::idem_key(caller, key))
            .map(|r| r.applied.clone())
    }

    /// The state a run reads (D46): the current version, shared.
    #[must_use]
    pub fn view(&self) -> Arc<dyn host::StateView> {
        Arc::clone(&self.state.kv) as Arc<dyn host::StateView>
    }

    fn apply_outcome(
        &mut self,
        index: u64,
        base_index: u64,
        ts_ms: u64,
        outcome: &Outcome,
        caller: Option<&str>,
    ) -> Applied {
        let conflict = base_index != self.state.last_index;
        if !conflict {
            self.state.last_ts_ms = self.state.last_ts_ms.max(ts_ms);
            if outcome.status == Status::Ok && !outcome.diff.is_empty() {
                let kv = Arc::make_mut(&mut self.state.kv);
                for w in &outcome.diff {
                    let _prev = match &w.value {
                        Some(v) => kv.insert(w.key.0.clone(), v.0.clone()),
                        None => kv.remove(&w.key.0),
                    };
                }
            }
            if outcome.status == Status::Ok {
                for (seq, e) in outcome.events.iter().enumerate() {
                    let rec = EventRecord {
                        index,
                        seq: u32::try_from(seq).unwrap_or(u32::MAX),
                        kind: e.kind.clone(),
                        data: e.data.clone(),
                        audience: e.audience.clone(),
                        caller: caller.map(str::to_owned),
                    };
                    self.state.events.push_back(rec.clone());
                    if self.state.events.len() > EVENT_RING {
                        let _old = self.state.events.pop_front();
                    }
                    let _receivers = self.events_tx.send(rec);
                }
            }
        }
        if conflict {
            return Applied {
                index,
                status: Status::Trap,
                result: Value::Null,
                error: json!({ "kind": "Conflict", "detail": "conflict: retry" }),
                events: Vec::new(),
                conflict: true,
                timeout: false,
            };
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

    fn remember(&mut self, index: u64, caller: &Caller, key: &str, applied: &Applied) {
        let id = Self::idem_key(caller, key);
        let _prev = self.state.idempotency.insert(
            id.clone(),
            IdemRecord {
                applied: applied.clone(),
            },
        );
        self.state.idempotency_order.push_back((index, id));
    }

    fn note_id(&mut self, term: u64, seq: u64) {
        self.state.last_id = self.state.last_id.max((term, seq));
    }

    /// Forget records older than the last [`IDEMPOTENCY_ENTRIES`] entries.
    fn expire(&mut self, index: u64) {
        while let Some((at, _)) = self.state.idempotency_order.front() {
            if at + IDEMPOTENCY_ENTRIES > index {
                break;
            }
            if let Some((at, key)) = self.state.idempotency_order.pop_front() {
                // A key reused later has a newer record; keep that one.
                if self
                    .state
                    .idempotency
                    .get(&key)
                    .is_some_and(|r| r.applied.index == at)
                {
                    let _gone = self.state.idempotency.remove(&key);
                }
            }
        }
    }
}

impl Machine for AppMachine {
    fn apply(&mut self, index: u64, command: &[u8]) -> Vec<u8> {
        let cmd = serde_json::from_slice::<AppCmd>(command);
        match &cmd {
            Ok(AppCmd::Init {
                id_term, id_seq, ..
            }) => self.note_id(*id_term, *id_seq),
            Ok(AppCmd::Call(call)) => self.note_id(call.id_term, call.id_seq),
            _ => {}
        }
        let applied = match cmd {
            Ok(AppCmd::Init {
                bundle,
                base_index,
                ts_ms,
                outcome,
                ..
            }) => {
                let a = self.apply_outcome(index, base_index, ts_ms, &outcome, None);
                if !a.conflict {
                    self.state.bundle = Some(bundle);
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
            Ok(AppCmd::Upgrade { bundle, base_index }) => {
                let a = self.apply_outcome(index, base_index, 0, &Outcome::default(), None);
                if !a.conflict {
                    self.state.bundle = Some(bundle);
                }
                a
            }
            Ok(AppCmd::Call(call)) => {
                let a = self.apply_outcome(
                    index,
                    call.base_index,
                    call.ts_ms,
                    &call.outcome,
                    call.caller.account(),
                );
                if let (Some(key), false) = (&call.idempotency_key, a.conflict) {
                    self.remember(index, &call.caller, key, &a);
                }
                a
            }
            Err(e) => Applied {
                index,
                status: Status::Trap,
                result: Value::Null,
                error: json!({ "kind": "Fault", "detail": format!("malformed entry: {e}") }),
                events: Vec::new(),
                conflict: true,
                timeout: false,
            },
        };
        self.expire(index);
        self.state.last_index = index;
        serde_json::to_vec(&applied).unwrap_or_default()
    }

    fn snapshot(&self) -> Vec<u8> {
        serde_json::to_vec(&self.state).unwrap_or_default()
    }

    fn restore(&mut self, data: &[u8]) -> eyre::Result<()> {
        self.state = serde_json::from_slice(data)?;
        Ok(())
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn call(index_base: u64, key: Option<&str>, diff: Vec<Write>) -> Vec<u8> {
        serde_json::to_vec(&AppCmd::Call(Box::new(CallEntry {
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

    #[test]
    fn the_highest_id_is_kept_even_from_a_conflicted_entry() {
        let mut m = AppMachine::new("app".into());
        let mut cmd: AppCmd = serde_json::from_slice(&call(0, None, Vec::new())).unwrap();
        if let AppCmd::Call(c) = &mut cmd {
            c.id_term = 5;
            c.id_seq = 9;
            c.base_index = 7; // not the applied index: a conflict
        }
        let out: Applied =
            serde_json::from_slice(&m.apply(1, &serde_json::to_vec(&cmd).unwrap())).unwrap();
        assert!(out.conflict);
        assert_eq!(m.state.last_id, (5, 9));
        let _a = m.apply(2, &call(1, None, Vec::new()));
        assert_eq!(m.state.last_id, (5, 9), "a lower id does not lower it");
    }

    #[test]
    fn a_view_keeps_its_version_while_apply_moves_on() {
        let mut m = AppMachine::new("app".into());
        let w = |v: &str| Write {
            key: Bytes(b"k".to_vec()),
            value: Some(Bytes(v.as_bytes().to_vec())),
        };
        let _a = m.apply(1, &call(0, None, vec![w("1")]));
        let held = m.view();
        let _a = m.apply(2, &call(1, None, vec![w("2")]));
        assert_eq!(held.get(b"k"), Some(b"1".to_vec()));
        assert_eq!(m.view().get(b"k"), Some(b"2".to_vec()));
    }

    #[test]
    fn idempotency_records_are_kept_for_a_number_of_entries() {
        let mut m = AppMachine::new("app".into());
        let caller = Caller::Account {
            account: "a".into(),
        };
        let _a = m.apply(1, &call(0, Some("k"), Vec::new()));
        assert!(m.replay(&caller, "k").is_some());
        // Entries without keys: the record lives until IDEMPOTENCY_ENTRIES later.
        m.state.last_index = IDEMPOTENCY_ENTRIES;
        let _a = m.apply(
            IDEMPOTENCY_ENTRIES,
            &call(IDEMPOTENCY_ENTRIES, None, Vec::new()),
        );
        assert!(m.replay(&caller, "k").is_some());
        let _a = m.apply(
            IDEMPOTENCY_ENTRIES + 1,
            &call(IDEMPOTENCY_ENTRIES, None, Vec::new()),
        );
        assert!(m.replay(&caller, "k").is_none());
    }
}
