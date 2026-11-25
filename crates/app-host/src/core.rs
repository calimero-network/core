//! The host core: the only code that reads or writes app state, applies access policy or
//! decides what an invocation may do (spec section 3). Engines map WIT calls onto it.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use ciborium::value::Integer;
use ciborium::Value;
use rand::Rng;
use rand_chacha::ChaCha20Rng;
use sha2::{Digest, Sha256};

use crate::abi::{Abi, MethodKind};
use crate::action::{
    subcall_key, ActionHost, ActionStart, HttpExchange, HttpHost, HttpRequest, HttpResponse,
    NoHttp, MAX_ACTION_SUBCALLS, MAX_HTTP_BODY_BYTES, MAX_HTTP_REQUESTS, MAX_STARTED_ACTIONS,
};
use crate::blobs::{blob_id_hex, blob_refs, parse_blob_id, BlobHost, NoBlobs, MAX_GRANT_TTL_SECS};
use crate::files::{Files, NoFiles, FILE_UNAVAILABLE};
use crate::fuel::{
    per_byte, SearchBudget, FUEL_HOST_CALL, FUEL_PER_RANGE_ENTRY, FUEL_PER_SEARCH_BYTE,
    FUEL_PER_SEARCH_HIT, FUEL_PER_SEARCH_MATCH, FUEL_SEARCH_QUERY, FUEL_STATE_ACCESS,
    SEARCH_TOO_MANY,
};
use crate::overlay::{Cursor, Overlay};
use crate::runs::{
    ActiveRun, NoRuns, RunInput, Runs, StartedRun, MAX_ACTIVE_RUNS, MAX_COMPUTE_ARGS_BYTES,
    MAX_RUN_INPUTS, MAX_RUN_INPUT_BYTES, MAX_STARTS_PER_RUN, NOT_A_COMPUTE_METHOD, NO_ACTIVE_RUN,
    NO_COMPUTE_COMPONENT, RUN_ARGS_TOO_LARGE, RUN_DUPLICATE_INPUT, RUN_INPUTS_TOO_LARGE,
    RUN_INPUT_MISSING, RUN_RECORDS_UNREADABLE, RUN_TOO_MANY_ACTIVE, RUN_TOO_MANY_INPUTS,
    RUN_TOO_MANY_STARTS,
};
use crate::search::{
    NoSearch, SearchHost, SearchPage, SearchQuery, MAX_SEARCH_LIMIT, MAX_SEARCH_QUERY_BYTES,
};
use crate::state::{stored_key, KeyValue, StateView, GENERATION};
use crate::time;
use crate::types::{
    Audience, BlobInfo, Caller, Directory, Event, HostDenied, InvalidInvocation, Invocation,
    InvocationKind, OrgRole, Outcome, Status, TrapKind, Usage, Xcall, WALL_CLOCK_EXCEEDED,
};

mod file_ops;

pub use file_ops::{ReaderId, WriterId};

use file_ops::FileTable;

/// Bytes of an app key (before the generation prefix). Values have no separate cap:
/// [`MAX_RUN_BYTES`] bounds them as they are written.
pub const MAX_KEY_BYTES: usize = 1_024;
/// Bytes one run may write: the keys and values of every `put`, `add` and
/// `delete` (each overwrite counts again: the overlay keeps every version) plus the name
/// and data of every event. The host call that would pass it is refused with
/// `HostDenied("entry too large")` before the bytes are kept, so the overlay and the events
/// of a run never hold more than this. Equal to the executor's log-entry cap
/// (`calimero_log::MAX_ENTRY_BYTES`, 1 MiB), which still checks the encoded entry.
pub const MAX_RUN_BYTES: usize = 1024 * 1024;
/// The refusal of a write past [`MAX_RUN_BYTES`]: the executor's entry-size refusal, made
/// early.
pub const ENTRY_TOO_LARGE: &str = "entry too large";
/// Writes per run: `put`, `delete` and `add` count one each; the next one is refused.
pub const MAX_WRITES: u32 = 10_000;
/// The trap detail of a write past [`MAX_WRITES`] through the trapping `storage.put`,
/// `delete` and `add` (components built before WIT 0.5.7).
pub const TOO_MANY_WRITES: &str = "more than 10,000 writes in one run";

/// Which of a run's write limits a refused write passed (WIT `run-limit`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunLimitKind {
    /// [`MAX_WRITES`].
    Writes,
    /// [`MAX_RUN_BYTES`].
    Bytes,
}

/// A write, event, xcall or started action past one of the run's limits, answered to the
/// guest as a soft error (WIT `limit-exceeded`, 0.5.7) rather than a trap. It depends only
/// on what the run wrote before it, so every node answers the same. Nothing of the refused
/// call is kept or counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunLimit {
    pub limit: RunLimitKind,
    /// The limit: [`MAX_WRITES`] or [`MAX_RUN_BYTES`].
    pub max: u64,
    /// What the run would have reached with the refused call: the write's number, or the
    /// run's bytes with it.
    pub attempted: u64,
}

impl RunLimit {
    /// The trap a component built before WIT 0.5.7 gets through `storage.put`, `delete` or
    /// `add`: the details it always got.
    #[must_use]
    pub fn denied(self) -> HostDenied {
        HostDenied::new(match self.limit {
            RunLimitKind::Writes => TOO_MANY_WRITES,
            RunLimitKind::Bytes => ENTRY_TOO_LARGE,
        })
    }
}

impl std::fmt::Display for RunLimit {
    /// The soft error string of `emit`, `xcall.send` and `start-action`: [`ENTRY_TOO_LARGE`]
    /// (or [`TOO_MANY_WRITES`]) with the run's total and the limit.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.limit {
            RunLimitKind::Writes => write!(
                f,
                "{TOO_MANY_WRITES}: write {} of at most {}",
                self.attempted, self.max
            ),
            RunLimitKind::Bytes => write!(
                f,
                "{ENTRY_TOO_LARGE}: the run would write {} bytes, over {}",
                self.attempted, self.max
            ),
        }
    }
}
/// Entries one `range_next` returns at most.
pub const MAX_RANGE_BATCH: u32 = 1_000;
/// Bytes of keys and values one `range_next` returns at most, past its first entry (which
/// is always returned, so a cursor never stalls). The host gathers a batch before the
/// guest sees it, so without it 1,000 values of about 1 MiB each (a value is bounded only
/// by [`MAX_RUN_BYTES`] as it is written) would make the host hold about 1 GiB for one
/// call, far past the 64 MiB a guest can take. 4 MiB stays well below that.
pub const MAX_RANGE_BATCH_BYTES: usize = 4 * 1024 * 1024;
/// Range cursors one run may hold open at once.
pub const MAX_OPEN_CURSORS: usize = 1_024;
/// Bytes of app log one run keeps; later lines are dropped after one marker line.
pub const MAX_LOG_BYTES: usize = 64 * 1024;

/// Bytes of a compute run's result; it travels in a log entry.
pub const MAX_COMPUTE_RESULT_BYTES: usize = 64 * 1024;
/// Bytes of a progress step label; longer ones are cut.
pub const MAX_PROGRESS_STEP_BYTES: usize = 128;
const RESULT_TOO_LARGE: &str = "result too large";
const NOT_A_COMPUTE_RUN: &str = "progress is only for compute runs";

/// Events one run may emit (Rooms emits up to 500 per timer tick).
pub const MAX_EVENTS: usize = 1_000;
/// Bytes of an event name.
pub const MAX_EVENT_NAME_BYTES: usize = 100;
/// Bytes of an event's data.
pub const MAX_EVENT_DATA_BYTES: usize = 16 * 1024;
/// Accounts one event's `accounts` audience may name: its strings count against the
/// run's bytes too.
pub const MAX_AUDIENCE_ACCOUNTS: usize = 1_000;

/// `emit`'s soft errors (the WIT `result<_, string>`), fixed so every engine and SDK
/// returns and matches the same strings.
pub const EMIT_TOO_MANY: &str = "too many events";
pub const EMIT_NAME_TOO_LONG: &str = "event name too long";
pub const EMIT_DATA_TOO_LARGE: &str = "event data too large";
pub const EMIT_BAD_AUDIENCE: &str = "malformed audience";
pub const EMIT_UNKNOWN_TEAM: &str = "unknown team";
pub const EMIT_FOREIGN_ORG: &str = "audience names another org";
pub const EMIT_TOO_MANY_ACCOUNTS: &str = "too many audience accounts";

/// The refusal of an `action.*` call outside an action, or without the node's runner.
const NOT_AN_ACTION: &str = "only an action has an action context";
/// xcalls one run may send.
pub const MAX_XCALLS: usize = 100;
/// Bytes of an xcall's target app id or method name.
pub const MAX_XCALL_NAME_BYTES: usize = 128;
/// `xcall.send`'s soft errors (the WIT `result<_, string>`).
pub const XCALL_TOO_MANY: &str = "too many xcalls";
pub const XCALL_BAD_TARGET: &str = "malformed xcall target";
const LOG_TRUNCATED: &str = "[log truncated]";

/// Where a compute run's progress goes: percent (0-100) and a step label.
pub type Progress = Arc<dyn Fn(u8, &str) + Send + Sync>;

/// A range cursor handed to the guest; the WIT `cursor` resource maps onto it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CursorId(pub u32);

/// The host side of one invocation. Built per run, consumed by `finish`.
///
/// It owns or shares everything it reads, so it is `Send + 'static` and can be a
/// wasmtime `Store`'s data.
pub struct HostCore {
    app_id: String,
    state: Arc<dyn StateView>,
    applied_index: u64,
    inv: Invocation,
    /// The app a person's call came through ([`HostCore::with_via`]).
    via: Option<String>,
    dir: Arc<dyn Directory>,
    blobs: Arc<dyn BlobHost>,
    search: Arc<dyn SearchHost>,
    action: Option<Arc<dyn ActionHost>>,
    http: Arc<dyn HttpHost>,
    /// Other apps this run may reach synchronously; `None`: it reaches none.
    xcall_host: Option<Arc<dyn crate::xcall::XcallHost>>,
    /// Synchronous xcalls made so far ([`crate::MAX_SYNC_XCALLS`]).
    sync_xcalls: u32,
    /// Per (app, method, args hash), the synchronous calls made so far.
    sync_occurrences: BTreeMap<(String, String, Vec<u8>), u32>,
    /// Actions this run started.
    started: Vec<ActionStart>,
    /// Per (method, args hash), the calls this action made so far.
    occurrences: BTreeMap<(String, Vec<u8>), u32>,
    subcalls: u32,
    http_requests: u32,
    /// A logged run's outbound exchanges, for its entry.
    http_log: Vec<HttpExchange>,
    overlay: Overlay,
    writes: u32,
    /// Bytes written so far ([`MAX_RUN_BYTES`]).
    run_bytes: usize,
    events: Vec<Event>,
    xcalls: Vec<Xcall>,
    logs: Vec<String>,
    log_bytes: usize,
    /// Whether `log` keeps lines at all (`with_logs`); off unless the embedder asks.
    keep_logs: bool,
    unique_ids: u64,
    rng: ChaCha20Rng,
    cursors: BTreeMap<u32, Cursor>,
    next_cursor: u32,
    peak_buffered: crate::overlay::Buffered,
    provider: Arc<dyn Files>,
    files: FileTable,
    progress: Option<Progress>,
    abi: Abi,
    runs: Arc<dyn Runs>,
    /// Whether the app's running release ships a compute component; `start` needs one.
    compute_component: bool,
    /// The app's active runs, read on the first `start` or `cancel`.
    active: Option<Vec<ActiveRun>>,
    starts: Vec<StartedRun>,
    cancels: Vec<String>,
    /// Fuel the run's host calls owe and the engine has not taken yet (`crate::fuel`).
    host_fuel: Cell<u64>,
    /// The searches the request has left, shared with the views its xcalls reach.
    searches: SearchBudget,
    /// When the run's wall clock ends (`with_deadline`); `None`: host calls wait as long as
    /// their hosts do.
    deadline: Option<Instant>,
    /// Whether a host call met the deadline: `finish` answers `Timeout`.
    timed_out: bool,
}

impl HostCore {
    /// `state` is the committed state at `applied_index`: the leader's latest for calls,
    /// the index the view reads at for views. A timer invocation must carry its
    /// slot and no other kind may.
    pub fn new(
        app_id: &str,
        state: Arc<dyn StateView>,
        applied_index: u64,
        inv: Invocation,
        dir: Arc<dyn Directory>,
    ) -> Result<Self, InvalidInvocation> {
        match (inv.kind == InvocationKind::Timer, inv.timer.is_some()) {
            (true, false) => return Err(InvalidInvocation::TimerWithoutSlot),
            (false, true) => return Err(InvalidInvocation::SlotOutsideTimer),
            _ => {}
        }
        if inv.kind != InvocationKind::Action && inv.compute.is_some() {
            return Err(InvalidInvocation::RunOutsideAction);
        }
        let rng = time::entropy(&inv);
        Ok(Self {
            app_id: app_id.to_owned(),
            state,
            applied_index,
            inv,
            via: None,
            dir,
            blobs: Arc::new(NoBlobs),
            search: Arc::new(NoSearch),
            action: None,
            http: Arc::new(NoHttp),
            xcall_host: None,
            sync_xcalls: 0,
            sync_occurrences: BTreeMap::new(),
            started: Vec::new(),
            occurrences: BTreeMap::new(),
            subcalls: 0,
            http_requests: 0,
            http_log: Vec::new(),
            overlay: Overlay::default(),
            writes: 0,
            run_bytes: 0,
            events: Vec::new(),
            xcalls: Vec::new(),
            logs: Vec::new(),
            log_bytes: 0,
            keep_logs: false,
            unique_ids: 0,
            rng,
            cursors: BTreeMap::new(),
            next_cursor: 0,
            peak_buffered: crate::overlay::Buffered::default(),
            provider: Arc::new(NoFiles),
            files: FileTable::default(),
            progress: None,
            abi: Abi::default(),
            runs: Arc::new(NoRuns),
            compute_component: true,
            active: None,
            starts: Vec::new(),
            cancels: Vec::new(),
            host_fuel: Cell::new(0),
            searches: SearchBudget::default(),
            deadline: None,
            timed_out: false,
        })
    }

    /// When the run's wall clock ends. The engine's own deadline counts only while the
    /// guest's code runs, so a host call that may wait (`http_send`, `xcall_*`,
    /// `action_view`, `action_call`) is refused once it has passed and ends the run if it
    /// returns after it: either way the run ends `Trap(Timeout, WALL_CLOCK_EXCEEDED)`, as a
    /// run that overran in its own code does. The embedder hands the same instant to the
    /// hosts that wait, so none waits past it.
    #[must_use]
    pub fn with_deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// `HostDenied` once the deadline has passed, and the run marked timed out.
    fn within_deadline(&mut self) -> Result<(), HostDenied> {
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            self.timed_out = true;
            return Err(HostDenied::new(WALL_CLOCK_EXCEEDED));
        }
        Ok(())
    }

    /// Runs `wait`, a host call that may block, within the run's deadline
    /// ([`Self::with_deadline`]).
    fn blocking<T>(&mut self, wait: impl FnOnce(&Self) -> T) -> Result<T, HostDenied> {
        self.within_deadline()?;
        let answer = wait(self);
        self.within_deadline()?;
        Ok(answer)
    }

    /// The app's blobs as the node records them; without it the app has none.
    #[must_use]
    pub fn with_blobs(mut self, blobs: Arc<dyn BlobHost>) -> Self {
        self.blobs = blobs;
        self
    }

    /// The node's search index of the app (spec section 7); without it no table is
    /// searchable.
    #[must_use]
    pub fn with_search(mut self, search: Arc<dyn SearchHost>) -> Self {
        self.search = search;
        self
    }

    /// The node's runner for an action: its key, views and calls.
    #[must_use]
    pub fn with_action(mut self, action: Arc<dyn ActionHost>) -> Self {
        self.action = Some(action);
        self
    }

    /// The other apps this run may reach synchronously (`xcall.call`, `xcall.view`);
    /// without it every one is refused with [`crate::XCALL_SYNC_UNAVAILABLE`].
    #[must_use]
    pub fn with_xcall_host(mut self, host: Arc<dyn crate::xcall::XcallHost>) -> Self {
        self.xcall_host = Some(host);
        self
    }

    /// The node's outbound gate for an action's HTTP; without it nothing is sent.
    #[must_use]
    pub fn with_http(mut self, http: Arc<dyn HttpHost>) -> Self {
        self.http = http;
        self
    }

    /// Keep the app's log lines (`log`, stdout, stderr) on the outcome. They may hold user
    /// data, so the default drops every line: only a development node or a local run asks.
    #[must_use]
    pub fn with_logs(mut self, keep: bool) -> Self {
        self.keep_logs = keep;
        self
    }

    /// Receives a compute run's `progress` calls; without it they are dropped.
    #[must_use]
    pub fn with_progress(mut self, progress: Progress) -> Self {
        self.progress = Some(progress);
        self
    }

    /// Serves `files` from the node's file store; the default is [`NoFiles`].
    #[must_use]
    pub fn with_files(mut self, files: Arc<dyn Files>) -> Self {
        self.provider = files;
        self
    }

    /// Lets the run start and cancel compute runs: `abi` is the running bundle's, `runs` the app's
    /// records; the default has no compute method and no active run.
    #[must_use]
    pub fn with_compute(mut self, abi: Abi, runs: Arc<dyn Runs>) -> Self {
        self.abi = abi;
        self.runs = runs;
        self
    }

    /// Whether the running release ships a compute component (the default is that it does): without
    /// one `start` answers `NO_COMPUTE_COMPONENT`.
    #[must_use]
    pub fn with_compute_component(mut self, present: bool) -> Self {
        self.compute_component = present;
        self
    }

    /// The searches the request this run serves has left: a sync xcall target shares its
    /// caller's ([`SearchBudget`]); the default is a fresh one of `MAX_SEARCHES_PER_RUN`.
    #[must_use]
    pub fn with_search_budget(mut self, searches: SearchBudget) -> Self {
        self.searches = searches;
        self
    }

    /// The request's search budget, for the views this run's xcalls reach.
    #[must_use]
    pub fn search_budget(&self) -> SearchBudget {
        self.searches.clone()
    }

    /// The fuel the host calls made since the last take owe; the engine takes it out of the
    /// run's fuel as each call returns and ends the run out of fuel when it cannot pay.
    pub fn take_host_fuel(&mut self) -> u64 {
        self.host_fuel.take()
    }

    /// Adds `fuel` to what the run owes for host work.
    fn burn(&self, fuel: u64) {
        self.host_fuel
            .set(self.host_fuel.get().saturating_add(fuel));
    }

    /// `base` plus `bytes` bytes.
    fn burn_call(&self, base: u64, bytes: usize) {
        self.burn(base.saturating_add(per_byte(bytes)));
    }

    /// The bytes a call was handed, owed before it checks anything.
    fn burn_bytes(&self, bytes: usize) {
        self.burn(per_byte(bytes));
    }

    /// The invocation being run; the engine reads kind, method and args from it.
    #[must_use]
    pub fn invocation(&self) -> &Invocation {
        &self.inv
    }

    // --- storage ---------------------------------------------------------------------

    pub fn storage_get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, HostDenied> {
        self.burn_bytes(key.len());
        self.may_read()?;
        check_key(key)?;
        let value = self.current(&stored_key(GENERATION, key));
        self.burn_call(FUEL_STATE_ACCESS, value.as_ref().map_or(0, Vec::len));
        Ok(value)
    }

    /// Sets `key` to `value`. A write past [`MAX_WRITES`] or [`MAX_RUN_BYTES`] is the inner
    /// [`RunLimit`] (WIT `put-checked`); every other refusal is the outer `HostDenied`. The
    /// bytes it was handed are owed even when it is refused.
    pub fn storage_put(
        &mut self,
        key: &[u8],
        value: &[u8],
    ) -> Result<Result<(), RunLimit>, HostDenied> {
        self.burn_bytes(key.len().saturating_add(value.len()));
        self.check_write()?;
        check_key(key)?;
        self.check_blob_refs(value)?;
        if let Err(limit) = self.count_write() {
            return Ok(Err(limit));
        }
        if let Err(limit) = self.charge(key.len().saturating_add(value.len())) {
            return Ok(Err(limit));
        }
        self.burn(FUEL_STATE_ACCESS);
        self.overlay
            .put(stored_key(GENERATION, key), value.to_vec());
        self.writes += 1;
        Ok(Ok(()))
    }

    /// Removes `key`; limits and fuel as [`HostCore::storage_put`].
    pub fn storage_delete(&mut self, key: &[u8]) -> Result<Result<(), RunLimit>, HostDenied> {
        self.burn_bytes(key.len());
        self.check_write()?;
        check_key(key)?;
        if let Err(limit) = self.count_write() {
            return Ok(Err(limit));
        }
        if let Err(limit) = self.charge(key.len()) {
            return Ok(Err(limit));
        }
        self.burn(FUEL_STATE_ACCESS);
        self.overlay.delete(stored_key(GENERATION, key));
        self.writes += 1;
        Ok(Ok(()))
    }

    /// Adds `delta` to the deterministic CBOR integer at `key` (absent is 0) without the
    /// guest reading it. One write; a non-integer value or an overflow is refused;
    /// limits and fuel as [`HostCore::storage_put`].
    pub fn storage_add(
        &mut self,
        key: &[u8],
        delta: i64,
    ) -> Result<Result<(), RunLimit>, HostDenied> {
        self.burn_bytes(key.len());
        self.check_write()?;
        check_key(key)?;
        if let Err(limit) = self.count_write() {
            return Ok(Err(limit));
        }
        let key = stored_key(GENERATION, key);
        let current = match self.current(&key) {
            None => 0,
            Some(bytes) => cbor_integer(&bytes)?,
        };
        let sum = current + i128::from(delta);
        let sum = Integer::try_from(sum)
            .map_err(|_| HostDenied::new("storage.add overflows a CBOR integer"))?;
        let mut value = Vec::new();
        ciborium::into_writer(&Value::Integer(sum), &mut value)
            .map_err(|e| HostDenied::new(format!("storage.add encode: {e}")))?;
        if let Err(limit) = self.charge((key.len() - GENERATION_PREFIX).saturating_add(value.len()))
        {
            return Ok(Err(limit));
        }
        self.burn_call(FUEL_STATE_ACCESS, value.len());
        self.overlay.put(key, value);
        self.writes += 1;
        Ok(Ok(()))
    }

    /// Opens a cursor over `[start, end)` (an empty `end` is the end of the keyspace). It
    /// reads the state as of opening: committed state plus the writes made before now.
    pub fn range_open(
        &mut self,
        start: &[u8],
        end: &[u8],
        reverse: bool,
    ) -> Result<CursorId, HostDenied> {
        self.burn_bytes(start.len().saturating_add(end.len()));
        self.may_read()?;
        check_key(start)?;
        check_key(end)?;
        if self.cursors.len() >= MAX_OPEN_CURSORS {
            return Err(HostDenied::new("too many open cursors"));
        }
        self.burn(FUEL_STATE_ACCESS);
        let start = stored_key(GENERATION, start);
        let end = if end.is_empty() {
            // The generation's own keyspace ends where the next generation starts.
            GENERATION
                .checked_add(1)
                .map_or_else(Vec::new, |next| stored_key(next, &[]))
        } else {
            stored_key(GENERATION, end)
        };
        let id = self.next_cursor;
        self.next_cursor = self
            .next_cursor
            .checked_add(1)
            .ok_or_else(|| HostDenied::new("cursor ids exhausted"))?;
        let _ = self
            .cursors
            .insert(id, Cursor::new(start, end, reverse, self.overlay.seq()));
        Ok(CursorId(id))
    }

    /// The next entries of a cursor, at most `max` (capped at `MAX_RANGE_BATCH`) and, past
    /// the first, at most `MAX_RANGE_BATCH_BYTES` of keys and values, with keys as the guest
    /// wrote them. A shorter batch is not the end: only an empty list is.
    pub fn range_next(&mut self, id: CursorId, max: u32) -> Result<Vec<KeyValue>, HostDenied> {
        let Some(cursor) = self.cursors.get_mut(&id.0) else {
            return Err(HostDenied::new("unknown cursor"));
        };
        let max = max.min(MAX_RANGE_BATCH) as usize;
        let batch = cursor.next_batch(
            &*self.state,
            &self.overlay,
            max,
            MAX_RANGE_BATCH_BYTES,
            &mut self.peak_buffered,
        );
        let bytes = batch.iter().fold(0usize, |n, (k, v)| {
            n.saturating_add(k.len()).saturating_add(v.len())
        });
        self.burn(
            FUEL_STATE_ACCESS
                .saturating_add(FUEL_PER_RANGE_ENTRY.saturating_mul(batch.len() as u64))
                .saturating_add(per_byte(bytes)),
        );
        Ok(batch
            .into_iter()
            .map(|(key, value)| (key[GENERATION_PREFIX..].to_vec(), value))
            .collect())
    }

    /// Drops a cursor (the WIT resource's destructor). Unknown ids are ignored.
    pub fn range_close(&mut self, id: CursorId) {
        // A resource's drop is not wrapped by the engine: its base is owed here.
        self.burn(FUEL_HOST_CALL);
        let _ = self.cursors.remove(&id.0);
    }

    // --- rebuild: migrations are not in this milestone --------------------------

    pub fn rebuild_get(&self, _key: &[u8]) -> Result<Option<Vec<u8>>, HostDenied> {
        Err(self.rebuild_denied())
    }

    pub fn rebuild_put(&mut self, _key: &[u8], _value: &[u8]) -> Result<(), HostDenied> {
        Err(self.rebuild_denied())
    }

    pub fn rebuild_delete(&mut self, _key: &[u8]) -> Result<(), HostDenied> {
        Err(self.rebuild_denied())
    }

    pub fn rebuild_add(&mut self, _key: &[u8], _delta: i64) -> Result<(), HostDenied> {
        Err(self.rebuild_denied())
    }

    pub fn rebuild_range(
        &mut self,
        _start: &[u8],
        _end: &[u8],
        _reverse: bool,
    ) -> Result<CursorId, HostDenied> {
        Err(self.rebuild_denied())
    }

    // --- context ---------------------------------------------------------------------

    #[must_use]
    pub fn caller(&self) -> &Caller {
        &self.inv.caller
    }

    /// The app this call came through (`context.via`): set by the node when another app
    /// called as the person it serves, who granted it this method (the caller is that
    /// person); none otherwise.
    #[must_use]
    pub fn via(&self) -> Option<&str> {
        self.via.as_deref()
    }

    /// Names the app a person's call came through. Only a run another app delivered (a
    /// delivery or a view) whose caller is that person, an account or an API key, carries
    /// one: anything else is the node's mistake, refused here.
    ///
    /// # Errors
    ///
    /// [`InvalidInvocation::ViaOutsideXcall`].
    pub fn with_via(mut self, via: Option<String>) -> Result<Self, InvalidInvocation> {
        let Some(via) = via else {
            self.via = None;
            return Ok(self);
        };
        let delivered = matches!(
            self.inv.kind,
            InvocationKind::XcallDeliver | InvocationKind::View
        );
        let person = matches!(self.inv.caller, Caller::Account(_) | Caller::ApiKey { .. });
        if !(delivered && person) || via.is_empty() {
            return Err(InvalidInvocation::ViaOutsideXcall);
        }
        self.via = Some(via);
        Ok(self)
    }

    #[must_use]
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    #[must_use]
    pub fn app_owner(&self) -> String {
        match &self.inv.compute {
            Some(run) => run.owner.clone(),
            None => self.dir.app_owner(),
        }
    }

    /// The app linked under `name` (`context.linked`); none in a compute run.
    #[must_use]
    pub fn linked(&self, name: &str) -> Option<String> {
        if self.is_compute() {
            return None;
        }
        self.dir.linked(name)
    }

    /// The app whose unit this one is a member of (`context.unit-of`); none in a compute run.
    #[must_use]
    pub fn unit_of(&self) -> Option<String> {
        if self.is_compute() {
            return None;
        }
        self.dir.unit_of()
    }

    /// The scope of the caller's grant on this app (`context.grant-scope`): the person's (an
    /// account or an API key's), as the directory holds it; none for any other caller and in
    /// a compute run.
    #[must_use]
    pub fn grant_scope(&self) -> Option<String> {
        if self.is_compute() {
            return None;
        }
        match &self.inv.caller {
            Caller::Account(account) | Caller::ApiKey { account, .. } => {
                self.dir.grant_scope(account)
            }
            _ => None,
        }
    }

    #[must_use]
    pub fn org(&self) -> Option<String> {
        if self.is_compute() {
            return None;
        }
        self.dir.org()
    }

    #[must_use]
    pub fn org_role(&self, account: &str) -> Option<OrgRole> {
        if self.is_compute() {
            return None;
        }
        self.dir.org_role(account)
    }

    /// Linearizable role read. The same as `org_role` in this milestone; refused during a
    /// migration (spec, env API).
    pub fn org_role_strict(&self, account: &str) -> Result<Option<OrgRole>, HostDenied> {
        if self.inv.kind == InvocationKind::Migrate {
            return Err(HostDenied::new("org_role_strict is not allowed in migrate"));
        }
        Ok(self.org_role(account))
    }

    #[must_use]
    pub fn orgs_of(&self, account: &str) -> Vec<(String, OrgRole)> {
        if self.is_compute() {
            return Vec::new();
        }
        self.dir.orgs_of(account)
    }

    #[must_use]
    pub fn in_team(&self, account: &str, team: &str) -> bool {
        !self.is_compute() && self.dir.in_team(account, team)
    }

    /// The log index the run reads at; a compute run has no log to read, so 0.
    #[must_use]
    pub fn applied_index(&self) -> u64 {
        if self.is_compute() {
            return 0;
        }
        self.applied_index
    }

    /// `"<id_term>.<id_seq>.<n>"`, `n` counting this run's calls from 0.
    /// Unique across runs because the leader never repeats `(term, seq)`; deterministic
    /// for a given invocation. Only in invocations that become log entries.
    pub fn unique_id(&mut self) -> Result<String, HostDenied> {
        if !self.is_logged_write() {
            return Err(self.denied("unique_id"));
        }
        let id = format!(
            "{}.{}.{}",
            self.inv.id_term, self.inv.id_seq, self.unique_ids
        );
        self.unique_ids += 1;
        Ok(id)
    }

    /// Starts a durable action: calls and timers only. The action is queued on the
    /// outcome, logged as started with the entry only if the run ends `Ok`, and run by the
    /// leader's runner after. Its key (`a.<term>.<seq>.<n>`, from the run's unique ids) is
    /// returned. More than [`MAX_STARTED_ACTIONS`] is the WIT result's error.
    pub fn start_action(
        &mut self,
        method: &str,
        args: &[u8],
    ) -> Result<Result<String, String>, HostDenied> {
        self.burn_bytes(method.len().saturating_add(args.len()));
        if !matches!(self.inv.kind, InvocationKind::Call | InvocationKind::Timer) {
            return Err(self.denied("start_action"));
        }
        if self.started.len() >= MAX_STARTED_ACTIONS {
            return Ok(Err("too many actions started".to_owned()));
        }
        if method.is_empty() || method.starts_with("__") {
            return Ok(Err("not an action".to_owned()));
        }
        if let Err(limit) = self.charge(method.len().saturating_add(args.len())) {
            return Ok(Err(limit.to_string()));
        }
        let id = format!("a.{}", self.unique_id()?);
        self.started.push(ActionStart {
            id: id.clone(),
            method: method.to_owned(),
            args: args.to_vec(),
        });
        Ok(Ok(id))
    }

    // --- compute -----------------------------------------------------------------------

    /// The run's id; none outside a compute run.
    #[must_use]
    pub fn compute_run_id(&self) -> Option<String> {
        self.inv.compute.as_ref().map(|run| run.run_id.clone())
    }

    /// The files the run may read; empty outside a compute run.
    #[must_use]
    pub fn compute_inputs(&self) -> Vec<String> {
        self.inv
            .compute
            .as_ref()
            .map_or_else(Vec::new, |run| run.inputs.clone())
    }

    /// Reports progress: `percent` is capped at 100 and `step` cut to
    /// `MAX_PROGRESS_STEP_BYTES` on a character boundary. Compute runs only; never logged.
    pub fn compute_progress(&mut self, percent: u8, step: &str) -> Result<(), HostDenied> {
        if !self.is_compute() {
            return Err(HostDenied::new(NOT_A_COMPUTE_RUN));
        }
        let end = step.floor_char_boundary(MAX_PROGRESS_STEP_BYTES);
        if let Some(progress) = &self.progress {
            progress(percent.min(100), &step[..end]);
        }
        Ok(())
    }

    /// Starts a compute run, or answers the active run that is the same (method, arguments and
    /// inputs). Logged writes only; the run exists once the entry that records it applies.
    pub fn compute_start(
        &mut self,
        method: &str,
        args: &[u8],
        inputs: &[String],
    ) -> Result<Result<String, String>, HostDenied> {
        self.burn_bytes(
            inputs
                .iter()
                .fold(method.len().saturating_add(args.len()), |n, i| {
                    n.saturating_add(i.len())
                }),
        );
        if !self.is_logged_write() {
            return Err(self.denied("runs.start"));
        }
        if !self.compute_component {
            return Ok(Err(NO_COMPUTE_COMPONENT.to_owned()));
        }
        let Some(MethodKind::Compute(caps)) = self.abi.methods.get(method).map(|m| m.kind.clone())
        else {
            return Ok(Err(NOT_A_COMPUTE_METHOD.to_owned()));
        };
        if args.len() > MAX_COMPUTE_ARGS_BYTES {
            return Ok(Err(RUN_ARGS_TOO_LARGE.to_owned()));
        }
        if inputs.len() > MAX_RUN_INPUTS {
            return Ok(Err(RUN_TOO_MANY_INPUTS.to_owned()));
        }
        let mut files = Vec::with_capacity(inputs.len());
        let mut total = 0u64;
        for id in inputs {
            if files.iter().any(|f: &RunInput| &f.id == id) {
                return Ok(Err(RUN_DUPLICATE_INPUT.to_owned()));
            }
            let Some(info) = self.provider.info(id) else {
                return Ok(Err(RUN_INPUT_MISSING.to_owned()));
            };
            total = total.saturating_add(info.size);
            files.push(RunInput {
                id: id.clone(),
                size: info.size,
                sha256: info.sha256,
            });
        }
        if total > MAX_RUN_INPUT_BYTES {
            return Ok(Err(RUN_INPUTS_TOO_LARGE.to_owned()));
        }
        let wanted = ActiveRun {
            run: String::new(),
            method: method.to_owned(),
            args_sha256: Sha256::digest(args).to_vec(),
            inputs: inputs.to_vec(),
            cancel_requested: false,
        };
        let started: Vec<ActiveRun> = self.starts.iter().map(StartedRun::to_active).collect();
        let cancelled = self.cancels.clone();
        if let Some(same) = (self.active_runs()?.iter())
            .chain(&started)
            .filter(|r| !r.cancel_requested && !cancelled.contains(&r.run))
            .find(|r| r.same_job(&wanted))
        {
            return Ok(Ok(same.run.clone()));
        }
        if self.starts.len() >= MAX_STARTS_PER_RUN {
            return Ok(Err(RUN_TOO_MANY_STARTS.to_owned()));
        }
        if self.active_runs()?.len() + self.starts.len() >= MAX_ACTIVE_RUNS {
            return Ok(Err(RUN_TOO_MANY_ACTIVE.to_owned()));
        }
        let run = self.unique_id()?;
        self.starts.push(StartedRun {
            run: run.clone(),
            method: method.to_owned(),
            args: args.to_vec(),
            inputs: files,
            caps,
        });
        Ok(Ok(run))
    }

    /// Asks for an active run to be cancelled. Logged writes only; recorded, not applied.
    pub fn compute_cancel(&mut self, run: &str) -> Result<Result<(), String>, HostDenied> {
        if !self.is_logged_write() {
            return Err(self.denied("runs.cancel"));
        }
        if !self.active_runs()?.iter().any(|r| r.run == run) {
            return Ok(Err(NO_ACTIVE_RUN.to_owned()));
        }
        if !self.cancels.iter().any(|c| c == run) {
            self.cancels.push(run.to_owned());
        }
        Ok(Ok(()))
    }

    pub(crate) fn active_runs(&mut self) -> Result<&[ActiveRun], HostDenied> {
        if self.active.is_none() {
            let active = self
                .runs
                .active()
                .map_err(|_| HostDenied::new(RUN_RECORDS_UNREADABLE))?;
            self.active = Some(active);
        }
        Ok(self.active.as_deref().unwrap_or_default())
    }

    // --- events, xcall, blobs, action ------------------------------------------------

    /// Queues an event, delivered only if the run ends `Ok`. A policy refusal is the
    /// outer `HostDenied`; the inner result is the WIT `result<_, string>`, one of the
    /// `EMIT_*` strings, and a refused event is not recorded.
    pub fn emit(&mut self, event: Event) -> Result<Result<(), String>, HostDenied> {
        self.burn_bytes(event_bytes(&event));
        if !self.is_logged_write() {
            return Err(self.denied("emit"));
        }
        if let Err(reason) = self.check_event(&event) {
            return Ok(Err(reason.to_owned()));
        }
        // The audience is kept with the event, so its strings count too.
        let audience = match &event.audience {
            Audience::All | Audience::Caller => 0,
            Audience::Accounts(accounts) => accounts
                .iter()
                .fold(0usize, |n, a| n.saturating_add(a.len())),
            Audience::Team(id) | Audience::OrgMembers(id) | Audience::OrgAdmins(id) => id.len(),
        };
        if let Err(limit) = self.charge(
            event
                .name
                .len()
                .saturating_add(event.data.len())
                .saturating_add(audience),
        ) {
            return Ok(Err(limit.to_string()));
        }
        self.events.push(event);
        Ok(Ok(()))
    }

    fn check_event(&self, event: &Event) -> Result<(), &'static str> {
        if self.events.len() >= MAX_EVENTS {
            return Err(EMIT_TOO_MANY);
        }
        if event.name.len() > MAX_EVENT_NAME_BYTES {
            return Err(EMIT_NAME_TOO_LONG);
        }
        if event.data.len() > MAX_EVENT_DATA_BYTES {
            return Err(EMIT_DATA_TOO_LARGE);
        }
        match &event.audience {
            Audience::All | Audience::Caller => Ok(()),
            Audience::Accounts(accounts) => {
                if accounts.is_empty() || accounts.iter().any(String::is_empty) {
                    return Err(EMIT_BAD_AUDIENCE);
                }
                if accounts.len() > MAX_AUDIENCE_ACCOUNTS {
                    return Err(EMIT_TOO_MANY_ACCOUNTS);
                }
                Ok(())
            }
            Audience::Team(team) => {
                if team.is_empty() {
                    return Err(EMIT_BAD_AUDIENCE);
                }
                if !self.dir.team_exists(team) {
                    return Err(EMIT_UNKNOWN_TEAM);
                }
                Ok(())
            }
            Audience::OrgMembers(org) | Audience::OrgAdmins(org) => {
                if org.is_empty() {
                    return Err(EMIT_BAD_AUDIENCE);
                }
                if self.dir.org().as_deref() != Some(org.as_str()) {
                    return Err(EMIT_FOREIGN_ORG);
                }
                Ok(())
            }
        }
    }

    /// Queues an xcall in the run's outbox: calls, timers, `init` and xcall
    /// deliveries only, delivered after the entry commits and only if the run ends `Ok`.
    /// A malformed target (empty, over [`MAX_XCALL_NAME_BYTES`], a reserved `__` method)
    /// or more than [`MAX_XCALLS`] is the WIT result's error; the arguments count toward
    /// [`MAX_RUN_BYTES`].
    pub fn xcall_send(
        &mut self,
        app: &str,
        method: &str,
        args: &[u8],
    ) -> Result<Result<(), String>, HostDenied> {
        self.burn_bytes(target_bytes(app, method, args));
        if !self.is_logged_write() {
            return Err(self.denied("xcall"));
        }
        if self.xcalls.len() >= MAX_XCALLS {
            return Ok(Err(XCALL_TOO_MANY.to_owned()));
        }
        let bad = |s: &str| s.is_empty() || s.len() > MAX_XCALL_NAME_BYTES;
        if bad(app) || bad(method) || method.starts_with("__") {
            return Ok(Err(XCALL_BAD_TARGET.to_owned()));
        }
        if let Err(limit) = self.charge(
            app.len()
                .saturating_add(method.len())
                .saturating_add(args.len()),
        ) {
            return Ok(Err(limit.to_string()));
        }
        self.xcalls.push(Xcall {
            app: app.to_owned(),
            method: method.to_owned(),
            args: args.to_vec(),
        });
        Ok(Ok(()))
    }

    /// Runs `method` of `app` synchronously, as one logged call of that app under a key
    /// derived from this run's (see [`crate::sync_xcall_key`]), and hands back its JSON
    /// result. Calls, timers and actions (`HostDenied` elsewhere: a delivered xcall makes
    /// none, so chains are one hop). A malformed target, the app itself, more than
    /// [`crate::MAX_SYNC_XCALLS`], arguments over [`crate::MAX_SYNC_XCALL_ARGS_BYTES`], an
    /// answer over [`crate::MAX_SYNC_XCALL_RESULT_BYTES`] or the target's failure is the
    /// WIT result's error.
    pub fn xcall_call(
        &mut self,
        app: &str,
        method: &str,
        args: &[u8],
    ) -> Result<Result<Vec<u8>, String>, HostDenied> {
        self.burn_bytes(target_bytes(app, method, args));
        if !self.may_xcall(crate::xcall::XcallKind::Call) {
            return Err(self.denied("xcall.call"));
        }
        let host = match self.sync_target(app, method, args) {
            Ok(host) => host,
            Err(e) => return Ok(Err(e)),
        };
        let key = self.next_sync_key(&host.key(), app, method, args);
        let answer = self.blocking(|_| bounded(host.call(app, method, args, &key)))?;
        Ok(self.answered(answer))
    }

    /// Runs view `method` of `app` on its latest committed state and hands back its JSON
    /// result. Calls, timers, views and actions (`HostDenied` elsewhere); refusals as for
    /// [`Self::xcall_call`].
    pub fn xcall_view(
        &mut self,
        app: &str,
        method: &str,
        args: &[u8],
    ) -> Result<Result<Vec<u8>, String>, HostDenied> {
        self.burn_bytes(target_bytes(app, method, args));
        if !self.may_xcall(crate::xcall::XcallKind::View) {
            return Err(self.denied("xcall.view"));
        }
        let host = match self.sync_target(app, method, args) {
            Ok(host) => host,
            Err(e) => return Ok(Err(e)),
        };
        let answer = self.blocking(|_| bounded(host.view(app, method, args)))?;
        Ok(self.answered(answer))
    }

    /// Runs every request of `requests` at once (the node's [`XcallHost::batch`]) and
    /// answers each in order, as [`Self::xcall_call`] and [`Self::xcall_view`] would have
    /// one after another: the same checks per request, each counted as one synchronous
    /// xcall, a call keyed with the occurrence it would have had in sequence (so a re-run
    /// replays it). A batch with a call where calls are refused, or any batch where views
    /// are, is `HostDenied`. A batch that would take the run past
    /// [`crate::MAX_SYNC_XCALLS`] is refused whole: every request that passed its own
    /// checks answers [`crate::XCALL_SYNC_TOO_MANY`] and none is counted.
    ///
    /// [`XcallHost::batch`]: crate::XcallHost::batch
    pub fn xcall_batch(
        &mut self,
        requests: &[crate::xcall::XcallRequest],
    ) -> Result<Vec<Result<Vec<u8>, String>>, HostDenied> {
        use crate::xcall::{SyncXcall, XcallKind};
        self.burn_bytes(requests.iter().fold(0usize, |n, r| {
            n.saturating_add(target_bytes(&r.app, &r.method, &r.args))
        }));
        if !self.may_xcall(XcallKind::View) {
            return Err(self.denied("xcall.batch"));
        }
        if requests.iter().any(|r| r.kind == XcallKind::Call) && !self.may_xcall(XcallKind::Call) {
            return Err(self.denied("xcall.call"));
        }
        let mut answers: Vec<Option<Result<Vec<u8>, String>>> = requests
            .iter()
            .map(|r| self.sync_check(&r.app, &r.method, &r.args).err().map(Err))
            .collect();
        let passed = answers.iter().filter(|a| a.is_none()).count();
        if passed == 0 {
            return Ok(answers.into_iter().flatten().collect());
        }
        let refusal = match &self.xcall_host {
            None => Some(crate::xcall::XCALL_SYNC_UNAVAILABLE),
            Some(_)
                if self.sync_xcalls as usize + passed > crate::xcall::MAX_SYNC_XCALLS as usize =>
            {
                Some(crate::xcall::XCALL_SYNC_TOO_MANY)
            }
            Some(_) => None,
        };
        let Some(host) = self.xcall_host.clone().filter(|_| refusal.is_none()) else {
            let refusal = refusal.unwrap_or(crate::xcall::XCALL_SYNC_UNAVAILABLE);
            return Ok(answers
                .into_iter()
                .map(|a| a.unwrap_or_else(|| Err(refusal.to_owned())))
                .collect());
        };
        // Within `MAX_SYNC_XCALLS` (checked above), so it fits.
        self.sync_xcalls += u32::try_from(passed).unwrap_or(crate::xcall::MAX_SYNC_XCALLS);
        let run_key = host.key();
        let keys: Vec<Option<String>> = requests
            .iter()
            .zip(&answers)
            .map(|(r, a)| match (a, r.kind) {
                (None, XcallKind::Call) => {
                    Some(self.next_sync_key(&run_key, &r.app, &r.method, &r.args))
                }
                _ => None,
            })
            .collect();
        let sent: Vec<SyncXcall<'_>> = requests
            .iter()
            .zip(&answers)
            .zip(&keys)
            .filter(|((_, a), _)| a.is_none())
            .map(|((r, _), key)| SyncXcall {
                app: &r.app,
                method: &r.method,
                args: &r.args,
                key: key.as_deref(),
            })
            .collect();
        let mut replies = self.blocking(|_| host.batch(&sent))?.into_iter();
        for answer in answers.iter_mut().filter(|a| a.is_none()) {
            let reply = replies
                .next()
                .unwrap_or_else(|| Err("the node gave no answer to this xcall".to_owned()));
            *answer = Some(self.answered(bounded(reply)));
        }
        Ok(answers.into_iter().flatten().collect())
    }

    /// Charges the bytes of an answer a call got back, and hands it on.
    fn answered(&self, answer: Result<Vec<u8>, String>) -> Result<Vec<u8>, String> {
        self.burn_bytes(answer.as_ref().map_or_else(String::len, Vec::len));
        answer
    }

    /// Whether this run may make a synchronous xcall of `kind`: calls in calls, timers and
    /// actions; views there and in views; never in a compute run.
    fn may_xcall(&self, kind: crate::xcall::XcallKind) -> bool {
        if self.is_compute() {
            return false;
        }
        match self.inv.kind {
            InvocationKind::Call | InvocationKind::Timer | InvocationKind::Action => true,
            InvocationKind::View => kind == crate::xcall::XcallKind::View,
            InvocationKind::XcallDeliver | InvocationKind::Init | InvocationKind::Migrate => false,
        }
    }

    /// The key of the next synchronous call of `app`'s `method` with `args` in this run, and
    /// that call counted for the occurrence of the next identical one.
    fn next_sync_key(&mut self, run_key: &str, app: &str, method: &str, args: &[u8]) -> String {
        let n = self
            .sync_occurrences
            .entry((app.to_owned(), method.to_owned(), sha_args(args)))
            .or_insert(0);
        let key = crate::xcall::sync_xcall_key(run_key, app, method, args, *n);
        *n += 1;
        key
    }

    /// The checks of one synchronous xcall that need nothing from the node: its target and
    /// the size of its arguments.
    fn sync_check(&self, app: &str, method: &str, args: &[u8]) -> Result<(), String> {
        let bad = |s: &str| s.is_empty() || s.len() > MAX_XCALL_NAME_BYTES;
        if bad(app) || bad(method) || method.starts_with("__") {
            return Err(XCALL_BAD_TARGET.to_owned());
        }
        if app == self.app_id {
            return Err(crate::xcall::XCALL_SYNC_SELF.to_owned());
        }
        if args.len() > crate::xcall::MAX_SYNC_XCALL_ARGS_BYTES {
            return Err("xcall arguments too large".to_owned());
        }
        Ok(())
    }

    /// The checks every synchronous xcall makes before it reaches the node, and its count.
    fn sync_target(
        &mut self,
        app: &str,
        method: &str,
        args: &[u8],
    ) -> Result<Arc<dyn crate::xcall::XcallHost>, String> {
        self.sync_check(app, method, args)?;
        let Some(host) = self.xcall_host.clone() else {
            return Err(crate::xcall::XCALL_SYNC_UNAVAILABLE.to_owned());
        };
        if self.sync_xcalls >= crate::xcall::MAX_SYNC_XCALLS {
            return Err(crate::xcall::XCALL_SYNC_TOO_MANY.to_owned());
        }
        self.sync_xcalls += 1;
        Ok(host)
    }

    /// A blob's metadata, if it is one of the app's blobs; `None` for any other id.
    /// Every kind but actions (which read through views) and migrations.
    pub fn blobs_info(&self, id: &str) -> Result<Option<BlobInfo>, HostDenied> {
        self.burn_bytes(id.len());
        self.may_use_blobs("blobs.info")?;
        let blob = parse_blob_id(id).and_then(|id| self.blobs.info(&id));
        Ok(blob.or_else(|| {
            self.file_info(id).map(|f| BlobInfo {
                size: f.size,
                sha256: f.sha256,
                mime: String::new(),
            })
        }))
    }

    /// A download grant for one of the app's blobs, valid for `ttl_seconds` (1 to
    /// [`MAX_GRANT_TTL_SECS`]) from the run's wall clock. An unknown blob or a TTL out of
    /// range, a sealed file (only the app's code reads those), or a published blob outside a
    /// logged write, is refused: the WIT result has no error case.
    pub fn blobs_grant(&mut self, id: &str, ttl_seconds: u32) -> Result<String, HostDenied> {
        self.burn_bytes(id.len());
        self.may_use_blobs("blobs.grant")?;
        if ttl_seconds == 0 || ttl_seconds > MAX_GRANT_TTL_SECS {
            return Err(HostDenied::new("grant ttl out of range"));
        }
        if self.file_info(id).is_some() {
            return Err(HostDenied::new("a sealed file cannot be granted"));
        }
        let Some(id) = parse_blob_id(id).filter(|id| self.blobs.info(id).is_some()) else {
            return Err(HostDenied::new("unknown blob"));
        };
        if self.blobs.published(&id) && !self.is_logged_write() {
            return Err(self.denied("blobs.grant of a published blob"));
        }
        let expires_ms = self
            .wall_ms()
            .saturating_add(u64::from(ttl_seconds).saturating_mul(1_000));
        Ok(self.blobs.grant(&id, expires_ms))
    }

    /// Queries a table's full-text index (spec section 7): views only, the index brought
    /// up to the state the view reads. A limit of 0 or past [`MAX_SEARCH_LIMIT`], and a
    /// query over [`MAX_SEARCH_QUERY_BYTES`], are the WIT result's error.
    pub fn search_query(
        &mut self,
        query: &SearchQuery,
    ) -> Result<Result<SearchPage, String>, HostDenied> {
        self.burn_bytes(query.table.len().saturating_add(query.text.len()));
        if self.inv.kind != InvocationKind::View {
            return Err(self.denied("search"));
        }
        if query.limit == 0 || query.limit > MAX_SEARCH_LIMIT {
            return Ok(Err(format!("limit must be 1 to {MAX_SEARCH_LIMIT}")));
        }
        if query.text.len() > MAX_SEARCH_QUERY_BYTES {
            return Ok(Err(format!(
                "a query is at most {MAX_SEARCH_QUERY_BYTES} bytes"
            )));
        }
        if !self.searches.take() {
            return Ok(Err(SEARCH_TOO_MANY.to_owned()));
        }
        let page = self.search.query(query);
        let mut fuel = FUEL_SEARCH_QUERY;
        if let Ok(page) = &page {
            let bytes = page.hits.iter().fold(0usize, |n, h| {
                n.saturating_add(h.key.len())
                    .saturating_add(h.snippet.len())
            });
            fuel = fuel
                .saturating_add(FUEL_PER_SEARCH_MATCH.saturating_mul(page.total))
                .saturating_add(FUEL_PER_SEARCH_HIT.saturating_mul(page.hits.len() as u64))
                .saturating_add(FUEL_PER_SEARCH_BYTE.saturating_mul(bytes as u64));
        }
        self.burn(fuel);
        Ok(page)
    }

    /// This action run's key: actions only.
    pub fn action_key(&self) -> Result<String, HostDenied> {
        Ok(self.action_host()?.key())
    }

    /// Runs view `method` for the action.
    pub fn action_view(
        &mut self,
        method: &str,
        args: &[u8],
    ) -> Result<Result<Vec<u8>, String>, HostDenied> {
        self.burn_bytes(method.len().saturating_add(args.len()));
        self.count_subcall()?;
        let host = self.action_host()?;
        let answer = self.blocking(|_| host.view(method, args))?;
        Ok(self.answered(answer))
    }

    /// Runs `method` as one logged call for the action, under the sub-call key of this
    /// (method, args) pair's next occurrence in the run.
    pub fn action_call(
        &mut self,
        method: &str,
        args: &[u8],
    ) -> Result<Result<Vec<u8>, String>, HostDenied> {
        self.burn_bytes(method.len().saturating_add(args.len()));
        self.count_subcall()?;
        let host = self.action_host()?;
        let hash = sha_args(args);
        let n = self
            .occurrences
            .entry((method.to_owned(), hash))
            .or_insert(0);
        let key = subcall_key(&host.key(), method, args, *n);
        *n += 1;
        let answer = self.blocking(|_| host.call(method, args, &key))?;
        Ok(self.answered(answer))
    }

    /// Sends a run's outgoing HTTP request through the node's gate: actions,
    /// calls, timers and deliveries; views, `init` and migrations trap `HostDenied`. Past
    /// [`MAX_HTTP_REQUESTS`] or a body over [`MAX_HTTP_BODY_BYTES`] the request is not sent
    /// and the guest gets the error. A logged run records the exchange for its entry
    /// ([`HttpExchange`]), so followers and replays read it and never fetch again.
    pub fn http_send(
        &mut self,
        request: &HttpRequest,
    ) -> Result<Result<HttpResponse, String>, HostDenied> {
        self.burn_bytes(request_bytes(request));
        let logged = matches!(
            self.inv.kind,
            InvocationKind::Call | InvocationKind::Timer | InvocationKind::XcallDeliver
        );
        if self.is_compute() || (!logged && self.inv.kind != InvocationKind::Action) {
            return Err(HostDenied::new(
                "outgoing http is not allowed in views, init or migrations",
            ));
        }
        if self.http_requests >= MAX_HTTP_REQUESTS {
            return Ok(Err("too many requests in one run".to_owned()));
        }
        if request.body.len() > MAX_HTTP_BODY_BYTES {
            return Ok(Err("request body too large".to_owned()));
        }
        self.http_requests += 1;
        let response = self.blocking(|core| core.http.send(request))?;
        self.burn_bytes(response.as_ref().map_or_else(String::len, |r| r.body.len()));
        if logged {
            self.http_log.push(HttpExchange::record(request, &response));
        }
        Ok(response)
    }

    fn action_host(&self) -> Result<Arc<dyn ActionHost>, HostDenied> {
        match (&self.action, self.inv.kind) {
            (Some(host), InvocationKind::Action) if !self.is_compute() => Ok(Arc::clone(host)),
            _ => Err(HostDenied::new(NOT_AN_ACTION)),
        }
    }

    fn count_subcall(&mut self) -> Result<(), HostDenied> {
        if self.subcalls >= MAX_ACTION_SUBCALLS {
            return Err(HostDenied::new("too many calls in one action run"));
        }
        self.subcalls += 1;
        Ok(())
    }

    // --- WASI served by the host -----------------------------------------------

    #[must_use]
    pub fn wall_ms(&self) -> u64 {
        time::wall_ms(&self.inv)
    }

    #[must_use]
    pub fn monotonic_ns(&self) -> u64 {
        time::monotonic_ns(&self.inv)
    }

    pub fn random_fill(&mut self, buf: &mut [u8]) {
        self.burn_bytes(buf.len());
        self.rng.fill_bytes(buf);
    }

    /// One line of the app's log, kept up to `MAX_LOG_BYTES` per run, and only when the
    /// embedder asked (`with_logs`); otherwise dropped at once.
    pub fn log(&mut self, line: String) {
        if !self.keep_logs || self.log_bytes > MAX_LOG_BYTES {
            return;
        }
        self.log_bytes = self.log_bytes.saturating_add(line.len());
        if self.log_bytes > MAX_LOG_BYTES {
            self.logs.push(LOG_TRUNCATED.to_owned());
            return;
        }
        self.logs.push(line);
    }

    // --- the end of the run ------------------------------------------------------------

    /// The outcome of the run. Anything but `Ok` drops the diff and the events; the
    /// result and the app log are kept. `usage.writes` is the core's own count.
    #[must_use]
    pub fn finish(self, status: Status, result: Vec<u8>, usage: Usage) -> Outcome {
        let reported = match &status {
            Status::Ok => result.len(),
            Status::AppError { code, message } => code.len().saturating_add(message.len()),
            Status::Trap { .. } => 0,
        };
        let too_large = self.is_compute() && reported > MAX_COMPUTE_RESULT_BYTES;
        let (status, result) = if too_large {
            let denied = Status::Trap {
                kind: TrapKind::HostDenied,
                detail: RESULT_TOO_LARGE.to_owned(),
            };
            (denied, Vec::new())
        } else {
            (status, result)
        };
        let status = if self.timed_out {
            Status::Trap {
                kind: TrapKind::Timeout,
                detail: WALL_CLOCK_EXCEEDED.to_owned(),
            }
        } else if self.files.unavailable() {
            Status::Trap {
                kind: TrapKind::Unavailable,
                detail: FILE_UNAVAILABLE.to_owned(),
            }
        } else {
            status
        };
        let ok = status == Status::Ok;
        let (new_files, published) = if ok {
            self.files.into_outputs()
        } else {
            (Vec::new(), Vec::new())
        };
        let (compute_starts, compute_cancels) = if ok {
            (self.starts, self.cancels)
        } else {
            (Vec::new(), Vec::new())
        };
        Outcome {
            status,
            result,
            diff: if ok {
                self.overlay.into_diff()
            } else {
                Vec::new()
            },
            events: if ok { self.events } else { Vec::new() },
            xcalls: if ok { self.xcalls } else { Vec::new() },
            actions: if ok { self.started } else { Vec::new() },
            logs: self.logs,
            http: self.http_log,
            new_files,
            published,
            compute_starts,
            compute_cancels,
            usage: Usage {
                writes: self.writes,
                ..usage
            },
        }
    }

    /// The most range entries held at once by any cursor of this run.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn peak_buffered(&self) -> usize {
        self.peak_buffered.entries
    }

    /// The most bytes of range keys and values held at once by any cursor of this run.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn peak_buffered_bytes(&self) -> usize {
        self.peak_buffered.bytes
    }

    // --- policy ----------------------------------------------------------------------

    pub(crate) fn is_compute(&self) -> bool {
        self.inv.compute.is_some()
    }

    /// Invocations that become log entries and may change state (spec, env API).
    fn is_logged_write(&self) -> bool {
        matches!(
            self.inv.kind,
            InvocationKind::Call
                | InvocationKind::Timer
                | InvocationKind::Init
                | InvocationKind::XcallDeliver
        )
    }

    /// State reads: everything but actions, which read through views.
    fn may_read(&self) -> Result<(), HostDenied> {
        if self.inv.kind == InvocationKind::Action {
            return Err(self.denied("storage"));
        }
        Ok(())
    }

    /// Blobs are read by every kind but actions and migrations.
    fn may_use_blobs(&self, what: &str) -> Result<(), HostDenied> {
        if matches!(
            self.inv.kind,
            InvocationKind::Action | InvocationKind::Migrate
        ) {
            return Err(self.denied(what));
        }
        Ok(())
    }

    /// A value may refer only to the app's own blobs: retention follows the
    /// references, so one to a blob the app does not hold would keep nothing alive.
    fn check_blob_refs(&self, value: &[u8]) -> Result<(), HostDenied> {
        if blob_refs(value).iter().any(|id| {
            self.blobs.info(id).is_none()
                && self.file_info(&blob_id_hex(id)).is_none()
                && !self.files.published(&blob_id_hex(id))
        }) {
            return Err(HostDenied::new("unknown blob"));
        }
        Ok(())
    }

    /// The policy gate for `put`, `delete` and `add`: the kind may write.
    fn check_write(&self) -> Result<(), HostDenied> {
        if !self.is_logged_write() {
            return Err(self.denied("storage writes"));
        }
        Ok(())
    }

    /// Whether the run is under [`MAX_WRITES`]: the soft refusal of `put`, `delete` and
    /// `add`. The caller counts the write once it is made.
    fn count_write(&self) -> Result<(), RunLimit> {
        if self.writes >= MAX_WRITES {
            return Err(RunLimit {
                limit: RunLimitKind::Writes,
                max: u64::from(MAX_WRITES),
                attempted: u64::from(self.writes) + 1,
            });
        }
        Ok(())
    }

    /// Counts `bytes` against [`MAX_RUN_BYTES`] before they are kept; past it the call is
    /// refused with the run's total and nothing is counted.
    fn charge(&mut self, bytes: usize) -> Result<(), RunLimit> {
        let total = self.run_bytes.saturating_add(bytes);
        if total > MAX_RUN_BYTES {
            return Err(RunLimit {
                limit: RunLimitKind::Bytes,
                max: MAX_RUN_BYTES as u64,
                attempted: total as u64,
            });
        }
        self.run_bytes = total;
        Ok(())
    }

    /// The current value of a stored key: the overlay, then the committed state.
    fn current(&self, key: &[u8]) -> Option<Vec<u8>> {
        match self.overlay.get(key) {
            Some(slot) => slot.map(<[u8]>::to_vec),
            None => self.state.get(key),
        }
    }

    fn denied(&self, what: &str) -> HostDenied {
        HostDenied::new(format!("{what} is not allowed in {:?}", self.inv.kind))
    }

    fn rebuild_denied(&self) -> HostDenied {
        if self.inv.kind == InvocationKind::Migrate {
            HostDenied::new("migrations are not enabled")
        } else {
            HostDenied::new("rebuild is only available while migrating")
        }
    }
}

/// A synchronous xcall's answer, refused past [`crate::MAX_SYNC_XCALL_RESULT_BYTES`].
fn bounded(answer: Result<Vec<u8>, String>) -> Result<Vec<u8>, String> {
    match answer {
        Ok(out) if out.len() > crate::xcall::MAX_SYNC_XCALL_RESULT_BYTES => {
            Err("xcall answer too large".to_owned())
        }
        other => other,
    }
}

/// The SHA-256 of an action call's arguments, the key of its occurrence counter (identical
/// calls in one run get distinct sub-call keys).
fn sha_args(args: &[u8]) -> Vec<u8> {
    use sha2::Digest as _;
    sha2::Sha256::digest(args).to_vec()
}

/// Refuses an app key over `MAX_KEY_BYTES`.
fn check_key(key: &[u8]) -> Result<(), HostDenied> {
    if key.len() > MAX_KEY_BYTES {
        return Err(HostDenied::new("key longer than 1 KiB"));
    }
    Ok(())
}

/// Bytes of the generation prefix on every stored key.
const GENERATION_PREFIX: usize = 8;

/// The value of a deterministic (shortest-form, single-item) CBOR integer.
fn cbor_integer(bytes: &[u8]) -> Result<i128, HostDenied> {
    let not_integer = || HostDenied::new("storage.add on a value that is not a CBOR integer");
    let value: Value = ciborium::from_reader(bytes).map_err(|_| not_integer())?;
    let Value::Integer(int) = value else {
        return Err(not_integer());
    };
    let mut canonical = Vec::new();
    ciborium::into_writer(&Value::Integer(int), &mut canonical).map_err(|_| not_integer())?;
    if canonical != bytes {
        return Err(not_integer());
    }
    Ok(i128::from(int))
}

/// The bytes an event hands the host: name, data and audience strings.
fn event_bytes(event: &Event) -> usize {
    let audience = match &event.audience {
        Audience::All | Audience::Caller => 0,
        Audience::Accounts(accounts) => accounts
            .iter()
            .fold(0usize, |n, a| n.saturating_add(a.len())),
        Audience::Team(id) | Audience::OrgMembers(id) | Audience::OrgAdmins(id) => id.len(),
    };
    event
        .name
        .len()
        .saturating_add(event.data.len())
        .saturating_add(audience)
}

/// The bytes an xcall hands the host: its target and arguments.
fn target_bytes(app: &str, method: &str, args: &[u8]) -> usize {
    app.len()
        .saturating_add(method.len())
        .saturating_add(args.len())
}

/// The bytes an HTTP request hands the host: URL, headers and body.
fn request_bytes(request: &HttpRequest) -> usize {
    request.headers.iter().fold(
        request.url.len().saturating_add(request.body.len()),
        |n, (k, v)| n.saturating_add(k.len()).saturating_add(v.len()),
    )
}
