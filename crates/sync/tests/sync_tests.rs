//! Integration tests for calimero-sync

use calimero_sync::config::{RetryConfig, SyncConfig};
use calimero_sync::events::{SyncEvent, SyncStatus};
use std::time::Duration;

#[test]
fn test_sync_config_defaults() {
    let config = SyncConfig::default();
    
    assert_eq!(config.timeout, Duration::from_secs(30));
    assert_eq!(config.max_concurrent_syncs, 10);
    assert!(config.enable_heartbeat);
    assert_eq!(config.heartbeat_interval, Duration::from_secs(60));
}

#[test]
fn test_sync_config_with_timeout() {
    let config = SyncConfig::with_timeout(Duration::from_secs(60));
    
    assert_eq!(config.timeout, Duration::from_secs(60));
    assert_eq!(config.max_concurrent_syncs, 10); // Default
}

#[test]
fn test_sync_config_without_heartbeat() {
    let config = SyncConfig::default().without_heartbeat();
    
    assert!(!config.enable_heartbeat);
}

#[test]
fn test_retry_config_defaults() {
    let config = RetryConfig::default();
    
    assert_eq!(config.max_retries, 3);
    assert_eq!(config.initial_backoff, Duration::from_secs(1));
    assert_eq!(config.max_backoff, Duration::from_secs(60));
    assert_eq!(config.backoff_multiplier, 2.0);
}

#[test]
fn test_retry_backoff_calculation() {
    let config = RetryConfig::default();
    
    // Simulate backoff growth
    let mut backoff = config.initial_backoff;
    
    // After 1st retry: 1s * 2.0 = 2s
    backoff = Duration::from_secs_f64(
        backoff.as_secs_f64() * config.backoff_multiplier
    );
    assert_eq!(backoff, Duration::from_secs(2));
    
    // After 2nd retry: 2s * 2.0 = 4s
    backoff = Duration::from_secs_f64(
        backoff.as_secs_f64() * config.backoff_multiplier
    );
    assert_eq!(backoff, Duration::from_secs(4));
    
    // After 3rd retry: 4s * 2.0 = 8s
    backoff = Duration::from_secs_f64(
        backoff.as_secs_f64() * config.backoff_multiplier
    );
    assert_eq!(backoff, Duration::from_secs(8));
}

#[test]
fn test_sync_event_started() {
    use calimero_primitives::context::ContextId;
    
    let context_id = ContextId::from([1; 32]);
    let peer_id = "12D3KooWTest".parse::<libp2p::PeerId>().unwrap();
    
    let event = SyncEvent::started(context_id, peer_id);
    
    assert_eq!(event.context_id, context_id);
    assert_eq!(event.peer_id, peer_id);
    assert!(matches!(event.status, SyncStatus::Started));
    assert!(event.duration_ms.is_none());
    assert!(event.error.is_none());
}

#[test]
fn test_sync_event_completed() {
    use calimero_primitives::context::ContextId;
    
    let context_id = ContextId::from([1; 32]);
    let peer_id = "12D3KooWTest".parse::<libp2p::PeerId>().unwrap();
    
    let event = SyncEvent::completed(
        context_id,
        peer_id,
        "dag_catchup".to_string(),
        Some(42),
        1000,
    );
    
    assert_eq!(event.context_id, context_id);
    assert_eq!(event.duration_ms, Some(1000));
    
    match event.status {
        SyncStatus::Completed { strategy, deltas_synced } => {
            assert_eq!(strategy, "dag_catchup");
            assert_eq!(deltas_synced, Some(42));
        }
        _ => panic!("Expected Completed status"),
    }
}

#[test]
fn test_sync_event_failed() {
    use calimero_primitives::context::ContextId;
    
    let context_id = ContextId::from([1; 32]);
    let peer_id = "12D3KooWTest".parse::<libp2p::PeerId>().unwrap();
    
    let event = SyncEvent::failed(
        context_id,
        peer_id,
        "timeout".to_string(),
        2,
        true,
    );
    
    assert_eq!(event.context_id, context_id);
    assert_eq!(event.error, Some("timeout".to_string()));
    
    match event.status {
        SyncStatus::Failed { retry_attempt, will_retry } => {
            assert_eq!(retry_attempt, 2);
            assert!(will_retry);
        }
        _ => panic!("Expected Failed status"),
    }
}

#[test]
fn test_sync_event_serialization() {
    use calimero_primitives::context::ContextId;
    
    let context_id = ContextId::from([1; 32]);
    let peer_id = "12D3KooWTest".parse::<libp2p::PeerId>().unwrap();
    
    let event = SyncEvent::started(context_id, peer_id);
    
    // Should serialize/deserialize correctly
    let json = serde_json::to_string(&event).unwrap();
    let deserialized: SyncEvent = serde_json::from_str(&json).unwrap();
    
    assert_eq!(deserialized.context_id, context_id);
    assert_eq!(deserialized.peer_id, peer_id);
}

// TODO: Add integration tests for:
// - SyncScheduler orchestration
// - Strategy execution
// - Retry logic
// - Event emission
// - Concurrent syncs
// - Heartbeat mechanism
//
// These require mock clients which we'll add next

