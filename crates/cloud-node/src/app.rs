//! An app group: the app's state, its idempotency table and recent events.
//!
//! The leader executes a call against the applied state and appends the
//! result as one entry: `{ call, outcome { result, diff, events } }`. Every
//! replica applies the diff; none re-executes.

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
/// Events kept for resuming subscriptions.
pub const EVENT_RING: usize = 10_000;

/// One storage write.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Write {
    pub key: Bytes,
    /// `None` deletes the key.
    pub value: Option<Bytes>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Ok,
    /// The app refused the call; `error` holds its error value.
    AppError,
    /// The call trapped (out of gas, panic, unknown method).
    Trap,
}

/// What executing a call produced.
#[derive(Clone, Debug, Serialize, Deserialize)]
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
    pub last_index: u64,
    idempotency: BTreeMap<String, IdemRecord>,
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
        format!("{}|{key}", caller.account().unwrap_or("public"))
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

    fn apply_outcome(&mut self, index: u64, base_index: u64, outcome: &Outcome) -> Applied {
        let conflict = base_index != self.state.last_index;
        if !conflict {
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
                let a = self.apply_outcome(index, base_index, &outcome);
                if !a.conflict {
                    self.state.bundle = Some(bundle);
                    self.state.initialized = outcome.status == Status::Ok;
                }
                a
            }
            Ok(AppCmd::Upgrade {
                bundle,
                base_index,
                outcome,
            }) => {
                let a = self.apply_outcome(index, base_index, &outcome);
                if !a.conflict && outcome.status == Status::Ok {
                    self.state.bundle = Some(bundle);
                }
                a
            }
            Ok(AppCmd::Call(call)) => {
                let a = self.apply_outcome(index, call.base_index, &call.outcome);
                if let (Some(key), false) = (&call.idempotency_key, a.conflict) {
                    let _prev = self.state.idempotency.insert(
                        Self::idem_key(&call.caller, key),
                        IdemRecord {
                            applied: a.clone(),
                            expires_ms: call.ts_ms + IDEMPOTENCY_TTL_MS,
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
        Ok(())
    }
}

/// Copy-on-write view of an app's state, recording writes in order.
pub struct Overlay<'a> {
    base: &'a BTreeMap<Bytes, Bytes>,
    writes: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    order: Vec<Vec<u8>>,
}

impl<'a> Overlay<'a> {
    #[must_use]
    pub fn new(base: &'a BTreeMap<Bytes, Bytes>) -> Self {
        Self {
            base,
            writes: BTreeMap::new(),
            order: Vec::new(),
        }
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
        match self.writes.get(key) {
            Some(v) => v.clone(),
            None => self.base.get(&Bytes(key.clone())).map(|v| v.0.clone()),
        }
    }

    fn set(&mut self, key: Key, value: StoreValue) -> Option<StoreValue> {
        let prev = self.get(&key);
        self.order.push(key.clone());
        let _prev = self.writes.insert(key, Some(value));
        prev
    }

    fn remove(&mut self, key: &Key) -> Option<Vec<u8>> {
        let prev = self.get(key);
        self.order.push(key.clone());
        let _prev = self.writes.insert(key.clone(), None);
        prev
    }

    fn has(&self, key: &Key) -> bool {
        self.get(key).is_some()
    }
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

    /// Run `method` against `base` as `caller`. Never fails: errors become the
    /// outcome's status.
    #[must_use]
    pub fn run(
        module: &calimero_runtime::Module,
        app_id: &str,
        caller: &Caller,
        method: &str,
        args: &Value,
        base: &BTreeMap<Bytes, Bytes>,
    ) -> Outcome {
        let mut overlay = Overlay::new(base);
        let input = if args.is_null() {
            Vec::new()
        } else {
            serde_json::to_vec(args).unwrap_or_default()
        };
        let context = calimero_identity::sha256(app_id.as_bytes());
        let account = calimero_account::AccountId::from(caller.account_bytes());
        let result = module.run(
            context.into(),
            account,
            [0; 32].into(),
            method,
            &input,
            &mut overlay,
            None,
            None,
        );
        match result {
            Ok(out) => {
                let events: Vec<(String, Value)> = out
                    .events
                    .iter()
                    .map(|e| (e.kind.clone(), json_or_string(&e.data)))
                    .collect();
                match out.returns {
                    Ok(ret) => Outcome {
                        status: Status::Ok,
                        result: ret.map_or(Value::Null, |r| json_or_string(&r)),
                        error: Value::Null,
                        diff: overlay.into_diff(),
                        events,
                        logs: out.logs,
                    },
                    Err(calimero_runtime::errors::FunctionCallError::ExecutionError(payload)) => {
                        Outcome {
                            status: Status::AppError,
                            result: Value::Null,
                            error: json_or_string(&payload),
                            diff: Vec::new(),
                            events: Vec::new(),
                            logs: out.logs,
                        }
                    }
                    Err(e) => Outcome {
                        status: Status::Trap,
                        result: Value::Null,
                        error: Value::String(format!("{e:?}")),
                        diff: Vec::new(),
                        events: Vec::new(),
                        logs: out.logs,
                    },
                }
            }
            Err(e) => Outcome {
                status: Status::Trap,
                result: Value::Null,
                error: Value::String(format!("{e:?}")),
                diff: Vec::new(),
                events: Vec::new(),
                logs: Vec::new(),
            },
        }
    }
}

/// Milliseconds now, for entries.
#[must_use]
pub fn stamp() -> u64 {
    now_ms()
}
