# Production Monitoring for Sync Performance

**Status**: Recommended alerts and dashboards for sync operations.

---

## Key Metrics

### Dial Phase Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `sync_dial_duration_seconds` | Histogram | Time to establish connection |
| `sync_dial_total` | Counter | Total dial attempts |
| `sync_dial_success_total` | Counter | Successful dials |
| `sync_dial_reused_total` | Counter | Dials that reused existing connection |

### Peer Finding Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `sync_peer_find_duration_seconds` | Histogram | Time to find viable peer |
| `sync_peer_candidates_total` | Gauge | Candidates found per attempt |

### Sync Operation Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `sync_duration_seconds` | Histogram | Total sync operation time |
| `sync_attempts_total` | Counter | Total sync attempts |
| `sync_successes_total` | Counter | Successful syncs |
| `sync_failures_total` | Counter | Failed syncs |
| `sync_active` | Gauge | Currently active sync operations |

---

## Critical Alerts

### Alert 1: Dial Latency Spike (P0)

```yaml
alert: SyncDialLatencyHigh
expr: histogram_quantile(0.99, rate(sync_dial_duration_seconds_bucket[5m])) > 2
for: 5m
labels:
  severity: critical
annotations:
  summary: "Sync dial P99 latency > 2 seconds"
  description: "P99 dial latency is {{ $value }}s. Check network connectivity and libp2p health."
  runbook: "Check peer connectivity, network partitions, and libp2p logs for connection errors."
```

### Alert 2: Low Connection Reuse (P1)

```yaml
alert: SyncConnectionReuseLow
expr: |
  rate(sync_dial_reused_total[5m]) / rate(sync_dial_total[5m]) < 0.3
  AND rate(sync_dial_total[5m]) > 0.1
for: 10m
labels:
  severity: warning
annotations:
  summary: "Connection reuse rate below 30%"
  description: "Only {{ $value | humanizePercentage }} of dials reuse existing connections. May indicate connection churn."
  runbook: "Check for network instability, peer disconnections, or excessive node restarts."
```

### Alert 3: Sync Failure Rate (P0)

```yaml
alert: SyncFailureRateHigh
expr: |
  rate(sync_failures_total[5m]) / rate(sync_attempts_total[5m]) > 0.1
  AND rate(sync_attempts_total[5m]) > 0.05
for: 5m
labels:
  severity: critical
annotations:
  summary: "Sync failure rate > 10%"
  description: "{{ $value | humanizePercentage }} of sync attempts are failing."
  runbook: "Check node logs for sync errors, verify peer health, check for network partitions."
```

### Alert 4: Churn Recovery Failure (P0)

```yaml
alert: SyncChurnRecoveryFailed
expr: |
  increase(sync_failures_total{reason="mesh_timeout"}[5m]) > 3
for: 2m
labels:
  severity: critical
annotations:
  summary: "Multiple mesh formation timeouts detected"
  description: "Node may be failing to recover from restart. Check gossipsub mesh health."
  runbook: "Verify gossipsub subscriptions, check for backoff penalties, consider manual peer injection."
```

### Alert 5: No Peers Available (P0)

```yaml
alert: SyncNoPeers
expr: sync_peer_candidates_total == 0
for: 1m
labels:
  severity: critical
annotations:
  summary: "No sync peer candidates available"
  description: "Node cannot find any peers to sync with. Likely network isolation."
  runbook: "Check network connectivity, bootstrap nodes, and gossipsub subscriptions."
```

### Alert 6: Peer Selection Dominates Latency (P1)

```yaml
alert: SyncPeerSelectionSlow
expr: |
  histogram_quantile(0.95, rate(sync_phase_peer_selection_seconds_bucket[5m])) 
  / histogram_quantile(0.95, rate(sync_duration_seconds_bucket[5m])) > 0.9
for: 15m
labels:
  severity: warning
annotations:
  summary: "Peer selection is >90% of sync time"
  description: "Dial latency is dominating sync performance. Consider connection pooling."
```

---

## Grafana Dashboard Queries

### Panel 1: Dial Latency Distribution

```promql
# P50, P90, P99 dial latency
histogram_quantile(0.50, rate(sync_dial_duration_seconds_bucket[5m]))
histogram_quantile(0.90, rate(sync_dial_duration_seconds_bucket[5m]))
histogram_quantile(0.99, rate(sync_dial_duration_seconds_bucket[5m]))
```

### Panel 2: Connection Reuse Rate

```promql
# Reuse rate over time
rate(sync_dial_reused_total[5m]) / rate(sync_dial_total[5m]) * 100
```

### Panel 3: Sync Success Rate

```promql
# Success rate percentage
rate(sync_successes_total[5m]) / rate(sync_attempts_total[5m]) * 100
```

### Panel 4: Peer Finding Latency

```promql
# P50, P95 peer finding time
histogram_quantile(0.50, rate(sync_peer_find_duration_seconds_bucket[5m]))
histogram_quantile(0.95, rate(sync_peer_find_duration_seconds_bucket[5m]))
```

### Panel 5: Active Syncs

```promql
# Currently active sync operations
sync_active
```

### Panel 6: Sync Phase Breakdown

```promql
# Average time per phase
rate(sync_phase_peer_selection_seconds_sum[5m]) / rate(sync_phase_peer_selection_seconds_count[5m])
rate(sync_phase_data_transfer_seconds_sum[5m]) / rate(sync_phase_data_transfer_seconds_count[5m])
rate(sync_phase_merge_seconds_sum[5m]) / rate(sync_phase_merge_seconds_count[5m])
```

---

## SLO Recommendations

| SLO | Target | Rationale |
|-----|--------|-----------|
| Sync success rate | ≥ 99% | Critical for data consistency |
| Dial P99 latency | < 2s | User-perceivable delay threshold |
| Connection reuse rate | ≥ 50% | Efficiency indicator |
| Churn recovery time | < 30s | Max acceptable catch-up time |
| Peer finding P95 | < 10ms | Already achieved (<0.12ms) |

---

## Log-Based Alerts (for log aggregation systems)

### Loki/Promtail Query: Dial Failures

```logql
{app="merod"} |= "PEER_DIAL_BREAKDOWN" |= "result=error"
| rate([5m]) > 0.1
```

### Loki/Promtail Query: Churn Detection

```logql
{app="merod"} |= "Gossipsub mesh failed to form"
| count_over_time([5m]) > 3
```

### Loki/Promtail Query: Slow Dials

```logql
{app="merod"} |= "PEER_DIAL_BREAKDOWN" 
| regexp `total_dial_ms=(?P<dial_ms>\d+\.\d+)`
| dial_ms > 1000
```

---

## Recommended Dashboard Layout

```
┌─────────────────────────────────────────────────────────────────┐
│                     SYNC PERFORMANCE                             │
├─────────────────────┬─────────────────────┬─────────────────────┤
│   Success Rate      │   Active Syncs      │   Failure Rate      │
│      99.2%          │        3            │      0.8%           │
├─────────────────────┴─────────────────────┴─────────────────────┤
│                   DIAL LATENCY (P50/P90/P99)                    │
│   [=========================================]  152ms / 380ms / 1.2s │
├─────────────────────────────────────────────────────────────────┤
│                   CONNECTION REUSE RATE                          │
│   [=====================================]  62%                   │
├─────────────────────┬─────────────────────┬─────────────────────┤
│   Peer Find P95     │   Candidates Avg    │   Mesh Peers        │
│     0.08ms          │       4.2           │        6            │
├─────────────────────┴─────────────────────┴─────────────────────┤
│                   SYNC PHASE BREAKDOWN                           │
│   peer_selection ████████████████████████████████████  94%      │
│   data_transfer  ██                                     4%      │
│   merge          █                                      2%      │
└─────────────────────────────────────────────────────────────────┘
```

---

*Last updated: January 31, 2026*
