//! High-level data structures for storage.

/// This module provides functionality for hashmap data structures.
pub mod hashmap;
pub use hashmap::HashMap;
/// This module provides functionality for hashset data structures.
pub mod hashset;
pub use hashset::HashSet;
/// This module provides functionality for vector data structures.
pub mod vector;
pub use vector::Vector;
/// This module provides functionality for handling errors.
pub mod error;
pub use error::StoreError;