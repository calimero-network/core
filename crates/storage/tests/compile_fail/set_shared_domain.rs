//! App code cannot mark an entity `Shared` (a cell anchor): only the SDK's
//! cells create one. If app code could, a fresh entity stamped `Shared` with
//! writers that exclude the caller would pass the local write check as a cell's
//! creation. `set_shared_domain_scoped` is crate-only, and the unscoped
//! `set_shared_domain` no longer exists.

use std::collections::{BTreeMap, BTreeSet};

use calimero_storage::collections::UnorderedMap;
use calimero_storage::entities::Data;

fn main() {
    let mut map = UnorderedMap::<String, String>::new();
    map.element_mut().set_shared_domain(BTreeSet::new());
    map.element_mut().set_shared_domain_scoped(BTreeMap::new());
}
