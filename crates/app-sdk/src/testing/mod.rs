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
    blob_refs, stored_key as host_key, Audience as HostAudience, BlobHost, BlobInfo, Budget,
    Caller as HostCaller, Engine, HostCore, Invocation, InvocationKind, MethodKind, OrgRole,
    Outcome, Prepared, SearchHit as HostHit, SearchHost, SearchPage, SearchQuery, Status, TimerCtx,
    Usage,
};
use calimero_app_host::{ActionHost, ActionStart, HttpHost};
use serde_json::Value;

pub use calimero_app_host::{HttpRequest, HttpResponse, TrapKind};
pub use directory::{Org, ORG_ID, OWNER};
pub use scenario::run_scenario;

use directory::HarnessDir;

/// The harness's blobs (R8): what `upload_blob` recorded; a grant is `"<id>.<expires_ms>"`.
struct HarnessBlobs(BTreeMap<[u8; 32], BlobInfo>);

impl BlobHost for HarnessBlobs {
    fn info(&self, id: &[u8; 32]) -> Option<BlobInfo> {
        self.0.get(id).cloned()
    }

    fn grant(&self, id: &[u8; 32], expires_ms: u64) -> String {
        format!("{}.{expires_ms}", hex32(id))
    }
}

/// The harness's search (spec section 7): a plain scan of the table's records that keeps
/// those whose declared fields contain every word of the query (case-insensitive), each
/// scored 1.0, in key order. The node ranks with its full-text index; tests should assert
/// which records match, not their order.
struct HarnessSearch {
    state: Arc<BTreeMap<Vec<u8>, Vec<u8>>>,
    fields: BTreeMap<String, Vec<String>>,
}

impl SearchHost for HarnessSearch {
    fn query(&self, q: &SearchQuery) -> Result<SearchPage, String> {
        let Some(fields) = self.fields.get(&q.table) else {
            return Err(format!("table {:?} declares no search fields", q.table));
        };
        let prefix = host_key(
            calimero_app_host::GENERATION,
            &crate::encoding::key::encode(&[
                crate::KeyPart::Str(q.table.clone()),
                crate::KeyPart::Str("r".into()),
            ]),
        );
        let words: Vec<String> = q.text.split_whitespace().map(str::to_lowercase).collect();
        let mut hits = Vec::new();
        for (k, v) in self.state.range(prefix.clone()..) {
            if !k.starts_with(&prefix) {
                break;
            }
            let Ok(ciborium::Value::Map(entries)) = ciborium::from_reader(v.as_slice()) else {
                continue;
            };
            let texts: Vec<String> = entries
                .iter()
                .filter(|(name, _)| {
                    name.as_text()
                        .is_some_and(|n| fields.iter().any(|f| f == n))
                })
                .filter_map(|(_, value)| value.as_text().map(str::to_owned))
                .collect();
            let all = texts.join(" ").to_lowercase();
            if !words.is_empty() && words.iter().all(|w| all.contains(w.as_str())) {
                hits.push(HostHit {
                    key: k[8..].to_vec(),
                    score: 1.0,
                    snippet: texts.first().cloned().unwrap_or_default(),
                });
            }
        }
        let total = hits.len() as u64;
        let start = q.cursor as usize;
        let page: Vec<HostHit> = hits
            .into_iter()
            .skip(start)
            .take(q.limit as usize)
            .collect();
        let end = start + page.len();
        Ok(SearchPage {
            next: (end < total as usize).then(|| u32::try_from(end).unwrap_or(u32::MAX)),
            hits: page,
            total,
        })
    }
}

fn hex32(id: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    id.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Why an invocation did not return a value.
#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    /// The method returned a declared `AppError` (or the SDK refused: `NotAView`,
    /// `Forbidden`, `LoginRequired`, `BadArgs`, ..).
    App { code: String, fields: Value },
    /// The invocation trapped.
    Trap(TrapKind),
    /// The guest's result or an event's data was not valid JSON; the invocation is not applied.
    BadOutput(String),
    /// A `confirm` method was called without `.confirmed()`.
    ConfirmRequired,
    /// The platform refused before the app ran, with the node's HTTP status and API code
    /// (Rulings 32, 35): `401 UNAUTHENTICATED` for an anonymous caller (the harness app is an
    /// org app), `404 NOT_FOUND` for an account the app's org does not hold.
    Refused { status: u16, code: String },
}

const APP_ID: &str = "test-app";
/// The largest call arguments a node runs (`node::MAX_ARGS_BYTES`, Ruling 30).
const MAX_ARGS_BYTES: usize = 512 * 1024;
/// Method names the node keeps for the platform (`node::RESERVED_PREFIX`).
const RESERVED_PREFIX: &str = "__";
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
    blobs: BTreeMap<[u8; 32], BlobInfo>,
    outbox: Vec<SentXcall>,
    /// Actions logged runs started, with their caller (D17), not run yet.
    started: Vec<(HostCaller, ActionStart)>,
    next_action: u64,
    http: BTreeMap<String, HttpHandler>,
    /// Set while an action's sub-calls run: `allow(action)` methods are reachable.
    in_action: bool,
}

/// What `mock_http` answers a host's requests with.
pub type HttpHandler = Arc<dyn Fn(&HttpRequest) -> HttpResponse + Send + Sync>;

/// An action's view of the app (D12): its sub-calls run on the harness itself, which the
/// action lends out while it runs (`TestApp::action`).
struct HarnessAction {
    app: Arc<std::sync::Mutex<TestApp>>,
    caller: HostCaller,
    key: String,
}

impl HarnessAction {
    fn with<R>(&self, f: impl FnOnce(&mut TestApp) -> R) -> R {
        let mut app = self
            .app
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        app.in_action = true;
        let out = f(&mut app);
        app.in_action = false;
        out
    }
}

fn failure_text(f: &Failure) -> String {
    match f {
        Failure::App { code, fields } => {
            serde_json::json!({ "code": code, "fields": fields }).to_string()
        }
        other => format!("{other:?}"),
    }
}

impl ActionHost for HarnessAction {
    fn key(&self) -> String {
        self.key.clone()
    }

    fn view(&self, method: &str, args: &[u8]) -> Result<Vec<u8>, String> {
        let args: Value = serde_json::from_slice(args).map_err(|e| e.to_string())?;
        self.with(|app| {
            app.caller(self.caller.clone())
                .view(method, args)
                .map(|v| v.to_string().into_bytes())
                .map_err(|f| failure_text(&f))
        })
    }

    fn call(&self, method: &str, args: &[u8], _key: &str) -> Result<Vec<u8>, String> {
        let args: Value = serde_json::from_slice(args).map_err(|e| e.to_string())?;
        self.with(|app| {
            app.caller(self.caller.clone())
                .call(method, args)
                .map(|v| v.to_string().into_bytes())
                .map_err(|f| failure_text(&f))
        })
    }
}

/// `mock_http`'s handlers, by host.
struct HarnessHttp(BTreeMap<String, HttpHandler>);

impl HttpHost for HarnessHttp {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, String> {
        let rest = request
            .url
            .split_once("://")
            .map_or(request.url.as_str(), |(_, r)| r);
        let host = rest.split(['/', ':', '?']).next().unwrap_or_default();
        let handler = self
            .0
            .get(host)
            .ok_or_else(|| format!("no mock for host {host} (t.mock_http)"))?;
        Ok(handler(request))
    }
}

/// An xcall a logged run sent (D25), as the platform would deliver it.
#[derive(Clone, Debug, PartialEq)]
pub struct SentXcall {
    /// The target app.
    pub app: String,
    /// The target method.
    pub method: String,
    /// The JSON arguments.
    pub args: Value,
}

/// One engine and compile cache for the whole test process: compiled components are keyed by
/// their SHA-256, so tests share them. The cache directory lives until the process ends.
fn shared_engine() -> &'static WasmtimeEngine {
    static ENGINE: OnceLock<(WasmtimeEngine, tempfile::TempDir)> = OnceLock::new();
    &ENGINE
        .get_or_init(|| {
            // SAFETY: a test harness cannot continue without its engine; panicking fails the test.
            let dir = tempfile::tempdir().expect("harness cache dir");
            // The cache lives in a directory this process just created and owns, so a
            // fixed key is enough to authenticate its artifacts (S8 binds nodes, not tests).
            // SAFETY: as above.
            let engine =
                WasmtimeEngine::new(dir.path().to_path_buf(), &[0; 32]).expect("wasmtime engine");
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
            blobs: BTreeMap::new(),
            outbox: Vec::new(),
            started: Vec::new(),
            next_action: 0,
            http: BTreeMap::new(),
            in_action: false,
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
                HostAudience::OrgMembers(_)
                | HostAudience::OrgAdmins(_)
                | HostAudience::Team(_)
                    if e.frozen.is_some() =>
                {
                    e.frozen
                        .as_ref()
                        .is_some_and(|f| f.iter().any(|a| a == account))
                }
                HostAudience::All => true,
                HostAudience::Caller => e.emitter.as_deref() == Some(account),
                HostAudience::Accounts(list) => list.iter().any(|a| a == account),
                HostAudience::Team(team) => self
                    .org
                    .teams
                    .get(team)
                    .is_some_and(|m| m.contains(account)),
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

    /// Removes the account from the org. Org and team events emitted so far stay with the accounts
    /// that were in the audience when it changed; later ones no longer reach `account`.
    pub fn remove_member(&mut self, account: &str) {
        let org = self.org.clone();
        for e in self.events.iter_mut().filter(|e| e.frozen.is_none()) {
            let min = match &e.audience {
                HostAudience::OrgMembers(_) => OrgRole::Member,
                HostAudience::OrgAdmins(_) => OrgRole::Admin,
                HostAudience::Team(team) => {
                    e.frozen = Some(org.team_members(team));
                    continue;
                }
                _ => continue,
            };
            let reached = org
                .accounts()
                .filter(|a| org.role(a).is_some_and(|r| r >= min));
            e.frozen = Some(reached.collect());
        }
        self.org.remove(account);
    }

    /// Uploads a blob as a client would (`POST /apps/{app}/blobs`): the app holds it from
    /// now on and runs may refer to it. Returns its id (64 lowercase hex digits).
    pub fn upload_blob(&mut self, bytes: &[u8], mime: &str) -> String {
        use sha2::Digest as _;
        let sha: [u8; 32] = sha2::Sha256::digest(bytes).into();
        let _prev = self.blobs.insert(
            sha,
            BlobInfo {
                size: bytes.len() as u64,
                sha256: sha.to_vec(),
                mime: mime.to_owned(),
            },
        );
        hex32(&sha)
    }

    /// How many stored values refer to blob `id` (hex): what the node's retention counts.
    #[must_use]
    pub fn blob_refs(&self, id: &str) -> usize {
        self.state
            .values()
            .map(|v| blob_refs(v).iter().filter(|r| hex32(r) == id).count())
            .sum()
    }

    /// The xcalls logged runs sent so far and the harness has not handed out yet, oldest
    /// first; the node would deliver them in this order (D25).
    pub fn take_xcalls(&mut self) -> Vec<SentXcall> {
        std::mem::take(&mut self.outbox)
    }

    /// Delivers an xcall from app `from` (D13: `Caller::App(from)`, kind `xcall-deliver`),
    /// after the node's checks: the method must exist, be a call or a view and allow `from`
    /// (`allow(app = ..)`), else `Forbidden`.
    ///
    /// # Errors
    ///
    /// The delivery's `Failure`.
    pub fn deliver(&mut self, from: &str, method: &str, args: Value) -> Result<Value, Failure> {
        let caller = HostCaller::App(from.to_owned());
        let args = args_bytes(&args);
        if let Some(f) = self.gate(method, &caller, false, Some(&args)) {
            return Err(f);
        }
        self.run(InvocationKind::XcallDeliver, caller, method, args, None)
    }

    /// Answers an action's HTTP requests to `host` (D12) with `handler`; a request to a
    /// host without one fails in the action (`Fetch`).
    pub fn mock_http(
        &mut self,
        host: &str,
        handler: impl Fn(&HttpRequest) -> HttpResponse + Send + Sync + 'static,
    ) {
        let _prev = self.http.insert(host.to_owned(), Arc::new(handler));
    }

    /// Runs every action logged runs started so far (`start_action`, or a call of an
    /// action), oldest first, each as the caller who started it; their results.
    pub fn run_started_actions(&mut self) -> Vec<Result<Value, Failure>> {
        let started = std::mem::take(&mut self.started);
        started
            .into_iter()
            .map(|(caller, a)| self.action_as(caller, &a.id, &a.method, a.args))
            .collect()
    }

    /// Runs `method` as an action (D12) for `caller`, with key `key`: no state access,
    /// views and calls on this harness as `caller`, HTTP through `mock_http`. Retries and
    /// the 10 s budget are the node's.
    fn action_as(
        &mut self,
        caller: HostCaller,
        key: &str,
        method: &str,
        args: Vec<u8>,
    ) -> Result<Value, Failure> {
        let inv = Invocation {
            kind: InvocationKind::Action,
            method: method.to_owned(),
            args,
            caller: caller.clone(),
            entry_ts_ms: self.now_ms,
            id_term: 0,
            id_seq: 0,
            seed: [0; 32],
            timer: None,
        };
        let (engine, prepared) = (self.engine, self.prepared.clone());
        let http = Arc::new(HarnessHttp(self.http.clone()));
        let dir = Arc::new(HarnessDir(self.org.clone()));
        // The action lends the harness to its own sub-calls while it runs.
        let blank = self.blank();
        let lent = Arc::new(std::sync::Mutex::new(std::mem::replace(self, blank)));
        let runner = Arc::new(HarnessAction {
            app: Arc::clone(&lent),
            caller,
            key: key.to_owned(),
        });
        let out = match HostCore::new(APP_ID, Arc::new(BTreeMap::new()), 0, inv, dir) {
            Ok(host) => engine.invoke(
                &prepared,
                host.with_action(runner).with_http(http),
                Budget::action(),
            ),
            Err(e) => {
                *self = take_back(lent);
                return Err(Failure::BadOutput(e.to_string()));
            }
        };
        *self = take_back(lent);
        self.usage = out.usage;
        self.finish(true, None, out)
    }

    fn is_action(&self, method: &str) -> bool {
        self.prepared
            .abi
            .methods
            .get(method)
            .is_some_and(|m| m.kind == MethodKind::Action)
    }

    /// Log the start of action `method` (D17) as a call would: its key, queued for
    /// `run_started_actions`.
    fn start(&mut self, caller: HostCaller, method: &str, args: Vec<u8>) -> Value {
        self.seq += 1;
        self.last_entry_ms = self.now_ms;
        let id = format!("a.{TERM}.{}.0", self.seq);
        self.started.push((
            caller,
            ActionStart {
                id: id.clone(),
                method: method.to_owned(),
                args,
            },
        ));
        serde_json::json!({ "actionId": id })
    }

    /// A harness with nothing in it, standing in while an action holds the real one.
    fn blank(&self) -> Self {
        Self {
            engine: self.engine,
            prepared: self.prepared.clone(),
            state: BTreeMap::new(),
            org: self.org.clone(),
            now_ms: self.now_ms,
            last_entry_ms: self.last_entry_ms,
            seed: 0,
            draws: 0,
            seq: 0,
            last_tick: BTreeMap::new(),
            events: Vec::new(),
            usage: Usage::default(),
            blobs: BTreeMap::new(),
            outbox: Vec::new(),
            started: Vec::new(),
            next_action: 0,
            http: BTreeMap::new(),
            in_action: false,
        }
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
        let (seq_before, entry_before) = (self.seq, self.last_entry_ms);
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
            caller: caller.clone(),
            entry_ts_ms,
            id_term: TERM,
            id_seq: self.seq,
            seed: self.next_seed(),
            timer,
        };
        let state = Arc::new(self.state.clone());
        let search = Arc::new(HarnessSearch {
            state: Arc::clone(&state),
            fields: self.prepared.abi.search.clone(),
        });
        let host = HostCore::new(
            APP_ID,
            state,
            self.seq,
            inv,
            Arc::new(HarnessDir(self.org.clone())),
        )
        // SAFETY: the harness builds timer slots exactly for timer invocations.
        .expect("harness builds valid invocations")
        .with_blobs(Arc::new(HarnessBlobs(self.blobs.clone())))
        .with_search(search);
        let budget = if is_view {
            Budget::view()
        } else {
            Budget::call()
        };
        let out = self.engine.invoke(&self.prepared, host, budget);
        self.usage = out.usage;
        if matches!(
            out.status,
            Status::Trap {
                kind: TrapKind::Timeout,
                ..
            }
        ) {
            // A timeout is not logged (D23): it consumes no `id_seq` and moves no entry time.
            self.seq = seq_before;
            self.last_entry_ms = entry_before;
        } else if !is_view {
            self.last_entry_ms = self.now_ms;
        }
        if out.status == Status::Ok && !is_view {
            self.started
                .extend(out.actions.iter().cloned().map(|a| (caller.clone(), a)));
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
                let result = if out.result.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&out.result)
                        .map_err(|e| Failure::BadOutput(format!("result is not JSON: {e}")))?
                };
                let mut events = Vec::new();
                for e in out.events {
                    let data = serde_json::from_slice(&e.data).map_err(|err| {
                        Failure::BadOutput(format!("event {} data is not JSON: {err}", e.name))
                    })?;
                    events.push((e, data));
                }
                let mut sent = Vec::new();
                for x in out.xcalls {
                    let args = serde_json::from_slice(&x.args).map_err(|e| {
                        Failure::BadOutput(format!("xcall {} args are not JSON: {e}", x.method))
                    })?;
                    sent.push(SentXcall {
                        app: x.app,
                        method: x.method,
                        args,
                    });
                }
                if !is_view {
                    self.outbox.extend(sent);
                    for w in out.diff {
                        match w.value {
                            Some(v) => self.state.insert(w.key, v),
                            None => self.state.remove(&w.key),
                        };
                    }
                    for (e, data) in events {
                        self.events.push(Logged {
                            name: e.name,
                            data,
                            audience: e.audience,
                            emitter: emitter.clone(),
                            frozen: None,
                        });
                    }
                }
                Ok(result)
            }
            Status::AppError { code, message } => Err(Failure::App {
                code,
                fields: serde_json::from_str(&message).unwrap_or(Value::String(message)),
            }),
            Status::Trap { kind, .. } => Err(Failure::Trap(kind)),
        }
    }

    /// The checks a node makes before `handle`, with its codes (Rulings 32, 35). First the
    /// platform's access gate: the harness app is an org app, so an account the org does not
    /// hold (never in it, or removed) is `404 NOT_FOUND`, and every anonymous caller is
    /// `401 UNAUTHENTICATED`, whatever the method's `public` flag (on a node only a public app
    /// lets anonymous callers reach the ABI). Then the ABI's, as the node makes them: a method
    /// the ABI does not name (or a reserved `__` name) `UnknownMethod`, `init` and timers
    /// `WrongKind`, a `confirm` method as a view `ConfirmNotAView`, any other non-view as a
    /// view `NotAView`, `Forbidden` for System without `allowSystem`, `ConfirmRequired`.
    fn gate(
        &self,
        method: &str,
        caller: &HostCaller,
        confirmed: bool,
        call_args: Option<&[u8]>,
    ) -> Option<Failure> {
        let view = call_args.is_none();
        if let HostCaller::Account(a) | HostCaller::ApiKey { account: a, .. } = caller {
            if self.org.role(a).is_none() {
                return Some(Failure::Refused {
                    status: 404,
                    code: "NOT_FOUND".to_owned(),
                });
            }
        }
        if matches!(caller, HostCaller::Public) {
            return Some(Failure::Refused {
                status: 401,
                code: "UNAUTHENTICATED".to_owned(),
            });
        }
        // A call's arguments over the node's cap are refused before anything runs.
        if call_args.is_some_and(|a| a.len() > MAX_ARGS_BYTES) {
            return Some(Failure::Refused {
                status: 413,
                code: "ARGS_TOO_LARGE".to_owned(),
            });
        }
        // The node's ABI checks (`node::abi_check`), in its order and with its codes.
        let refuse = |code: &str| {
            Some(Failure::App {
                code: code.to_owned(),
                fields: Value::Object(serde_json::Map::new()),
            })
        };
        let Some(m) = self
            .prepared
            .abi
            .methods
            .get(method)
            .filter(|_| !method.starts_with(RESERVED_PREFIX))
        else {
            return refuse("UnknownMethod");
        };
        let starts_action = m.kind == MethodKind::Action && !view && !self.in_action;
        if !matches!(m.kind, MethodKind::Call | MethodKind::View) && !starts_action {
            return refuse("WrongKind");
        }
        if m.action_only && !self.in_action {
            return refuse("Forbidden");
        }
        if view && m.confirm {
            return refuse("ConfirmNotAView");
        }
        if view && m.kind != MethodKind::View {
            return refuse("NotAView");
        }
        match caller {
            HostCaller::System if !m.allow_system => refuse("Forbidden"),
            HostCaller::App(app) if !m.allows_app(app) => refuse("Forbidden"),
            _ if m.confirm && !view && !confirmed => Some(Failure::ConfirmRequired),
            _ => None,
        }
    }

    /// The stored key of a key the app wrote, for tests that read state directly.
    #[must_use]
    pub fn stored_key(key: &[u8]) -> Vec<u8> {
        host_key(calimero_app_host::GENERATION, key)
    }
}

/// The harness back from an action's runner, which dropped its handle when the run ended.
fn take_back(lent: Arc<std::sync::Mutex<TestApp>>) -> TestApp {
    match Arc::try_unwrap(lent) {
        Ok(m) => m
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        // SAFETY: the engine drops the store, and with it the runner's handle, before
        // `invoke` returns; nothing else holds one.
        Err(_) => panic!("harness: an action's runner outlived its run"),
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
        let args = args_bytes(&args);
        if let Some(f) = self
            .app
            .gate(method, &self.caller, self.confirmed, Some(&args))
        {
            return Err(f);
        }
        // A call of an action starts it, as on a node (D17): logged, run later.
        if self.app.is_action(method) {
            return Ok(self.app.start(self.caller.clone(), method, args));
        }
        self.app.run(
            InvocationKind::Call,
            self.caller.clone(),
            method,
            args,
            None,
        )
    }

    /// Starts `method` as an action and runs it at once (spec section 10: `t.as_("bob")
    /// .action("enrich_company", args)`), as this caller; its result.
    ///
    /// # Errors
    ///
    /// The node's refusals before it starts, or the action's `Failure`.
    pub fn action(&mut self, method: &str, args: Value) -> Result<Value, Failure> {
        let args = args_bytes(&args);
        if let Some(f) = self.app.gate(method, &self.caller, false, Some(&args)) {
            return Err(f);
        }
        if !self.app.is_action(method) {
            return Err(Failure::App {
                code: "WrongKind".to_owned(),
                fields: Value::Object(serde_json::Map::new()),
            });
        }
        self.app.next_action += 1;
        let key = format!("a.harness.{}", self.app.next_action);
        self.app.action_as(self.caller.clone(), &key, method, args)
    }

    /// Runs a view: nothing is logged and nothing written.
    ///
    /// # Errors
    ///
    /// The view's `Failure`.
    pub fn view(&mut self, method: &str, args: Value) -> Result<Value, Failure> {
        if let Some(f) = self.app.gate(method, &self.caller, self.confirmed, None) {
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
