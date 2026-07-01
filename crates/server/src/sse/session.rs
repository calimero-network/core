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
            event_counter: AtomicU64::new(0),
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
        now_secs() - last > SESSION_EXPIRY_SECS
    }
}

/// Thread-safe session state wrapper
#[derive(Clone, Debug)]
pub struct SessionState {
    pub inner: Arc<RwLock<SessionStateInner>>,
}

/// Get current timestamp in seconds since UNIX epoch
#[must_use]
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("System time before UNIX epoch")
        .as_secs()
}
