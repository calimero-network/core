# Phase 2: Dial/Connection Optimization Analysis

**Date**: January 31, 2026  
**Branch**: test/tree_sync  
**Status**: ✅ Complete (Infrastructure Ready)

---

## Problem Statement

Phase 1 analysis proved that peer **finding** is sub-millisecond (<0.12ms).  
The actual bottleneck is peer **dialing** (connection establishment):

| Phase | Latency |
|-------|---------|
| Peer Finding | 0.04 - 0.12ms |
| **Peer Dialing** | **150 - 200ms P50, >1s P99** |

This document tracks Phase 2: reducing `time_to_connected_peer_ms`.

---

## Primary Objective

Minimize:

```
time_to_connected_peer_ms = time from peer selected → stream ready for reconciliation
```

Goals:
- Lower median dial time
- Reduce tail latency (P99)
- Higher connection reuse rate
- Faster recovery after restart

---

## Instrumentation

### New Log Marker: `PEER_DIAL_BREAKDOWN`

```
PEER_DIAL_BREAKDOWN
  peer_id=<id>
  was_connected_initially=<bool>
  total_dial_ms=<float>
  reuse_connection=<bool>
  attempt_index=<int>
  result=<success|timeout|refused|no_route|error>
```

### Key Metrics Tracked

| Metric | Description |
|--------|-------------|
| `total_dial_ms` | Time for libp2p `open_stream` |
| `was_connected_initially` | Did we have a connection before dial? |
| `reuse_connection` | Heuristic: dial < 50ms suggests reuse |
| `result` | Dial outcome |

### Connection Pool Stats: `DIAL_POOL_STATS`

```
DIAL_POOL_STATS
  total_dials=<int>
  reused_connections=<int>
  new_connections=<int>
  reuse_rate=<float>%
  successes=<int>
  failures=<int>
  avg_reuse_dial_ms=<float>
  avg_new_dial_ms=<float>
```

---

## Experiments

### 1. Connection Pooling Design

**Goal**: Reduce dialing by reusing live connections.

**Test Variants**:
- A: No pooling (baseline)
- B: LRU connection cache
- C: Per-context connection pools
- D: Keep-alive duration variations

**Metrics**:
- Connection reuse rate
- Dial latency reduction
- Memory overhead
- Idle connection churn

**Implementation Status**: Tracking infrastructure added via `ConnectionStateTracker`

### 2. Peer Scoring for Dialing

**Goal**: Prefer peers likely to respond quickly.

**Score Inputs**:
- Last success time
- Failure count
- RTT history (exponential moving average)
- Sync freshness

**Compare**:
- Random selection (baseline)
- Score-based selection
- Score + health filtering

**Metrics**:
- Attempts before success
- Dial latency P95/P99
- False candidate rate

**Implementation Status**: `PeerConnectionState` tracks RTT estimate via EMA

### 3. Churn Recovery Tuning

**Goal**: Ensure restarted nodes reconnect quickly.

**Test**:
- Mesh backoff tuning (already reduced from 60s to 5s)
- Priority dialing for lagging peers
- Peer warm-up strategies

**Metrics**:
- Time to first successful sync
- % recovery within 10s/30s
- Dial failure rate

### 4. libp2p Parameter Optimization

**Test impact of**:
- Connection timeout values
- Stream negotiation limits
- Keep-alive duration
- Multiplexing configuration
- Parallel dialing limits

**Measure**:
- Dial latency
- Connection stability
- Resource overhead

---

## Current Architecture

### libp2p Stream Opening Flow

```
open_stream(peer_id)
    │
    ├─► If connected: open substream only (~10-50ms)
    │
    └─► If not connected:
        │
        ├─► TCP connect (~50-100ms local, ~200ms+ remote)
        ├─► TLS handshake (~20-50ms)
        ├─► Muxer negotiation (~10-20ms)
        └─► Substream open (~10-20ms)
```

**Total for new connection**: 150-400ms typical, 1s+ under load

### What We Can Control

1. **At application level**:
   - Peer selection (prefer peers we're already connected to)
   - Request batching (reuse streams for multiple requests)
   - Connection caching (track connection state)

2. **At libp2p config level**:
   - Timeout values
   - Backoff parameters (already tuned)
   - Keep-alive intervals

3. **At protocol level**:
   - Stream multiplexing
   - Pipelining requests

---

## Implementation Plan

### Short-term Wins (1-2 days)

1. ✅ Add dial instrumentation (`PEER_DIAL_BREAKDOWN`)
2. ✅ Track connection state (`ConnectionStateTracker`)
3. ⏳ Prefer already-connected peers in selection
4. ⏳ Log connection reuse rate

### Medium-term Protocol Changes (1 week)

1. Implement connection caching with TTL
2. Add RTT-based peer scoring
3. Parallel dial attempts (try 2-3 peers simultaneously)
4. Keep streams open across sync rounds

### Long-term Architectural Improvements

1. Connection pool with health monitoring
2. Proactive connection establishment to likely peers
3. Persistent connections for active contexts
4. Stream multiplexing optimization

---

## Benchmark Workflows

### `bench-dial-warm.yml`
Test dial latency with warm connections (back-to-back syncs).

### `bench-dial-cold.yml`
Test dial latency after connection close.

### `bench-dial-churn.yml`
Test dial behavior during peer churn.

### `bench-dial-storm.yml`
Test concurrent dial behavior (10+ simultaneous dials).

---

## Success Criteria

A strategy is better if it:
- Reduces P95 dial latency ≥2×
- Reduces P99 dial latency ≥2×
- Improves churn recovery reliability
- Does not significantly increase network overhead

---

## Appendix: Code Changes

### New Files

- `crates/node/src/sync/dial_tracker.rs`: Dial instrumentation and connection state tracking

### Modified Files

- `crates/node/src/sync/manager.rs`: Integrated `DialTracker` into `initiate_sync_inner`
- `crates/node/src/sync/mod.rs`: Exported new dial tracking types
- `scripts/extract-sync-metrics.sh`: Added `PEER_DIAL_BREAKDOWN` extraction

---

---

## Completion Summary

### What Was Implemented

1. ✅ **Dial instrumentation** (`PEER_DIAL_BREAKDOWN` logs)
2. ✅ **Connection state tracking** (`ConnectionStateTracker`)
3. ✅ **RTT-based peer sorting** (prefer already-connected, lower RTT)
4. ✅ **Parallel dial infrastructure** (`ParallelDialTracker`)
5. ✅ **Catch-up mode** for lagging nodes
6. ✅ **Production monitoring** (PromQL alerts + Grafana)

### Benchmark Results (Warm Connection)

| Metric | Value |
|--------|-------|
| Dial P50 | 173ms |
| Dial P95 | 455ms |
| Dial P99 | 538ms |
| Connection Reuse | Heuristic tracking enabled |

### Next Steps (Future Work)

1. **Integrate parallel dialing** into main sync path
2. **Tune libp2p parameters** (timeouts, keep-alive)
3. **Add connection pool** with TTL
4. **Enable stream multiplexing** optimization

See also:
- [BENCHMARK-RESULTS-2026-01.md](BENCHMARK-RESULTS-2026-01.md) - Fresh benchmark results
- [PRODUCTION-MONITORING.md](PRODUCTION-MONITORING.md) - Monitoring setup
- [DECISION-LOG.md](DECISION-LOG.md) - Design decisions

*Last updated: January 31, 2026*
