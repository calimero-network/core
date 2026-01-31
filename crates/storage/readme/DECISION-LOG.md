# Sync Performance Investigation - Decision Log

**Branch**: `test/tree_sync`  
**Date**: January 2026  
**Authors**: Calimero Team

---

## Overview

This document records key architectural and implementation decisions made during the sync performance investigation (Phase 1: Peer Finding, Phase 2: Dial Optimization).

---

## Decision 1: Separate Peer Finding from Dialing

**Date**: January 31, 2026  
**Status**: Accepted

### Context

Initial instrumentation showed "peer selection" taking 99%+ of sync latency. However, this metric conflated two distinct operations:
- **Finding**: Identifying viable peer candidates (mesh lookup, filtering)
- **Dialing**: Establishing TCP/TLS connection to selected peer

### Decision

Separate instrumentation into two distinct phases:
1. `PEER_FIND_PHASES` - measures finding only (no network I/O)
2. `PEER_DIAL_BREAKDOWN` - measures connection establishment only

### Consequences

**Positive**:
- Clear bottleneck identification (dialing is 1500x slower than finding)
- Targeted optimization opportunities
- Accurate latency attribution

**Negative**:
- Two log markers instead of one
- Slightly more complex instrumentation code

### Alternatives Considered

1. **Single combined metric**: Rejected - obscures actual bottleneck
2. **libp2p internal instrumentation**: Rejected - too invasive, version-dependent

---

## Decision 2: RTT-Based Peer Sorting

**Date**: January 31, 2026  
**Status**: Accepted

### Context

Dial latency varies significantly:
- Already-connected peer: ~10-50ms (substream only)
- New connection: ~150-400ms (full TCP+TLS+mux)

### Decision

Sort peer candidates by:
1. Connection state (connected first)
2. RTT estimate (fastest first)

```rust
let score = if is_connected { rtt } else { 1000.0 + rtt };
```

### Consequences

**Positive**:
- Maximizes connection reuse
- Reduces average dial latency
- No additional network overhead

**Negative**:
- May create "hot peer" problem (always selecting same peer)
- RTT estimates can be stale

### Alternatives Considered

1. **Random selection**: Rejected - doesn't leverage connection reuse
2. **Round-robin**: Rejected - ignores connection state
3. **Weighted random**: Considered for future - balances load vs latency

---

## Decision 3: Exponential Moving Average for RTT

**Date**: January 31, 2026  
**Status**: Accepted

### Context

Need to track peer RTT for scoring without storing full history.

### Decision

Use EMA with α=0.2:
```rust
rtt_estimate = old_estimate * 0.8 + new_sample * 0.2
```

### Consequences

**Positive**:
- O(1) space per peer
- Adapts to changing conditions
- Recent samples weighted more

**Negative**:
- Slow to react to sudden changes
- Initial estimate based on single sample

### Alternatives Considered

1. **Simple average**: Rejected - doesn't adapt to changes
2. **Sliding window**: Rejected - O(n) space
3. **Kalman filter**: Rejected - overkill for this use case

---

## Decision 4: Parallel Dialing Support

**Date**: January 31, 2026  
**Status**: Accepted (Infrastructure Only)

### Context

P99 dial latency can exceed 1 second. Trying multiple peers simultaneously could reduce tail latency.

### Decision

Add infrastructure for parallel dialing:
- `ParallelDialConfig` with configurable concurrency
- `ParallelDialTracker` for result aggregation
- Cancel-on-success option

**Not yet integrated** into main sync path - infrastructure only.

### Consequences

**Positive**:
- Reduces P99 latency potential
- First success wins
- Configurable concurrency

**Negative**:
- Wasted connections (cancelled dials)
- Higher resource usage
- Complexity in error handling

### Alternatives Considered

1. **Sequential with short timeout**: Current approach - simpler but slower
2. **Speculative dialing**: Considered - start second dial if first is slow
3. **Connection pool**: Complementary - pre-establish connections

---

## Decision 5: Catch-Up Mode for Churn Recovery

**Date**: January 31, 2026  
**Status**: Accepted

### Context

Edge case benchmarks showed 2/3 restarted nodes failed to catch up within 45 seconds. Gossipsub mesh reformation takes 15-30 seconds, during which sync fails.

### Decision

Add catch-up mode configuration:
- `enable_catchup_mode`: Toggle aggressive sync
- `catchup_mode_threshold`: Failures before entering catch-up (default: 3)
- `max_retries_per_peer`: Retry limit per peer (default: 2)

### Consequences

**Positive**:
- Faster recovery after restart
- Configurable behavior
- Clear failure detection

**Negative**:
- More aggressive network usage during catch-up
- Potential for sync storms if many nodes restart

### Alternatives Considered

1. **Fixed aggressive settings**: Rejected - wastes resources in steady state
2. **External catch-up trigger**: Rejected - requires operator intervention
3. **Peer-assisted catch-up**: Considered for future - peers help lagging nodes

---

## Decision 6: Gossipsub Backoff Reduction

**Date**: January 31, 2026  
**Status**: Accepted

### Context

libp2p gossipsub default `prune_backoff` is 60 seconds. After a node restarts, it must wait this long before re-grafting to mesh, causing sync failures.

### Decision

Reduce gossipsub backoff parameters:
```rust
prune_backoff: Duration::from_secs(5),  // Was 60s
graft_flood_threshold: Duration::from_secs(5),
heartbeat_interval: Duration::from_secs(1),
```

### Consequences

**Positive**:
- Faster mesh reformation after restart
- Reduced sync failures during churn

**Negative**:
- More GRAFT/PRUNE message overhead
- Potential for mesh instability under high churn

### Alternatives Considered

1. **Keep defaults**: Rejected - too slow for our use case
2. **Disable backoff entirely**: Rejected - could cause mesh thrashing
3. **Dynamic backoff**: Considered - adjust based on churn rate

---

## Decision 7: Log Marker Naming Convention

**Date**: January 31, 2026  
**Status**: Accepted

### Context

Need consistent, greppable log markers for metrics extraction.

### Decision

Use `SCREAMING_SNAKE_CASE` markers at start of log message:
- `PEER_FIND_PHASES`
- `PEER_DIAL_BREAKDOWN`
- `PARALLEL_DIAL_RESULT`
- `SYNC_PHASE_BREAKDOWN`
- `DIAL_POOL_STATS`

### Consequences

**Positive**:
- Easy to grep/filter
- Consistent with existing markers
- Clear separation from regular logs

**Negative**:
- Verbose log output
- Requires post-processing to extract metrics

### Alternatives Considered

1. **Prometheus metrics only**: Rejected - loses per-event detail
2. **Structured JSON logs**: Considered - would improve parsing
3. **OpenTelemetry spans**: Considered for future - better distributed tracing

---

## Decision 8: Connection State Tracking Scope

**Date**: January 31, 2026  
**Status**: Accepted

### Context

Need to track which peers are likely connected for RTT-based sorting.

### Decision

Track connection state **per SyncManager instance** (not globally):
- `ConnectionStateTracker` is per-SyncManager
- State is lost on node restart
- No persistence to disk

### Consequences

**Positive**:
- Simple implementation
- No persistence overhead
- Naturally resets on restart

**Negative**:
- Cold start after restart
- No cross-context sharing

### Alternatives Considered

1. **Global singleton**: Rejected - harder to test
2. **Persisted state**: Rejected - complexity not justified
3. **Network-level tracking**: Considered - hook into libp2p events

---

## Decision 9: Production Monitoring Strategy

**Date**: January 31, 2026  
**Status**: Accepted

### Context

Need to monitor sync performance in production.

### Decision

Provide:
1. PromQL alerts for critical conditions
2. Grafana dashboard queries
3. SLO recommendations
4. Log-based alerts for systems without Prometheus

Key SLOs:
- Sync success rate: ≥99%
- Dial P99 latency: <2s
- Connection reuse rate: ≥50%

### Consequences

**Positive**:
- Actionable alerts
- Clear SLO targets
- Multiple monitoring options

**Negative**:
- Requires Prometheus/Grafana setup
- Alert tuning needed per deployment

### Alternatives Considered

1. **Metrics only, no alerts**: Rejected - reactive, not proactive
2. **Custom monitoring daemon**: Rejected - unnecessary complexity
3. **Third-party APM integration**: Considered for future

---

## Decision 10: Benchmark Workflow Design

**Date**: January 31, 2026  
**Status**: Accepted

### Context

Need repeatable benchmarks to validate optimizations.

### Decision

Use merobox workflow YAML files:
- `bench-dial-warm.yml`: Back-to-back syncs (connection reuse)
- `bench-dial-cold.yml`: After restart (new connections)
- `bench-*` prefix for benchmarks
- `test-*` prefix for functional tests

### Consequences

**Positive**:
- Repeatable
- CI/CD compatible
- Self-documenting

**Negative**:
- Requires merobox installation
- Local-only (not distributed)

### Alternatives Considered

1. **Unit test benchmarks**: Rejected - don't test real network
2. **Manual testing**: Rejected - not repeatable
3. **Distributed test framework**: Considered for future

---

## Summary Table

| # | Decision | Status | Impact |
|---|----------|--------|--------|
| 1 | Separate finding from dialing | ✅ Accepted | High - correct bottleneck identification |
| 2 | RTT-based peer sorting | ✅ Accepted | Medium - reduces dial latency |
| 3 | EMA for RTT tracking | ✅ Accepted | Low - implementation detail |
| 4 | Parallel dialing infrastructure | ✅ Accepted | Medium - enables P99 reduction |
| 5 | Catch-up mode | ✅ Accepted | High - fixes churn recovery |
| 6 | Gossipsub backoff reduction | ✅ Accepted | High - faster mesh reformation |
| 7 | Log marker convention | ✅ Accepted | Low - consistency |
| 8 | Connection state scope | ✅ Accepted | Low - simplicity |
| 9 | Production monitoring | ✅ Accepted | High - operational readiness |
| 10 | Benchmark workflows | ✅ Accepted | Medium - validation |

---

## Open Questions / Future Decisions

1. **Should we implement speculative dialing?** (Start second dial if first is slow)
2. **Should we persist connection state across restarts?**
3. **Should we implement a connection pool with keep-alive?**
4. **Should we use weighted random instead of sorted selection?**
5. **Should we add OpenTelemetry tracing?**

---

*Last updated: January 31, 2026*
