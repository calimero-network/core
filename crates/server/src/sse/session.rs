use calimero_primitives::context::ContextId;
use calimero_server_primitives::sse::Command;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, RwLock};

use super::config::SESSION_EXPIRY_SECS;

/// Persistable session data (stored in database)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedSessionData {
    pub subscriptions: HashSet<ContextId>,
    pub event_counter: u64,
    pub last_activity: u64, // Unix timestamp
}

/// In-memory session state
#[derive(Debug)]
pub struct SessionStateInner {
    pub subscriptions: HashSet<ContextId>,
    pub event_counter: AtomicU64,
    pub last_activity: AtomicU64,
}

impl Default for SessionStateInner {
    fn default() -> Self {
        Self {
            subscriptions: HashSet::new(),
            event_counter: AtomicU64::new(0),
            last_activity: AtomicU64::new(now_secs()),
        }
    }
}

impl SessionStateInner {
    /// Create session state from persisted data
    #[must_use]
    pub fn from_persisted(data: PersistedSessionData) -> Self {
        Self {
            subscriptions: data.subscriptions,
            event_counter: AtomicU64::new(data.event_counter),
            last_activity: AtomicU64::new(data.last_activity),
        }
    }

    /// Convert to persistable data
    #[must_use]
    pub fn to_persisted(&self) -> PersistedSessionData {
        PersistedSessionData {
            subscriptions: self.subscriptions.clone(),
            event_counter: self.event_counter.load(Ordering::SeqCst),
            last_activity: self.last_activity.load(Ordering::SeqCst),
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

/// Active SSE connection
#[derive(Clone, Debug)]
pub struct ActiveConnection {
    pub commands: mpsc::Sender<Command>,
}

/// Get current timestamp in seconds since UNIX epoch
#[must_use]
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("System time before UNIX epoch")
        .as_secs()
}
