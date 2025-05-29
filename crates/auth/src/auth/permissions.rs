use std::collections::HashMap;

use axum::http::Request;
use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Mapping between URL paths and required permissions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionMapping {
    pub path_pattern: String,
    pub methods: HashMap<String, Vec<String>>,
    pub resource_extractor: Option<String>,
}

lazy_static! {
    static ref PERMISSION_MAPPINGS: Vec<PermissionMapping> = vec![
        // Root Key Management
        PermissionMapping {
            path_pattern: "^/auth/keys$".to_string(),
            methods: HashMap::from([
                ("GET".to_string(), vec!["keys:list".to_string()]),
                ("POST".to_string(), vec!["keys:create".to_string()]),
            ]),
            resource_extractor: None,
        },
        PermissionMapping {
            path_pattern: "^/auth/keys/([^/]+)$".to_string(),
            methods: HashMap::from([
                ("DELETE".to_string(), vec!["keys:delete".to_string()]),
            ]),
            resource_extractor: Some("1".to_string()),
        },
        // Client Key Management
        PermissionMapping {
            path_pattern: "^/auth/keys/([^/]+)/clients$".to_string(),
            methods: HashMap::from([
                ("GET".to_string(), vec!["clients:list".to_string()]),
                ("POST".to_string(), vec!["clients:create".to_string()]),
            ]),
            resource_extractor: Some("1".to_string()),
        },
        PermissionMapping {
            path_pattern: "^/auth/keys/([^/]+)/clients/([^/]+)$".to_string(),
            methods: HashMap::from([
                ("DELETE".to_string(), vec!["clients:delete".to_string()]),
            ]),
            resource_extractor: Some("1".to_string()),
        },
        // Admin API - Applications
        PermissionMapping {
            path_pattern: "^/admin-api/install-application$".to_string(),
            methods: HashMap::from([
                ("POST".to_string(), vec!["admin:applications:install".to_string()]),
            ]),
            resource_extractor: None,
        },
        PermissionMapping {
            path_pattern: "^/admin-api/uninstall-application$".to_string(),
            methods: HashMap::from([
                ("POST".to_string(), vec!["admin:applications:uninstall".to_string()]),
            ]),
            resource_extractor: None,
        },
        PermissionMapping {
            path_pattern: "^/admin-api/applications$".to_string(),
            methods: HashMap::from([
                ("GET".to_string(), vec!["admin:applications:list".to_string()]),
            ]),
            resource_extractor: None,
        },
        PermissionMapping {
            path_pattern: "^/admin-api/applications/([^/]+)$".to_string(),
            methods: HashMap::from([
                ("GET".to_string(), vec!["admin:applications:read".to_string()]),
            ]),
            resource_extractor: Some("1".to_string()),
        },
        // Admin API - Contexts
        PermissionMapping {
            path_pattern: "^/admin-api/contexts$".to_string(),
            methods: HashMap::from([
                ("GET".to_string(), vec!["admin:contexts:list".to_string()]),
                ("POST".to_string(), vec!["admin:contexts:create".to_string()]),
            ]),
            resource_extractor: None,
        },
        PermissionMapping {
            path_pattern: "^/admin-api/contexts/([^/]+)$".to_string(),
            methods: HashMap::from([
                ("GET".to_string(), vec!["admin:contexts:read".to_string()]),
                ("DELETE".to_string(), vec!["admin:contexts:delete".to_string()]),
            ]),
            resource_extractor: Some("1".to_string()),
        },
        PermissionMapping {
            path_pattern: "^/admin-api/contexts/([^/]+)/client-keys$".to_string(),
            methods: HashMap::from([
                ("GET".to_string(), vec!["admin:contexts:keys:list".to_string()]),
            ]),
            resource_extractor: Some("1".to_string()),
        },
        PermissionMapping {
            path_pattern: "^/admin-api/contexts/([^/]+)/storage$".to_string(),
            methods: HashMap::from([
                ("GET".to_string(), vec!["admin:contexts:storage:read".to_string()]),
            ]),
            resource_extractor: Some("1".to_string()),
        },
        PermissionMapping {
            path_pattern: "^/admin-api/contexts/([^/]+)/identities$".to_string(),
            methods: HashMap::from([
                ("GET".to_string(), vec!["admin:contexts:identities:list".to_string()]),
            ]),
            resource_extractor: Some("1".to_string()),
        },
        // Admin API - DID
        PermissionMapping {
            path_pattern: "^/admin-api/did$".to_string(),
            methods: HashMap::from([
                ("GET".to_string(), vec!["admin:did:read".to_string()]),
                ("DELETE".to_string(), vec!["admin:did:delete".to_string()]),
            ]),
            resource_extractor: None,
        },
        // JSON-RPC
        PermissionMapping {
            path_pattern: "^/jsonrpc/contexts/([^/]+)/execute$".to_string(),
            methods: HashMap::from([
                ("POST".to_string(), vec!["jsonrpc:contexts:execute".to_string()]),
            ]),
            resource_extractor: Some("1".to_string()),
        },
    ];
}

/// Permission validator for checking request permissions
#[derive(Debug, Default)]
pub struct PermissionValidator;

impl PermissionValidator {
    pub fn new() -> Self {
        Self
    }

    /// Determine required permissions for a given request
    pub fn determine_required_permissions(
        &self,
        request: &Request<axum::body::Body>,
    ) -> Vec<String> {
        let path = request.uri().path();
        let method = request.method();

        let mut required_permissions = Vec::new();

        for mapping in PERMISSION_MAPPINGS.iter() {
            if let Ok(regex) = Regex::new(&mapping.path_pattern) {
                if let Some(captures) = regex.captures(path) {
                    // Get method-specific permissions
                    if let Some(method_perms) = mapping.methods.get(method.as_str()) {
                        for perm in method_perms {
                            // If there's a resource extractor, append the resource ID
                            if let Some(extractor) = &mapping.resource_extractor {
                                if let Some(resource_id) =
                                    captures.get(extractor.parse::<usize>().unwrap_or(1))
                                {
                                    required_permissions.push(format!(
                                        "{}[{}]",
                                        perm,
                                        resource_id.as_str()
                                    ));
                                } else {
                                    required_permissions.push(perm.clone());
                                }
                            } else {
                                required_permissions.push(perm.clone());
                            }
                        }
                    }
                }
            }
        }

        required_permissions
    }

    /// Validate if user permissions satisfy required permissions
    pub fn validate_permissions(
        &self,
        user_permissions: &[String],
        required_permissions: &[String],
    ) -> bool {
        for required in required_permissions {
            let mut has_permission = false;

            for user_perm in user_permissions {
                // Check exact match
                if user_perm == required {
                    has_permission = true;
                    break;
                }

                // Check wildcard permissions
                if user_perm.ends_with('*') {
                    let prefix = &user_perm[..user_perm.len() - 1];
                    if required.starts_with(prefix) {
                        has_permission = true;
                        break;
                    }
                }

                // Check parent permissions
                let required_parts: Vec<&str> = required.split(&[':', '[', ']']).collect();
                let user_parts: Vec<&str> = user_perm.split(&[':', '[', ']']).collect();

                if required_parts.len() > 1 && user_parts.len() > 0 {
                    // Check if user has parent permission (e.g. "application" for "application:list")
                    if user_parts[0] == required_parts[0] && user_parts.len() == 1 {
                        has_permission = true;
                        break;
                    }

                    // Check if user has action permission without resource ID
                    if user_parts.len() == 2 && required_parts.len() == 3 {
                        if user_parts[0] == required_parts[0] && user_parts[1] == required_parts[1]
                        {
                            has_permission = true;
                            break;
                        }
                    }
                }
            }

            if !has_permission {
                return false;
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use axum::http::Method;

    use super::*;

    #[test]
    fn test_permission_validation() {
        let validator = PermissionValidator::new();

        // Test exact match
        assert!(validator.validate_permissions(
            &["application:list".to_string()],
            &["application:list".to_string()]
        ));

        // Test parent permission
        assert!(validator.validate_permissions(
            &["application".to_string()],
            &["application:list".to_string()]
        ));

        // Test resource-specific permission
        assert!(validator.validate_permissions(
            &["context:read[123]".to_string()],
            &["context:read[123]".to_string()]
        ));

        // Test parent permission with resource
        assert!(validator.validate_permissions(
            &["context:read".to_string()],
            &["context:read[123]".to_string()]
        ));

        // Test wildcard permission
        assert!(validator.validate_permissions(
            &["application:*".to_string()],
            &["application:list".to_string()]
        ));

        // Test permission denied
        assert!(!validator.validate_permissions(
            &["context:read".to_string()],
            &["application:list".to_string()]
        ));
    }
}
