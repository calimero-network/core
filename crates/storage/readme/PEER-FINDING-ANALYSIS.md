# Peer Finding Performance Analysis

**Date**: January 31, 2026  
**Branch**: test/tree_sync  
**Status**: In Progress

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

## Primary KPI

```
time_to_viable_peer_ms = time from "sync tick starts" → "we have at least 1 viable peer candidate"
```

A peer is **viable** if:
- ✅ Reachable (we have an address/routing path)
- ✅ In same context/topic membership
- ✅ Not in backoff / not recently failing
- ✅ Likely to have needed state (recently active)

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

### Phase 1: Instrumentation
1. Add `PEER_FIND_BREAKDOWN` logging
2. Separate "find" time from "dial" time
3. Track peer sources and quality metrics

### Phase 2: Strategy Implementation
1. Add `RecentPeerCache` (LRU of last successful peers)
2. Add `PeerQualityTracker` (failure counts, last success time)
3. Implement alternative strategies (A1-A5)

### Phase 3: Benchmarking
1. Create workflows for each scenario
2. Run each strategy × scenario combination
3. Analyze results and recommend default

---

## Related Files

- `crates/node/src/sync/manager.rs` - Peer selection logic
- `crates/node/src/sync/peer_tracker.rs` - Peer tracking (to be created)
- `crates/network/src/lib.rs` - Network manager
- `EDGE-CASE-BENCHMARK-RESULTS.md` - Baseline data

---

*Analysis in progress on test/tree_sync branch*
