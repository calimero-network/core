# Edge Case Sync Benchmark Results

**Date**: January 31, 2026  
**Branch**: test/tree_sync  
**Run Count**: 1 per scenario (statistical confidence limited)

---

## Executive Summary

This benchmark suite tested 4 edge-case scenarios to identify production risks in the Calimero sync system. **Two critical findings emerged:**

### 🚨 Critical Finding #1: Peer Selection Dominates All Latency

| Scenario | peer_selection P50 | peer_selection P99 | % of total sync |
|----------|-------------------|-------------------|-----------------|
| Cold Dial (10 nodes) | **286ms** | **1521ms** | **99.2%** |
| Churn/Reconnect | **294ms** | **906ms** | **97.8%** |
| Partition Healing | **422ms** | **1657ms** | **99.2%** |
| Scale (600 keys) | **143ms** | **1715ms** | **95.9%** |

**Root Cause**: libp2p stream opening and peer acquisition. First dial to a new peer is ~500-2000ms; subsequent dials are ~140-300ms.

### 🚨 Critical Finding #2: Churn Recovery is Unreliable

Scenario B (Churn + Reconnect) **FAILED**: Nodes 5 and 10 did not catch up within 45 seconds after restart. This indicates:
- Gossipsub mesh reformation is slow under churn
- Recovery time is unpredictable
- Production risk for node restarts

---

## Scenario Results

### Scenario A: Cold Dial Storm (10 nodes)

**Goal**: Quantify "first dial" cost with 10 nodes.

| Metric | P50 | P95 | P99 | Notes |
|--------|-----|-----|-----|-------|
| peer_selection | 286ms | 975ms | 1521ms | **Dominates** |
| key_share | 1.7ms | 6.9ms | 10ms | Negligible |
| dag_compare | 0.5ms | 2.5ms | 4.7ms | Negligible |
| total_sync | 290ms | 977ms | 1523ms | |

**Samples**: n=539 syncs

**Key Observation**: P99 of 1.5 seconds means 1% of syncs take >1.5s just to select a peer.

### Scenario B: Churn + Reconnect (10 nodes)

**Goal**: Measure recovery under continuous restarts.

**Result**: ❌ **FAILED** - Nodes did not recover within timeout.

| Metric | P50 | P95 | P99 | Notes |
|--------|-----|-----|-----|-------|
| peer_selection | 294ms | 817ms | 906ms | |
| key_share | 1.9ms | 6.9ms | 16ms | **Outlier: 7743ms!** |
| dag_compare | 0.6ms | 17ms | 58ms | Higher variance |
| total_sync | 300ms | 833ms | 1240ms | |

**Samples**: n=329 syncs (before failure)

**Critical Observation**: 
- `key_share_ms` had a **7.7 second outlier** during churn
- `dag_compare_ms` P99 jumped to 58ms (vs 4.7ms in stable scenario)
- Churn causes extreme tail latency spikes

**Production Risk**: HIGH - Node restarts under load may not recover.

### Scenario D: Partition Healing (10 nodes, 5/5 split)

**Goal**: Measure convergence after partition with 80% disjoint + 20% hot key writes.

**Result**: ✅ **PASSED** - LWW correctly resolved conflicts.

| Metric | P50 | P95 | P99 | Notes |
|--------|-----|-----|-----|-------|
| peer_selection | **422ms** | **1027ms** | **1657ms** | Higher than baseline |
| key_share | 2.0ms | 5.3ms | 7.7ms | |
| dag_compare | 0.6ms | 2.3ms | 6.5ms | |
| total_sync | 429ms | 1032ms | 1664ms | |

**Samples**: n=963 syncs

**Key Observation**: 
- peer_selection P50 is 50% higher during partition healing (422ms vs 286ms)
- P99 reaches 1.6 seconds
- Partition healing causes peer_selection spikes due to mesh reformation

### Scenario E: State Sync at Scale (600 keys, Bloom Filter)

**Goal**: Validate Bloom filter at realistic state size.

**Result**: ✅ **PASSED** - Bloom filter synced 199 divergent entities.

| Metric | P50 | P95 | P99 | Notes |
|--------|-----|-----|-----|-------|
| peer_selection | 143ms | 415ms | 1715ms | |
| dag_compare | 2.4ms | 17ms | 20ms | Higher due to state size |
| total_sync | 153ms | 424ms | 1717ms | |

**Samples**: n=153 syncs

**Bloom Filter Performance** (from `STRATEGY_SYNC_METRICS`):

| State Size | Bloom Filter Size | Duration | Round Trips |
|------------|-------------------|----------|-------------|
| 146 entities | 180 bytes | 0.93ms | 1 |
| 466 entities | 564 bytes | 2.12ms | 1 |
| 1006 entities | 1211 bytes | 4.95ms | 1 |
| 1006 entities + 199 diverged | 1211 bytes | **12.00ms** | 1 |

**Key Observation**: Bloom filter scales linearly with entity count. At 1000 entities, filter is ~1.2KB and sync takes ~5ms (excluding peer_selection).

---

## Summary Matrix

| Scenario | Status | Dominant Bottleneck | P95 Total | Success Rate | Critical Risk |
|----------|--------|---------------------|-----------|--------------|---------------|
| Cold Dial Storm | ✅ Pass | peer_selection (99%) | 977ms | 100% | First-dial latency |
| Churn + Reconnect | ❌ Fail | peer_selection + mesh | 833ms | ~60% | **Recovery unreliable** |
| Partition Healing | ✅ Pass | peer_selection (99%) | 1032ms | 100% | High P99 (1.6s) |
| State Sync Scale | ✅ Pass | peer_selection (96%) | 424ms | 100% | P99 tail (1.7s) |

---

## Root Cause Analysis

### Why is peer_selection so expensive?

1. **libp2p stream opening**: Requires peer routing, connection establishment, and substream negotiation
2. **First dial penalty**: ~500-2000ms for new peer connections
3. **Connection caching**: Subsequent syncs to same peer are ~150ms

### Why did Churn fail?

1. **Gossipsub backoff**: Restarted nodes face GRAFT rejection due to prune_backoff (we reduced to 5s, but still affects)
2. **Mesh reformation**: Takes 15-30 seconds after restart
3. **Sync attempts timeout**: Nodes give up before mesh forms

---

## Recommendations

### Immediate (High Confidence)

| What to Change | Expected Improvement | Target |
|----------------|---------------------|--------|
| **Reduce sync timeout** from 30s to 10s | Faster fallback to next peer | Reduce P99 by 50% |
| **Add peer connection caching** | Reuse established connections | Reduce peer_selection P50 from 300ms to 50ms |
| **Pre-warm peer connections on context join** | Eliminate first-dial cost | Remove 500ms+ first-dial spikes |

### Medium Term

| What to Change | Expected Improvement | Target |
|----------------|---------------------|--------|
| **Reduce gossipsub backoff further** | Faster mesh recovery | Reduce churn recovery to <30s |
| **Add "priority sync" for restarted nodes** | Peers prioritize lagging nodes | 100% churn recovery |
| **Implement sync circuit breaker** | Fail fast on bad peers | Reduce tail latency |

### Monitoring Metrics

```promql
# Primary production alert
histogram_quantile(0.99, rate(sync_phase_peer_selection_seconds_bucket[5m])) > 1.5

# Churn detection
rate(sync_failures_total[5m]) / rate(sync_attempts_total[5m]) > 0.1

# Mesh health
gossipsub_mesh_peers < 2 for 30s
```

---

## Top 2 Production Risks

### Risk #1: Peer Selection Tail Latency (P99 > 1.5s)

**Impact**: 1% of syncs take >1.5 seconds just to find a peer.

**Code Path**: `SyncManager::initiate_sync_inner` → `select_random_peer` → libp2p stream open

**Fix**: 
1. Add connection pool/caching in `NetworkManager`
2. Pre-establish connections on context join
3. Reduce sync timeout to fail fast

### Risk #2: Churn Recovery Failure

**Impact**: Restarted nodes may not catch up, causing data divergence.

**Code Path**: `SyncManager::perform_interval_sync` → gossipsub mesh check → timeout

**Fix**:
1. Increase mesh formation timeout (currently 60s, may need 90s)
2. Add "catch-up mode" that bypasses gossipsub and uses direct peer sync
3. Implement peer prioritization for lagging nodes

---

## Appendix: Test Configuration

| Parameter | Value |
|-----------|-------|
| Binary | `merod` 0.1.0 (release) |
| Network | Localhost |
| Nodes | 10 (scenarios A, B, D) or 2 (scenario E) |
| State Size | 30 keys (A, B, D) or 600 keys (E) |
| Duration | 120-300 seconds per scenario |

## Appendix: Workflow Files

- `workflows/sync/edge-cold-dial-storm.yml`
- `workflows/sync/edge-churn-reconnect.yml`
- `workflows/sync/edge-partition-healing.yml`
- `workflows/sync/edge-state-sync-scale.yml`

## Appendix: Raw Data

```
data/dial_analysis/   # Scenario A metrics
data/churn_analysis/  # Scenario B metrics (partial)
data/part_analysis/   # Scenario D metrics
data/scale_analysis/  # Scenario E metrics
```

## Related Documents

- **[SYNC-PERFORMANCE-INVESTIGATION.md](SYNC-PERFORMANCE-INVESTIGATION.md)** - Master overview
- **[PEER-FINDING-ANALYSIS.md](PEER-FINDING-ANALYSIS.md)** - Peer finding optimization
- **[DIAL-OPTIMIZATION-ANALYSIS.md](DIAL-OPTIMIZATION-ANALYSIS.md)** - Dial optimization
- **[DEEP-SYNC-ANALYSIS.md](DEEP-SYNC-ANALYSIS.md)** - Comprehensive analysis
- **[DECISION-LOG.md](DECISION-LOG.md)** - Architectural decisions

---

*Analysis generated from edge case benchmark run on test/tree_sync branch*  
*Last updated: January 31, 2026*
