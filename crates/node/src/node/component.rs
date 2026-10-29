//! The node's component executor: running `calimero:app` components (the v2 SDK)
//! on wasmtime through the host core, for the apps whose bundle is a component.
//!
//! Everything an app does besides running its code (routing, the API, the system
//! group, sessions, packages, presence, receipts, upgrades' authority) is the
//! node's, shared with the wasmer executor. What is here: preparing components
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

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use calimero_app_host::{
    Budget, Engine as _, HostCore, Invocation, InvocationKind, MethodKind, Prepared, TimerCtx,
    TrapKind,
};
use calimero_log::{Group, GroupId, Proposal, WriteError};
use serde_json::{json, Value};
use tracing::debug;

use super::{status_of, CallRequest, Node, ViewAnswer, ViewRequest};
use crate::app::{AppMachine, Status};
use crate::component::{
    utc_day, Applied, CallEntry, ComponentCmd, DayUsage, Outcome, SysDirectory, TickEntry,
};
use crate::error::ApiError;
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

/// The largest call or `init` arguments a component runs (Ruling 30): well under
/// `MAX_ENTRY_BYTES`, so the entry that logs a refused run always fits.
pub const MAX_ARGS_BYTES: usize = 512 * 1024;

/// How often an app group's leader looks for due timer ticks (D34).
pub const TIMER_POLL: Duration = Duration::from_millis(100);

/// An app's usage book: [`DayUsage`] per UTC day ([`crate::component::ComponentHead::usage`]).
pub type UsageBook = BTreeMap<u64, DayUsage>;

/// How long the fuel quota reuses a peer's answer about an app this node does not
/// host. Apps hosted here are read from the replica every time.
pub const USAGE_CACHE: Duration = Duration::from_secs(1);

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
    use ring::hmac;
    let tag = hmac::sign(
        &hmac::Key::new(hmac::HMAC_SHA256, storage_key),
        b"calimero.components.v1",
    );
    let mut key = [0; 32];
    key.copy_from_slice(tag.as_ref());
    key
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
) -> Result<(), ApiError> {
    if super::reserved_method(method) {
        return Err(app_refusal("UnknownMethod"));
    }
    let Some(m) = prepared.abi.methods.get(method) else {
        return Err(app_refusal("UnknownMethod"));
    };
    if !matches!(m.kind, MethodKind::Call | MethodKind::View) {
        return Err(app_refusal("WrongKind"));
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
        _ if m.confirm && !view && !consented => Err(app_refusal("ConfirmRequired")),
        _ => Ok(()),
    }
}

/// The entry of a call as bytes; one over `MAX_ENTRY_BYTES` becomes the same call
/// trapped `HostDenied` with nothing in it, decided before proposing (D50).
fn entry_bytes(mut cmd: ComponentCmd) -> Vec<u8> {
    let bytes = serde_json::to_vec(&cmd).unwrap_or_default();
    if bytes.len() <= calimero_log::MAX_ENTRY_BYTES {
        return bytes;
    }
    const DENIED: &str = "entry too large";
    match &mut cmd {
        ComponentCmd::Call(entry) => entry.outcome = entry.outcome.denied(DENIED),
        ComponentCmd::Tick(entry) => entry.outcome = entry.outcome.denied(DENIED),
        ComponentCmd::Init { outcome, .. } => **outcome = outcome.denied(DENIED),
        ComponentCmd::Upgrade { .. } => {}
    }
    serde_json::to_vec(&cmd).unwrap_or_default()
}

impl Node {
    /// App time: what a component's entry stamps and timer slots read (D16, D19, D34).
    #[must_use]
    pub fn app_clock(&self) -> Clock {
        self.clock.clone()
    }

    /// Moves this node's app time by `ms` (negative: back). Tests only.
    #[cfg(feature = "testing")]
    pub fn advance_clock(&self, ms: i64) {
        self.clock.advance(ms);
    }

    /// The engine every component app runs on.
    #[must_use]
    pub fn app_engine(&self) -> &calimero_app_engine_wasmtime::WasmtimeEngine {
        &self.wasm
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

    /// Run one invocation of `app_id` against `state` at `applied`.
    fn run_component(
        &self,
        prepared: &Prepared,
        app_id: &str,
        state: Arc<dyn calimero_app_host::StateView>,
        applied: u64,
        invocation: Invocation,
    ) -> Outcome {
        let this = self.this.lock().map(|w| w.clone()).unwrap_or_default();
        let directory = Arc::new(SysDirectory::new(this, app_id.to_owned()));
        match HostCore::new(app_id, state, applied, invocation, directory) {
            Ok(host) => Outcome::from_host(self.wasm.invoke(prepared, host, self.budget())),
            Err(e) => Outcome::trap(TrapKind::Fault, &e.to_string()),
        }
    }

    /// [`Self::run_component`] for a run that becomes a log entry (call, tick,
    /// `init`), which runs inside `Group::propose_led`, on the machine's read lock,
    /// for up to its deadline: off the async workers ([`Node::metered`], which
    /// adds what it took to `meter`'s app).
    fn run_logged(
        &self,
        meter: &Meter,
        prepared: &Prepared,
        app_id: &str,
        state: Arc<dyn calimero_app_host::StateView>,
        applied: u64,
        invocation: Invocation,
    ) -> Outcome {
        self.metered(meter, || {
            self.run_component(prepared, app_id, state, applied, invocation)
        })
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
        // A component the engine refuses fails `init` for good (reported as
        // INIT_FAILED); anything else is retried by `reconcile`.
        // The bundle the group names if an upgrade gave it one before init applied
        // (its first bundle's init timed out), else the app record's.
        let bundle = g
            .read_local(|m, _| m.state.bundle.clone())
            .unwrap_or_else(|| app.bundle.clone());
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
        let meter = self.meter(&app.app_id, &bundle);
        let p = g
            .propose_led(|m, applied, leadership| {
                if m.state.initialized || m.state.init_error.is_some() {
                    return Proposal::Reply(Vec::new());
                }
                let ts_ms = self.stamp(m);
                let (id_term, id_seq) =
                    self.next_id(g.id(), leadership.term, m.state.component.head.last_id);
                let outcome = match &prepared {
                    Ok(prepared) => self.run_logged(
                        &meter,
                        prepared,
                        &app.app_id,
                        m.view(),
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
                        },
                    ),
                    Err(why) => Outcome::trap(TrapKind::Fault, why),
                };
                if outcome.is_timeout() {
                    // Not logged (D23): reconcile runs init again.
                    timed_out = true;
                    return Proposal::Reply(Vec::new());
                }
                timed_out = false;
                Proposal::Append(entry_bytes(ComponentCmd::Init {
                    bundle: bundle.clone(),
                    base_index: m.state.last_index,
                    ts_ms,
                    id_term,
                    id_seq,
                    outcome: Box::new(outcome),
                }))
            })
            .await
            .map_err(|e| status_of(&e))?;
        if timed_out {
            return Err(timeout_error());
        }
        if !p.appended {
            return Err(ApiError::conflict("already initialized"));
        }
        let a: Applied =
            serde_json::from_slice(&p.output).map_err(|e| ApiError::internal(&e.to_string()))?;
        let a = to_wasmer_applied(a);
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
        let led: Vec<(GroupId, Arc<Group<AppMachine>>)> = self
            .apps
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
            .unwrap_or_default();
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

    /// Propose the latest due slot of each of the app's timers not yet applied
    /// (one catch-up tick for a gap). Nothing runs before the app is initialized.
    async fn tick_app(&self, g: &Group<AppMachine>) {
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
        let Ok(prepared) = self.prepared_for(&bundle).await else {
            return;
        };
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
                let outcome = self.run_logged(
                    &meter,
                    prepared,
                    app_id,
                    m.view(),
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
                    },
                );
                if outcome.is_timeout() {
                    debug!(app = %app_id, timer, slot, "tick timed out; proposed again");
                    return Proposal::Reply(Vec::new());
                }
                Proposal::Append(entry_bytes(ComponentCmd::Tick(Box::new(TickEntry {
                    timer: timer.to_owned(),
                    scheduled_ms: slot,
                    previous_ms,
                    ts_ms,
                    base_index: m.state.last_index,
                    id_term,
                    id_seq,
                    outcome,
                }))))
            })
            .await?;
        // A tick that timed out was not logged and runs again.
        if p.appended {
            self.usage.called(&meter, RunKind::FollowUp);
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
        // wherever it was committed.
        self.sys_fresh().await;
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
            Some(app) => crate::api::read_only_in(self, app, req.caller.account()).await,
            None => false,
        };
        // An owner over its daily fuel is refused before the run (Q20); a retry of a
        // call that committed still replays, below.
        let over_quota = match &app {
            Some(app) => self.fuel_quota_refusal(app).await,
            None => None,
        };
        let args = args_bytes(&req.args);
        let mut refused = None;
        let meter = self.meter(&req.app_id, &bundle);
        let mut answered = false;
        let p = g
            .propose_led(|m, applied, leadership| {
                refused = None;
                answered = false;
                // A retry of a call that committed is its recorded answer, whatever
                // the bundle runs now (D70): the replay comes before the ABI check.
                if let Some(key) = &req.idempotency_key {
                    if let Some(prev) = m.component_replay(&req.caller, key) {
                        return Proposal::Reply(serde_json::to_vec(&prev).unwrap_or_default());
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
                ) {
                    refused = Some(e);
                    return Proposal::Reply(Vec::new());
                }
                let ts_ms = self.stamp(m);
                let (id_term, id_seq) =
                    self.next_id(g.id(), leadership.term, m.state.component.head.last_id);
                let outcome = self.run_logged(
                    &meter,
                    &prepared,
                    &req.app_id,
                    m.view(),
                    applied,
                    Invocation {
                        kind: InvocationKind::Call,
                        method: req.method.clone(),
                        args: args.clone(),
                        caller: req.caller.to_host(),
                        entry_ts_ms: ts_ms,
                        id_term,
                        id_seq,
                        seed: calimero_identity::random::<32>(),
                        timer: None,
                    },
                );
                // A timeout is not logged and does not spend the key (D23).
                if outcome.is_timeout() {
                    let a = Applied::unlogged(applied, outcome);
                    return Proposal::Reply(serde_json::to_vec(&a).unwrap_or_default());
                }
                answered = true;
                // Events are logged side effects too (Ruling 41): an emit-only call
                // is refused like one that writes keys.
                if read_only && (!outcome.diff.is_empty() || !outcome.events.is_empty()) {
                    let refused = Applied::unlogged(
                        applied,
                        Outcome {
                            status: Status::AppError,
                            error: Value::String(super::READ_ONLY.into()),
                            ..Outcome::default()
                        },
                    );
                    return Proposal::Reply(serde_json::to_vec(&refused).unwrap_or_default());
                }
                Proposal::Append(entry_bytes(ComponentCmd::Call(Box::new(CallEntry {
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
                }))))
            })
            .await?;
        if let Some(e) = refused {
            return Err(LocalError::Refused(e));
        }
        let a: Applied = serde_json::from_slice(&p.output)
            .map_err(|e| LocalError::Write(WriteError::Unavailable(e.to_string())))?;
        if p.appended && a.conflict {
            let _prev = self
                .conflicts
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if answered && !a.conflict {
            self.usage.called(&meter, RunKind::Mutation);
        }
        Ok(a)
    }

    /// `app`'s usage book (Q20), from a replica: this node's if it hosts the app,
    /// else one that does. A wasmer app has none.
    pub async fn app_usage(&self, app: &AppRecord) -> Result<UsageBook, ApiError> {
        if app.executor != Executor::Component {
            return Ok(UsageBook::new());
        }
        if let Some(g) = self.hosted(&app.app_id) {
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

    /// Today's usage (UTC, on app time) of `app`: this node's replica if it hosts the
    /// app, else a replica's answer at most [`USAGE_CACHE`] old. An app no replica
    /// answers for counts as nothing.
    async fn app_usage_today(&self, app: &AppRecord, today: u64) -> DayUsage {
        if let Some(g) = self.hosted(&app.app_id) {
            return g.read_local(|m, _| m.state.component.head.usage_on(today));
        }
        let cached = self.usage_cache.lock().ok().and_then(|c| {
            c.get(&app.app_id)
                .filter(|(at, _)| at.elapsed() < USAGE_CACHE)
                .map(|(_, book)| book.get(&today).copied().unwrap_or_default())
        });
        if let Some(day) = cached {
            return day;
        }
        match self.app_usage(app).await {
            Ok(book) => {
                let day = book.get(&today).copied().unwrap_or_default();
                if let Ok(mut c) = self.usage_cache.lock() {
                    // Apps that left this account (deleted, moved) leave the cache too.
                    c.retain(|_, (at, _)| at.elapsed() < USAGE_CACHE);
                    let _prev = c.insert(app.app_id.clone(), (std::time::Instant::now(), book));
                }
                day
            }
            Err(e) => {
                debug!(app = %app.app_id, error = %e, "usage not known; counted as none");
                DayUsage::default()
            }
        }
    }

    /// What `payer`'s apps used today (UTC, on app time), summed over its component
    /// apps (Q20, D78). The payer is `usage::payer`'s: an organization's id for its
    /// apps, `account:{owner}` (`usage::account_payer`) for an account's personal ones.
    pub async fn payer_usage_today(&self, payer: &str) -> DayUsage {
        let today = utc_day(self.clock.now_ms());
        let apps: Vec<AppRecord> = self.sys(|s| {
            s.apps
                .values()
                .filter(|a| a.executor == Executor::Component && crate::usage::payer(a) == payer)
                .cloned()
                .collect()
        });
        let mut total = DayUsage::default();
        for app in &apps {
            total.add(&self.app_usage_today(app, today).await);
        }
        total
    }

    /// The refusal of a new call to `app`, of either executor, when its payer (its
    /// organization, or for a personal app its owner; D78) is at or over
    /// `max_fuel_per_payer_per_day` (D76; 0: no limit, nothing is read). Only a
    /// client's call is refused: `init`, timer ticks and follow-ups still run, and
    /// the component runs among them still count toward the payer's day.
    pub(super) async fn fuel_quota_refusal(&self, app: &AppRecord) -> Option<ApiError> {
        let limit = self.sys(|s| s.quotas.max_fuel_per_payer_per_day);
        if limit == 0 {
            return None;
        }
        let used = self.payer_usage_today(&crate::usage::payer(app)).await.fuel;
        (used >= limit).then(|| {
            super::rejected(&format!(
                "{}: fuel per payer per day ({used} of {limit})",
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
        abi_check(&prepared, method, caller, view, consented)
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
                    .map_err(|e| ApiError::unavailable(&e.to_string()))?;
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
        let (bundle, state, applied, ts_ms) = g.read_local(|m, applied| {
            (
                m.component_bundle(),
                m.view(),
                applied,
                m.state.component.head.last_ts_ms,
            )
        });
        let bundle =
            bundle.ok_or_else(|| ApiError::unavailable("the app is not initialized yet"))?;
        let prepared = self.prepared_for(&bundle).await?;
        abi_check(&prepared, &req.method, &req.caller, true, false)?;
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
            },
        );
        let meter = self.meter(&req.app_id, &bundle);
        // A view may run up to its deadline: off the async workers.
        let (out, run) = tokio::task::spawn_blocking(move || {
            crate::usage::measure(|| {
                node.run_component(&prepared, &app_id, state, applied, invocation)
            })
        })
        .await
        .map_err(|e| ApiError::internal(&e.to_string()))?;
        self.usage.ran(&meter, run);
        if !out.is_timeout() {
            self.usage.called(&meter, RunKind::View);
        }
        Ok(ViewAnswer {
            index: applied,
            timeout: out.is_timeout(),
            status: out.status,
            result: out.result,
            error: out.error,
        })
    }

    /// Switch a component app to another component, code only, on this node, the
    /// app group's leader. A named migration is refused: migrations are rebuilds
    /// (D38), not in this milestone. Who may upgrade was judged by the API.
    pub(super) async fn upgrade_component(
        &self,
        g: &Group<AppMachine>,
        bundle: &str,
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
        match self.prepared_for(bundle).await {
            Ok(_) => {}
            Err(e) if e.code == BAD_BUNDLE => return Ok(refuse(BAD_BUNDLE, &e.message)),
            Err(e) => return Err(WriteError::Unavailable(e.message)),
        }
        let p = g
            .propose_led(|m, _, _| {
                Proposal::Append(entry_bytes(ComponentCmd::Upgrade {
                    bundle: bundle.to_owned(),
                    base_index: m.state.last_index,
                }))
            })
            .await?;
        let a: Applied = serde_json::from_slice(&p.output)
            .map_err(|e| WriteError::Unavailable(e.to_string()))?;
        let a = to_wasmer_applied(a);
        self.count_conflict(&a);
        Ok(a)
    }
}

/// A component's answer in the shape the node's own commands (`init`, upgrades)
/// answer with: their callers read the status, the error and the conflict flag.
fn to_wasmer_applied(a: Applied) -> crate::app::Applied {
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
}
