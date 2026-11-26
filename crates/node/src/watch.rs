//! Live views: which views the clients connected to this node watch, what each one read,
//! and which have changed since their last notice.
//!
//! A watch belongs to a watch stream (`GET /apps/{app}/events?watch=true`), held by the
//! node that serves that stream: a replica of the app, since every replica applies every
//! entry's diff ([`crate::app::Changed`]). The stream's task (`api::watches`) marks the
//! watches a change touches (a key or range it read; for a view that read the clock, any of
//! the app's entries; for one that read a blob's metadata, an entry that changed it; for one
//! that read the system log, a system entry that may change the very facts it read,
//! [`crate::system::effects`]), runs each marked view again as its caller, at once for the
//! first change and then at most once per the watch's interval, and pushes the new answer
//! (`view-result`) unless it is the one last sent; an answer over
//! [`MAX_PUSHED_ANSWER_BYTES`] goes as a bare `view-changed` the client re-reads.
//!
//! What protects the node's CPU ([`Runs`]): watches of the same view, arguments and caller
//! (any caller, when the view did not read who called it) share one run per state; at most
//! [`MAX_CONCURRENT_RERUNS`] runs at once and [`MAX_RERUNS_PER_SEC`] a second over every
//! stream, past which a watch waits a longer interval (its changes coalesce) instead of
//! queueing. A shared run keeps its answer only until the state moves on.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use calimero_app_host::{ReadSet, SystemRead};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, Notify};

use crate::app::Changed;
use crate::system::effects::{SysEffect, SysTouch};
use crate::types::Caller;

/// Watches one stream holds at most.
pub const MAX_WATCHES_PER_STREAM: usize = 64;
/// Watches one node holds at most, over every stream.
pub const MAX_WATCHES_PER_NODE: usize = 2_048;
/// Bytes of one watch's view arguments (they are kept for as long as the watch).
pub const MAX_WATCH_ARGS_BYTES: usize = 4 * 1024;
/// A watch's interval when the app's ABI names none: its view runs at most once per
/// interval (the first change after a quiet one at once).
pub const DEFAULT_WATCH_INTERVAL: Duration = Duration::from_millis(1_000);
/// The range an app's `watchIntervalMs` and a client's `intervalMs` are held to.
pub use calimero_app_host::{MAX_WATCH_INTERVAL, MIN_WATCH_INTERVAL};
/// Bytes of an answer's JSON a frame carries at most; a larger answer is sent as a bare
/// `view-changed` the client re-reads.
pub const MAX_PUSHED_ANSWER_BYTES: usize = 64 * 1024;
/// How often a stream with nothing due checks whether it has outlived its client.
pub const EXPIRY_CHECK: Duration = Duration::from_secs(1);
/// How long a stream's watches outlive its connection, for the client to resume it.
pub const RESUME_GRACE: Duration = Duration::from_secs(30);
/// Frames a stream's connection buffers before notices wait for the next interval.
pub const STREAM_BUFFER: usize = 256;
/// View runs for watches this node makes at once at most, over every stream (each holds a
/// blocking thread); fewer on a machine with fewer cores ([`concurrent_reruns`]).
pub const MAX_CONCURRENT_RERUNS: usize = 16;
/// View runs for watches this node starts in a second at most, over every stream. Past it
/// a due watch waits another, longer interval (its changes coalesce) rather than queue: a
/// burst stretches how often watches update, never how much the node runs.
pub const MAX_RERUNS_PER_SEC: u32 = 2_000;
/// Shared runs (one per view, arguments and caller watched) this node keeps at most.
pub const MAX_SHARED_RUNS: usize = MAX_WATCHES_PER_NODE;

/// The view runs this machine makes at once: its cores, at most [`MAX_CONCURRENT_RERUNS`].
#[must_use]
pub fn concurrent_reruns() -> usize {
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .clamp(1, MAX_CONCURRENT_RERUNS)
}

/// Who opened a stream, with the credential they opened it with: judged again, as
/// `/events` judges its streams (`api::still_admitted`), before every notice.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Holder {
    pub caller: Caller,
    /// The session's claims, when signed in with one (revocation, passkey, the package it
    /// is bound to).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<calimero_identity::SessionClaims>,
    /// The API key, when called with one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<crate::types::ApiKeyRecord>,
    /// The verified release of the node that signed the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer_release: Option<String>,
}

/// What a stream sends besides its first frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    /// `event: view-result`, `id: <index>:<seq>`, `data: {"sub", "index", "answer"}` (or
    /// `"error"` in place of `"answer"` when the view answered an app error).
    Result {
        sub: String,
        index: u64,
        seq: u64,
        answer: Result<Value, Value>,
    },
    /// `event: view-changed`, `id: <index>:<seq>`, `data: {"sub", "index"}`: the answer
    /// changed but is over [`MAX_PUSHED_ANSWER_BYTES`]; the client reads it.
    Changed { sub: String, index: u64, seq: u64 },
    /// `event: watch-ended`, `data: {"sub", "reason"}`: the watch is gone.
    Ended { sub: String, reason: EndReason },
    /// `event: watch-reset`, `data: {}`: the stream asked for is gone; this one holds no
    /// watches, and the client adds its own again.
    Reset,
}

/// Why a watch ended (`watch-ended`'s `reason`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// The stream's holder may no longer call the view.
    AccessLost,
    /// The view now refuses the holder or no longer exists.
    ViewRefused,
    /// The view now reads more than a read set holds.
    ReadSetUnbounded,
}

impl EndReason {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AccessLost => "access-lost",
            Self::ViewRefused => "view-refused",
            Self::ReadSetUnbounded => "read-set-unbounded",
        }
    }
}

/// A watch's interval: the app sets it (the view's `watchIntervalMs`, else
/// [`DEFAULT_WATCH_INTERVAL`]); a client's `interval_ms` can only make it longer, up to
/// [`MAX_WATCH_INTERVAL`].
#[must_use]
pub fn interval_of(app: Option<Duration>, interval_ms: Option<u64>) -> Duration {
    let app = app
        .unwrap_or(DEFAULT_WATCH_INTERVAL)
        .clamp(MIN_WATCH_INTERVAL, MAX_WATCH_INTERVAL);
    interval_ms.map_or(app, |ms| {
        Duration::from_millis(ms).clamp(app, MAX_WATCH_INTERVAL)
    })
}

/// The hash a frame's answer is compared by.
#[must_use]
pub fn answer_hash(answer: &Result<Value, Value>) -> [u8; 32] {
    let (tag, v) = match answer {
        Ok(v) => (b'o', v),
        Err(v) => (b'e', v),
    };
    let mut h = ring::digest::Context::new(&ring::digest::SHA256);
    h.update(&[tag]);
    h.update(&serde_json::to_vec(v).unwrap_or_default());
    let mut out = [0; 32];
    out.copy_from_slice(h.finish().as_ref());
    out
}

/// One watch: a view, its arguments and what it read last.
#[derive(Clone, Debug)]
pub struct Watch {
    pub view: String,
    pub args: Value,
    read_set: ReadSet,
    /// The app's last entry the read set was taken at: older changes are in it.
    since: u64,
    /// The newest change that touched it and has not been noticed yet.
    dirty: Option<u64>,
    /// The newest change that touched it (noticed or not), for a resumed stream.
    touched: u64,
    /// At most one run of the view per interval.
    interval: Duration,
    /// When the view last ran for a change; `None`: the next change runs it at once.
    last_run: Option<Instant>,
    /// The hash of the answer the client holds (the last one sent, or the one `POST
    /// /watches` answered); `None`: the next answer is sent whatever it is.
    last_hash: Option<[u8; 32]>,
    /// Runs the node's budget put off in a row: each one waits one more interval.
    deferred: u32,
}

impl Watch {
    /// A watch of `view(args)` whose read set was taken at the app's entry `since`, where
    /// the view answered `answer`.
    #[must_use]
    pub fn new(
        view: String,
        args: Value,
        read_set: ReadSet,
        since: u64,
        interval: Duration,
        answer: &Result<Value, Value>,
    ) -> Self {
        Self {
            view,
            args,
            read_set,
            since,
            dirty: None,
            touched: since,
            interval,
            last_run: None,
            last_hash: Some(answer_hash(answer)),
            deferred: 0,
        }
    }

    /// The wait between two runs: the interval, one more for each run the node's budget put
    /// off in a row, at most [`MAX_WATCH_INTERVAL`] (or the interval, if longer).
    fn wait(&self) -> Duration {
        let stretched = self
            .interval
            .saturating_mul(self.deferred.saturating_add(1));
        stretched.min(MAX_WATCH_INTERVAL.max(self.interval))
    }

    /// When its next run may start (`now` if it may start now), if it is marked.
    fn ready_at(&self, now: Instant) -> Option<Instant> {
        self.dirty?;
        Some(self.last_run.map_or(now, |t| (t + self.wait()).max(now)))
    }

    /// Whether a write to the stored key `key`, by an entry that changed the app's blobs
    /// when `blobs`, may change the view's answer. Every entry moves the clock a view sees.
    fn touched_by(&self, keys: Option<&[Vec<u8>]>, blobs: bool) -> bool {
        let rs = &self.read_set;
        match keys {
            None => true,
            Some(keys) => {
                rs.reads_clock()
                    || (blobs && rs.reads_blobs())
                    || keys.iter().any(|k| rs.touches_key(k))
            }
        }
    }
}

/// Whether a system entry of `effect` may change what a view that read `rs` answers, in an
/// app of `org`.
#[must_use]
pub fn system_touches(rs: &ReadSet, effect: &SysEffect, org: Option<&str>) -> bool {
    match effect {
        SysEffect::Quiet | SysEffect::Access => false,
        SysEffect::All => rs.reads_system(),
        SysEffect::Touches(touches) => match rs.system_reads() {
            None => true,
            Some(reads) => touches.iter().any(|t| fact_touched(reads, t, org)),
        },
    }
}

/// Whether `touch` may change one of `reads`, the facts a view of an app in `org` read.
fn fact_touched(reads: &BTreeSet<SystemRead>, touch: &SysTouch, org: Option<&str>) -> bool {
    // A fact of the app's org: an entry of another org (named) cannot change it.
    let ours = |o: &Option<String>| o.as_deref().is_none_or(|o| Some(o) == org);
    let same = |a: &Option<String>, b: &str| a.as_deref().is_none_or(|a| a == b);
    reads.iter().any(|read| match (read, touch) {
        (SystemRead::App, _) => false,
        (SystemRead::Role(a), SysTouch::Member { org: o, account }) => ours(o) && a == account,
        (SystemRead::OrgsOf(a), SysTouch::Member { account, .. }) => a == account,
        (SystemRead::Team { account, .. }, SysTouch::Member { org: o, account: b }) => {
            ours(o) && account == b
        }
        (
            SystemRead::Team { account, team },
            SysTouch::Team {
                org: o,
                team: t,
                account: b,
            },
        ) => ours(o) && same(t, team) && same(b, account),
        (SystemRead::Grant(a), SysTouch::Member { org: o, account }) => ours(o) && a == account,
        (
            SystemRead::Grant(a),
            SysTouch::Team {
                org: o, account, ..
            },
        )
        | (SystemRead::Grant(a), SysTouch::Grant { org: o, account }) => {
            ours(o) && same(account, a)
        }
        _ => false,
    })
}

/// A watch that is due a notice.
#[derive(Clone, Debug)]
pub struct Due {
    pub sub: String,
    pub view: String,
    pub args: Value,
    pub index: u64,
}

/// Why a watch was not added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Full {
    Stream,
    Node,
}

struct StreamState {
    watches: BTreeMap<String, Watch>,
    /// The connection while a client holds the stream.
    out: Option<mpsc::Sender<Notice>>,
    /// When the last connection went.
    detached_at: Option<Instant>,
    /// The last notice's sequence number on this stream.
    seq: u64,
}

/// One watch stream, served by this node.
pub struct WatchStream {
    pub id: String,
    pub app: String,
    pub holder: Holder,
    state: Mutex<StreamState>,
    /// Asks the stream's task to send what is due now (a client resumed).
    pub wake: Notify,
}

impl WatchStream {
    fn lock(&self) -> std::sync::MutexGuard<'_, StreamState> {
        // SAFETY: guaranteed by every critical section below: none panics while holding it.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Takes a client's connection; `resume_after` (the index of its `Last-Event-ID`)
    /// marks every watch touched after it, so its current answer is sent again at once,
    /// even if it is the one last sent (that frame may not have arrived).
    pub fn attach(&self, out: mpsc::Sender<Notice>, resume_after: Option<u64>) {
        let mut st = self.lock();
        st.out = Some(out);
        st.detached_at = None;
        if let Some(after) = resume_after {
            for w in st.watches.values_mut() {
                if w.touched > after {
                    w.dirty = Some(w.dirty.map_or(w.touched, |d| d.max(w.touched)));
                    w.last_run = None;
                    w.last_hash = None;
                }
            }
        }
        drop(st);
        self.wake.notify_one();
    }

    /// The client's connection `out` went; the watches stay for [`RESUME_GRACE`]. A
    /// connection that a resumed one replaced changes nothing.
    pub fn detach(&self, out: &mpsc::Sender<Notice>) {
        let mut st = self.lock();
        if st.out.as_ref().is_some_and(|o| o.same_channel(out)) {
            st.out = None;
            st.detached_at = Some(Instant::now());
        }
    }

    /// Whether the stream has been without a client longer than [`RESUME_GRACE`].
    #[must_use]
    pub fn expired(&self) -> bool {
        self.lock()
            .detached_at
            .is_some_and(|at| at.elapsed() > RESUME_GRACE)
    }

    /// The connection, if a client holds the stream.
    #[must_use]
    pub fn out(&self) -> Option<mpsc::Sender<Notice>> {
        self.lock().out.clone()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().watches.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Marks every watch `change` touches (an entry after the one its read set was taken
    /// at whose diff lands in it; `keys: None`, everything).
    pub fn mark(&self, change: &Changed) {
        let mut st = self.lock();
        for w in st.watches.values_mut() {
            if change.index <= w.since {
                continue;
            }
            if w.touched_by(change.keys.as_deref(), change.blobs) {
                w.dirty = Some(w.dirty.map_or(change.index, |d| d.max(change.index)));
                w.touched = w.touched.max(change.index);
            }
        }
    }

    /// Marks every watch whose view read a system fact `effect` may change; `org` is the
    /// app's org, `index` the app's last entry. Answers how many it marked.
    pub fn mark_system(&self, effect: &SysEffect, org: Option<&str>, index: u64) -> usize {
        let mut st = self.lock();
        let mut marked = 0;
        for w in st.watches.values_mut() {
            if system_touches(&w.read_set, effect, org) {
                marked += 1;
                let at = index.max(w.since);
                w.dirty = Some(w.dirty.map_or(at, |d| d.max(at)));
                w.touched = w.touched.max(at);
            }
        }
        marked
    }

    /// Puts a run the node's budget refused back, one interval later than the last wait.
    pub fn defer(&self, sub: &str, index: u64) {
        if let Some(w) = self.lock().watches.get_mut(sub) {
            w.dirty = Some(w.dirty.map_or(index, |d| d.max(index)));
            w.deferred = w.deferred.saturating_add(1);
            w.last_run = Some(Instant::now());
        }
    }

    /// A run went ahead: the watch is back to its own interval.
    pub fn ran(&self, sub: &str) {
        if let Some(w) = self.lock().watches.get_mut(sub) {
            w.deferred = 0;
        }
    }

    /// Takes the marked watches whose interval has passed since their view last ran
    /// (clearing their marks and starting their next interval), and when the next of the
    /// others will be.
    #[must_use]
    pub fn take_due(&self, now: Instant) -> (Vec<Due>, Option<Instant>) {
        let mut st = self.lock();
        let mut next: Option<Instant> = None;
        let mut due = Vec::new();
        for (sub, w) in &mut st.watches {
            let Some(at) = w.ready_at(now) else {
                continue;
            };
            if at > now {
                next = Some(next.map_or(at, |n| n.min(at)));
                continue;
            }
            let Some(index) = w.dirty.take() else {
                continue;
            };
            w.last_run = Some(now);
            due.push(Due {
                sub: sub.clone(),
                view: w.view.clone(),
                args: w.args.clone(),
                index,
            });
        }
        (due, next)
    }

    /// When the next marked watch may run, if any is marked.
    #[must_use]
    pub fn next_due(&self) -> Option<Instant> {
        let now = Instant::now();
        self.lock()
            .watches
            .values()
            .filter_map(|w| w.ready_at(now))
            .min()
    }

    /// Puts a mark back (the view could not run, or its frame could not be sent); with
    /// `resend`, the next answer is sent even if it repeats the last.
    pub fn remark(&self, sub: &str, index: u64, resend: bool) {
        if let Some(w) = self.lock().watches.get_mut(sub) {
            w.dirty = Some(w.dirty.map_or(index, |d| d.max(index)));
            if resend {
                w.last_hash = None;
            }
        }
    }

    /// Whether `answer` differs from the one the client holds; if so it becomes the one it
    /// holds (the caller sends it).
    #[must_use]
    pub fn answer_changed(&self, sub: &str, answer: &Result<Value, Value>) -> bool {
        let hash = answer_hash(answer);
        let mut st = self.lock();
        let Some(w) = st.watches.get_mut(sub) else {
            return false;
        };
        if w.last_hash == Some(hash) {
            return false;
        }
        w.last_hash = Some(hash);
        true
    }

    /// Every watch's id.
    #[must_use]
    pub fn subs(&self) -> Vec<String> {
        self.lock().watches.keys().cloned().collect()
    }

    /// The next notice sequence number.
    pub fn next_seq(&self) -> u64 {
        let mut st = self.lock();
        st.seq += 1;
        st.seq
    }

    /// Replaces a watch's read set with one taken at `since`; `now` is the app's last entry
    /// once it is in place: anything applied in between may have been checked against the
    /// old set, so it counts as a change.
    pub fn refresh(&self, sub: &str, read_set: ReadSet, since: u64, now: impl FnOnce() -> u64) {
        {
            let mut st = self.lock();
            let Some(w) = st.watches.get_mut(sub) else {
                return;
            };
            w.read_set = read_set;
            w.since = w.since.max(since);
        }
        self.caught_up(sub, now());
    }

    /// Marks watch `sub` if the app's last entry `now` (read once the watch's read set is in
    /// place) is past the one its read set was taken at.
    fn caught_up(&self, sub: &str, now: u64) {
        let mut st = self.lock();
        let Some(w) = st.watches.get_mut(sub) else {
            return;
        };
        if now > w.since {
            w.dirty = Some(w.dirty.map_or(now, |d| d.max(now)));
            w.touched = w.touched.max(now);
        }
    }

    fn insert(&self, sub: String, watch: Watch) -> Result<(), Full> {
        let mut st = self.lock();
        if st.watches.len() >= MAX_WATCHES_PER_STREAM {
            return Err(Full::Stream);
        }
        let _prev = st.watches.insert(sub, watch);
        Ok(())
    }

    fn remove(&self, sub: &str) -> bool {
        self.lock().watches.remove(sub).is_some()
    }
}

/// Every watch stream this node serves.
#[derive(Default)]
pub struct Watches {
    streams: Mutex<HashMap<String, Arc<WatchStream>>>,
    /// Watch id to its stream.
    subs: Mutex<HashMap<String, String>>,
    total: AtomicUsize,
    /// The node's view runs for watches: shared, bounded and budgeted.
    pub runs: Runs,
}

/// One view's answer as a watch run made it, shared by every watch of the same view,
/// arguments and caller (any caller, if `caller_free`) while the state it read is current.
#[derive(Debug)]
pub struct SharedRun {
    /// `(the app's last entry, the system's applied index)` before the run started.
    pub version: (u64, u64),
    pub answer: Result<Value, Value>,
    pub read_set: ReadSet,
    /// The app's last entry in the state the view read.
    pub last_entry: u64,
}

/// `(app, view, arguments as JSON)` and the caller (`None`: a view that did not read it).
type SharedKey = (String, String, String, Option<String>);

/// A slot one run fills and the watches of the same key wait on (single flight).
pub type SharedSlot = Arc<tokio::sync::Mutex<Option<Arc<SharedRun>>>>;

/// Counts of what [`Runs`] did, for the metrics and tests.
#[derive(Debug, Default)]
pub struct RunStats {
    /// Views run for watches.
    pub reruns: AtomicU64,
    /// Watches answered from another watch's run.
    pub shared: AtomicU64,
    /// Answers that were not sent: the client holds them already.
    pub unchanged: AtomicU64,
    /// Runs the per-second budget put off.
    pub deferred: AtomicU64,
}

/// The node's view runs for watches.
pub struct Runs {
    /// At most [`concurrent_reruns`] at once.
    pub permits: tokio::sync::Semaphore,
    /// Runs a second at most ([`MAX_RERUNS_PER_SEC`]; tests lower it).
    per_sec: AtomicU32,
    /// `(when this second's window started, runs in it)`.
    window: Mutex<(Instant, u32)>,
    slots: Mutex<HashMap<SharedKey, SharedSlot>>,
    pub stats: RunStats,
}

impl Default for Runs {
    fn default() -> Self {
        Self {
            permits: tokio::sync::Semaphore::new(concurrent_reruns()),
            per_sec: AtomicU32::new(MAX_RERUNS_PER_SEC),
            window: Mutex::new((Instant::now(), 0)),
            slots: Mutex::new(HashMap::new()),
            stats: RunStats::default(),
        }
    }
}

impl Runs {
    /// Sets the per-second budget (tests).
    pub fn set_per_sec(&self, n: u32) {
        self.per_sec.store(n, Ordering::Relaxed);
    }

    /// Takes one run from this second's budget; `false` when it is spent.
    pub fn take(&self) -> bool {
        let mut w = self
            .window
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if w.0.elapsed() >= Duration::from_secs(1) {
            *w = (Instant::now(), 0);
        }
        if w.1 >= self.per_sec.load(Ordering::Relaxed) {
            return false;
        }
        w.1 += 1;
        true
    }

    /// The slot of `key`, made if new. Past [`MAX_SHARED_RUNS`] the slots nobody waits on
    /// are dropped first (their answers are of states gone by).
    #[must_use]
    pub fn slot(&self, key: (String, String, String, Option<String>)) -> SharedSlot {
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !slots.contains_key(&key) && slots.len() >= MAX_SHARED_RUNS {
            slots.retain(|_, s| Arc::strong_count(s) > 1);
            if slots.len() >= MAX_SHARED_RUNS {
                slots.clear();
            }
        }
        Arc::clone(slots.entry(key).or_default())
    }

    /// The caller-free run of `(app, view, args)` at `version`, if one is kept.
    #[must_use]
    pub fn caller_free(
        &self,
        app: &str,
        view: &str,
        args: &str,
        version: (u64, u64),
    ) -> Option<Arc<SharedRun>> {
        let key = (app.to_owned(), view.to_owned(), args.to_owned(), None);
        let slot = self
            .slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
            .cloned()?;
        let held = slot.try_lock().ok()?;
        held.as_ref().filter(|r| r.version == version).cloned()
    }
}

impl Watches {
    fn streams(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<WatchStream>>> {
        // SAFETY: guaranteed by every critical section below: none panics while holding it.
        self.streams
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn subs(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        // SAFETY: as for `streams`.
        self.subs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A new stream, or `None` past `MAX_EVENT_STREAMS` streams held.
    #[must_use]
    pub fn open(&self, id: String, app: String, holder: Holder) -> Option<Arc<WatchStream>> {
        let mut streams = self.streams();
        if streams.len() >= crate::api::MAX_EVENT_STREAMS {
            return None;
        }
        let stream = Arc::new(WatchStream {
            id: id.clone(),
            app,
            holder,
            state: Mutex::new(StreamState {
                watches: BTreeMap::new(),
                out: None,
                detached_at: None,
                seq: 0,
            }),
            wake: Notify::new(),
        });
        let _prev = streams.insert(id, Arc::clone(&stream));
        Some(stream)
    }

    #[must_use]
    pub fn stream(&self, id: &str) -> Option<Arc<WatchStream>> {
        self.streams().get(id).cloned()
    }

    /// The stream that holds watch `sub`.
    #[must_use]
    pub fn stream_of(&self, sub: &str) -> Option<Arc<WatchStream>> {
        let id = self.subs().get(sub).cloned()?;
        self.stream(&id)
    }

    /// Forgets a stream and its watches.
    pub fn close(&self, id: &str) {
        let Some(stream) = self.streams().remove(id) else {
            return;
        };
        let gone: Vec<String> = stream.lock().watches.keys().cloned().collect();
        let mut subs = self.subs();
        for sub in &gone {
            let _ = subs.remove(sub);
        }
        let _ = self.total.fetch_sub(gone.len(), Ordering::SeqCst);
    }

    /// Adds `watch` as `sub`; `now` as in [`WatchStream::refresh`].
    pub fn add(
        &self,
        stream: &WatchStream,
        sub: String,
        watch: Watch,
        now: impl FnOnce() -> u64,
    ) -> Result<(), Full> {
        if self.total.fetch_add(1, Ordering::SeqCst) >= MAX_WATCHES_PER_NODE {
            let _ = self.total.fetch_sub(1, Ordering::SeqCst);
            return Err(Full::Node);
        }
        if let Err(full) = stream.insert(sub.clone(), watch) {
            let _ = self.total.fetch_sub(1, Ordering::SeqCst);
            return Err(full);
        }
        let _prev = self.subs().insert(sub.clone(), stream.id.clone());
        stream.caught_up(&sub, now());
        Ok(())
    }

    /// Removes a watch; `false` if the stream holds no such watch.
    pub fn remove(&self, stream: &WatchStream, sub: &str) -> bool {
        if !stream.remove(sub) {
            return false;
        }
        let _ = self.subs().remove(sub);
        let _ = self.total.fetch_sub(1, Ordering::SeqCst);
        true
    }

    /// Watches held on this node.
    #[must_use]
    pub fn total(&self) -> usize {
        self.total.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_set_of(state: &[(&[u8], &[u8])], key: &[u8]) -> ReadSet {
        recorded(state, |h| {
            let _ = h.storage_get(key).unwrap();
        })
    }

    /// A set taken from a view that read `account`'s role in the app's org.
    fn role_read_set(account: &str) -> ReadSet {
        recorded(&[], |h| {
            let _ = h.org_role(account);
        })
    }

    fn member(org: &str, account: &str) -> SysEffect {
        SysEffect::Touches(vec![SysTouch::Member {
            org: Some(org.into()),
            account: account.into(),
        }])
    }

    fn recorded(
        state: &[(&[u8], &[u8])],
        read: impl FnOnce(&calimero_app_host::HostCore),
    ) -> ReadSet {
        use calimero_app_host::{
            Directory, HostCore, Invocation, InvocationKind, OrgRole, Status, Usage,
        };
        struct Nobody;
        impl Directory for Nobody {
            fn org(&self) -> Option<String> {
                None
            }
            fn app_owner(&self) -> String {
                String::new()
            }
            fn org_role(&self, _: &str) -> Option<OrgRole> {
                None
            }
            fn in_team(&self, _: &str, _: &str) -> bool {
                false
            }
            fn team_exists(&self, _: &str) -> bool {
                false
            }
            fn orgs_of(&self, _: &str) -> Vec<(String, OrgRole)> {
                Vec::new()
            }
        }
        let state: BTreeMap<Vec<u8>, Vec<u8>> =
            state.iter().map(|(k, v)| (stored(k), v.to_vec())).collect();
        let inv = Invocation {
            kind: InvocationKind::View,
            method: "m".into(),
            args: Vec::new(),
            caller: calimero_app_host::Caller::Public,
            entry_ts_ms: 0,
            id_term: 0,
            id_seq: 0,
            seed: [0; 32],
            timer: None,
            compute: None,
        };
        let host = HostCore::new("a", Arc::new(state), 0, inv, Arc::new(Nobody))
            .unwrap()
            .with_read_set();
        read(&host);
        host.finish(Status::Ok, Vec::new(), Usage::default())
            .read_set
            .unwrap()
    }

    fn stored(key: &[u8]) -> Vec<u8> {
        calimero_app_host::stored_key(calimero_app_host::GENERATION, key)
    }

    fn change(index: u64, key: &[u8]) -> Changed {
        Changed {
            index,
            keys: Some(vec![stored(key)]),
            blobs: false,
        }
    }

    fn holder() -> Holder {
        Holder {
            caller: Caller::Public,
            session: None,
            api_key: None,
            issuer_release: None,
        }
    }

    fn watch(rs: ReadSet, since: u64) -> Watch {
        Watch::new(
            "v".into(),
            Value::Null,
            rs,
            since,
            DEFAULT_WATCH_INTERVAL,
            &Ok(Value::Null),
        )
    }

    fn due_now(stream: &WatchStream) -> Vec<Due> {
        stream.take_due(Instant::now()).0
    }

    #[test]
    fn changes_mark_only_the_watches_they_touch_and_coalesce() {
        let watches = Watches::default();
        let stream = watches.open("s".into(), "app".into(), holder()).unwrap();
        let rs = read_set_of(&[(b"a", b"1")], b"a");
        watches
            .add(&stream, "w1".into(), watch(rs, 5), || 5)
            .unwrap();
        // At or before the index the set was taken at: already in what the view read.
        stream.mark(&change(5, b"a"));
        assert!(due_now(&stream).is_empty());
        stream.mark(&change(6, b"b"));
        assert!(due_now(&stream).is_empty());
        // The first change runs at once.
        stream.mark(&change(7, b"a"));
        let due = due_now(&stream);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].index, 7);
        // Changes within the interval wait for it, and coalesce to the newest.
        stream.mark(&change(8, b"a"));
        stream.mark(&change(9, b"a"));
        let (due, next) = stream.take_due(Instant::now());
        assert!(due.is_empty());
        let next = next.expect("the next run is scheduled");
        let (due, _) = stream.take_due(next);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].index, 9);
        assert!(stream.take_due(next + DEFAULT_WATCH_INTERVAL).0.is_empty());
        // A resumed client is sent what it missed after its last id, at once.
        let (tx, _rx) = mpsc::channel(1);
        stream.attach(tx, Some(8));
        assert_eq!(due_now(&stream)[0].index, 9);
        stream.attach(mpsc::channel(1).0, Some(9));
        assert!(stream.take_due(next + MAX_WATCH_INTERVAL).0.is_empty());
    }

    #[test]
    fn an_answer_is_sent_only_when_it_differs_from_the_one_the_client_holds() {
        let watches = Watches::default();
        let stream = watches.open("s".into(), "app".into(), holder()).unwrap();
        watches
            .add(&stream, "w".into(), watch(ReadSet::default(), 0), || 0)
            .unwrap();
        // The client holds the answer `POST /watches` gave it.
        assert!(!stream.answer_changed("w", &Ok(Value::Null)));
        assert!(stream.answer_changed("w", &Ok(Value::from(1))));
        assert!(!stream.answer_changed("w", &Ok(Value::from(1))));
        // An app error with the same JSON is a different answer.
        assert!(stream.answer_changed("w", &Err(Value::from(1))));
        // A frame that could not be sent is sent again even if the answer repeats.
        stream.remark("w", 3, true);
        assert!(stream.answer_changed("w", &Err(Value::from(1))));
    }

    #[test]
    fn the_app_sets_the_interval_and_a_client_can_only_slow_it() {
        let fast = Some(MIN_WATCH_INTERVAL);
        assert_eq!(interval_of(None, None), DEFAULT_WATCH_INTERVAL);
        assert_eq!(interval_of(fast, None), MIN_WATCH_INTERVAL);
        // A client asking faster than the app gets the app's rate.
        assert_eq!(interval_of(None, Some(1)), DEFAULT_WATCH_INTERVAL);
        assert_eq!(interval_of(fast, Some(1)), MIN_WATCH_INTERVAL);
        // Slower is allowed, up to the cap.
        assert_eq!(interval_of(fast, Some(2_000)), Duration::from_secs(2));
        assert_eq!(interval_of(None, Some(u64::MAX)), MAX_WATCH_INTERVAL);
    }

    #[test]
    fn system_entries_mark_only_views_that_read_a_fact_they_change() {
        let watches = Watches::default();
        let stream = watches.open("s".into(), "app".into(), holder()).unwrap();
        let plain = read_set_of(&[], b"a");
        for (sub, rs) in [
            ("plain", plain),
            ("ada", role_read_set("ada")),
            ("bob", role_read_set("bob")),
        ] {
            watches
                .add(&stream, sub.into(), watch(rs, 4), || 4)
                .unwrap();
        }
        let subs = |stream: &WatchStream| -> Vec<String> {
            due_now(stream).into_iter().map(|d| d.sub).collect()
        };
        // A refreshed token or a revoked session changes no fact a view reads.
        assert_eq!(stream.mark_system(&SysEffect::Quiet, Some("o"), 4), 0);
        assert_eq!(stream.mark_system(&SysEffect::Access, Some("o"), 4), 0);
        // Ada promoted in another org: not this app's role.
        assert_eq!(stream.mark_system(&member("other", "ada"), Some("o"), 4), 0);
        // Ada promoted in the app's org: only Ada's view.
        assert_eq!(stream.mark_system(&member("o", "ada"), Some("o"), 4), 1);
        assert_eq!(subs(&stream), vec!["ada".to_owned()]);
        // An entry nobody classified: every view that read the system.
        assert_eq!(stream.mark_system(&SysEffect::All, Some("o"), 4), 2);
    }

    #[test]
    fn a_clock_reader_follows_every_entry_and_a_blob_reader_blob_changes() {
        let watches = Watches::default();
        let stream = watches.open("s".into(), "app".into(), holder()).unwrap();
        let clock = recorded(&[], |h| {
            let _ = h.wall_ms();
        });
        let blobs = recorded(&[], |h| {
            let _ = h.blobs_info(&"0".repeat(64));
        });
        watches
            .add(&stream, "clock".into(), watch(clock, 1), || 1)
            .unwrap();
        watches
            .add(&stream, "blobs".into(), watch(blobs, 1), || 1)
            .unwrap();
        stream.mark(&change(2, b"z"));
        assert_eq!(due_now(&stream)[0].sub, "clock");
        stream.mark(&Changed {
            index: 3,
            keys: Some(Vec::new()),
            blobs: true,
        });
        let due: Vec<String> = stream
            .take_due(Instant::now() + MAX_WATCH_INTERVAL)
            .0
            .into_iter()
            .map(|d| d.sub)
            .collect();
        assert_eq!(due, vec!["blobs".to_owned(), "clock".to_owned()]);
    }

    #[test]
    fn a_deferred_watch_waits_a_longer_interval_and_keeps_its_mark() {
        let watches = Watches::default();
        let stream = watches.open("s".into(), "app".into(), holder()).unwrap();
        watches
            .add(&stream, "w".into(), watch(read_set_of(&[], b"a"), 1), || 1)
            .unwrap();
        stream.mark(&change(2, b"a"));
        assert_eq!(due_now(&stream).len(), 1);
        stream.defer("w", 2);
        stream.defer("w", 2);
        let now = Instant::now();
        // Two deferrals in a row: three intervals before it may run again.
        assert!(stream
            .take_due(now + DEFAULT_WATCH_INTERVAL * 2)
            .0
            .is_empty());
        let due = stream.take_due(now + DEFAULT_WATCH_INTERVAL * 3).0;
        assert_eq!(due[0].index, 2);
        stream.ran("w");
        stream.mark(&change(3, b"a"));
        let at = Instant::now() + DEFAULT_WATCH_INTERVAL * 4;
        assert_eq!(stream.take_due(at).0.len(), 1);
    }

    #[test]
    fn the_run_budget_is_per_second_and_slots_are_shared_by_key() {
        let runs = Runs::default();
        runs.set_per_sec(2);
        assert!(runs.take() && runs.take());
        assert!(!runs.take());
        let key = |caller: Option<&str>| {
            (
                "app".to_owned(),
                "v".to_owned(),
                "{}".to_owned(),
                caller.map(str::to_owned),
            )
        };
        let a = runs.slot(key(Some("ada")));
        assert!(Arc::ptr_eq(&a, &runs.slot(key(Some("ada")))));
        assert!(!Arc::ptr_eq(&a, &runs.slot(key(Some("bob")))));
        let free = runs.slot(key(None));
        *free.try_lock().unwrap() = Some(Arc::new(SharedRun {
            version: (5, 9),
            answer: Ok(Value::Null),
            read_set: ReadSet::default(),
            last_entry: 5,
        }));
        assert!(runs.caller_free("app", "v", "{}", (5, 9)).is_some());
        // A state moved on: the kept answer is of a state gone by.
        assert!(runs.caller_free("app", "v", "{}", (6, 9)).is_none());
    }

    #[test]
    fn a_change_applied_while_a_watch_is_added_is_not_lost() {
        let watches = Watches::default();
        let stream = watches.open("s".into(), "app".into(), holder()).unwrap();
        let rs = read_set_of(&[], b"a");
        // The task saw entry 6 before the watch was in place; the app is at 6 now.
        watches
            .add(&stream, "w".into(), watch(rs, 5), || 6)
            .unwrap();
        assert_eq!(due_now(&stream)[0].index, 6);
    }

    #[test]
    fn watches_are_bounded_per_stream_and_freed_with_it() {
        let watches = Watches::default();
        let stream = watches.open("s".into(), "app".into(), holder()).unwrap();
        for i in 0..MAX_WATCHES_PER_STREAM {
            watches
                .add(&stream, format!("w{i}"), watch(ReadSet::default(), 0), || 0)
                .unwrap();
        }
        let over = watches.add(&stream, "x".into(), watch(ReadSet::default(), 0), || 0);
        assert_eq!(over, Err(Full::Stream));
        assert_eq!(watches.total(), MAX_WATCHES_PER_STREAM);
        assert!(watches.stream_of("w3").is_some());
        watches.close("s");
        assert_eq!(watches.total(), 0);
        assert!(watches.stream_of("w3").is_none());
    }
}
