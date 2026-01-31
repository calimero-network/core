# Sync Performance Investigation

**Branch**: `test/tree_sync`  
**Date Range**: January 2026  
**Status**: ✅ Phase 1 & Phase 2 Complete

---

## Executive Summary

This investigation identified and optimized sync latency bottlenecks in the Calimero node. The work is divided into two phases:

| Phase | Focus | Finding | Status |
|-------|-------|---------|--------|
| **Phase 1** | Peer Finding | Finding is fast (<0.12ms) | ✅ Complete |
| **Phase 2** | Peer Dialing | Dialing is the bottleneck (150-200ms) | ✅ Complete |

### Key Insight

The initial hypothesis that "peer selection dominates sync time" was partially correct but misleading. After proper instrumentation:

```
Peer Selection = Peer Finding + Peer Dialing
                 └── <0.12ms    └── 150-200ms (THE ACTUAL BOTTLENECK)
```

---

## Phase 1: Peer Finding Analysis

### Objective

Measure and optimize the time to identify a viable synchronization peer.

### Instrumentation Added

#### Log Marker: `PEER_FIND_PHASES`

```
PEER_FIND_PHASES
  context_id=<id>
  strategy=<strategy>
  time_to_candidate_ms=<float>      # Time to get raw candidates
  filtering_ms=<float>              # Time to apply quality filters
  selection_ms=<float>              # Time to pick final peer
  time_to_viable_peer_ms=<float>    # Total finding time (no dial)
  candidates_raw=<int>
  candidates_filtered=<int>
  attempt_count=<int>
  from_mesh=<int>
  from_recent=<int>
  from_book=<int>
  from_routing=<int>
  peer_source=<mesh|recent|address_book|routing|unknown>
  was_recent_success=<bool>
  result=<success|no_candidates|all_filtered|timeout|unknown>
```

### Strategies Tested

| Strategy | Description |
|----------|-------------|
| `A0_Baseline` | Current mesh-only approach |
| `A1_MeshFirst` | Only gossipsub mesh peers, no fallback |
| `A2_RecentFirst` | LRU cache → mesh → routing |
| `A3_AddressBookFirst` | Persisted peers → mesh → routing |
| `A4_ParallelFind` | Query all sources in parallel |
| `A5_HealthFiltered` | Exclude peers with recent failures |

### Results

| Phase | P50 Latency |
|-------|-------------|
| `candidate_lookup_ms` | 0.00 - 0.01ms |
| `filtering_ms` | 0.00ms |
| `selection_ms` | 0.03 - 0.11ms |
| **Total Finding** | **0.04 - 0.12ms** |

### Conclusion

**Peer finding is NOT a bottleneck.** Strategy optimization has minimal impact when finding is already sub-millisecond.

### Files Added/Modified

- `crates/node/src/sync/peer_finder.rs`: Finding strategies and tracking
- `crates/storage/readme/PEER-FINDING-ANALYSIS.md`: Full analysis

---

## Phase 2: Dial/Connection Optimization

### Objective

Minimize `time_to_connected_peer_ms` - the time from peer selection to stream ready.

### Problem Statement

| Metric | Latency |
|--------|---------|
| Peer Finding | 0.04 - 0.12ms |
| **Peer Dialing** | **150-200ms P50, >1s P99** |

The dialing phase includes:
- TCP connection establishment
- TLS handshake
- Muxer negotiation
- Substream opening

### Optimizations Implemented

#### 1. RTT-Based Peer Sorting
Peers are sorted to prefer already-connected peers first, then by RTT estimate.

#### 2. Connection State Tracking
`ConnectionStateTracker` maintains per-peer state with exponential moving average RTT.

#### 3. Parallel Dialing Support
`ParallelDialTracker` enables trying multiple peers simultaneously for P99 reduction.

#### 4. Churn Recovery Mode
Configurable catch-up mode that detects lagging nodes and increases sync frequency.

#### 5. Production Monitoring
Comprehensive PromQL alerts and Grafana dashboard queries. See `PRODUCTION-MONITORING.md`.

### Instrumentation Added

#### Log Marker: `PEER_DIAL_BREAKDOWN`

```
PEER_DIAL_BREAKDOWN
  peer_id=<id>
  was_connected_initially=<bool>    # Did we have a connection?
  total_dial_ms=<float>             # Time for libp2p open_stream
  reuse_connection=<bool>           # Heuristic: dial <50ms = reused
  attempt_index=<int>               # Which attempt (1 = first)
  result=<success|timeout|refused|error>
```

#### Log Marker: `DIAL_POOL_STATS`

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

### Optimizations Implemented

#### 1. Connection State Tracking

`ConnectionStateTracker` maintains per-peer state:
- `connected_since`: When connection was established
- `rtt_estimate_ms`: RTT estimate (exponential moving average)
- `consecutive_failures`: Failure count

#### 2. RTT-Based Peer Sorting

Selection phase now prefers already-connected peers:

```rust
// Score: connected peers first (by RTT), then disconnected
let score = if is_connected { rtt } else { 1000.0 + rtt };
peers_with_score.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
```

### Experiments Completed

| Experiment | Goal | Status |
|------------|------|--------|
| Connection Pooling | Reduce dialing by reusing live connections | ✅ Tracking added |
| Peer Scoring | Prefer peers likely to respond quickly | ✅ RTT-based sorting |
| Churn Recovery | Fast reconnection after restart | ✅ Catch-up mode added |
| Parallel Dialing | Try multiple peers for P99 reduction | ✅ Infrastructure ready |
| Production Monitoring | PromQL alerts and Grafana | ✅ Complete |

### Benchmark Workflows

| Workflow | Tests |
|----------|-------|
| `bench-dial-warm.yml` | Back-to-back syncs (connection reuse) |
| `bench-dial-cold.yml` | After node restart (new connections) |

### Files Added/Modified

- `crates/node/src/sync/dial_tracker.rs`: Dial instrumentation
- `crates/node/src/sync/manager.rs`: Integrated dial tracking + RTT sorting
- `crates/storage/readme/DIAL-OPTIMIZATION-ANALYSIS.md`: Phase 2 docs
- `scripts/benchmark-dial-latency.sh`: Benchmark runner
- `scripts/extract-sync-metrics.sh`: Added dial breakdown extraction

---

## Complete File Inventory

### New Sync Infrastructure Files

| File | Purpose |
|------|---------|
| `crates/node/src/sync/peer_finder.rs` | Peer finding strategies and instrumentation |
| `crates/node/src/sync/dial_tracker.rs` | Dial instrumentation and connection state |
| `crates/node/src/sync/tree_sync.rs` | State sync protocols (Hash, Bloom, etc.) |
| `crates/node/src/sync/metrics.rs` | Prometheus metrics for sync |

### Documentation Files

| File | Content |
|------|---------|
| `crates/storage/readme/CIP-sync-protocol.md` | Master sync protocol specification |
| `crates/storage/readme/PEER-FINDING-ANALYSIS.md` | Phase 1 analysis and results |
| `crates/storage/readme/DIAL-OPTIMIZATION-ANALYSIS.md` | Phase 2 analysis and roadmap |
| `crates/storage/readme/PRODUCTION-MONITORING.md` | PromQL alerts and Grafana dashboards |
| `crates/storage/readme/DEEP-SYNC-ANALYSIS.md` | Comprehensive benchmark analysis |
| `crates/storage/readme/SYNC-STRATEGY-ANALYSIS.md` | State sync strategy comparison |
| `crates/storage/readme/EDGE-CASE-BENCHMARK-RESULTS.md` | Edge case test results |
| `crates/storage/readme/BENCHMARK-RESULTS.md` | General benchmark results |
| `crates/storage/readme/BENCHMARK-RESULTS-2026-01.md` | January 2026 benchmark results |
| `crates/storage/readme/DECISION-LOG.md` | Architectural decision log |

### Benchmark Workflows

| Workflow | Tests |
|----------|-------|
| `workflows/sync/bench-dial-warm.yml` | Warm connection dial latency |
| `workflows/sync/bench-dial-cold.yml` | Cold connection dial latency |
| `workflows/sync/bench-fresh-node-snapshot.yml` | Fresh node bootstrap (snapshot) |
| `workflows/sync/bench-fresh-node-delta.yml` | Fresh node bootstrap (delta) |
| `workflows/sync/bench-3n-10k-disjoint.yml` | 3 nodes, 10 disjoint keys |
| `workflows/sync/bench-3n-50k-disjoint.yml` | 3 nodes, 50 disjoint keys |
| `workflows/sync/bench-3n-50k-conflicts.yml` | 3 nodes, LWW conflicts |
| `workflows/sync/bench-3n-late-joiner.yml` | Late joining node |
| `workflows/sync/bench-3n-restart-catchup.yml` | Restart and catchup |
| `workflows/sync/bench-continuous-write.yml` | Continuous write load |
| `workflows/sync/bench-partition-healing.yml` | Partition healing |
| `workflows/sync/bench-hot-key-contention.yml` | Hot key contention |
| `workflows/sync/test-bloom-filter.yml` | Bloom filter sync test |
| `workflows/sync/test-hash-comparison.yml` | Hash comparison sync test |
| `workflows/sync/test-subtree-prefetch.yml` | Subtree prefetch sync test |
| `workflows/sync/test-level-wise.yml` | Level-wise sync test |

### Scripts

| Script | Purpose |
|--------|---------|
| `scripts/extract-sync-metrics.sh` | Extract metrics from node logs |
| `scripts/benchmark-dial-latency.sh` | Run dial latency benchmarks |
| `scripts/benchmark-sync-strategies.sh` | Compare state sync strategies |
| `scripts/benchmark-peer-finding.sh` | Test peer finding strategies |
| `scripts/run-sync-benchmarks.sh` | Master benchmark orchestrator |

---

## Log Markers Reference

### Peer Finding

| Marker | When Emitted |
|--------|--------------|
| `PEER_FIND_PHASES` | After peer selection (before dial) |

### Dialing

| Marker | When Emitted |
|--------|--------------|
| `PEER_DIAL_BREAKDOWN` | After each dial attempt |
| `PEER_DIAL_TIMING` | After dial (with finding time) |
| `DIAL_POOL_STATS` | Periodic pool statistics |

### Sync Operations

| Marker | When Emitted |
|--------|--------------|
| `SYNC_PHASE_BREAKDOWN` | After sync completes |
| `DELTA_APPLY_TIMING` | After delta application |
| `STRATEGY_SYNC_METRICS` | After state sync strategy runs |

---

## CLI Arguments Added

```bash
merod run \
  --sync-strategy <snapshot|delta|adaptive> \
  --state-sync-strategy <adaptive|hash-comparison|snapshot|bloom-filter|subtree-prefetch|level-wise> \
  --force-state-sync \
  --peer-find-strategy <baseline|mesh-first|recent-first|address-book-first|parallel|health-filtered>
```

---

## Metrics Extraction

### Run Extraction

```bash
./scripts/extract-sync-metrics.sh <prefix> <data_dir>

# Example:
./scripts/extract-sync-metrics.sh dial ./data
```

### Output

- `<prefix>_metrics/summary.md`: Human-readable summary
- `<prefix>_metrics/dial_breakdown_raw.csv`: Raw dial timing data
- `<prefix>_metrics/peer_find_raw.csv`: Raw peer finding data
- `<prefix>_metrics/<strategy>_raw.csv`: Per-strategy data

---

## Git Commits (Phase 1 + Phase 2)

```
889a9973 feat(sync): Phase 2 - Dial latency instrumentation and optimization
8a42c33e feat(sync): Separate peer finding from dialing with proper phase tracking
6266d2f7 docs: Update peer finding analysis with benchmark results
4edd6048 feat(sync): Implement peer finding strategies A0-A5
2803000b feat(sync): Update peer finding docs and metrics extraction
2dcbf39b feat(sync): Add peer finding instrumentation (PEER_FIND_BREAKDOWN)
32dda8fd docs: Add peer finding analysis plan
85b7af70 docs: Add edge case benchmark references to CIP and analysis docs
8e674fcb feat(bench): Add edge case benchmarks and analysis
```

---

## Completed Work Summary

### Phase 1: Peer Finding ✅
- Separated finding from dialing instrumentation
- Tested 6 peer finding strategies (A0-A5)
- Confirmed finding is NOT the bottleneck (<0.12ms)

### Phase 2: Dial Optimization ✅
- Implemented RTT-based peer sorting
- Added connection state tracking
- Built parallel dialing infrastructure
- Created catch-up mode for churn recovery
- Added production monitoring alerts

### Deliverables
- ✅ `DECISION-LOG.md` - Key architectural decisions
- ✅ `BENCHMARK-RESULTS-2026-01.md` - January benchmark results
- ✅ `PRODUCTION-MONITORING.md` - PromQL alerts + Grafana
- ✅ Fixed dial warm/cold benchmark workflows

## Future Phases (Roadmap)

- **Phase 3**: Stream Multiplexing - reuse streams for multiple requests
- **Phase 4**: Proactive Connection Pool - pre-establish likely connections
- **Phase 5**: Protocol Optimization - pipeline requests, batch operations

---

## Appendix: Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                        SyncManager                               │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────────┐    ┌──────────────────┐                   │
│  │   PeerFinder     │    │   DialTracker    │                   │
│  │                  │    │                  │                   │
│  │ - strategies     │    │ - dial timing    │                   │
│  │ - recent cache   │    │ - RTT tracking   │                   │
│  │ - quality filter │    │ - pool stats     │                   │
│  └────────┬─────────┘    └────────┬─────────┘                   │
│           │                       │                              │
│           │  PEER_FIND_PHASES     │  PEER_DIAL_BREAKDOWN        │
│           │  (<0.12ms)            │  (150-200ms)                │
│           ▼                       ▼                              │
│  ┌─────────────────────────────────────────┐                    │
│  │           Peer Selection                 │                    │
│  │  1. Mesh query                          │                    │
│  │  2. Recent cache lookup                 │                    │
│  │  3. Health filtering                    │                    │
│  │  4. RTT-based sorting                   │◄── NEW             │
│  └─────────────────────────────────────────┘                    │
│                       │                                          │
│                       ▼                                          │
│  ┌─────────────────────────────────────────┐                    │
│  │        libp2p open_stream               │                    │
│  │  - TCP connect (~50-100ms)              │                    │
│  │  - TLS handshake (~20-50ms)             │                    │
│  │  - Muxer negotiation (~10-20ms)         │                    │
│  │  - Substream open (~10-20ms)            │                    │
│  └─────────────────────────────────────────┘                    │
│                       │                                          │
│                       ▼                                          │
│  ┌─────────────────────────────────────────┐                    │
│  │           Sync Protocol                  │                    │
│  │  - Hash Comparison                      │                    │
│  │  - Bloom Filter                         │                    │
│  │  - Subtree Prefetch                     │                    │
│  │  - Level-Wise                           │                    │
│  │  - Snapshot                             │                    │
│  └─────────────────────────────────────────┘                    │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

*Last updated: January 31, 2026*
