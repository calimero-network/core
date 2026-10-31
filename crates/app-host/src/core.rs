//! The host core: the only code that reads or writes app state, applies access policy or
//! decides what an invocation may do (spec section 3). Engines map WIT calls onto it.

use std::collections::BTreeMap;
use std::sync::Arc;

use ciborium::value::Integer;
use ciborium::Value;
use rand::Rng;
use rand_chacha::ChaCha20Rng;

use crate::blobs::{blob_refs, parse_blob_id, BlobHost, NoBlobs, MAX_GRANT_TTL_SECS};
use crate::overlay::{Cursor, Overlay};
use crate::search::{
    NoSearch, SearchHost, SearchPage, SearchQuery, MAX_SEARCH_LIMIT, MAX_SEARCH_QUERY_BYTES,
};
use crate::state::{stored_key, KeyValue, StateView, GENERATION};
use crate::time;
use crate::types::{
    Audience, BlobInfo, Caller, Directory, Event, HostDenied, InvalidInvocation, Invocation,
    InvocationKind, OrgRole, Outcome, Status, Usage,
};

/// Bytes of an app key (before the generation prefix). Values have no separate cap:
/// [`MAX_RUN_BYTES`] bounds them as they are written.
pub const MAX_KEY_BYTES: usize = 1_024;
/// Bytes one run may write (Ruling 37): the keys and values of every `put`, `add` and
/// `delete` (each overwrite counts again: the overlay keeps every version) plus the name
/// and data of every event. The host call that would pass it is refused with
/// `HostDenied("entry too large")` before the bytes are kept, so the overlay and the events
/// of a run never hold more than this. Equal to the executor's log-entry cap
/// (`calimero_log::MAX_ENTRY_BYTES`, 1 MiB), which still checks the encoded entry.
pub const MAX_RUN_BYTES: usize = 1024 * 1024;
/// The refusal of a write past [`MAX_RUN_BYTES`]: the executor's D50 refusal, made early.
pub const ENTRY_TOO_LARGE: &str = "entry too large";
/// Writes per run: `put`, `delete` and `add` count one each; the next one is refused.
pub const MAX_WRITES: u32 = 10_000;
/// Entries one `range_next` returns at most.
pub const MAX_RANGE_BATCH: u32 = 1_000;
/// Range cursors one run may hold open at once.
pub const MAX_OPEN_CURSORS: usize = 1_024;
/// Bytes of app log one run keeps; later lines are dropped after one marker line.
pub const MAX_LOG_BYTES: usize = 64 * 1024;

/// Events one run may emit (Rooms emits up to 500 per timer tick).
pub const MAX_EVENTS: usize = 1_000;
/// Bytes of an event name.
pub const MAX_EVENT_NAME_BYTES: usize = 100;
/// Bytes of an event's data.
pub const MAX_EVENT_DATA_BYTES: usize = 16 * 1024;
/// Accounts one event's `accounts` audience may name (Ruling 43).
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

const ACTIONS_DISABLED: &str = "actions are not enabled";
const XCALL_DISABLED: &str = "xcall is not enabled";
const LOG_TRUNCATED: &str = "[log truncated]";

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
    dir: Arc<dyn Directory>,
    blobs: Arc<dyn BlobHost>,
    search: Arc<dyn SearchHost>,
    overlay: Overlay,
    writes: u32,
    /// Bytes written so far ([`MAX_RUN_BYTES`]).
    run_bytes: usize,
    events: Vec<Event>,
    logs: Vec<String>,
    log_bytes: usize,
    unique_ids: u64,
    rng: ChaCha20Rng,
    cursors: BTreeMap<u32, Cursor>,
    next_cursor: u32,
    peak_buffered: usize,
}

impl HostCore {
    /// `state` is the committed state at `applied_index`: the leader's latest for calls,
    /// the index the view reads at for views (D46). A timer invocation must carry its
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
        let rng = time::entropy(&inv);
        Ok(Self {
            app_id: app_id.to_owned(),
            state,
            applied_index,
            inv,
            dir,
            blobs: Arc::new(NoBlobs),
            search: Arc::new(NoSearch),
            overlay: Overlay::default(),
            writes: 0,
            run_bytes: 0,
            events: Vec::new(),
            logs: Vec::new(),
            log_bytes: 0,
            unique_ids: 0,
            rng,
            cursors: BTreeMap::new(),
            next_cursor: 0,
            peak_buffered: 0,
        })
    }

    /// The app's blobs as the node records them (R8); without it the app has none.
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

    /// The invocation being run; the engine reads kind, method and args from it.
    #[must_use]
    pub fn invocation(&self) -> &Invocation {
        &self.inv
    }

    // --- storage ---------------------------------------------------------------------

    pub fn storage_get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, HostDenied> {
        self.may_read()?;
        check_key(key)?;
        Ok(self.current(&stored_key(GENERATION, key)))
    }

    pub fn storage_put(&mut self, key: &[u8], value: &[u8]) -> Result<(), HostDenied> {
        self.check_write()?;
        check_key(key)?;
        self.check_blob_refs(value)?;
        self.charge(key.len().saturating_add(value.len()))?;
        self.overlay
            .put(stored_key(GENERATION, key), value.to_vec());
        self.writes += 1;
        Ok(())
    }

    pub fn storage_delete(&mut self, key: &[u8]) -> Result<(), HostDenied> {
        self.check_write()?;
        check_key(key)?;
        self.charge(key.len())?;
        self.overlay.delete(stored_key(GENERATION, key));
        self.writes += 1;
        Ok(())
    }

    /// Adds `delta` to the deterministic CBOR integer at `key` (absent is 0) without the
    /// guest reading it (D47). One write; a non-integer value or an overflow is refused.
    pub fn storage_add(&mut self, key: &[u8], delta: i64) -> Result<(), HostDenied> {
        self.check_write()?;
        check_key(key)?;
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
        self.charge((key.len() - GENERATION_PREFIX).saturating_add(value.len()))?;
        self.overlay.put(key, value);
        self.writes += 1;
        Ok(())
    }

    /// Opens a cursor over `[start, end)` (an empty `end` is the end of the keyspace). It
    /// reads the state as of opening: committed state plus the writes made before now.
    pub fn range_open(
        &mut self,
        start: &[u8],
        end: &[u8],
        reverse: bool,
    ) -> Result<CursorId, HostDenied> {
        self.may_read()?;
        check_key(start)?;
        check_key(end)?;
        if self.cursors.len() >= MAX_OPEN_CURSORS {
            return Err(HostDenied::new("too many open cursors"));
        }
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

    /// The next entries of a cursor, at most `max` (capped at `MAX_RANGE_BATCH`), with
    /// keys as the guest wrote them. An exhausted cursor returns an empty list.
    pub fn range_next(&mut self, id: CursorId, max: u32) -> Result<Vec<KeyValue>, HostDenied> {
        let Some(cursor) = self.cursors.get_mut(&id.0) else {
            return Err(HostDenied::new("unknown cursor"));
        };
        let max = max.min(MAX_RANGE_BATCH) as usize;
        let batch = cursor.next_batch(&*self.state, &self.overlay, max, &mut self.peak_buffered);
        Ok(batch
            .into_iter()
            .map(|(key, value)| (key[GENERATION_PREFIX..].to_vec(), value))
            .collect())
    }

    /// Drops a cursor (the WIT resource's destructor). Unknown ids are ignored.
    pub fn range_close(&mut self, id: CursorId) {
        let _ = self.cursors.remove(&id.0);
    }

    // --- rebuild (D38): migrations are not in this milestone --------------------------

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

    #[must_use]
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    #[must_use]
    pub fn app_owner(&self) -> String {
        self.dir.app_owner()
    }

    #[must_use]
    pub fn org(&self) -> Option<String> {
        self.dir.org()
    }

    #[must_use]
    pub fn org_role(&self, account: &str) -> Option<OrgRole> {
        self.dir.org_role(account)
    }

    /// Linearizable role read. The same as `org_role` in this milestone; refused during a
    /// migration (spec, env API).
    pub fn org_role_strict(&self, account: &str) -> Result<Option<OrgRole>, HostDenied> {
        if self.inv.kind == InvocationKind::Migrate {
            return Err(HostDenied::new("org_role_strict is not allowed in migrate"));
        }
        Ok(self.dir.org_role(account))
    }

    #[must_use]
    pub fn orgs_of(&self, account: &str) -> Vec<(String, OrgRole)> {
        self.dir.orgs_of(account)
    }

    #[must_use]
    pub fn in_team(&self, account: &str, team: &str) -> bool {
        self.dir.in_team(account, team)
    }

    #[must_use]
    pub fn applied_index(&self) -> u64 {
        self.applied_index
    }

    /// `"<id_term>.<id_seq>.<n>"`, `n` counting this run's calls from 0 (D47, Ruling 12).
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

    /// Starts a durable action (D17): allowed in calls and timers; not enabled in this
    /// milestone, so the guest gets the WIT `result` error.
    pub fn start_action(
        &mut self,
        _method: &str,
        _args: &[u8],
    ) -> Result<Result<String, String>, HostDenied> {
        if !matches!(self.inv.kind, InvocationKind::Call | InvocationKind::Timer) {
            return Err(self.denied("start_action"));
        }
        Ok(Err(ACTIONS_DISABLED.to_owned()))
    }

    // --- events, xcall, blobs, action ------------------------------------------------

    /// Queues an event, delivered only if the run ends `Ok`. A policy refusal is the
    /// outer `HostDenied`; the inner result is the WIT `result<_, string>`, one of the
    /// `EMIT_*` strings, and a refused event is not recorded.
    pub fn emit(&mut self, event: Event) -> Result<Result<(), String>, HostDenied> {
        if !self.is_logged_write() {
            return Err(self.denied("emit"));
        }
        if let Err(reason) = self.check_event(&event) {
            return Ok(Err(reason.to_owned()));
        }
        // The audience is kept with the event, so its strings count too (Ruling 43).
        let audience = match &event.audience {
            Audience::All | Audience::Caller => 0,
            Audience::Accounts(accounts) => accounts
                .iter()
                .fold(0usize, |n, a| n.saturating_add(a.len())),
            Audience::Team(id) | Audience::OrgMembers(id) | Audience::OrgAdmins(id) => id.len(),
        };
        self.charge(
            event
                .name
                .len()
                .saturating_add(event.data.len())
                .saturating_add(audience),
        )?;
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

    pub fn xcall_send(
        &mut self,
        _app: &str,
        _method: &str,
        _args: &[u8],
    ) -> Result<(), HostDenied> {
        Err(HostDenied::new(XCALL_DISABLED))
    }

    /// A blob's metadata (R8), if it is one of the app's blobs; `None` for any other id.
    /// Every kind but actions (which read through views) and migrations.
    pub fn blobs_info(&self, id: &str) -> Result<Option<BlobInfo>, HostDenied> {
        self.may_use_blobs("blobs.info")?;
        Ok(parse_blob_id(id).and_then(|id| self.blobs.info(&id)))
    }

    /// A download grant for one of the app's blobs, valid for `ttl_seconds` (1 to
    /// [`MAX_GRANT_TTL_SECS`]) from the run's wall clock. An unknown blob or a TTL out of
    /// range is refused: the WIT result has no error case.
    pub fn blobs_grant(&mut self, id: &str, ttl_seconds: u32) -> Result<String, HostDenied> {
        self.may_use_blobs("blobs.grant")?;
        if ttl_seconds == 0 || ttl_seconds > MAX_GRANT_TTL_SECS {
            return Err(HostDenied::new("grant ttl out of range"));
        }
        let Some(id) = parse_blob_id(id).filter(|id| self.blobs.info(id).is_some()) else {
            return Err(HostDenied::new("unknown blob"));
        };
        let expires_ms = self
            .wall_ms()
            .saturating_add(u64::from(ttl_seconds).saturating_mul(1_000));
        Ok(self.blobs.grant(&id, expires_ms))
    }

    /// Queries a table's full-text index (spec section 7): views only, the index brought
    /// up to the state the view reads. A limit of 0 or past [`MAX_SEARCH_LIMIT`], and a
    /// query over [`MAX_SEARCH_QUERY_BYTES`], are the WIT result's error.
    pub fn search_query(
        &self,
        query: &SearchQuery,
    ) -> Result<Result<SearchPage, String>, HostDenied> {
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
        Ok(self.search.query(query))
    }

    pub fn action_key(&self) -> Result<String, HostDenied> {
        Err(HostDenied::new(ACTIONS_DISABLED))
    }

    pub fn action_view(
        &mut self,
        _method: &str,
        _args: &[u8],
    ) -> Result<Result<Vec<u8>, String>, HostDenied> {
        Err(HostDenied::new(ACTIONS_DISABLED))
    }

    pub fn action_call(
        &mut self,
        _method: &str,
        _args: &[u8],
    ) -> Result<Result<Vec<u8>, String>, HostDenied> {
        Err(HostDenied::new(ACTIONS_DISABLED))
    }

    // --- WASI served by the host (D19) -----------------------------------------------

    #[must_use]
    pub fn wall_ms(&self) -> u64 {
        time::wall_ms(&self.inv)
    }

    #[must_use]
    pub fn monotonic_ns(&self) -> u64 {
        time::monotonic_ns(&self.inv)
    }

    pub fn random_fill(&mut self, buf: &mut [u8]) {
        self.rng.fill_bytes(buf);
    }

    /// One line of the owner-only app log, kept up to `MAX_LOG_BYTES` per run.
    pub fn log(&mut self, line: String) {
        if self.log_bytes > MAX_LOG_BYTES {
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
        let ok = status == Status::Ok;
        Outcome {
            status,
            result,
            diff: if ok {
                self.overlay.into_diff()
            } else {
                Vec::new()
            },
            events: if ok { self.events } else { Vec::new() },
            logs: self.logs,
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
        self.peak_buffered
    }

    // --- policy ----------------------------------------------------------------------

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

    /// State reads: everything but actions, which read through views (D12).
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

    /// A value may refer only to the app's own blobs (R8): retention follows the
    /// references, so one to a blob the app does not hold would keep nothing alive.
    fn check_blob_refs(&self, value: &[u8]) -> Result<(), HostDenied> {
        if blob_refs(value)
            .iter()
            .any(|id| self.blobs.info(id).is_none())
        {
            return Err(HostDenied::new("unknown blob"));
        }
        Ok(())
    }

    /// The one gate for `put`, `delete` and `add`: the kind may write, and the run is
    /// under `MAX_WRITES`. The caller counts the write once it is made.
    fn check_write(&self) -> Result<(), HostDenied> {
        if !self.is_logged_write() {
            return Err(self.denied("storage writes"));
        }
        if self.writes >= MAX_WRITES {
            return Err(HostDenied::new("more than 10,000 writes in one run"));
        }
        Ok(())
    }

    /// Counts `bytes` against [`MAX_RUN_BYTES`] before they are kept; past it the call is
    /// refused and nothing is counted.
    fn charge(&mut self, bytes: usize) -> Result<(), HostDenied> {
        let total = self.run_bytes.saturating_add(bytes);
        if total > MAX_RUN_BYTES {
            return Err(HostDenied::new(ENTRY_TOO_LARGE));
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
