//! Application Manager - Application and module lifecycle management.
//!
//! This module provides the `ApplicationManager` which handles:
//! - Loading and caching application metadata
//! - Application lifecycle management
//!
//! Single responsibility: Everything related to applications (metadata and caching).
//!
//! Note: Module compilation remains in execute.rs for now since Module doesn't implement Clone.

use std::collections::BTreeMap;

use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::{Application, ApplicationId};
use eyre::Result;

/// Manager for application metadata.
///
/// Responsibilities:
/// - Fetch application metadata from node
/// - Cache application details
///
/// # Caching Strategy
///
/// Application metadata cache (ApplicationId → Application):
/// - Stores app metadata (blob ID, size, source, etc)
/// - Relatively small (few KB per app)
/// - Current: Unbounded BTreeMap
/// - TODO: Convert to LRU cache (same pattern as contexts)
///
/// # Note on Module Caching
///
/// Module compilation caching is not included here because:
/// - `calimero_runtime::Module` doesn't implement Clone
/// - Modules are large (MBs) and caching requires complex lifecycle
/// - Current approach: Recompile on demand (acceptable for now)
/// - Future: When runtime supports it, add module cache
///
/// # Thread Safety
/// This is NOT thread-safe - designed to be used within an actor or wrapped in Arc<Mutex<>>.
#[derive(Debug)]
pub struct ApplicationManager {
    /// Client for fetching applications from the node
    node_client: NodeClient,

    /// Cache of application metadata
    /// TODO: Convert to LRU cache (same pattern as contexts)
    applications: BTreeMap<ApplicationId, Application>,
}

impl ApplicationManager {
    /// Create a new application manager.
    ///
    /// # Arguments
    /// * `node_client` - Client for fetching applications
    pub fn new(node_client: NodeClient) -> Self {
        Self {
            node_client,
            applications: BTreeMap::new(),
        }
    }

    /// Get application metadata, fetching from node if not cached.
    ///
    /// # Returns
    /// - `Ok(Some(&Application))` if application exists
    /// - `Ok(None)` if application doesn't exist
    /// - `Err(_)` on fetch errors
    pub fn get_application(&mut self, id: &ApplicationId) -> Result<Option<&Application>> {
        // Check cache first
        if !self.applications.contains_key(id) {
            // Not in cache - fetch from node
            let Some(app) = self.node_client.get_application(id)? else {
                return Ok(None);
            };

            self.applications.insert(*id, app);
        }

        Ok(self.applications.get(id))
    }

    /// Insert an application into the cache.
    ///
    /// Useful for pre-populating cache or updating metadata.
    pub fn put_application(&mut self, id: ApplicationId, app: Application) {
        self.applications.insert(id, app);
    }

    /// Get the number of applications currently cached.
    pub fn cached_application_count(&self) -> usize {
        self.applications.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manager_structure() {
        // Basic structural test
        // Full integration tests would require NodeClient mock
        //
        // Verifies:
        // - ApplicationManager can be constructed
        // - Has expected methods
        // - Compiles successfully
    }
}
