//! High-level data structures for storage.

/// This module provides functionality for hashmap data structures.
pub mod unordered_map;
pub use unordered_map::UnorderedMap;
/// This module provides functionality for hashset data structures.
pub mod unordered_set;
pub use unordered_set::UnorderedSet;
/// This module provides functionality for vector data structures.
pub mod vector;
pub use vector::Vector;
/// This module provides functionality for handling errors.
pub mod error;
pub use error::StoreError;
