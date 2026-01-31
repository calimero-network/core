# Peer Finding Performance Analysis

**Date**: January 31, 2026  
**Branch**: test/tree_sync  
**Status**: ✅ Complete

---

## Problem Statement

Edge case benchmarks revealed that **peer_selection dominates 99%+ of sync latency**:

| Scenario | peer_selection P99 | % of total sync |
|----------|-------------------|-----------------|
| Cold Dial Storm | 1521ms | 99.2% |
| Partition Healing | 1657ms | 99.2% |
| State Sync Scale | 1715ms | 95.9% |

**Root Cause Hypothesis**: The current peer finding path combines multiple slow operations:
1. Gossipsub mesh membership check
2. libp2p routing/Kademlia lookup
3. Address book queries
4. Connection establishment (dial)

We need to isolate and measure each component to optimize.

---

## Primary KPIs (Finding vs Connecting)

**CRITICAL DISTINCTION**: This analysis measures **finding** time, not **connecting** time.

| KPI | Description | Includes Dial? |
|-----|-------------|----------------|
| `time_to_candidate_ms` | Time to produce raw candidate list | ❌ NO |
| `time_to_viable_peer_ms` | Time to select peer after filters | ❌ NO |
| `dial_ms` | Time to connect to selected peer | ✅ YES (separate) |

```
time_to_viable_peer_ms = candidate_lookup_ms + filtering_ms + selection_ms
```

A peer is **viable** if:
- ✅ In same context/topic membership
- ✅ Not in backoff / not recently failing
- ✅ Likely to have needed state (recently active)

**Note**: "Reachable" is NOT determined during finding - that's what dial tests.

---

## Secondary KPIs

| Metric | Description |
|--------|-------------|
| `peer_find_success_rate` | % of attempts that find a peer within 1s/3s/10s |
| `candidates_found` | Number of candidates per attempt |
| `time_to_first_reconcile_ms` | End-to-end sync (secondary) |
| `churn_recovery_time` | Time to find peer after restart |
| `false_candidate_rate` | % candidates that fail when contacted |

---

## Instrumentation Design

### New Log Marker: `PEER_FIND_BREAKDOWN`

```
PEER_FIND_BREAKDOWN
  context_id=<id>
  peer_find_total_ms=<float>
  from_mesh_ms=<float>
  from_routing_table_ms=<float>
  from_address_book_ms=<float>
  from_recent_peers_ms=<float>
  candidates_total=<int>
  candidates_from_mesh=<int>
  candidates_from_routing=<int>
  candidates_from_book=<int>
  candidates_from_recent=<int>
  candidates_after_filters=<int>
  selected_peer_source=<mesh|routing|book|recent>
  was_recently_successful=<bool>
  recent_failure_count=<int>
  last_success_ms_ago=<int|null>
```

---

## Peer Finding Strategies to Test

### A0: Baseline (Current)
Current implementation: mesh check → fail if empty.

### A1: Mesh-First
Only gossipsub mesh peers; no routing lookup.
- Fastest when mesh is populated
- Fails if mesh is empty (restart scenario)

### A2: Recent-First
LRU cache of last successful peers → mesh → routing.
- Prioritizes known-good peers
- Requires maintaining recent peer cache

### A3: Address-Book-First
Persisted known peers (from previous runs) → mesh → routing.
- Helps cold start and restart
- Requires persistent peer storage

### A4: Parallel Find
Query mesh + recent + address-book + routing in parallel; take first viable.
- Lowest latency in theory
- Higher resource usage

### A5: Health-Filtered
Exclude peers with failures in last X seconds, then select.
- Reduces false candidate rate
- May reduce candidates in degraded network

---

## Test Scenarios

| Scenario | Description | Expected Challenge |
|----------|-------------|-------------------|
| **Warm Steady-State** | Network stable, sync already running | Baseline performance |
| **Cold Start Join** | Node joins context fresh (no recent peers) | No cached peers |
| **Churn Restart** | Node restarts while others continue | Mesh empty, backoff active |
| **Partition Heal** | 10 nodes split 5/5 for 30s, then reconnect | Stale peer info |
| **Dial Storm** | 10 nodes start simultaneously | Contention |

---

## Success Criteria

A strategy is **better** if it:
- ✅ Reduces P95 `time_to_viable_peer_ms` by ≥ 2×
- ✅ Reduces P99 by ≥ 2×
- ✅ Improves churn restart "find peer within 10s" to ~100%
- ✅ Does not materially increase false candidate rate

---

## Current Architecture

### Peer Selection Code Path

```
SyncManager::initiate_sync_inner()
  → select_random_peer()              // Current: simple random from mesh
    → get_context_peers()             // Query gossipsub mesh
    → filter_by_backoff()             // Remove recently failed
    → random_choice()                 // Pick one
```

### Bottleneck Analysis (from edge case data)

| Phase | Current Time | Target |
|-------|--------------|--------|
| Mesh membership check | ~10ms | ~10ms (acceptable) |
| **Peer dial/stream open** | **500-2000ms** | **<100ms** |
| Total peer_selection | 286-422ms P50 | <100ms P50 |

**Key Insight**: The "peer_selection" phase in our logs includes the dial time. We need to separate:
1. **Finding** a peer (should be <10ms)
2. **Connecting** to a peer (currently 500-2000ms)

---

## Implementation Plan

### Phase 1: Instrumentation ✅ COMPLETE
1. ✅ Add `PEER_FIND_BREAKDOWN` logging
2. ✅ Separate "find" time from "dial" time  
3. ✅ Track peer sources and quality metrics

**Implemented in**: `crates/node/src/sync/peer_finder.rs`

### Phase 2: Strategy Implementation ✅ COMPLETE
1. ✅ Add `RecentPeerCache` (LRU of last successful peers) - **Implemented**
2. ✅ Add `PeerQualityTracker` (failure counts, last success time) - **Implemented**
3. ✅ Implement alternative strategies (A0-A5) - **Implemented**

Strategies available via `--peer-find-strategy`:
- `baseline` (A0): Current mesh-only
- `mesh-first` (A1): Only mesh peers
- `recent-first` (A2): LRU cache → mesh
- `address-book-first` (A3): Persisted → mesh (stub)
- `parallel` (A4): All sources in parallel
- `health-filtered` (A5): Exclude failing peers

### Phase 3: Benchmarking ✅ COMPLETE

## Benchmark Results

### Executive Summary

**CRITICAL FINDING**: Peer FINDING is NOT the bottleneck. Peer DIALING is.

| Phase | P50 Latency | Bottleneck? |
|-------|-------------|-------------|
| **Peer Finding** | **0.04 - 0.12ms** | ❌ NO |
| **Peer Dialing** | **152 - 185ms** | ✅ YES |

The previous analysis conflated finding and dialing. With proper separation:

- **Finding** (candidate lookup → filter → select) = sub-millisecond
- **Dialing** (TCP connect → TLS → substream negotiate) = ~170ms

### Finding Phase Breakdown (sample run)

| Phase | Time |
|-------|------|
| `candidate_lookup_ms` | 0.00 - 0.01ms |
| `filtering_ms` | 0.00ms |
| `selection_ms` | 0.03 - 0.11ms |
| **Total Finding** | **0.04 - 0.12ms** |

### Dialing Phase

| Metric | P50 |
|--------|-----|
| `dial_ms` | 152 - 185ms |

### Strategy Comparison (Finding Only)

Since finding is already sub-millisecond, strategy optimization has minimal impact:

| Strategy | Finding P50 | Finding Improvement |
|----------|-------------|---------------------|
| Baseline (mesh) | 0.08ms | - |
| Recent-First | 0.04ms | 50% faster (but both <1ms) |

**Conclusion**: Strategy choice matters little when finding is already <1ms.

### Where Optimization Should Focus

The **dial phase** at ~170ms is 1500x slower than finding. Optimization should target:

1. **Connection reuse** - keep streams open across sync rounds
2. **Multiplexing** - use existing connections when available
3. **Parallel dial** - try multiple peers simultaneously
4. **Warm connection pool** - pre-establish connections to likely peers

### Recommendation

1. **Production default**: Keep `baseline` - finding is fast enough
2. **Optimize dial path**: Connection pooling and reuse
3. **Monitor `dial_ms`**: This is the true latency indicator

## Running Instrumentation

The `PEER_FIND_BREAKDOWN` log marker is now emitted on every peer finding attempt. Extract metrics with:

```bash
./scripts/extract-sync-metrics.sh <prefix>
```

Output includes:
- `peer_find_total_ms`: P50/P95 of total peer finding time
- `from_mesh_ms`: Time spent querying gossipsub mesh
- `candidates_total`: Average candidates found

---

## Related Files

- `crates/node/src/sync/manager.rs` - Peer selection logic
- `crates/node/src/sync/peer_tracker.rs` - Peer tracking (to be created)
- `crates/network/src/lib.rs` - Network manager
- `EDGE-CASE-BENCHMARK-RESULTS.md` - Baseline data

---

---

## Conclusion

Phase 1 analysis is complete. Key findings:

1. **Peer finding is NOT a bottleneck** - sub-millisecond performance
2. **Peer dialing IS the bottleneck** - 150-200ms P50
3. **Strategy optimization has minimal impact** when finding is already <1ms
4. **Recommendation**: Focus optimization on dial path (Phase 2)

See also:
- [DIAL-OPTIMIZATION-ANALYSIS.md](DIAL-OPTIMIZATION-ANALYSIS.md) - Phase 2 analysis
- [BENCHMARK-RESULTS-2026-01.md](BENCHMARK-RESULTS-2026-01.md) - Detailed results
- [DECISION-LOG.md](DECISION-LOG.md) - Architectural decisions

*Last updated: January 31, 2026*
