//! Nested CRDT Test Application
//!
//! Tests all nesting patterns for CRDT support:
//! - Map<String, Counter> - counters should sum
//! - Map<String, LwwRegister<T>> - timestamps should win
//! - Map<String, Map<String, String>> - nested maps
//! - Vector<Counter> - element-wise merge
//! - Map<String, Set<String>> - union merge

#![allow(
    unused_crate_dependencies,
    reason = "Dependencies used in build process"
)]

use calimero_sdk::app;
use calimero_storage::collections::{
    Counter, LwwRegister, SortedMap, UnorderedMap, UnorderedSet, Vector,
};

#[app::state(emits = TestEvent)]
pub struct NestedCrdtTest {
    /// Map of counters - concurrent increments should sum
    pub counters: UnorderedMap<String, Counter>,

    /// Map of LWW registers - latest timestamp wins
    pub registers: UnorderedMap<String, LwwRegister<String>>,

    /// Nested maps - field-level merge
    pub metadata: UnorderedMap<String, UnorderedMap<String, LwwRegister<String>>>,

    /// Vector of counters - element-wise merge
    pub metrics: Vector<Counter>,

    /// Map of sets - union merge
    pub tags: UnorderedMap<String, UnorderedSet<String>>,

    /// Sorted map of registers - same add-wins/LWW merge as `registers`, but
    /// iterated in ascending key order (exercises SortedMap range queries and
    /// a CRDT value nested inside a SortedMap).
    pub sorted_scores: SortedMap<String, LwwRegister<u64>>,
}

#[app::event]
pub enum TestEvent {
    CounterIncremented {
        key: String,
        value: u64,
    },
    RegisterSet {
        key: String,
        value: String,
    },
    MetadataSet {
        outer_key: String,
        inner_key: String,
        value: String,
    },
    MetricPushed {
        value: u64,
    },
    TagAdded {
        key: String,
        tag: String,
    },
    SortedScoreSet {
        key: String,
        value: u64,
    },
}

#[app::logic]
impl NestedCrdtTest {
    /// Initialize with empty state
    #[app::init]
    pub fn init() -> NestedCrdtTest {
        NestedCrdtTest {
            counters: UnorderedMap::new_with_field_name("counters"),
            registers: UnorderedMap::new_with_field_name("registers"),
            metadata: UnorderedMap::new_with_field_name("metadata"),
            metrics: Vector::new_with_field_name("metrics"),
            tags: UnorderedMap::new_with_field_name("tags"),
            sorted_scores: SortedMap::new_with_field_name("sorted_scores"),
        }
    }

    // ===== Counter Operations =====

    pub fn increment_counter(&mut self, key: String) -> app::Result<u64> {
        let mut counter = self.counters.get(&key)?.unwrap_or_else(|| Counter::new());

        counter.increment()?;

        let value = counter.value()?;

        self.counters.insert(key.clone(), counter)?;

        app::emit!(TestEvent::CounterIncremented { key, value });

        Ok(value)
    }

    pub fn get_counter(&self, key: String) -> app::Result<u64> {
        let Some(counter) = self.counters.get(&key)? else {
            app::bail!("Counter not found");
        };

        Ok(counter.value()?)
    }

    // ===== LwwRegister Operations =====

    pub fn set_register(&mut self, key: String, value: String) -> app::Result<()> {
        let register = LwwRegister::new(value.clone());

        self.registers.insert(key.clone(), register)?;

        app::emit!(TestEvent::RegisterSet { key, value });

        Ok(())
    }

    pub fn get_register(&self, key: String) -> app::Result<String> {
        self.registers
            .get(&key)?
            .map(|r| r.get().clone())
            .ok_or_else(|| app::err!("Register not found"))
    }

    // ===== SortedMap Operations =====

    pub fn set_sorted_score(&mut self, key: String, value: u64) -> app::Result<()> {
        drop(
            self.sorted_scores
                .insert(key.clone(), LwwRegister::new(value))?,
        );

        app::emit!(TestEvent::SortedScoreSet { key, value });

        Ok(())
    }

    pub fn get_sorted_score(&self, key: String) -> app::Result<u64> {
        let Some(register) = self.sorted_scores.get(&key)? else {
            app::bail!("Score not found");
        };

        Ok(*register.get())
    }

    /// Keys in ascending order — the property that distinguishes SortedMap from
    /// the unordered `registers` field.
    pub fn sorted_score_keys(&self) -> app::Result<Vec<String>> {
        Ok(self.sorted_scores.keys()?.collect())
    }

    /// Scores whose keys fall within `[start, end)`, in ascending order.
    pub fn sorted_scores_range(
        &self,
        start: String,
        end: String,
    ) -> app::Result<Vec<(String, u64)>> {
        Ok(self
            .sorted_scores
            .range(start..end)?
            .map(|(k, v)| (k, *v.get()))
            .collect())
    }

    // ===== Nested Map Operations =====

    pub fn set_metadata(
        &mut self,
        outer_key: String,
        inner_key: String,
        value: String,
    ) -> app::Result<()> {
        let mut inner_map = self
            .metadata
            .get(&outer_key)?
            .unwrap_or_else(|| UnorderedMap::new());

        inner_map.insert(inner_key.clone(), value.clone().into())?;

        self.metadata.insert(outer_key.clone(), inner_map)?;

        app::emit!(TestEvent::MetadataSet {
            outer_key,
            inner_key,
            value,
        });

        Ok(())
    }

    pub fn get_metadata(&self, outer_key: String, inner_key: String) -> app::Result<String> {
        self.metadata
            .get(&outer_key)?
            .ok_or_else(|| app::err!("Outer key not found"))?
            .get(&inner_key)?
            .ok_or_else(|| app::err!("Inner key not found"))
            .map(|v| v.get().clone())
    }

    // ===== Vector Operations =====

    pub fn push_metric(&mut self, value: u64) -> app::Result<usize> {
        let mut counter = Counter::new();
        for _ in 0..value {
            counter.increment()?;
        }

        self.metrics.push(counter)?;

        let len = self.metrics.len()?;

        app::emit!(TestEvent::MetricPushed { value });

        Ok(len)
    }

    pub fn get_metric(&self, index: usize) -> app::Result<u64> {
        self.metrics
            .get(index)?
            .ok_or_else(|| app::err!("Index out of bounds"))?
            .value()
            .map_err(Into::into)
    }

    pub fn metrics_len(&self) -> app::Result<usize> {
        self.metrics.len().map_err(Into::into)
    }

    // ===== Set Operations =====

    pub fn add_tag(&mut self, key: String, tag: String) -> app::Result<()> {
        let mut set = self.tags.get(&key)?.unwrap_or_else(|| UnorderedSet::new());

        set.insert(tag.clone())?;

        self.tags.insert(key.clone(), set)?;

        app::emit!(TestEvent::TagAdded { key, tag });

        Ok(())
    }

    pub fn has_tag(&self, key: String, tag: String) -> app::Result<bool> {
        let Some(set) = self.tags.get(&key)? else {
            app::bail!("Key not found");
        };

        Ok(set.contains(&tag)?)
    }

    pub fn get_tag_count(&self, key: String) -> app::Result<u64> {
        let count = self
            .tags
            .get(&key)?
            .ok_or_else(|| app::err!("Key not found"))?
            .iter()?
            .count();

        Ok(count as u64)
    }
}

#[cfg(test)]
mod tests {
    use calimero_sdk::testing::TestHost;

    use super::*;

    #[test]
    fn counter_increment_and_view() {
        let mut app = TestHost::new(NestedCrdtTest::init);

        assert_eq!(app.call(|s| s.increment_counter("a".into())).unwrap(), 1);
        assert_eq!(app.call(|s| s.increment_counter("a".into())).unwrap(), 2);

        assert_eq!(app.view(|s| s.get_counter("a".into())).unwrap(), 2);
    }

    #[test]
    fn emits_event_on_increment() {
        let mut app = TestHost::new(NestedCrdtTest::init);

        app.call(|s| s.increment_counter("x".into())).unwrap();

        let events = app.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "CounterIncremented");
    }

    #[test]
    fn counter_sums_across_authors() {
        let mut app = TestHost::new(NestedCrdtTest::init);

        // The G-counter tracks a per-author tally; distinct executors each
        // contribute, and `value()` sums them.
        app.call_as([1; 32], |s| s.increment_counter("a".into()))
            .unwrap();
        app.call_as([2; 32], |s| s.increment_counter("a".into()))
            .unwrap();

        assert_eq!(app.view(|s| s.get_counter("a".into())).unwrap(), 2);
    }
}
