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

use std::collections::HashSet;
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
use crate::component::blobs::{grant_key, Blobs, RunBlobs, COLLECT_BATCH};
use crate::component::xcall::{backoff_ms, Outgoing, XcallFrom, DELIVER_BATCH, XCALL_WINDOW_MS};
use crate::component::{Applied, CallEntry, ComponentCmd, Outcome, SysDirectory, TickEntry};
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
/// replica made before running (its own checks, against its own state).
enum LocalError {
    Write(WriteError),
    Refused(ApiError),
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
    if !matches!(m.kind, MethodKind::Call | MethodKind::View) && !starts_action {
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

/// The entry of a call as bytes; one over `MAX_ENTRY_BYTES` becomes the same call
/// trapped `HostDenied` with nothing in it, decided before proposing (D50).
fn entry_bytes(mut cmd: ComponentCmd) -> Vec<u8> {
    let bytes = serde_json::to_vec(&cmd).unwrap_or_default();
    if bytes.len() <= calimero_log::MAX_ENTRY_BYTES {
        return bytes;
    }
    let denied = Outcome::trap(TrapKind::HostDenied, "entry too large");
    match &mut cmd {
        ComponentCmd::Call(entry) => entry.outcome = denied,
        ComponentCmd::Tick(entry) => entry.outcome = denied,
        ComponentCmd::Init { outcome, .. } => **outcome = denied,
        ComponentCmd::Upgrade { .. }
        | ComponentCmd::Blob { .. }
        | ComponentCmd::BlobForget { .. }
        | ComponentCmd::BlobsCollected { .. }
        | ComponentCmd::XcallDone { .. }
        | ComponentCmd::ActionEnd { .. } => {}
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
    ) -> Outcome {
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
    ) -> Outcome {
        let http = extras.outbound.then(|| {
            let meter = self.meter(app_id, &prepared.hash);
            Arc::new(TimedHttp {
                inner: Arc::new(NodeHttp(self.outbound_for(
                    app_id,
                    &prepared.hash,
                    self.usage.outbound(&meter.app, &meter.payer),
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
        let blobs = Arc::new(RunBlobs {
            app_id: app_id.to_owned(),
            records: blobs,
            key: self.blob_grant_key(),
        });
        match HostCore::new(app_id, state, applied, invocation, directory) {
            Ok(host) => {
                let host = host.with_blobs(blobs);
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
                Outcome::from_host(self.wasm.invoke(prepared, host, self.budget()))
            }
            Err(e) => Outcome::trap(TrapKind::Fault, &e.to_string()),
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
    ) -> Outcome {
        self.metered(meter, || {
            self.run_component(prepared, app_id, state, applied, invocation, extras)
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
                        },
                        RunExtras::default(),
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
    async fn tick_app(&self, g: &Arc<Group<AppMachine>>) {
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
                    },
                    RunExtras {
                        key: Some(format!("tick:{timer}:{slot}")),
                        outbound: true,
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
                let ts_ms = self.stamp(m);
                let (id_term, id_seq) =
                    self.next_id(g.id(), leadership.term, m.state.component.head.last_id);
                let starts_action = prepared
                    .abi
                    .methods
                    .get(&req.method)
                    .is_some_and(|m| m.kind == MethodKind::Action);
                let outcome = if starts_action {
                    // Started, not run here: logged as started, run by the runner (D17).
                    let id = format!("a.{id_term}.{id_seq}.0");
                    Outcome {
                        status: Status::Ok,
                        result: json!({ "actionId": id }),
                        actions: vec![crate::component::action::ActionOut {
                            id,
                            method: req.method.clone(),
                            args: req.args.clone(),
                        }],
                        ..Outcome::default()
                    }
                } else {
                    self.run_logged(
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
                        },
                        RunExtras {
                            // The caller is part of the key: two callers' calls under the
                            // same key are two calls of the target.
                            key: Some(format!(
                                "call:{}:{}",
                                serde_json::to_string(&req.caller).unwrap_or_default(),
                                req.idempotency_key.as_deref().unwrap_or_default()
                            )),
                            // A read-only member's run reaches no outbound host, as on the
                            // wasmer path.
                            outbound: !read_only,
                        },
                    )
                };
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
                    xcall: req.xcall.clone(),
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
                    },
                    search,
                )
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

    /// A component's blob command (R8) on this node, the app group's leader: an upload
    /// (`__blob { id, size, mime }`) or its owner's delete (`__blob_forget { id }`), which
    /// only the API builds (`CallRequest::host`), after judging who may.
    pub(super) async fn component_blob_cmd(
        &self,
        g: &Group<AppMachine>,
        req: &CallRequest,
    ) -> Result<crate::app::Applied, WriteError> {
        let id = req.args["id"].as_str().unwrap_or_default().to_owned();
        let upload = req.method == "__blob";
        let p = g
            .propose_led(|m, _, _| {
                let cmd = if upload {
                    ComponentCmd::Blob {
                        id: id.clone(),
                        size: req.args["size"].as_u64().unwrap_or_default(),
                        mime: req.args["mime"].as_str().unwrap_or_default().to_owned(),
                        ts_ms: self.stamp(m),
                    }
                } else {
                    ComponentCmd::BlobForget { id: id.clone() }
                };
                Proposal::Append(serde_json::to_vec(&cmd).unwrap_or_default())
            })
            .await?;
        let a: Applied = serde_json::from_slice(&p.output)
            .map_err(|e| WriteError::Unavailable(e.to_string()))?;
        Ok(to_wasmer_applied(a))
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
                _ if out.is_timeout() && record.attempts + 1 < MAX_ATTEMPTS => (None, true),
                _ if out.is_timeout() => (Some(format!("timed out {MAX_ATTEMPTS} times")), false),
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
            self.usage.outbound(&meter.app, &meter.payer),
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
        if let Err(e) = g
            .propose_led(|_, _, _| Proposal::Append(serde_json::to_vec(&cmd).unwrap_or_default()))
            .await
        {
            debug!(error = %e, "collected blobs not logged; collected again next round");
        }
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
#[derive(Clone, Debug, Default)]
struct RunExtras {
    /// The run's own key (a call's caller and idempotency key, a tick's slot, a view's
    /// placeholder): with one, the run reaches other apps synchronously ([`NodeXcall`]).
    key: Option<String>,
    /// The app's outbound gate (D76): calls (but a read-only member's), timers and
    /// deliveries; never views or `init`.
    outbound: bool,
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
struct NodeHttp(Option<Arc<dyn calimero_runtime::logic::OutboundHost>>);

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
/// personal apps of one owner (wasmer's xcall rule, `Node::xcall_refusal`).
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
