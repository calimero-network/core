//! Purpose-built guest for one question: does a single-call write ceiling
//! near ~490 entries (measured for `ReplicatedGrowableArray::insert_str` in
//! `crates/runtime/tests/rga_wall.rs`) belong to RGA specifically, or to
//! every collection alike?
//!
//! Each method below inserts `n` synthetic entries into an EMPTY collection
//! of the named kind, in a single call, exactly mirroring the shape of
//! `rga_wall.rs`'s `single_call_paste_wall`: state is reset every call (a
//! fresh `#[app::init]`), so what is measured is the flat per-entry insert
//! cost, not a cost that grows with pre-existing collection size.
//!
//! No app in `apps/` already exposes a bulk-insert-in-one-call entry point
//! for `UnorderedMap`, `Vector`, or `UnorderedSet` — this crate exists only
//! to provide one. It carries no other functionality and is not meant as a
//! usage example.

use calimero_sdk::app;
use calimero_storage::collections::{LwwRegister, UnorderedMap, UnorderedSet, Vector};

#[app::state]
pub struct BulkWriteBench {
    map: UnorderedMap<String, LwwRegister<String>>,
    vec: Vector<LwwRegister<String>>,
    set: UnorderedSet<String>,
}

#[app::logic]
impl BulkWriteBench {
    #[app::init]
    pub fn init() -> BulkWriteBench {
        BulkWriteBench {
            map: UnorderedMap::new(),
            vec: Vector::new(),
            set: UnorderedSet::new(),
        }
    }

    /// Insert `n` distinct `(key, value)` pairs into `map` in one call.
    /// Keys follow `tools/storage-cost`'s `key{i}` convention, for
    /// comparability with the committed cost snapshot.
    pub fn insert_n_map(&mut self, n: u32) -> app::Result<()> {
        for i in 0..n {
            self.map
                .insert(format!("key{i}"), LwwRegister::new("value".to_owned()))?;
        }
        Ok(())
    }

    /// Push `n` values onto `vec` in one call.
    pub fn insert_n_vec(&mut self, n: u32) -> app::Result<()> {
        for i in 0..n {
            self.vec.push(LwwRegister::new(format!("value{i}")))?;
        }
        Ok(())
    }

    /// Insert `n` distinct values into `set` in one call.
    pub fn insert_n_set(&mut self, n: u32) -> app::Result<()> {
        for i in 0..n {
            self.set.insert(format!("value{i}"))?;
        }
        Ok(())
    }

    pub fn map_len(&self) -> app::Result<usize> {
        Ok(self.map.len()?)
    }

    pub fn vec_len(&self) -> app::Result<usize> {
        Ok(self.vec.len()?)
    }

    pub fn set_len(&self) -> app::Result<usize> {
        Ok(self.set.len()?)
    }
}

#[cfg(test)]
mod tests {
    use calimero_sdk::testing::TestHost;

    use super::*;

    #[test]
    fn inserts_land_in_each_collection() {
        let mut app = TestHost::new(BulkWriteBench::init);

        app.call(|s| s.insert_n_map(5)).unwrap();
        app.call(|s| s.insert_n_vec(5)).unwrap();
        app.call(|s| s.insert_n_set(5)).unwrap();

        assert_eq!(app.view(|s| s.map_len()).unwrap(), 5);
        assert_eq!(app.view(|s| s.vec_len()).unwrap(), 5);
        assert_eq!(app.view(|s| s.set_len()).unwrap(), 5);
    }
}
