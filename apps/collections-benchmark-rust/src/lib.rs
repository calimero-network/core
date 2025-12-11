//! Collections Benchmark App (Rust SDK)
//!
//! Benchmarks CRDT collection performance for:
//! - Different collection types (LwwRegister, ReplicatedGrowableArray, UnorderedMap, UnorderedSet, Vector)
//! - Different sizes (small: 10-100, medium: 1000-10000, large: 100000+)
//! - Different nesting levels (1, 2, 3)
//!
//! Measures: insert, get, merge, serialization operations

#![allow(
    unused_crate_dependencies,
    reason = "Dependencies used in build process"
)]

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::{
    LwwRegister, ReplicatedGrowableArray, UnorderedMap, UnorderedSet, Vector,
};

/// Type alias for Counter with default const generic (no decrement)
pub type Counter = calimero_storage::collections::Counter<false>;
/// Type alias for PNCounter (supports both increment and decrement)
pub type PNCounter = calimero_storage::collections::Counter<true>;
use serde::{Deserialize, Serialize};

/// Benchmark result structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResult {
    pub collection_type: String,
    pub operation: String,
    pub size: u32,
    pub size_category: String,
    pub nesting_level: u32,
    pub time_ms: f64,
    pub time_us: u64,
    pub throughput_ops_per_sec: u64,
    pub iterations: u32,
    pub platform: String,
}

/// Application state for benchmarking
#[app::state(emits = BenchmarkEvent)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct CollectionsBenchmark {
    /// Counter for tracking benchmark runs
    pub run_count: Counter,

    /// Store benchmark results
    pub results: UnorderedMap<String, LwwRegister<String>>,

    // Collections for benchmarking
    pub test_map: UnorderedMap<String, Counter>,
    pub test_nested_map: UnorderedMap<String, UnorderedMap<String, Counter>>,
    pub test_deep_nested_map:
        UnorderedMap<String, UnorderedMap<String, UnorderedMap<String, Counter>>>,
    pub test_vector: Vector<Counter>,
    pub test_set: UnorderedSet<String>,
    pub test_registers: UnorderedMap<String, LwwRegister<String>>,
}

#[app::event]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub enum BenchmarkEvent {
    BenchmarkCompleted {
        collection_type: String,
        operation: String,
        size: u32,
        time_ms: f64,
    },
}

fn get_size_category(size: u32) -> String {
    if size <= 100 {
        "small".to_string()
    } else if size <= 10000 {
        "medium".to_string()
    } else {
        "large".to_string()
    }
}

fn calculate_throughput(size: u32, time_us: u64) -> u64 {
    if time_us == 0 {
        return 0;
    }
    ((size as f64) / (time_us as f64 / 1_000_000.0)) as u64
}

#[app::logic]
impl CollectionsBenchmark {
    #[app::init]
    pub fn init() -> CollectionsBenchmark {
        CollectionsBenchmark {
            run_count: Counter::new(),
            results: UnorderedMap::new(),
            test_map: UnorderedMap::new(),
            test_nested_map: UnorderedMap::new(),
            test_deep_nested_map: UnorderedMap::new(),
            test_vector: Vector::new(),
            test_set: UnorderedSet::new(),
            test_registers: UnorderedMap::new(),
        }
    }

    // COUNTER BENCHMARKS

    /// Benchmark Counter increment operations
    pub fn benchmark_counter_increment(&mut self, size: u32) -> Result<String, String> {
        let start_ns = calimero_sdk::env::time_now();

        let mut counter = Counter::new();
        for _ in 0..size {
            counter
                .increment()
                .map_err(|e| format!("Increment failed: {:?}", e))?;
        }

        let end_ns = calimero_sdk::env::time_now();
        let duration_ns = end_ns.saturating_sub(start_ns);
        let time_us = duration_ns / 1_000;
        let time_ms = (duration_ns as f64) / 1_000_000.0;

        let result = BenchmarkResult {
            collection_type: "Counter".to_string(),
            operation: "increment".to_string(),
            size,
            size_category: get_size_category(size),
            nesting_level: 1,
            time_ms,
            time_us,
            throughput_ops_per_sec: calculate_throughput(size, time_us),
            iterations: 1,
            platform: "rust".to_string(),
        };

        app::emit!(BenchmarkEvent::BenchmarkCompleted {
            collection_type: result.collection_type.clone(),
            operation: result.operation.clone(),
            size,
            time_ms: result.time_ms,
        });

        app::log!(
            "BENCHMARK: {} {} (size={}, time={:.2}ms, throughput={} ops/sec)",
            result.collection_type,
            result.operation,
            result.size,
            result.time_ms,
            result.throughput_ops_per_sec
        );

        Ok(format!(
            "{} {} completed: {:.2}ms, {} ops/sec",
            result.collection_type, result.operation, result.time_ms, result.throughput_ops_per_sec
        ))
    }

    // UNORDERED MAP BENCHMARKS (Level 1 - Simple)

    /// Benchmark UnorderedMap insert operations (nesting level 1)
    pub fn benchmark_map_insert(&mut self, size: u32) -> Result<String, String> {
        let start_ns = calimero_sdk::env::time_now();

        let mut map: UnorderedMap<String, Counter> = UnorderedMap::new();
        for i in 0..size {
            let key = format!("key_{}", i);
            let value = Counter::new();
            drop(
                map.insert(key, value)
                    .map_err(|e| format!("Insert failed: {:?}", e))?,
            );
        }

        let end_ns = calimero_sdk::env::time_now();
        let duration_ns = end_ns.saturating_sub(start_ns);
        let time_us = duration_ns / 1_000;
        let time_ms = (duration_ns as f64) / 1_000_000.0;

        let result = BenchmarkResult {
            collection_type: "UnorderedMap".to_string(),
            operation: "insert".to_string(),
            size,
            size_category: get_size_category(size),
            nesting_level: 1,
            time_ms,
            time_us,
            throughput_ops_per_sec: calculate_throughput(size, time_us),
            iterations: 1,
            platform: "rust".to_string(),
        };

        app::emit!(BenchmarkEvent::BenchmarkCompleted {
            collection_type: result.collection_type.clone(),
            operation: result.operation.clone(),
            size,
            time_ms: result.time_ms,
        });

        app::log!(
            "BENCHMARK: {} {} (size={}, time={:.2}ms, throughput={} ops/sec)",
            result.collection_type,
            result.operation,
            result.size,
            result.time_ms,
            result.throughput_ops_per_sec
        );

        Ok(format!(
            "{} {} completed: {:.2}ms, {} ops/sec",
            result.collection_type, result.operation, result.time_ms, result.throughput_ops_per_sec
        ))
    }

    /// Benchmark UnorderedMap get operations (nesting level 1)
    pub fn benchmark_map_get(&mut self, size: u32) -> Result<String, String> {
        // First, populate the map
        let mut map: UnorderedMap<String, Counter> = UnorderedMap::new();
        for i in 0..size {
            let key = format!("key_{}", i);
            let value = Counter::new();
            drop(
                map.insert(key, value)
                    .map_err(|e| format!("Insert failed: {:?}", e))?,
            );
        }

        // Now benchmark get operations
        let start_ns = calimero_sdk::env::time_now();

        for i in 0..size {
            let key = format!("key_{}", i);
            let _ = map.get(&key).map_err(|e| format!("Get failed: {:?}", e))?;
        }

        let end_ns = calimero_sdk::env::time_now();
        let duration_ns = end_ns.saturating_sub(start_ns);
        let time_us = duration_ns / 1_000;
        let time_ms = (duration_ns as f64) / 1_000_000.0;

        let result = BenchmarkResult {
            collection_type: "UnorderedMap".to_string(),
            operation: "get".to_string(),
            size,
            size_category: get_size_category(size),
            nesting_level: 1,
            time_ms,
            time_us,
            throughput_ops_per_sec: calculate_throughput(size, time_us),
            iterations: 1,
            platform: "rust".to_string(),
        };

        app::emit!(BenchmarkEvent::BenchmarkCompleted {
            collection_type: result.collection_type.clone(),
            operation: result.operation.clone(),
            size,
            time_ms: result.time_ms,
        });

        app::log!(
            "BENCHMARK: {} {} (size={}, time={:.2}ms, throughput={} ops/sec)",
            result.collection_type,
            result.operation,
            result.size,
            result.time_ms,
            result.throughput_ops_per_sec
        );

        Ok(format!(
            "{} {} completed: {:.2}ms, {} ops/sec",
            result.collection_type, result.operation, result.time_ms, result.throughput_ops_per_sec
        ))
    }

    // NESTED MAP BENCHMARKS (Level 2)

    /// Benchmark nested UnorderedMap insert operations (nesting level 2)
    pub fn benchmark_nested_map_insert(&mut self, size: u32) -> Result<String, String> {
        let start_ns = calimero_sdk::env::time_now();

        let mut outer_map: UnorderedMap<String, UnorderedMap<String, Counter>> =
            UnorderedMap::new();
        let sqrt_size = (size as f64).sqrt() as u32;

        for i in 0..sqrt_size {
            let outer_key = format!("outer_{}", i);
            let mut inner_map: UnorderedMap<String, Counter> = UnorderedMap::new();

            for j in 0..sqrt_size {
                let inner_key = format!("inner_{}", j);
                let value = Counter::new();
                drop(
                    inner_map
                        .insert(inner_key, value)
                        .map_err(|e| format!("Inner insert failed: {:?}", e))?,
                );
            }

            drop(
                outer_map
                    .insert(outer_key, inner_map)
                    .map_err(|e| format!("Outer insert failed: {:?}", e))?,
            );
        }

        let end_ns = calimero_sdk::env::time_now();
        let duration_ns = end_ns.saturating_sub(start_ns);
        let time_us = duration_ns / 1_000;
        let time_ms = (duration_ns as f64) / 1_000_000.0;
        let actual_size = sqrt_size * sqrt_size;

        let result = BenchmarkResult {
            collection_type: "UnorderedMap<UnorderedMap>".to_string(),
            operation: "insert".to_string(),
            size: actual_size,
            size_category: get_size_category(actual_size),
            nesting_level: 2,
            time_ms,
            time_us,
            throughput_ops_per_sec: calculate_throughput(actual_size, time_us),
            iterations: 1,
            platform: "rust".to_string(),
        };

        app::emit!(BenchmarkEvent::BenchmarkCompleted {
            collection_type: result.collection_type.clone(),
            operation: result.operation.clone(),
            size: actual_size,
            time_ms: result.time_ms,
        });

        serde_json::to_string(&result).map_err(|e| e.to_string())
    }

    /// Benchmark nested UnorderedMap get operations (nesting level 2)
    pub fn benchmark_nested_map_get(&mut self, size: u32) -> Result<String, String> {
        // First, populate the nested map
        let mut outer_map: UnorderedMap<String, UnorderedMap<String, Counter>> =
            UnorderedMap::new();
        let sqrt_size = (size as f64).sqrt() as u32;

        for i in 0..sqrt_size {
            let outer_key = format!("outer_{}", i);
            let mut inner_map: UnorderedMap<String, Counter> = UnorderedMap::new();

            for j in 0..sqrt_size {
                let inner_key = format!("inner_{}", j);
                let value = Counter::new();
                drop(
                    inner_map
                        .insert(inner_key, value)
                        .map_err(|e| format!("Inner insert failed: {:?}", e))?,
                );
            }

            drop(
                outer_map
                    .insert(outer_key, inner_map)
                    .map_err(|e| format!("Outer insert failed: {:?}", e))?,
            );
        }

        // Now benchmark get operations
        let start_ns = calimero_sdk::env::time_now();

        for i in 0..sqrt_size {
            let outer_key = format!("outer_{}", i);
            if let Some(inner_map) = outer_map
                .get(&outer_key)
                .map_err(|e| format!("Outer get failed: {:?}", e))?
            {
                for j in 0..sqrt_size {
                    let inner_key = format!("inner_{}", j);
                    let _ = inner_map
                        .get(&inner_key)
                        .map_err(|e| format!("Inner get failed: {:?}", e))?;
                }
            }
        }

        let end_ns = calimero_sdk::env::time_now();
        let duration_ns = end_ns.saturating_sub(start_ns);
        let time_us = duration_ns / 1_000;
        let time_ms = (duration_ns as f64) / 1_000_000.0;
        let actual_size = sqrt_size * sqrt_size;

        let result = BenchmarkResult {
            collection_type: "UnorderedMap<UnorderedMap>".to_string(),
            operation: "get".to_string(),
            size: actual_size,
            size_category: get_size_category(actual_size),
            nesting_level: 2,
            time_ms,
            time_us,
            throughput_ops_per_sec: calculate_throughput(actual_size, time_us),
            iterations: 1,
            platform: "rust".to_string(),
        };

        app::emit!(BenchmarkEvent::BenchmarkCompleted {
            collection_type: result.collection_type.clone(),
            operation: result.operation.clone(),
            size: actual_size,
            time_ms: result.time_ms,
        });

        serde_json::to_string(&result).map_err(|e| e.to_string())
    }

    // DEEP NESTED MAP BENCHMARKS (Level 3)

    /// Benchmark deep nested UnorderedMap insert operations (nesting level 3)
    pub fn benchmark_deep_nested_map_insert(&mut self, size: u32) -> Result<String, String> {
        let start_ns = calimero_sdk::env::time_now();

        let mut level1: UnorderedMap<String, UnorderedMap<String, UnorderedMap<String, Counter>>> =
            UnorderedMap::new();
        let cbrt_size = (size as f64).cbrt() as u32;

        for i in 0..cbrt_size {
            let key1 = format!("l1_{}", i);
            let mut level2: UnorderedMap<String, UnorderedMap<String, Counter>> =
                UnorderedMap::new();

            for j in 0..cbrt_size {
                let key2 = format!("l2_{}", j);
                let mut level3: UnorderedMap<String, Counter> = UnorderedMap::new();

                for k in 0..cbrt_size {
                    let key3 = format!("l3_{}", k);
                    let value = Counter::new();
                    drop(
                        level3
                            .insert(key3, value)
                            .map_err(|e| format!("L3 insert failed: {:?}", e))?,
                    );
                }

                drop(
                    level2
                        .insert(key2, level3)
                        .map_err(|e| format!("L2 insert failed: {:?}", e))?,
                );
            }

            drop(
                level1
                    .insert(key1, level2)
                    .map_err(|e| format!("L1 insert failed: {:?}", e))?,
            );
        }

        let end_ns = calimero_sdk::env::time_now();
        let duration_ns = end_ns.saturating_sub(start_ns);
        let time_us = duration_ns / 1_000;
        let time_ms = (duration_ns as f64) / 1_000_000.0;
        let actual_size = cbrt_size * cbrt_size * cbrt_size;

        let result = BenchmarkResult {
            collection_type: "UnorderedMap<UnorderedMap<UnorderedMap>>".to_string(),
            operation: "insert".to_string(),
            size: actual_size,
            size_category: get_size_category(actual_size),
            nesting_level: 3,
            time_ms,
            time_us,
            throughput_ops_per_sec: calculate_throughput(actual_size, time_us),
            iterations: 1,
            platform: "rust".to_string(),
        };

        app::emit!(BenchmarkEvent::BenchmarkCompleted {
            collection_type: result.collection_type.clone(),
            operation: result.operation.clone(),
            size: actual_size,
            time_ms: result.time_ms,
        });

        serde_json::to_string(&result).map_err(|e| e.to_string())
    }

    // VECTOR BENCHMARKS

    /// Benchmark Vector push operations
    pub fn benchmark_vector_push(&mut self, size: u32) -> Result<String, String> {
        let start_ns = calimero_sdk::env::time_now();

        let mut vector: Vector<Counter> = Vector::new();
        for _ in 0..size {
            let value = Counter::new();
            vector
                .push(value)
                .map_err(|e| format!("Push failed: {:?}", e))?;
        }

        let end_ns = calimero_sdk::env::time_now();
        let duration_ns = end_ns.saturating_sub(start_ns);
        let time_us = duration_ns / 1_000;
        let time_ms = (duration_ns as f64) / 1_000_000.0;

        let result = BenchmarkResult {
            collection_type: "Vector".to_string(),
            operation: "push".to_string(),
            size,
            size_category: get_size_category(size),
            nesting_level: 1,
            time_ms,
            time_us,
            throughput_ops_per_sec: calculate_throughput(size, time_us),
            iterations: 1,
            platform: "rust".to_string(),
        };

        app::emit!(BenchmarkEvent::BenchmarkCompleted {
            collection_type: result.collection_type.clone(),
            operation: result.operation.clone(),
            size,
            time_ms: result.time_ms,
        });

        app::log!(
            "BENCHMARK: {} {} (size={}, time={:.2}ms, throughput={} ops/sec)",
            result.collection_type,
            result.operation,
            result.size,
            result.time_ms,
            result.throughput_ops_per_sec
        );

        Ok(format!(
            "{} {} completed: {:.2}ms, {} ops/sec",
            result.collection_type, result.operation, result.time_ms, result.throughput_ops_per_sec
        ))
    }

    /// Benchmark Vector get operations
    pub fn benchmark_vector_get(&mut self, size: u32) -> Result<String, String> {
        // First, populate the vector
        let mut vector: Vector<Counter> = Vector::new();
        for _ in 0..size {
            let value = Counter::new();
            vector
                .push(value)
                .map_err(|e| format!("Push failed: {:?}", e))?;
        }

        // Now benchmark get operations
        let start_ns = calimero_sdk::env::time_now();

        for i in 0..size {
            let _ = vector
                .get(i as usize)
                .map_err(|e| format!("Get failed: {:?}", e))?;
        }

        let end_ns = calimero_sdk::env::time_now();
        let duration_ns = end_ns.saturating_sub(start_ns);
        let time_us = duration_ns / 1_000;
        let time_ms = (duration_ns as f64) / 1_000_000.0;

        let result = BenchmarkResult {
            collection_type: "Vector".to_string(),
            operation: "get".to_string(),
            size,
            size_category: get_size_category(size),
            nesting_level: 1,
            time_ms,
            time_us,
            throughput_ops_per_sec: calculate_throughput(size, time_us),
            iterations: 1,
            platform: "rust".to_string(),
        };

        app::emit!(BenchmarkEvent::BenchmarkCompleted {
            collection_type: result.collection_type.clone(),
            operation: result.operation.clone(),
            size,
            time_ms: result.time_ms,
        });

        app::log!(
            "BENCHMARK: {} {} (size={}, time={:.2}ms, throughput={} ops/sec)",
            result.collection_type,
            result.operation,
            result.size,
            result.time_ms,
            result.throughput_ops_per_sec
        );

        Ok(format!(
            "{} {} completed: {:.2}ms, {} ops/sec",
            result.collection_type, result.operation, result.time_ms, result.throughput_ops_per_sec
        ))
    }

    // UNORDERED SET BENCHMARKS

    /// Benchmark UnorderedSet insert operations
    pub fn benchmark_set_insert(&mut self, size: u32) -> Result<String, String> {
        let start_ns = calimero_sdk::env::time_now();

        let mut set: UnorderedSet<String> = UnorderedSet::new();
        for i in 0..size {
            let value = format!("value_{}", i);
            let _ = set
                .insert(value)
                .map_err(|e| format!("Insert failed: {:?}", e))?;
        }

        let end_ns = calimero_sdk::env::time_now();
        let duration_ns = end_ns.saturating_sub(start_ns);
        let time_us = duration_ns / 1_000;
        let time_ms = (duration_ns as f64) / 1_000_000.0;

        let result = BenchmarkResult {
            collection_type: "UnorderedSet".to_string(),
            operation: "insert".to_string(),
            size,
            size_category: get_size_category(size),
            nesting_level: 1,
            time_ms,
            time_us,
            throughput_ops_per_sec: calculate_throughput(size, time_us),
            iterations: 1,
            platform: "rust".to_string(),
        };

        app::emit!(BenchmarkEvent::BenchmarkCompleted {
            collection_type: result.collection_type.clone(),
            operation: result.operation.clone(),
            size,
            time_ms: result.time_ms,
        });

        app::log!(
            "BENCHMARK: {} {} (size={}, time={:.2}ms, throughput={} ops/sec)",
            result.collection_type,
            result.operation,
            result.size,
            result.time_ms,
            result.throughput_ops_per_sec
        );

        Ok(format!(
            "{} {} completed: {:.2}ms, {} ops/sec",
            result.collection_type, result.operation, result.time_ms, result.throughput_ops_per_sec
        ))
    }

    /// Benchmark UnorderedSet contains operations
    pub fn benchmark_set_contains(&mut self, size: u32) -> Result<String, String> {
        // First, populate the set
        let mut set: UnorderedSet<String> = UnorderedSet::new();
        for i in 0..size {
            let value = format!("value_{}", i);
            let _ = set
                .insert(value)
                .map_err(|e| format!("Insert failed: {:?}", e))?;
        }

        // Now benchmark contains operations
        let start_ns = calimero_sdk::env::time_now();

        for i in 0..size {
            let value = format!("value_{}", i);
            let _ = set
                .contains(&value)
                .map_err(|e| format!("Contains failed: {:?}", e))?;
        }

        let end_ns = calimero_sdk::env::time_now();
        let duration_ns = end_ns.saturating_sub(start_ns);
        let time_us = duration_ns / 1_000;
        let time_ms = (duration_ns as f64) / 1_000_000.0;

        let result = BenchmarkResult {
            collection_type: "UnorderedSet".to_string(),
            operation: "contains".to_string(),
            size,
            size_category: get_size_category(size),
            nesting_level: 1,
            time_ms,
            time_us,
            throughput_ops_per_sec: calculate_throughput(size, time_us),
            iterations: 1,
            platform: "rust".to_string(),
        };

        app::emit!(BenchmarkEvent::BenchmarkCompleted {
            collection_type: result.collection_type.clone(),
            operation: result.operation.clone(),
            size,
            time_ms: result.time_ms,
        });

        app::log!(
            "BENCHMARK: {} {} (size={}, time={:.2}ms, throughput={} ops/sec)",
            result.collection_type,
            result.operation,
            result.size,
            result.time_ms,
            result.throughput_ops_per_sec
        );

        Ok(format!(
            "{} {} completed: {:.2}ms, {} ops/sec",
            result.collection_type, result.operation, result.time_ms, result.throughput_ops_per_sec
        ))
    }

    // LWW REGISTER BENCHMARKS

    /// Benchmark LwwRegister set operations
    pub fn benchmark_register_set(&mut self, size: u32) -> Result<String, String> {
        let start_ns = calimero_sdk::env::time_now();

        let mut map: UnorderedMap<String, LwwRegister<String>> = UnorderedMap::new();
        for i in 0..size {
            let key = format!("key_{}", i);
            let value = format!("value_{}", i);
            let register = LwwRegister::new(value);
            drop(
                map.insert(key, register)
                    .map_err(|e| format!("Insert failed: {:?}", e))?,
            );
        }

        let end_ns = calimero_sdk::env::time_now();
        let duration_ns = end_ns.saturating_sub(start_ns);
        let time_us = duration_ns / 1_000;
        let time_ms = (duration_ns as f64) / 1_000_000.0;

        let result = BenchmarkResult {
            collection_type: "LwwRegister".to_string(),
            operation: "set".to_string(),
            size,
            size_category: get_size_category(size),
            nesting_level: 1,
            time_ms,
            time_us,
            throughput_ops_per_sec: calculate_throughput(size, time_us),
            iterations: 1,
            platform: "rust".to_string(),
        };

        app::emit!(BenchmarkEvent::BenchmarkCompleted {
            collection_type: result.collection_type.clone(),
            operation: result.operation.clone(),
            size,
            time_ms: result.time_ms,
        });

        app::log!(
            "BENCHMARK: {} {} (size={}, time={:.2}ms, throughput={} ops/sec)",
            result.collection_type,
            result.operation,
            result.size,
            result.time_ms,
            result.throughput_ops_per_sec
        );

        Ok(format!(
            "{} {} completed: {:.2}ms, {} ops/sec",
            result.collection_type, result.operation, result.time_ms, result.throughput_ops_per_sec
        ))
    }

    /// Benchmark LwwRegister get operations
    pub fn benchmark_register_get(&mut self, size: u32) -> Result<String, String> {
        // First, populate the map with registers
        let mut map: UnorderedMap<String, LwwRegister<String>> = UnorderedMap::new();
        for i in 0..size {
            let key = format!("key_{}", i);
            let value = format!("value_{}", i);
            let register = LwwRegister::new(value);
            drop(
                map.insert(key, register)
                    .map_err(|e| format!("Insert failed: {:?}", e))?,
            );
        }

        // Now benchmark get operations
        let start_ns = calimero_sdk::env::time_now();

        for i in 0..size {
            let key = format!("key_{}", i);
            if let Some(register) = map.get(&key).map_err(|e| format!("Get failed: {:?}", e))? {
                let _ = register.get();
            }
        }

        let end_ns = calimero_sdk::env::time_now();
        let duration_ns = end_ns.saturating_sub(start_ns);
        let time_us = duration_ns / 1_000;
        let time_ms = (duration_ns as f64) / 1_000_000.0;

        let result = BenchmarkResult {
            collection_type: "LwwRegister".to_string(),
            operation: "get".to_string(),
            size,
            size_category: get_size_category(size),
            nesting_level: 1,
            time_ms,
            time_us,
            throughput_ops_per_sec: calculate_throughput(size, time_us),
            iterations: 1,
            platform: "rust".to_string(),
        };

        app::emit!(BenchmarkEvent::BenchmarkCompleted {
            collection_type: result.collection_type.clone(),
            operation: result.operation.clone(),
            size,
            time_ms: result.time_ms,
        });

        app::log!(
            "BENCHMARK: {} {} (size={}, time={:.2}ms, throughput={} ops/sec)",
            result.collection_type,
            result.operation,
            result.size,
            result.time_ms,
            result.throughput_ops_per_sec
        );

        Ok(format!(
            "{} {} completed: {:.2}ms, {} ops/sec",
            result.collection_type, result.operation, result.time_ms, result.throughput_ops_per_sec
        ))
    }

    // PN-COUNTER BENCHMARKS (Increment + Decrement)

    /// Benchmark PNCounter increment operations
    pub fn benchmark_pncounter_increment(&mut self, size: u32) -> Result<String, String> {
        let start_ns = calimero_sdk::env::time_now();

        let mut counter = PNCounter::new();
        for _ in 0..size {
            counter
                .increment()
                .map_err(|e| format!("Increment failed: {:?}", e))?;
        }

        let end_ns = calimero_sdk::env::time_now();
        let duration_ns = end_ns.saturating_sub(start_ns);
        let time_us = duration_ns / 1_000;
        let time_ms = (duration_ns as f64) / 1_000_000.0;

        let result = BenchmarkResult {
            collection_type: "PNCounter".to_string(),
            operation: "increment".to_string(),
            size,
            size_category: get_size_category(size),
            nesting_level: 1,
            time_ms,
            time_us,
            throughput_ops_per_sec: calculate_throughput(size, time_us),
            iterations: 1,
            platform: "rust".to_string(),
        };

        app::emit!(BenchmarkEvent::BenchmarkCompleted {
            collection_type: result.collection_type.clone(),
            operation: result.operation.clone(),
            size,
            time_ms: result.time_ms,
        });

        app::log!(
            "BENCHMARK: {} {} (size={}, time={:.2}ms, throughput={} ops/sec)",
            result.collection_type,
            result.operation,
            result.size,
            result.time_ms,
            result.throughput_ops_per_sec
        );

        Ok(format!(
            "{} {} completed: {:.2}ms, {} ops/sec",
            result.collection_type, result.operation, result.time_ms, result.throughput_ops_per_sec
        ))
    }

    /// Benchmark PNCounter decrement operations
    pub fn benchmark_pncounter_decrement(&mut self, size: u32) -> Result<String, String> {
        // First increment to have values to decrement
        let mut counter = PNCounter::new();
        for _ in 0..size {
            counter
                .increment()
                .map_err(|e| format!("Increment failed: {:?}", e))?;
        }

        // Now benchmark decrement operations
        let start_ns = calimero_sdk::env::time_now();

        for _ in 0..size {
            counter
                .decrement()
                .map_err(|e| format!("Decrement failed: {:?}", e))?;
        }

        let end_ns = calimero_sdk::env::time_now();
        let duration_ns = end_ns.saturating_sub(start_ns);
        let time_us = duration_ns / 1_000;
        let time_ms = (duration_ns as f64) / 1_000_000.0;

        let result = BenchmarkResult {
            collection_type: "PNCounter".to_string(),
            operation: "decrement".to_string(),
            size,
            size_category: get_size_category(size),
            nesting_level: 1,
            time_ms,
            time_us,
            throughput_ops_per_sec: calculate_throughput(size, time_us),
            iterations: 1,
            platform: "rust".to_string(),
        };

        app::emit!(BenchmarkEvent::BenchmarkCompleted {
            collection_type: result.collection_type.clone(),
            operation: result.operation.clone(),
            size,
            time_ms: result.time_ms,
        });

        app::log!(
            "BENCHMARK: {} {} (size={}, time={:.2}ms, throughput={} ops/sec)",
            result.collection_type,
            result.operation,
            result.size,
            result.time_ms,
            result.throughput_ops_per_sec
        );

        Ok(format!(
            "{} {} completed: {:.2}ms, {} ops/sec",
            result.collection_type, result.operation, result.time_ms, result.throughput_ops_per_sec
        ))
    }

    /// Benchmark PNCounter mixed increment/decrement operations
    pub fn benchmark_pncounter_mixed(&mut self, size: u32) -> Result<String, String> {
        let start_ns = calimero_sdk::env::time_now();

        let mut counter = PNCounter::new();
        for i in 0..size {
            if i % 2 == 0 {
                counter
                    .increment()
                    .map_err(|e| format!("Increment failed: {:?}", e))?;
            } else {
                counter
                    .decrement()
                    .map_err(|e| format!("Decrement failed: {:?}", e))?;
            }
        }

        let end_ns = calimero_sdk::env::time_now();
        let duration_ns = end_ns.saturating_sub(start_ns);
        let time_us = duration_ns / 1_000;
        let time_ms = (duration_ns as f64) / 1_000_000.0;

        let result = BenchmarkResult {
            collection_type: "PNCounter".to_string(),
            operation: "mixed_inc_dec".to_string(),
            size,
            size_category: get_size_category(size),
            nesting_level: 1,
            time_ms,
            time_us,
            throughput_ops_per_sec: calculate_throughput(size, time_us),
            iterations: 1,
            platform: "rust".to_string(),
        };

        app::emit!(BenchmarkEvent::BenchmarkCompleted {
            collection_type: result.collection_type.clone(),
            operation: result.operation.clone(),
            size,
            time_ms: result.time_ms,
        });

        app::log!(
            "BENCHMARK: {} {} (size={}, time={:.2}ms, throughput={} ops/sec)",
            result.collection_type,
            result.operation,
            result.size,
            result.time_ms,
            result.throughput_ops_per_sec
        );

        Ok(format!(
            "{} {} completed: {:.2}ms, {} ops/sec",
            result.collection_type, result.operation, result.time_ms, result.throughput_ops_per_sec
        ))
    }

    // RGA (REPLICATED GROWABLE ARRAY) BENCHMARKS

    /// Benchmark RGA insert operations (character by character)
    pub fn benchmark_rga_insert(&mut self, size: u32) -> Result<String, String> {
        let start_ns = calimero_sdk::env::time_now();

        let mut rga = ReplicatedGrowableArray::new();
        for i in 0..size {
            // Insert characters at the end
            let char_to_insert = char::from_u32((b'a' as u32) + (i % 26)).unwrap_or('a');
            rga.insert(i as usize, char_to_insert)
                .map_err(|e| format!("Insert failed: {:?}", e))?;
        }

        let end_ns = calimero_sdk::env::time_now();
        let duration_ns = end_ns.saturating_sub(start_ns);
        let time_us = duration_ns / 1_000;
        let time_ms = (duration_ns as f64) / 1_000_000.0;

        let result = BenchmarkResult {
            collection_type: "ReplicatedGrowableArray".to_string(),
            operation: "insert".to_string(),
            size,
            size_category: get_size_category(size),
            nesting_level: 1,
            time_ms,
            time_us,
            throughput_ops_per_sec: calculate_throughput(size, time_us),
            iterations: 1,
            platform: "rust".to_string(),
        };

        app::emit!(BenchmarkEvent::BenchmarkCompleted {
            collection_type: result.collection_type.clone(),
            operation: result.operation.clone(),
            size,
            time_ms: result.time_ms,
        });

        app::log!(
            "BENCHMARK: {} {} (size={}, time={:.2}ms, throughput={} ops/sec)",
            result.collection_type,
            result.operation,
            result.size,
            result.time_ms,
            result.throughput_ops_per_sec
        );

        Ok(format!(
            "{} {} completed: {:.2}ms, {} ops/sec",
            result.collection_type, result.operation, result.time_ms, result.throughput_ops_per_sec
        ))
    }

    /// Benchmark RGA insert_str operations (bulk string insert)
    pub fn benchmark_rga_insert_str(&mut self, size: u32) -> Result<String, String> {
        // Create a test string of the desired size
        let test_string: String = (0..size)
            .map(|i| char::from_u32((b'a' as u32) + (i % 26)).unwrap_or('a'))
            .collect();

        let start_ns = calimero_sdk::env::time_now();

        let mut rga = ReplicatedGrowableArray::new();
        rga.insert_str(0, &test_string)
            .map_err(|e| format!("Insert str failed: {:?}", e))?;

        let end_ns = calimero_sdk::env::time_now();
        let duration_ns = end_ns.saturating_sub(start_ns);
        let time_us = duration_ns / 1_000;
        let time_ms = (duration_ns as f64) / 1_000_000.0;

        let result = BenchmarkResult {
            collection_type: "ReplicatedGrowableArray".to_string(),
            operation: "insert_str".to_string(),
            size,
            size_category: get_size_category(size),
            nesting_level: 1,
            time_ms,
            time_us,
            throughput_ops_per_sec: calculate_throughput(size, time_us),
            iterations: 1,
            platform: "rust".to_string(),
        };

        app::emit!(BenchmarkEvent::BenchmarkCompleted {
            collection_type: result.collection_type.clone(),
            operation: result.operation.clone(),
            size,
            time_ms: result.time_ms,
        });

        app::log!(
            "BENCHMARK: {} {} (size={}, time={:.2}ms, throughput={} ops/sec)",
            result.collection_type,
            result.operation,
            result.size,
            result.time_ms,
            result.throughput_ops_per_sec
        );

        Ok(format!(
            "{} {} completed: {:.2}ms, {} ops/sec",
            result.collection_type, result.operation, result.time_ms, result.throughput_ops_per_sec
        ))
    }

    /// Benchmark RGA get_text operations
    pub fn benchmark_rga_get_text(&mut self, size: u32) -> Result<String, String> {
        // First, populate the RGA
        let test_string: String = (0..size)
            .map(|i| char::from_u32((b'a' as u32) + (i % 26)).unwrap_or('a'))
            .collect();

        let mut rga = ReplicatedGrowableArray::new();
        rga.insert_str(0, &test_string)
            .map_err(|e| format!("Insert str failed: {:?}", e))?;

        // Now benchmark get_text operations
        let start_ns = calimero_sdk::env::time_now();

        let _text = rga
            .get_text()
            .map_err(|e| format!("Get text failed: {:?}", e))?;

        let end_ns = calimero_sdk::env::time_now();
        let duration_ns = end_ns.saturating_sub(start_ns);
        let time_us = duration_ns / 1_000;
        let time_ms = (duration_ns as f64) / 1_000_000.0;

        let result = BenchmarkResult {
            collection_type: "ReplicatedGrowableArray".to_string(),
            operation: "get_text".to_string(),
            size,
            size_category: get_size_category(size),
            nesting_level: 1,
            time_ms,
            time_us,
            throughput_ops_per_sec: calculate_throughput(size, time_us),
            iterations: 1,
            platform: "rust".to_string(),
        };

        app::emit!(BenchmarkEvent::BenchmarkCompleted {
            collection_type: result.collection_type.clone(),
            operation: result.operation.clone(),
            size,
            time_ms: result.time_ms,
        });

        app::log!(
            "BENCHMARK: {} {} (size={}, time={:.2}ms, throughput={} ops/sec)",
            result.collection_type,
            result.operation,
            result.size,
            result.time_ms,
            result.throughput_ops_per_sec
        );

        Ok(format!(
            "{} {} completed: {:.2}ms, {} ops/sec",
            result.collection_type, result.operation, result.time_ms, result.throughput_ops_per_sec
        ))
    }

    /// Benchmark RGA delete operations
    pub fn benchmark_rga_delete(&mut self, size: u32) -> Result<String, String> {
        // First, populate the RGA
        let test_string: String = (0..size)
            .map(|i| char::from_u32((b'a' as u32) + (i % 26)).unwrap_or('a'))
            .collect();

        let mut rga = ReplicatedGrowableArray::new();
        rga.insert_str(0, &test_string)
            .map_err(|e| format!("Insert str failed: {:?}", e))?;

        // Now benchmark delete operations (delete from the end)
        let start_ns = calimero_sdk::env::time_now();

        for i in (0..size).rev() {
            rga.delete(i as usize)
                .map_err(|e| format!("Delete failed: {:?}", e))?;
        }

        let end_ns = calimero_sdk::env::time_now();
        let duration_ns = end_ns.saturating_sub(start_ns);
        let time_us = duration_ns / 1_000;
        let time_ms = (duration_ns as f64) / 1_000_000.0;

        let result = BenchmarkResult {
            collection_type: "ReplicatedGrowableArray".to_string(),
            operation: "delete".to_string(),
            size,
            size_category: get_size_category(size),
            nesting_level: 1,
            time_ms,
            time_us,
            throughput_ops_per_sec: calculate_throughput(size, time_us),
            iterations: 1,
            platform: "rust".to_string(),
        };

        app::emit!(BenchmarkEvent::BenchmarkCompleted {
            collection_type: result.collection_type.clone(),
            operation: result.operation.clone(),
            size,
            time_ms: result.time_ms,
        });

        app::log!(
            "BENCHMARK: {} {} (size={}, time={:.2}ms, throughput={} ops/sec)",
            result.collection_type,
            result.operation,
            result.size,
            result.time_ms,
            result.throughput_ops_per_sec
        );

        Ok(format!(
            "{} {} completed: {:.2}ms, {} ops/sec",
            result.collection_type, result.operation, result.time_ms, result.throughput_ops_per_sec
        ))
    }

    /// Benchmark RGA delete_range operations
    pub fn benchmark_rga_delete_range(&mut self, size: u32) -> Result<String, String> {
        // First, populate the RGA
        let test_string: String = (0..size)
            .map(|i| char::from_u32((b'a' as u32) + (i % 26)).unwrap_or('a'))
            .collect();

        let mut rga = ReplicatedGrowableArray::new();
        rga.insert_str(0, &test_string)
            .map_err(|e| format!("Insert str failed: {:?}", e))?;

        // Now benchmark delete_range (delete all at once)
        let start_ns = calimero_sdk::env::time_now();

        rga.delete_range(0, size as usize)
            .map_err(|e| format!("Delete range failed: {:?}", e))?;

        let end_ns = calimero_sdk::env::time_now();
        let duration_ns = end_ns.saturating_sub(start_ns);
        let time_us = duration_ns / 1_000;
        let time_ms = (duration_ns as f64) / 1_000_000.0;

        let result = BenchmarkResult {
            collection_type: "ReplicatedGrowableArray".to_string(),
            operation: "delete_range".to_string(),
            size,
            size_category: get_size_category(size),
            nesting_level: 1,
            time_ms,
            time_us,
            throughput_ops_per_sec: calculate_throughput(size, time_us),
            iterations: 1,
            platform: "rust".to_string(),
        };

        app::emit!(BenchmarkEvent::BenchmarkCompleted {
            collection_type: result.collection_type.clone(),
            operation: result.operation.clone(),
            size,
            time_ms: result.time_ms,
        });

        app::log!(
            "BENCHMARK: {} {} (size={}, time={:.2}ms, throughput={} ops/sec)",
            result.collection_type,
            result.operation,
            result.size,
            result.time_ms,
            result.throughput_ops_per_sec
        );

        Ok(format!(
            "{} {} completed: {:.2}ms, {} ops/sec",
            result.collection_type, result.operation, result.time_ms, result.throughput_ops_per_sec
        ))
    }

    // COMPREHENSIVE BENCHMARK SUITE

    /// Run all benchmarks for a given size and return aggregated results
    pub fn run_benchmark_suite(&mut self, size: u32) -> Result<String, String> {
        let mut results: Vec<BenchmarkResult> = Vec::new();

        // Counter benchmarks
        if let Ok(json) = self.benchmark_counter_increment(size) {
            if let Ok(result) = serde_json::from_str::<BenchmarkResult>(&json) {
                results.push(result);
            }
        }

        // Map benchmarks (level 1)
        if let Ok(json) = self.benchmark_map_insert(size) {
            if let Ok(result) = serde_json::from_str::<BenchmarkResult>(&json) {
                results.push(result);
            }
        }
        if let Ok(json) = self.benchmark_map_get(size) {
            if let Ok(result) = serde_json::from_str::<BenchmarkResult>(&json) {
                results.push(result);
            }
        }

        // Nested map benchmarks (level 2)
        if let Ok(json) = self.benchmark_nested_map_insert(size) {
            if let Ok(result) = serde_json::from_str::<BenchmarkResult>(&json) {
                results.push(result);
            }
        }
        if let Ok(json) = self.benchmark_nested_map_get(size) {
            if let Ok(result) = serde_json::from_str::<BenchmarkResult>(&json) {
                results.push(result);
            }
        }

        // Deep nested map benchmarks (level 3)
        if let Ok(json) = self.benchmark_deep_nested_map_insert(size) {
            if let Ok(result) = serde_json::from_str::<BenchmarkResult>(&json) {
                results.push(result);
            }
        }

        // Vector benchmarks
        if let Ok(json) = self.benchmark_vector_push(size) {
            if let Ok(result) = serde_json::from_str::<BenchmarkResult>(&json) {
                results.push(result);
            }
        }
        if let Ok(json) = self.benchmark_vector_get(size) {
            if let Ok(result) = serde_json::from_str::<BenchmarkResult>(&json) {
                results.push(result);
            }
        }

        // Set benchmarks
        if let Ok(json) = self.benchmark_set_insert(size) {
            if let Ok(result) = serde_json::from_str::<BenchmarkResult>(&json) {
                results.push(result);
            }
        }
        if let Ok(json) = self.benchmark_set_contains(size) {
            if let Ok(result) = serde_json::from_str::<BenchmarkResult>(&json) {
                results.push(result);
            }
        }

        // LwwRegister benchmarks
        if let Ok(json) = self.benchmark_register_set(size) {
            if let Ok(result) = serde_json::from_str::<BenchmarkResult>(&json) {
                results.push(result);
            }
        }
        if let Ok(json) = self.benchmark_register_get(size) {
            if let Ok(result) = serde_json::from_str::<BenchmarkResult>(&json) {
                results.push(result);
            }
        }

        // PNCounter benchmarks
        if let Ok(json) = self.benchmark_pncounter_increment(size) {
            if let Ok(result) = serde_json::from_str::<BenchmarkResult>(&json) {
                results.push(result);
            }
        }
        if let Ok(json) = self.benchmark_pncounter_decrement(size) {
            if let Ok(result) = serde_json::from_str::<BenchmarkResult>(&json) {
                results.push(result);
            }
        }
        if let Ok(json) = self.benchmark_pncounter_mixed(size) {
            if let Ok(result) = serde_json::from_str::<BenchmarkResult>(&json) {
                results.push(result);
            }
        }

        // RGA benchmarks (use smaller size for RGA due to complexity)
        let rga_size = size.min(1000); // Cap RGA size to prevent timeout
        if let Ok(json) = self.benchmark_rga_insert(rga_size) {
            if let Ok(result) = serde_json::from_str::<BenchmarkResult>(&json) {
                results.push(result);
            }
        }
        if let Ok(json) = self.benchmark_rga_insert_str(rga_size) {
            if let Ok(result) = serde_json::from_str::<BenchmarkResult>(&json) {
                results.push(result);
            }
        }
        if let Ok(json) = self.benchmark_rga_get_text(rga_size) {
            if let Ok(result) = serde_json::from_str::<BenchmarkResult>(&json) {
                results.push(result);
            }
        }
        if let Ok(json) = self.benchmark_rga_delete(rga_size) {
            if let Ok(result) = serde_json::from_str::<BenchmarkResult>(&json) {
                results.push(result);
            }
        }
        if let Ok(json) = self.benchmark_rga_delete_range(rga_size) {
            if let Ok(result) = serde_json::from_str::<BenchmarkResult>(&json) {
                results.push(result);
            }
        }

        // Increment run count
        drop(self.run_count.increment());

        serde_json::to_string(&results).map_err(|e| e.to_string())
    }

    // UTILITY METHODS

    /// Get the number of benchmark runs
    pub fn get_run_count(&self) -> Result<u64, String> {
        self.run_count
            .value()
            .map_err(|e| format!("Get run count failed: {:?}", e))
    }

    /// Store a benchmark result for later retrieval
    pub fn store_result(&mut self, key: String, result: String) -> Result<(), String> {
        let register = LwwRegister::new(result);
        drop(
            self.results
                .insert(key, register)
                .map_err(|e| format!("Store result failed: {:?}", e))?,
        );
        Ok(())
    }

    /// Retrieve a stored benchmark result
    pub fn get_stored_result(&self, key: String) -> Result<Option<String>, String> {
        match self
            .results
            .get(&key)
            .map_err(|e| format!("Get result failed: {:?}", e))?
        {
            Some(register) => Ok(Some(register.get().clone())),
            None => Ok(None),
        }
    }
}
