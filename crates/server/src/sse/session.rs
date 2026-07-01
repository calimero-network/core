use calimero_primitives::context::ContextId;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

use super::config::SESSION_EXPIRY_SECS;

/// Persistable session data (stored in database)
///
/// # Event Counter Semantics
///
/// The `event_counter` tracks the next event ID to be assigned. It persists across
/// reconnections to maintain a monotonically increasing sequence for each session.
///
/// **Important**: This counter increments regardless of whether events are successfully
/// delivered. When clients reconnect after a disconnection, they will observe gaps in
/// event IDs corresponding to events that occurred while they were offline. Events are
/// **not buffered** - the counter simply continues from where it left off.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedSessionData {
    pub subscriptions: HashSet<ContextId>,
    pub event_counter: u64,
    pub last_activity: u64, // Unix timestamp
    /// Principal that owns this session (the authenticated caller that created
    /// it). `None` for sessions created with auth disabled, and for sessions
    /// persisted before owner-binding existed — both are treated as unowned and
    /// freely accessible for backward compatibility. `#[serde(default)]` lets
    /// pre-upgrade persisted records deserialize with `owner: None`.
    #[serde(default)]
    pub owner: Option<String>,
}

/// In-memory session state
#[derive(Debug)]
pub struct SessionStateInner {
    pub subscriptions: HashSet<ContextId>,
    pub event_counter: AtomicU64,
    pub last_activity: AtomicU64,
    /// See [`PersistedSessionData::owner`]. Set once at session creation and
    /// never mutated; reconnects and session lookups compare the caller's
    /// principal against it to prevent cross-principal session access (IDOR).
    pub owner: Option<String>,
}

impl Default for SessionStateInner {
    fn default() -> Self {
        Self {
            subscriptions: HashSet::new(),
            // Event IDs start at 1. The connection's initial "connect" event is
            // emitted with the reserved id `{session_id}-0`, so the first real
            // event must not also be `-0` (`fetch_add` returns the pre-increment
            // value). Starting at 1 keeps every real event id distinct from the
            // connect frame.
            event_counter: AtomicU64::new(1),
            last_activity: AtomicU64::new(now_secs()),
            owner: None,
        }
    }
}

impl SessionStateInner {
    /// Create a fresh session owned by `owner`.
    #[must_use]
    pub fn with_owner(owner: Option<String>) -> Self {
        Self {
            owner,
            ..Self::default()
        }
    }

    /// Create session state from persisted data
    #[must_use]
    pub fn from_persisted(data: PersistedSessionData) -> Self {
        Self {
            subscriptions: data.subscriptions,
            event_counter: AtomicU64::new(data.event_counter),
            last_activity: AtomicU64::new(data.last_activity),
            owner: data.owner,
        }
    }

    /// Convert to persistable data
    #[must_use]
    pub fn to_persisted(&self) -> PersistedSessionData {
        PersistedSessionData {
            subscriptions: self.subscriptions.clone(),
            event_counter: self.event_counter.load(Ordering::SeqCst),
            last_activity: self.last_activity.load(Ordering::SeqCst),
            owner: self.owner.clone(),
        }
    }

    /// Update last activity timestamp
    pub fn touch(&self) {
        self.last_activity.store(now_secs(), Ordering::SeqCst);
    }

    /// Check if session has expired
    #[must_use]
    pub fn is_expired(&self) -> bool {
        let last = self.last_activity.load(Ordering::SeqCst);
        // `last` is persisted; a backward wall-clock step (NTP correction) can
        // make it exceed `now`, so subtract saturatingly to avoid an underflow
        // panic/wrap that would spuriously flag the session as fresh forever.
        now_secs().saturating_sub(last) > SESSION_EXPIRY_SECS
    }
}

/// Thread-safe session state wrapper
#[derive(Clone, Debug)]
pub struct SessionState {
    pub inner: Arc<RwLock<SessionStateInner>>,
    /// Abort handle for the node-event task currently bound to this session.
    ///
    /// A session outlives the individual SSE connections that use it (it
    /// persists across reconnects). Each connection spawns its own node-event
    /// task that forwards matching broadcast events into that connection's
    /// command channel, and the connection's response stream stamps each
    /// forwarded event with the next value of the shared `event_counter`. When a
    /// new connection (re)binds to this session, the task from the previous
    /// connection must be aborted: otherwise two live connections drain the
    /// broadcast stream in parallel and their response streams both bump the one
    /// shared `event_counter`, corrupting event IDs and delivering each event
    /// more than once.
    ///
    /// Aborting the prior task cannot bump-without-deliver: the counter is
    /// incremented in the response stream as an event is emitted, not by the
    /// task being aborted (which only forwards commands), so a killed task drops
    /// only events it had not yet forwarded — no phantom gap beyond the
    /// skip-on-disconnect gaps the session already tolerates. `None` until the
    /// first task is bound.
    ///
    /// Private on purpose: the abort-before-replace invariant only holds if
    /// every mutation goes through [`SessionState::bind_event_task`], so callers
    /// must not touch the handle directly.
    ///
    /// Dropping a `SessionState` does not abort the task. `SessionState` is
    /// `Clone` (several clones coexist — in the session map, in the request
    /// handler, and inside the task itself), so a `Drop` impl here would abort
    /// on every transient clone drop, not on eviction. Eviction-time
    /// cancellation is instead deferred to the task noticing its connection's
    /// command channel has closed; this handle exists only to cancel a
    /// superseded task when a new connection rebinds the same session.
    event_task: Arc<std::sync::Mutex<Option<tokio::task::AbortHandle>>>,
    /// Serializes persistence of this session's state (the subscribe/unsubscribe
    /// store writes) so concurrent mutations of the *same* session commit to the
    /// store in the order they mutated the in-memory state.
    ///
    /// Held (via [`SessionState::persist_guard`]) across the blocking
    /// `save_session` call, but deliberately kept separate from `inner`: event
    /// delivery only reads `inner`, so a slow store write serializes persists
    /// without stalling the broadcast fan-out. Lock order is always
    /// persist-guard → `inner`; nothing acquires them the other way.
    persist_lock: Arc<tokio::sync::Mutex<()>>,
}

impl SessionState {
    /// Wrap freshly-built session state, with no node-event task bound yet.
    #[must_use]
    pub fn new(inner: SessionStateInner) -> Self {
        Self {
            inner: Arc::new(RwLock::new(inner)),
            event_task: Arc::new(std::sync::Mutex::new(None)),
            persist_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Acquire this session's persistence guard. Hold it across a `save_session`
    /// call so concurrent subscribe/unsubscribe requests persist in mutation
    /// order. Snapshot `inner` (a brief write-lock, no I/O) and drop that lock
    /// before the store write, so event delivery — which reads `inner` — is
    /// never blocked on the store.
    pub async fn persist_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.persist_lock.lock().await
    }

    /// Bind a newly-spawned node-event task to this session, aborting any task
    /// bound by a previous connection. Aborting an already-finished task is a
    /// no-op, so a normally-closed prior connection costs nothing here.
    pub fn bind_event_task(&self, handle: tokio::task::AbortHandle) {
        // Swap the handle under the lock, then release the lock before calling
        // `abort()` — never hold this std::sync::Mutex across external code.
        let prev = {
            let mut slot = self
                .event_task
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            slot.replace(handle)
        };
        if let Some(prev) = prev {
            prev.abort();
        }
    }
}

/// Get current timestamp in seconds since UNIX epoch
#[must_use]
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("System time before UNIX epoch")
        .as_secs()
}
