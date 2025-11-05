//! Sync events for observability

use calimero_primitives::context::ContextId;
use serde::{Deserialize, Serialize};

/// Sync event for observability
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncEvent {
    /// Context being synced
    pub context_id: ContextId,
    
    /// Peer being synced with (serialized as string)
    #[serde(serialize_with = "serialize_peer_id", deserialize_with = "deserialize_peer_id")]
    pub peer_id: libp2p::PeerId,
    
    /// Sync status
    pub status: SyncStatus,
    
    /// Duration (for completed syncs)
    pub duration_ms: Option<u64>,
    
    /// Error message (for failed syncs)
    pub error: Option<String>,
}

// Serde helpers for PeerId
fn serialize_peer_id<S>(peer_id: &libp2p::PeerId, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&peer_id.to_string())
}

fn deserialize_peer_id<'de, D>(deserializer: D) -> Result<libp2p::PeerId, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    s.parse().map_err(serde::de::Error::custom)
}

/// Sync status
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SyncStatus {
    /// Sync started
    Started,
    
    /// Sync completed successfully
    Completed {
        /// Strategy used
        strategy: String,
        
        /// Number of deltas synced (for delta sync)
        deltas_synced: Option<usize>,
    },
    
    /// Sync failed
    Failed {
        /// Retry attempt number
        retry_attempt: usize,
        
        /// Will retry?
        will_retry: bool,
    },
}

impl SyncEvent {
    /// Create a "started" event
    pub fn started(context_id: ContextId, peer_id: libp2p::PeerId) -> Self {
        Self {
            context_id,
            peer_id,
            status: SyncStatus::Started,
            duration_ms: None,
            error: None,
        }
    }
    
    /// Create a "completed" event
    pub fn completed(
        context_id: ContextId,
        peer_id: libp2p::PeerId,
        strategy: String,
        deltas_synced: Option<usize>,
        duration_ms: u64,
    ) -> Self {
        Self {
            context_id,
            peer_id,
            status: SyncStatus::Completed {
                strategy,
                deltas_synced,
            },
            duration_ms: Some(duration_ms),
            error: None,
        }
    }
    
    /// Create a "failed" event
    pub fn failed(
        context_id: ContextId,
        peer_id: libp2p::PeerId,
        error: String,
        retry_attempt: usize,
        will_retry: bool,
    ) -> Self {
        Self {
            context_id,
            peer_id,
            status: SyncStatus::Failed {
                retry_attempt,
                will_retry,
            },
            duration_ms: None,
            error: Some(error),
        }
    }
}

