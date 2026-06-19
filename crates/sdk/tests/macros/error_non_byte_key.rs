//! A collection key that isn't byte-encodable (`u64` lacks `AsRef<[u8]>`) must
//! give the `StorageKey` diagnostic at the first key operation.
//!
//! The map is a local (not a `#[app::state]` field) so the `StorageKey` error
//! surfaces directly: an in-state `UnorderedMap<u64, _>` would first fail the
//! `Mergeable` field check, shadowing this message.

use calimero_sdk::app;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

#[app::state]
struct S;

#[app::logic]
impl S {
    #[app::init]
    pub fn init() -> S {
        S
    }

    pub fn put(&self) -> app::Result<()> {
        let mut map: UnorderedMap<u64, LwwRegister<String>> = UnorderedMap::new();
        map.insert(1u64, "x".to_owned().into())?;
        Ok(())
    }
}

fn main() {}
