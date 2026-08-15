use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_server_primitives::sse::{Command, Response};
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
    /// Group ids observed for `GroupMembership` events. `#[serde(default)]` so
    /// records persisted before group subscriptions existed still deserialize.
    #[serde(default)]
    pub group_subscriptions: HashSet<Hash>,
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
    pub group_subscriptions: HashSet<Hash>,
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
            group_subscriptions: HashSet::new(),
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
            group_subscriptions: data.group_subscriptions,
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
            group_subscriptions: self.group_subscriptions.clone(),
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
    /// Sink of the SSE connection currently bound to this session, for events
    /// addressed to *this client only* rather than broadcast to every
    /// subscriber — currently the presence replay a subscriber is seeded with
    /// (see `crate::ephemeral_replay`).
    ///
    /// Needed because SSE splits subscribe (a POST) from the stream (a
    /// long-lived GET): the POST handler that authorizes a context has no other
    /// handle on the connection it must seed. Bound alongside the event task,
    /// by the same [`SessionState::bind_connection`] call, so the sink and the
    /// task can never describe different connections.
    ///
    /// Held **weakly** on purpose. The stream's receiver dropping is what tells
    /// the event task its connection is gone (`Sender::closed()`), and a
    /// session outlives its connections; a strong clone parked here would keep
    /// a dead connection's channel — and its buffered frames — alive until the
    /// next connection replaced it. A failed upgrade means "no live connection
    /// to seed", which is exactly the right answer.
    connection: Arc<std::sync::Mutex<Option<tokio::sync::mpsc::WeakSender<Command>>>>,
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
            connection: Arc::new(std::sync::Mutex::new(None)),
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

    /// Bind a new connection to this session: its node-event task (see
    /// [`SessionState::bind_event_task`]) and its command sink, together, so
    /// the two can never point at different connections.
    pub fn bind_connection(
        &self,
        handle: tokio::task::AbortHandle,
        sink: tokio::sync::mpsc::WeakSender<Command>,
    ) {
        {
            let mut slot = self
                .connection
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            drop(slot.replace(sink));
        }
        self.bind_event_task(handle);
    }

    /// Push an event to the SSE connection currently bound to this session, and
    /// to no one else.
    ///
    /// Best-effort by design, matching the broadcast path: no live connection,
    /// or a full command channel (slow or closing client), drops the event
    /// rather than blocking the caller. The one caller — the presence seed —
    /// converges on the next delta when a frame is dropped.
    ///
    /// Returns whether the event was queued, which the tests assert on;
    /// production callers ignore it.
    pub fn try_push(&self, response: Response) -> bool {
        let sink = {
            let slot = self
                .connection
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            slot.clone()
        };
        // A `None` upgrade means the connection's receiver is already gone —
        // nothing to seed.
        let Some(sink) = sink.and_then(|weak| weak.upgrade()) else {
            return false;
        };
        sink.try_send(Command::Send(response)).is_ok()
    }

    /// Bind a newly-spawned node-event task to this session, aborting any task
    /// bound by a previous connection. Aborting an already-finished task is a
    /// no-op, so a normally-closed prior connection costs nothing here.
    fn bind_event_task(&self, handle: tokio::task::AbortHandle) {
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

/// Get current timestamp in seconds since UNIX epoch.
///
/// Degrades to `0` if the system clock is set before 1970 rather than panicking,
/// so a misconfigured clock can't crash the SSE service. Callers subtract this
/// saturatingly, so a `0` reading just makes a session look maximally stale.
#[must_use]
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
