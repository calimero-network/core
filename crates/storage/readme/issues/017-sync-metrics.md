# Issue 017: Sync Metrics & Observability

**Priority**: P2  
**CIP Section**: Non-normative (Observability)  
**Depends On**: All core issues

## Summary

Add Prometheus metrics and structured logging for sync operations to enable debugging and performance monitoring.

## Prometheus Metrics

### Overall Sync Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `sync_duration_seconds` | Histogram | Duration of sync operations |
| `sync_attempts_total` | Counter | Total sync attempts |
| `sync_successes_total` | Counter | Successful completions |
| `sync_failures_total` | Counter | Failed syncs |
| `sync_active` | Gauge | Currently active syncs |

### Per-Phase Timing

| Metric | Type | Description |
|--------|------|-------------|
| `sync_phase_peer_selection_seconds` | Histogram | Time selecting peer |
| `sync_phase_handshake_seconds` | Histogram | Handshake duration |
| `sync_phase_data_transfer_seconds` | Histogram | Data transfer time |
| `sync_phase_merge_seconds` | Histogram | Merge operation time |

### Protocol-Specific

| Metric | Type | Description |
|--------|------|-------------|
| `sync_protocol_selected` | Counter | Protocol selection counts (by type) |
| `sync_entities_transferred` | Counter | Entities transferred |
| `sync_bytes_transferred` | Counter | Bytes transferred |
| `sync_merge_operations` | Counter | CRDT merge operations |

### Safety Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `sync_snapshot_blocked` | Counter | Snapshot attempts blocked (I5) |
| `sync_verification_failures` | Counter | Verification failures |
| `sync_lww_fallback` | Counter | LWW fallback due to missing crdt_type |

## Structured Logging

### Sync Session Start

```json
{
  "event": "sync_start",
  "context_id": "...",
  "peer_id": "...",
  "local_root_hash": "...",
  "local_entity_count": 1000,
  "trigger": "timer|divergence|manual"
}
```

### Protocol Selection

```json
{
  "event": "protocol_selected",
  "context_id": "...",
  "protocol": "HashComparison",
  "divergence_ratio": 0.15,
  "local_has_state": true
}
```

### Sync Complete

```json
{
  "event": "sync_complete",
  "context_id": "...",
  "duration_ms": 150,
  "entities_received": 50,
  "merge_operations": 30,
  "new_root_hash": "..."
}
```

## Implementation Tasks

- [ ] Define metric structs in `crates/node/src/sync/metrics.rs`
- [ ] Register metrics with Prometheus
- [ ] Add timing instrumentation to SyncManager
- [ ] Add phase timers
- [ ] Add structured logging
- [ ] Create Grafana dashboard template

## Phase Timer Helper

```rust
pub struct PhaseTimer {
    start: Instant,
    phase: &'static str,
}

impl PhaseTimer {
    pub fn start(phase: &'static str) -> Self {
        Self { start: Instant::now(), phase }
    }
    
    pub fn stop(self) -> Duration {
        let elapsed = self.start.elapsed();
        PHASE_HISTOGRAM
            .with_label_values(&[self.phase])
            .observe(elapsed.as_secs_f64());
        elapsed
    }
}
```

## Acceptance Criteria

- [ ] All metrics exposed via /metrics endpoint
- [ ] Phase timing is accurate
- [ ] Logs are structured JSON
- [ ] Dashboard shows sync health
- [ ] Safety metrics track blocked operations

## Files to Modify

- `crates/node/src/sync/metrics.rs` (new)
- `crates/node/src/sync/manager.rs`
- `crates/server/src/metrics.rs`

## POC Reference

See metrics implementation in POC branch `crates/node/src/sync/metrics.rs`.
