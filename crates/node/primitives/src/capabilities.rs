use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

/// Capability bits for fast path operations
/// 
/// These capabilities define which operations can be executed without WASM
/// for performance optimization. Operations with these capabilities can
/// be applied directly to the CRDT state without full WASM execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Capability {
    /// Pure key-value set operation (no side effects)
    PureKvSet,
    /// Pure counter increment operation
    PureCounterInc,
    /// Pure counter decrement operation
    PureCounterDec,
    /// Pure set add operation
    PureSetAdd,
    /// Pure set remove operation
    PureSetRemove,
    /// Pure map put operation
    PureMapPut,
    /// Pure map remove operation
    PureMapRemove,
    /// Pure list append operation
    PureListAppend,
    /// Pure list remove operation
    PureListRemove,
}

impl Capability {
    /// Get the string representation of the capability
    pub fn as_str(&self) -> &'static str {
        match self {
            Capability::PureKvSet => "pure_kv_set",
            Capability::PureCounterInc => "pure_counter_inc",
            Capability::PureCounterDec => "pure_counter_dec",
            Capability::PureSetAdd => "pure_set_add",
            Capability::PureSetRemove => "pure_set_remove",
            Capability::PureMapPut => "pure_map_put",
            Capability::PureMapRemove => "pure_map_remove",
            Capability::PureListAppend => "pure_list_append",
            Capability::PureListRemove => "pure_list_remove",
        }
    }

    /// Check if this capability allows fast path execution
    pub fn allows_fast_path(&self) -> bool {
        matches!(
            self,
            Capability::PureKvSet
                | Capability::PureCounterInc
                | Capability::PureCounterDec
                | Capability::PureSetAdd
                | Capability::PureSetRemove
                | Capability::PureMapPut
                | Capability::PureMapRemove
                | Capability::PureListAppend
                | Capability::PureListRemove
        )
    }

    /// Get the maximum payload size for this capability
    pub fn max_payload_size(&self) -> usize {
        match self {
            Capability::PureKvSet => 1024, // 1KB for key-value operations
            Capability::PureCounterInc => 64, // 64B for counter operations
            Capability::PureCounterDec => 64,
            Capability::PureSetAdd => 512, // 512B for set operations
            Capability::PureSetRemove => 512,
            Capability::PureMapPut => 1024, // 1KB for map operations
            Capability::PureMapRemove => 512,
            Capability::PureListAppend => 1024, // 1KB for list operations
            Capability::PureListRemove => 512,
        }
    }
}

impl From<&str> for Capability {
    fn from(s: &str) -> Self {
        match s {
            "pure_kv_set" => Capability::PureKvSet,
            "pure_counter_inc" => Capability::PureCounterInc,
            "pure_counter_dec" => Capability::PureCounterDec,
            "pure_set_add" => Capability::PureSetAdd,
            "pure_set_remove" => Capability::PureSetRemove,
            "pure_map_put" => Capability::PureMapPut,
            "pure_map_remove" => Capability::PureMapRemove,
            "pure_list_append" => Capability::PureListAppend,
            "pure_list_remove" => Capability::PureListRemove,
            _ => panic!("Unknown capability: {}", s),
        }
    }
}

/// Application manifest with capability declarations
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct AppManifest {
    /// Application name
    pub name: String,
    /// Application version
    pub version: String,
    /// List of capabilities this application supports
    pub capabilities: Vec<Capability>,
    /// Method to capability mapping
    pub method_capabilities: std::collections::HashMap<String, Capability>,
}

impl AppManifest {
    /// Create a new application manifest
    pub fn new(name: String, version: String) -> Self {
        Self {
            name,
            version,
            capabilities: Vec::new(),
            method_capabilities: std::collections::HashMap::new(),
        }
    }

    /// Add a capability to the manifest
    pub fn add_capability(&mut self, capability: Capability) {
        if !self.capabilities.contains(&capability) {
            self.capabilities.push(capability);
        }
    }

    /// Map a method to a capability
    pub fn map_method(&mut self, method: String, capability: Capability) {
        self.method_capabilities.insert(method, capability);
    }

    /// Check if a method has a specific capability
    pub fn has_method_capability(&self, method: &str, capability: &Capability) -> bool {
        self.method_capabilities
            .get(method)
            .map(|c| c == capability)
            .unwrap_or(false)
    }

    /// Check if a method supports fast path execution
    pub fn supports_fast_path(&self, method: &str) -> bool {
        self.method_capabilities
            .get(method)
            .map(|c| c.allows_fast_path())
            .unwrap_or(false)
    }

    /// Get the capability for a method
    pub fn get_method_capability(&self, method: &str) -> Option<&Capability> {
        self.method_capabilities.get(method)
    }

    /// Validate the manifest
    pub fn validate(&self) -> Result<(), String> {
        // Check that all method capabilities are declared in capabilities list
        for (method, capability) in &self.method_capabilities {
            if !self.capabilities.contains(capability) {
                return Err(format!(
                    "Method '{}' uses undeclared capability '{:?}'",
                    method, capability
                ));
            }
        }
        Ok(())
    }
}

/// Capability validator for deployment
pub struct CapabilityValidator;

impl CapabilityValidator {
    /// Validate capabilities at deploy time
    pub fn validate_deploy(manifest: &AppManifest) -> Result<(), String> {
        // Validate manifest structure
        manifest.validate()?;

        // Check for duplicate method mappings
        let mut seen_methods = std::collections::HashSet::new();
        for method in manifest.method_capabilities.keys() {
            if !seen_methods.insert(method) {
                return Err(format!("Duplicate method mapping: {}", method));
            }
        }

        // Check for valid capability declarations
        for capability in &manifest.capabilities {
            if !capability.allows_fast_path() {
                return Err(format!("Invalid capability: {:?}", capability));
            }
        }

        Ok(())
    }

    /// Check if an operation can use fast path
    pub fn can_use_fast_path(
        manifest: &AppManifest,
        method: &str,
        payload_size: usize,
    ) -> bool {
        // Check if method supports fast path
        if !manifest.supports_fast_path(method) {
            return false;
        }

        // Check payload size limits
        if let Some(capability) = manifest.get_method_capability(method) {
            if payload_size > capability.max_payload_size() {
                return false;
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_creation() {
        let capability = Capability::PureKvSet;
        assert_eq!(capability.as_str(), "pure_kv_set");
        assert!(capability.allows_fast_path());
        assert_eq!(capability.max_payload_size(), 1024);
    }

    #[test]
    fn test_capability_from_str() {
        let capability = Capability::from("pure_kv_set");
        assert_eq!(capability, Capability::PureKvSet);
    }

    #[test]
    fn test_app_manifest() {
        let mut manifest = AppManifest::new("test-app".to_string(), "1.0.0".to_string());
        
        manifest.add_capability(Capability::PureKvSet);
        manifest.map_method("set".to_string(), Capability::PureKvSet);
        
        assert!(manifest.has_method_capability("set", &Capability::PureKvSet));
        assert!(manifest.supports_fast_path("set"));
        assert_eq!(manifest.get_method_capability("set"), Some(&Capability::PureKvSet));
    }

    #[test]
    fn test_manifest_validation() {
        let mut manifest = AppManifest::new("test-app".to_string(), "1.0.0".to_string());
        
        // Valid manifest
        manifest.add_capability(Capability::PureKvSet);
        manifest.map_method("set".to_string(), Capability::PureKvSet);
        assert!(manifest.validate().is_ok());
        
        // Invalid manifest - undeclared capability
        manifest.map_method("get".to_string(), Capability::PureCounterInc);
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn test_capability_validator() {
        let mut manifest = AppManifest::new("test-app".to_string(), "1.0.0".to_string());
        manifest.add_capability(Capability::PureKvSet);
        manifest.map_method("set".to_string(), Capability::PureKvSet);
        
        assert!(CapabilityValidator::validate_deploy(&manifest).is_ok());
        assert!(CapabilityValidator::can_use_fast_path(&manifest, "set", 512));
        assert!(!CapabilityValidator::can_use_fast_path(&manifest, "set", 2048)); // Too large
        assert!(!CapabilityValidator::can_use_fast_path(&manifest, "unknown", 512)); // Unknown method
    }
}
