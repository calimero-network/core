//! The test harness (spec section 10, R17): the real component on the real wasmtime
//! engine with an in-memory host, so a passing test means the same as on a node.
//!
//! The harness plays the node's executor: it applies a logged invocation's diff only when
//! it ends `Ok`, gives every logged invocation a fresh `id_seq` (and a seed derived from
//! [`TestApp::seed`], so runs repeat), shows views the last logged entry's timestamp (D19),
//! and resolves event audiences against the org when events are read (R9).

mod directory;
mod scenario;

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use calimero_app_engine_wasmtime::WasmtimeEngine;
use calimero_app_host::{
    app_key as host_key, Audience as HostAudience, Budget, Caller as HostCaller, Engine, HostCore,
    Invocation, InvocationKind, MethodKind, OrgRole, Outcome, Prepared, Status, TimerCtx, TrapKind,
    Usage,
};
use serde_json::Value;

pub use directory::{Org, ORG_ID, OWNER};
pub use scenario::run_scenario;

use directory::HarnessDir;

/// Why an invocation did not return a value.
#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    /// The method returned a declared `AppError` (or the SDK refused: `NotAView`,
    /// `Forbidden`, `LoginRequired`, `BadArgs`, ..).
    App { code: String, fields: Value },
    /// The invocation trapped.
    Trap(TrapKind),
    /// A `confirm` method was called without `.confirmed()`.
    ConfirmRequired,
}

const APP_ID: &str = "test-app";
const TERM: u64 = 1;

struct Logged {
    name: String,
    data: Value,
    audience: HostAudience,
    emitter: Option<String>,
    /// Who an org audience reached, fixed when the org changed after the emit (R9).
    frozen: Option<Vec<String>>,
}

/// A real component with in-memory state, an org and a clock.
pub struct TestApp {
    engine: &'static WasmtimeEngine,
    prepared: Prepared,
    state: BTreeMap<Vec<u8>, Vec<u8>>,
    org: Org,
    now_ms: u64,
    last_entry_ms: u64,
    seed: u64,
    draws: u64,
    seq: u64,
    last_tick: BTreeMap<String, u64>,
    events: Vec<Logged>,
    usage: Usage,
}

/// One engine and compile cache for the whole test process: compiled components are keyed by
/// their SHA-256, so tests share them. The cache directory lives until the process ends.
fn shared_engine() -> &'static WasmtimeEngine {
    static ENGINE: OnceLock<(WasmtimeEngine, tempfile::TempDir)> = OnceLock::new();
    &ENGINE
        .get_or_init(|| {
            // SAFETY: a test harness cannot continue without its engine; panicking fails the test.
            let dir = tempfile::tempdir().expect("harness cache dir");
            // SAFETY: as above.
            let engine = WasmtimeEngine::new(dir.path().to_path_buf()).expect("wasmtime engine");
            (engine, dir)
        })
        .0
}

impl TestApp {
    /// Prepares `component` and runs `init` as the owner (`"owner"`) at `now_ms`.
    ///
    /// # Panics
    ///
    /// When the component is invalid or `init` fails: the test cannot start.
    #[must_use]
    pub fn new(component: &[u8], org: Org, now_ms: u64) -> Self {
        let engine = shared_engine();
        let prepared = engine
            // SAFETY: an unusable component fails the test at setup.
            .prepare("test-component", component)
            .unwrap_or_else(|e| panic!("harness: component not usable: {e}"));
        let mut app = Self {
            engine,
            prepared,
            state: BTreeMap::new(),
            org,
            now_ms,
            last_entry_ms: now_ms,
            seed: 0,
            draws: 0,
            seq: 0,
            last_tick: BTreeMap::new(),
            events: Vec::new(),
            usage: Usage::default(),
        };
        let owner = HostCaller::Account(OWNER.to_owned());
        let res = app.run(InvocationKind::Init, owner, "init", Vec::new(), None);
        // SAFETY: a failing `init` fails the test at setup.
        res.unwrap_or_else(|f| panic!("harness: init failed: {f:?}"));
        app
    }

    /// Calls as a signed-in account.
    pub fn as_(&mut self, account: &str) -> As<'_> {
        self.caller(HostCaller::Account(account.to_owned()))
    }

    /// Calls with no account.
    pub fn anonymous(&mut self) -> As<'_> {
        self.caller(HostCaller::Public)
    }

    /// Calls as the platform.
    pub fn system(&mut self) -> As<'_> {
        self.caller(HostCaller::System)
    }

    /// Calls with an API key acting for `account`.
    pub fn api_key(&mut self, key_id: &str, account: &str) -> As<'_> {
        self.caller(HostCaller::ApiKey {
            key_id: key_id.to_owned(),
            account: account.to_owned(),
        })
    }

    fn caller(&mut self, caller: HostCaller) -> As<'_> {
        As {
            app: self,
            caller,
            confirmed: false,
        }
    }

    /// Sets the clock.
    pub fn set_time(&mut self, ms: u64) {
        self.now_ms = ms;
    }

    /// Moves the clock forward.
    pub fn advance(&mut self, ms: u64) {
        self.now_ms = self.now_ms.saturating_add(ms);
    }

    /// Seeds the randomness of every later logged invocation; the same seed repeats a run.
    pub fn seed(&mut self, n: u64) {
        self.seed = n;
    }

    /// Runs the timer `method` for the slot at the current time; `previous` is the slot of
    /// the last tick of the same method.
    ///
    /// # Errors
    ///
    /// The tick's `Failure`.
    pub fn tick(&mut self, method: &str) -> Result<Value, Failure> {
        let previous = self.last_tick.get(method).copied();
        let ctx = TimerCtx {
            scheduled_ms: self.now_ms,
            previous_ms: previous,
        };
        let res = self.run(
            InvocationKind::Timer,
            HostCaller::System,
            method,
            Vec::new(),
            Some(ctx),
        );
        // A failed tick counts as applied (D34).
        if !matches!(res, Err(Failure::Trap(TrapKind::Timeout))) {
            self.last_tick.insert(method.to_owned(), ctx.scheduled_ms);
        }
        res
    }

    /// The events `account` receives, audiences resolved now against the org (R9).
    #[must_use]
    pub fn events_for(&self, account: &str) -> Vec<(String, Value)> {
        let role = self.org.role(account);
        self.events
            .iter()
            .filter(|e| match &e.audience {
                HostAudience::OrgMembers(_) | HostAudience::OrgAdmins(_) if e.frozen.is_some() => e
                    .frozen
                    .as_ref()
                    .is_some_and(|f| f.iter().any(|a| a == account)),
                HostAudience::All => true,
                HostAudience::Caller => e.emitter.as_deref() == Some(account),
                HostAudience::Accounts(list) => list.iter().any(|a| a == account),
                HostAudience::Team(_) => false,
                HostAudience::OrgMembers(org) => {
                    org == ORG_ID && role.is_some_and(|r| r >= OrgRole::Member)
                }
                HostAudience::OrgAdmins(org) => {
                    org == ORG_ID && role.is_some_and(|r| r >= OrgRole::Admin)
                }
            })
            .map(|e| (e.name.clone(), e.data.clone()))
            .collect()
    }

    /// Removes the account from the org. Org events emitted so far stay with the accounts
    /// that were in the audience when it changed; later ones no longer reach `account`.
    pub fn remove_member(&mut self, account: &str) {
        let org = self.org.clone();
        for e in self.events.iter_mut().filter(|e| e.frozen.is_none()) {
            let min = match e.audience {
                HostAudience::OrgMembers(_) => OrgRole::Member,
                HostAudience::OrgAdmins(_) => OrgRole::Admin,
                _ => continue,
            };
            let reached = org
                .accounts()
                .filter(|a| org.role(a).is_some_and(|r| r >= min));
            e.frozen = Some(reached.collect());
        }
        self.org.remove(account);
    }

    /// The usage of the last invocation.
    #[must_use]
    pub fn last_usage(&self) -> Usage {
        self.usage
    }

    /// The state as deterministic CBOR (RFC 8949 4.2.1): a map of stored key (generation
    /// prefix included) to value, both byte strings, ordered by their encoded bytes.
    ///
    /// # Panics
    ///
    /// Never in practice: encoding byte strings cannot fail.
    #[must_use]
    pub fn export_state(&self) -> Vec<u8> {
        let mut entries: Vec<(Vec<u8>, ciborium::Value)> = self
            .state
            .iter()
            .map(|(k, v)| {
                let mut enc = Vec::new();
                // SAFETY: writing a byte string to a Vec cannot fail.
                ciborium::into_writer(&ciborium::Value::Bytes(k.clone()), &mut enc)
                    .expect("encode key");
                (enc, ciborium::Value::Bytes(v.clone()))
            })
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let map = entries
            .into_iter()
            .map(|(enc, v)| {
                // SAFETY: `enc` was produced by the encoder above.
                let key: ciborium::Value = ciborium::from_reader(enc.as_slice()).expect("key");
                (key, v)
            })
            .collect();
        let mut out = Vec::new();
        // SAFETY: writing to a Vec cannot fail.
        ciborium::into_writer(&ciborium::Value::Map(map), &mut out).expect("encode state");
        out
    }

    fn next_seed(&mut self) -> [u8; 32] {
        self.draws += 1;
        let mut x = self.seed ^ self.draws.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut out = [0_u8; 32];
        for chunk in out.chunks_mut(8) {
            x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            chunk.copy_from_slice(&(z ^ (z >> 31)).to_le_bytes());
        }
        out
    }

    fn run(
        &mut self,
        kind: InvocationKind,
        caller: HostCaller,
        method: &str,
        args: Vec<u8>,
        timer: Option<TimerCtx>,
    ) -> Result<Value, Failure> {
        let is_view = kind == InvocationKind::View;
        if !is_view {
            self.seq += 1;
        }
        let entry_ts_ms = if is_view {
            self.last_entry_ms
        } else {
            self.now_ms
        };
        let emitter = match &caller {
            HostCaller::Account(a) | HostCaller::ApiKey { account: a, .. } => Some(a.clone()),
            _ => None,
        };
        let inv = Invocation {
            kind,
            method: method.to_owned(),
            args,
            caller,
            entry_ts_ms,
            id_term: TERM,
            id_seq: self.seq,
            seed: self.next_seed(),
            timer,
        };
        let host = HostCore::new(
            APP_ID,
            Arc::new(self.state.clone()),
            self.seq,
            inv,
            Arc::new(HarnessDir(self.org.clone())),
        )
        // SAFETY: the harness builds timer slots exactly for timer invocations.
        .expect("harness builds valid invocations");
        let budget = if is_view {
            Budget::view()
        } else {
            Budget::call()
        };
        let out = self.engine.invoke(&self.prepared, host, budget);
        self.usage = out.usage;
        if !is_view {
            self.last_entry_ms = self.now_ms;
        }
        self.finish(is_view, emitter, out)
    }

    fn finish(
        &mut self,
        is_view: bool,
        emitter: Option<String>,
        out: Outcome,
    ) -> Result<Value, Failure> {
        match out.status {
            Status::Ok => {
                if !is_view {
                    for w in out.diff {
                        match w.value {
                            Some(v) => self.state.insert(w.key, v),
                            None => self.state.remove(&w.key),
                        };
                    }
                    for e in out.events {
                        let data = serde_json::from_slice(&e.data).unwrap_or(Value::Null);
                        self.events.push(Logged {
                            name: e.name,
                            data,
                            audience: e.audience,
                            emitter: emitter.clone(),
                            frozen: None,
                        });
                    }
                }
                if out.result.is_empty() {
                    return Ok(Value::Null);
                }
                Ok(serde_json::from_slice(&out.result).unwrap_or(Value::Null))
            }
            Status::AppError { code, message } => Err(Failure::App {
                code,
                fields: serde_json::from_str(&message).unwrap_or(Value::String(message)),
            }),
            Status::Trap { kind, .. } => Err(Failure::Trap(kind)),
        }
    }

    /// The host-side ABI checks (confirm, public, allowSystem) a node makes before `handle`.
    fn gate(
        &self,
        method: &str,
        caller: &HostCaller,
        confirmed: bool,
        view: bool,
    ) -> Option<Failure> {
        let m = self.prepared.abi.methods.get(method)?;
        let refuse = |code: &str| {
            Some(Failure::App {
                code: code.to_owned(),
                fields: Value::Object(serde_json::Map::new()),
            })
        };
        if view && (m.confirm || m.kind != MethodKind::View) {
            return refuse("NotAView");
        }
        if !matches!(m.kind, MethodKind::Call | MethodKind::View) {
            // Timers and `init` are refused by the guest with `WrongKind`.
            return None;
        }
        if m.confirm && !confirmed {
            return Some(Failure::ConfirmRequired);
        }
        match caller {
            HostCaller::Public if !m.public => refuse("LoginRequired"),
            HostCaller::System if !m.allow_system => refuse("Forbidden"),
            _ => None,
        }
    }

    /// The stored key of an app key, for tests that read state directly.
    #[must_use]
    pub fn stored_key(key: &[u8]) -> Vec<u8> {
        host_key(calimero_app_host::GENERATION, key)
    }
}

/// An invocation as one caller: `t.as_("bob").call("book", args)`.
pub struct As<'a> {
    app: &'a mut TestApp,
    caller: HostCaller,
    confirmed: bool,
}

impl As<'_> {
    /// Marks the caller as having passed the step-up confirmation.
    #[must_use]
    pub fn confirmed(mut self) -> Self {
        self.confirmed = true;
        self
    }

    /// Runs a call and logs it.
    ///
    /// # Errors
    ///
    /// The call's `Failure`.
    pub fn call(&mut self, method: &str, args: Value) -> Result<Value, Failure> {
        if let Some(f) = self.app.gate(method, &self.caller, self.confirmed, false) {
            return Err(f);
        }
        self.app.run(
            InvocationKind::Call,
            self.caller.clone(),
            method,
            args_bytes(&args),
            None,
        )
    }

    /// Runs a view: nothing is logged and nothing written.
    ///
    /// # Errors
    ///
    /// The view's `Failure`.
    pub fn view(&mut self, method: &str, args: Value) -> Result<Value, Failure> {
        if let Some(f) = self.app.gate(method, &self.caller, self.confirmed, true) {
            return Err(f);
        }
        self.app.run(
            InvocationKind::View,
            self.caller.clone(),
            method,
            args_bytes(&args),
            None,
        )
    }
}

fn args_bytes(args: &Value) -> Vec<u8> {
    if args.is_null() {
        return b"{}".to_vec();
    }
    // SAFETY: a `Value` always serializes.
    serde_json::to_vec(args).expect("serialize args")
}
