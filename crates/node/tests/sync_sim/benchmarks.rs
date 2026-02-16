//! Sync protocol benchmarks using the simulation framework.
//!
//! These benchmarks compare protocol efficiency across different scenarios
//! using deterministic simulation for reproducible results.
//!
//! # Metrics Collected
//!
//! - **Round trips**: Request-response exchanges (latency-sensitive)
//! - **Entities transferred**: Number of entities sent over the network
//! - **Bytes transferred**: Total payload bytes
//! - **Merges**: CRDT merge operations performed
//! - **Time to converge**: Simulated time until state convergence
//!
//! # Integration with SyncMetricsCollector
//!
//! Benchmarks use [`SimMetricsCollector`] through the [`SyncMetricsCollector`] trait
//! to validate that the metrics implementation works correctly with real simulation data.
//!
//! # Running Benchmarks
//!
//! ```bash
//! cargo test --package calimero-node --test sync_tests benchmark -- --nocapture
//! ```

use std::fmt;
use std::sync::Arc;

use calimero_node::sync::metrics::SyncMetricsCollector;

use super::metrics::SimMetrics;
use super::metrics_adapter::SimMetricsCollector;
use super::node::SimNode;
use super::scenarios::Scenario;
use super::sim_runtime::SimRuntime;

/// Benchmark result for a sync scenario.
#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    /// Scenario name.
    pub scenario: &'static str,
    /// Protocol used (from handshake selection).
    pub protocol: String,
    /// Number of request-response round trips.
    pub round_trips: u64,
    /// Number of entities transferred.
    pub entities_transferred: u64,
    /// Total bytes transferred.
    pub bytes_transferred: u64,
    /// CRDT merge operations performed.
    pub merges: u64,
    /// Simulated time to converge (milliseconds).
    pub time_to_converge_ms: u64,
    /// Whether convergence was achieved.
    pub converged: bool,
}

impl fmt::Display for BenchmarkResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let status = if self.converged { "OK" } else { "FAIL" };
        write!(
            f,
            "{:<25} {:<20} RT:{:>4}  Ent:{:>6}  Bytes:{:>8}  Merges:{:>6}  Time:{:>6}ms  [{}]",
            self.scenario,
            self.protocol,
            self.round_trips,
            self.entities_transferred,
            self.bytes_transferred,
            self.merges,
            self.time_to_converge_ms,
            status,
        )
    }
}

impl BenchmarkResult {
    /// Create a benchmark result from simulation metrics.
    pub fn from_metrics(
        scenario: &'static str,
        protocol: String,
        metrics: &SimMetrics,
        converged: bool,
    ) -> Self {
        Self {
            scenario,
            protocol,
            round_trips: metrics.protocol.round_trips,
            entities_transferred: metrics.protocol.entities_transferred,
            bytes_transferred: metrics.protocol.payload_bytes,
            merges: metrics.protocol.merges_performed,
            time_to_converge_ms: metrics
                .convergence
                .time_to_converge
                .map(|t| t.as_millis())
                .unwrap_or(0),
            converged,
        }
    }
}

/// Run a benchmark for a two-node scenario.
///
/// Returns the benchmark result after running the simulation to convergence.
///
/// This function uses [`SimMetricsCollector`] through the [`SyncMetricsCollector`]
/// trait interface to validate that the metrics implementation works correctly
/// with real simulation data.
pub fn run_two_node_benchmark(
    scenario_name: &'static str,
    mut node_a: SimNode,
    mut node_b: SimNode,
) -> BenchmarkResult {
    use calimero_node_primitives::sync::select_protocol;

    // Determine expected protocol via handshake simulation
    let handshake_a = node_a.build_handshake();
    let handshake_b = node_b.build_handshake();
    let selection = select_protocol(&handshake_a, &handshake_b);
    let protocol_name = format!("{:?}", selection.protocol.kind());

    // Create metrics collector using the SyncMetricsCollector trait
    let collector = Arc::new(SimMetricsCollector::new());

    // Create runtime and add nodes
    let mut rt = SimRuntime::new(42);
    rt.add_existing_node(node_a);
    rt.add_existing_node(node_b);

    // Run until convergence
    let converged = rt.run_until_converged();

    // Record metrics through the SyncMetricsCollector trait interface
    // This validates our trait implementation works with real simulation data
    record_simulation_metrics(&*collector, rt.metrics(), &protocol_name);

    // Get final metrics through the collector (validates snapshot() works)
    let metrics = collector.snapshot();

    BenchmarkResult::from_metrics(scenario_name, protocol_name, &metrics, converged)
}

/// Record simulation metrics through the SyncMetricsCollector trait.
///
/// This function exercises the trait interface to validate the implementation
/// works correctly with real simulation data.
fn record_simulation_metrics(
    collector: &dyn SyncMetricsCollector,
    sim_metrics: &SimMetrics,
    protocol: &str,
) {
    // Record protocol cost metrics through the trait
    for _ in 0..sim_metrics.protocol.messages_sent {
        // We don't have per-message byte counts, so use average
        let avg_bytes = if sim_metrics.protocol.messages_sent > 0 {
            (sim_metrics.protocol.payload_bytes / sim_metrics.protocol.messages_sent) as usize
        } else {
            0
        };
        collector.record_message_sent(protocol, avg_bytes);
    }

    for _ in 0..sim_metrics.protocol.round_trips {
        collector.record_round_trip(protocol);
    }

    collector.record_entities_transferred(sim_metrics.protocol.entities_transferred as usize);

    for _ in 0..sim_metrics.protocol.merges_performed {
        collector.record_merge("unknown"); // Sim doesn't track CRDT types
    }

    for _ in 0..sim_metrics.protocol.entities_compared {
        collector.record_comparison();
    }

    // Record safety metrics
    for _ in 0..sim_metrics.effects.buffer_drops {
        collector.record_buffer_drop();
    }

    // Record sync lifecycle (if converged)
    if sim_metrics.convergence.converged {
        let duration = sim_metrics
            .convergence
            .time_to_converge
            .map(|t| std::time::Duration::from_micros(t.as_micros()))
            .unwrap_or_default();
        collector.record_sync_complete(
            "benchmark",
            duration,
            sim_metrics.protocol.entities_transferred as usize,
        );
    }
}

/// Benchmark summary statistics.
#[derive(Debug, Default)]
pub struct BenchmarkSummary {
    /// Total benchmarks run.
    pub total: usize,
    /// Benchmarks that converged.
    pub converged: usize,
    /// Benchmark with lowest round trips.
    pub lowest_round_trips: Option<BenchmarkResult>,
    /// Benchmark with highest bandwidth usage.
    pub highest_bandwidth: Option<BenchmarkResult>,
    /// Benchmark with fastest convergence.
    pub fastest_convergence: Option<BenchmarkResult>,
}

impl BenchmarkSummary {
    /// Add a result to the summary.
    pub fn add(&mut self, result: BenchmarkResult) {
        self.total += 1;
        if result.converged {
            self.converged += 1;
        }

        // Update lowest round trips
        if result.converged {
            match &self.lowest_round_trips {
                None => self.lowest_round_trips = Some(result.clone()),
                Some(best) if result.round_trips < best.round_trips => {
                    self.lowest_round_trips = Some(result.clone());
                }
                _ => {}
            }
        }

        // Update highest bandwidth
        match &self.highest_bandwidth {
            None => self.highest_bandwidth = Some(result.clone()),
            Some(best) if result.bytes_transferred > best.bytes_transferred => {
                self.highest_bandwidth = Some(result.clone());
            }
            _ => {}
        }

        // Update fastest convergence
        if result.converged && result.time_to_converge_ms > 0 {
            match &self.fastest_convergence {
                None => self.fastest_convergence = Some(result.clone()),
                Some(best) if result.time_to_converge_ms < best.time_to_converge_ms => {
                    self.fastest_convergence = Some(result);
                }
                _ => {}
            }
        }
    }
}

impl fmt::Display for BenchmarkSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "=== Benchmark Summary ===")?;
        writeln!(f, "Total: {} / Converged: {}", self.total, self.converged)?;

        if let Some(ref best) = self.lowest_round_trips {
            writeln!(
                f,
                "Most efficient (lowest RT): {} ({} round trips)",
                best.scenario, best.round_trips
            )?;
        }
        if let Some(ref best) = self.highest_bandwidth {
            writeln!(
                f,
                "Highest bandwidth: {} ({} bytes)",
                best.scenario, best.bytes_transferred
            )?;
        }
        if let Some(ref best) = self.fastest_convergence {
            writeln!(
                f,
                "Fastest convergence: {} ({}ms)",
                best.scenario, best.time_to_converge_ms
            )?;
        }

        Ok(())
    }
}

/// Run all standard benchmarks and return results.
pub fn run_all_benchmarks() -> (Vec<BenchmarkResult>, BenchmarkSummary) {
    let mut results = Vec::new();
    let mut summary = BenchmarkSummary::default();

    // Define benchmark scenarios
    let scenarios: Vec<(&'static str, (SimNode, SimNode))> = vec![
        ("same_state", Scenario::force_none()),
        ("fresh_bootstrap", Scenario::force_snapshot()),
        ("high_divergence", Scenario::force_hash_high_divergence()),
        ("partial_overlap", Scenario::partial_overlap()),
        ("deep_tree_localized", Scenario::force_subtree_prefetch()),
        ("wide_shallow", Scenario::force_levelwise()),
        ("delta_sync", Scenario::force_delta_sync()),
        ("bloom_filter", Scenario::force_bloom_filter()),
    ];

    for (name, (node_a, node_b)) in scenarios {
        let result = run_two_node_benchmark(name, node_a, node_b);
        summary.add(result.clone());
        results.push(result);
    }

    (results, summary)
}

/// Run scaling benchmarks with increasing entity counts.
pub fn run_scaling_benchmarks(entity_counts: &[usize]) -> (Vec<BenchmarkResult>, BenchmarkSummary) {
    use super::scenarios::deterministic::generate_entities;

    let mut results = Vec::new();
    let mut summary = BenchmarkSummary::default();

    for &count in entity_counts {
        // Create two nodes with diverged state
        let mut node_a = SimNode::new("a");
        let mut node_b = SimNode::new("b");

        // A has half the entities
        for (id, data, metadata) in generate_entities(count / 2, 1) {
            node_a.insert_entity_with_metadata(id, data, metadata);
        }

        // B has all entities
        for (id, data, metadata) in generate_entities(count, 2) {
            node_b.insert_entity_with_metadata(id, data, metadata);
        }

        let scenario_name: &'static str = Box::leak(format!("diverged_{count}").into_boxed_str());
        let result = run_two_node_benchmark(scenario_name, node_a, node_b);
        summary.add(result.clone());
        results.push(result);
    }

    (results, summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run all standard protocol benchmarks.
    ///
    /// Execute with: `cargo test benchmark_all_scenarios -- --nocapture`
    #[test]
    fn benchmark_all_scenarios() {
        println!("\n=== Sync Protocol Benchmarks ===\n");

        let (results, summary) = run_all_benchmarks();

        for result in &results {
            println!("{result}");
        }

        println!();
        println!("{summary}");

        // Verify at least some benchmarks converged
        assert!(
            summary.converged > 0,
            "Expected at least some benchmarks to converge"
        );
    }

    /// Run scaling benchmarks to test performance with increasing data.
    ///
    /// Execute with: `cargo test benchmark_scaling -- --nocapture`
    #[test]
    fn benchmark_scaling() {
        println!("\n=== Scaling Benchmark ===\n");

        let entity_counts = vec![10, 50, 100, 200, 500];
        let (results, summary) = run_scaling_benchmarks(&entity_counts);

        for result in &results {
            println!("{result}");
        }

        println!();
        println!("{summary}");
    }

    /// Test that same_state scenario uses None protocol (no sync needed).
    #[test]
    fn test_same_state_uses_none_protocol() {
        let (node_a, node_b) = Scenario::force_none();
        let result = run_two_node_benchmark("same_state", node_a, node_b);

        // Same state should result in None protocol (no sync needed)
        assert!(
            result.protocol.contains("None"),
            "Expected None protocol for same_state, got {}",
            result.protocol
        );
        assert!(result.converged);
    }

    /// Test that fresh bootstrap uses Snapshot protocol.
    #[test]
    fn test_fresh_bootstrap_uses_snapshot() {
        let (fresh, source) = Scenario::force_snapshot();
        let result = run_two_node_benchmark("fresh_bootstrap", fresh, source);

        // Fresh node syncing from initialized should use Snapshot
        assert!(
            result.protocol.contains("Snapshot"),
            "Expected Snapshot protocol for fresh_bootstrap, got {}",
            result.protocol
        );
    }

    /// Test that high divergence uses HashComparison protocol.
    #[test]
    fn test_high_divergence_uses_hash_comparison() {
        let (node_a, node_b) = Scenario::force_hash_high_divergence();
        let result = run_two_node_benchmark("high_divergence", node_a, node_b);

        // High divergence should use HashComparison
        assert!(
            result.protocol.contains("HashComparison"),
            "Expected HashComparison protocol for high_divergence, got {}",
            result.protocol
        );
    }

    /// Test benchmark result formatting.
    #[test]
    fn test_benchmark_result_display() {
        let result = BenchmarkResult {
            scenario: "test_scenario",
            protocol: "HashComparison".to_string(),
            round_trips: 5,
            entities_transferred: 100,
            bytes_transferred: 10240,
            merges: 50,
            time_to_converge_ms: 150,
            converged: true,
        };

        let display = result.to_string();
        assert!(display.contains("test_scenario"));
        assert!(display.contains("HashComparison"));
        assert!(display.contains("OK"));
    }

    /// Test that SyncMetricsCollector trait integration works with real simulation data.
    ///
    /// This validates that:
    /// 1. SimMetricsCollector correctly implements SyncMetricsCollector
    /// 2. Metrics recorded via the trait match expected values
    /// 3. The snapshot() method returns correct aggregated data
    #[test]
    fn test_sync_metrics_collector_integration() {
        use std::sync::Arc;

        // Run a scenario that produces measurable metrics
        let (mut node_a, mut node_b) = Scenario::force_hash_high_divergence();

        // Get protocol info
        use calimero_node_primitives::sync::select_protocol;
        let handshake_a = node_a.build_handshake();
        let handshake_b = node_b.build_handshake();
        let selection = select_protocol(&handshake_a, &handshake_b);
        let protocol_name = format!("{:?}", selection.protocol.kind());

        // Create collector and run simulation
        let collector = Arc::new(SimMetricsCollector::new());
        let mut rt = SimRuntime::new(42);
        rt.add_existing_node(node_a);
        rt.add_existing_node(node_b);

        let converged = rt.run_until_converged();
        let sim_metrics = rt.metrics().clone();

        // Record through trait interface
        record_simulation_metrics(&*collector, &sim_metrics, &protocol_name);

        // Validate metrics were recorded correctly through the trait
        let collected = collector.snapshot();

        // Verify protocol metrics match
        assert_eq!(
            collected.protocol.messages_sent, sim_metrics.protocol.messages_sent,
            "Message count mismatch"
        );
        assert_eq!(
            collected.protocol.round_trips, sim_metrics.protocol.round_trips,
            "Round trip count mismatch"
        );
        assert_eq!(
            collected.protocol.entities_transferred, sim_metrics.protocol.entities_transferred,
            "Entities transferred mismatch"
        );
        assert_eq!(
            collected.protocol.merges_performed, sim_metrics.protocol.merges_performed,
            "Merges performed mismatch"
        );
        assert_eq!(
            collected.protocol.entities_compared, sim_metrics.protocol.entities_compared,
            "Entities compared mismatch"
        );

        // Verify effect metrics
        assert_eq!(
            collected.effects.buffer_drops, sim_metrics.effects.buffer_drops,
            "Buffer drops mismatch"
        );

        println!("SyncMetricsCollector integration test passed!");
        println!("  Protocol: {protocol_name}");
        println!("  Converged: {converged}");
        println!("  Messages: {}", collected.protocol.messages_sent);
        println!("  Round trips: {}", collected.protocol.round_trips);
        println!("  Entities: {}", collected.protocol.entities_transferred);
        println!("  Merges: {}", collected.protocol.merges_performed);
    }

    /// Test that metrics collector works with the trait as a type-erased reference.
    ///
    /// This validates that the trait object works correctly when passed around
    /// as &dyn SyncMetricsCollector, which is how production code will use it.
    #[test]
    fn test_trait_object_usage() {
        // Create concrete collector
        let collector = SimMetricsCollector::new();

        // Function that accepts trait object reference
        fn record_via_trait(metrics: &dyn SyncMetricsCollector) {
            metrics.record_message_sent("TestProtocol", 1024);
            metrics.record_message_sent("TestProtocol", 2048);
            metrics.record_round_trip("TestProtocol");
            metrics.record_entities_transferred(5);
            metrics.record_merge("GCounter");
            metrics.record_comparison();
            metrics.record_buffer_drop();

            // Test phase timing through trait
            let timer = metrics.start_phase("test_phase");
            std::thread::sleep(std::time::Duration::from_millis(1));
            metrics.record_phase_complete(timer);

            // Test lifecycle methods
            metrics.record_sync_start("ctx-123", "TestProtocol", "manual");
            metrics.record_sync_complete("ctx-123", std::time::Duration::from_millis(100), 5);
            metrics.record_protocol_selected("TestProtocol", "test", 0.5);
        }

        // Use through trait interface
        record_via_trait(&collector);

        // Verify metrics were recorded
        let metrics = collector.snapshot();

        assert_eq!(metrics.protocol.messages_sent, 2);
        assert_eq!(metrics.protocol.payload_bytes, 3072);
        assert_eq!(metrics.protocol.round_trips, 1);
        assert_eq!(metrics.protocol.entities_transferred, 5);
        assert_eq!(metrics.protocol.merges_performed, 1);
        assert_eq!(metrics.protocol.entities_compared, 1);
        assert_eq!(metrics.effects.buffer_drops, 1);

        println!("Trait object usage test passed!");
        println!("  All metrics recorded correctly through &dyn SyncMetricsCollector");
    }
}
