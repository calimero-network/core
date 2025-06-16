use axum::body::Body;
use axum::http::Request;
use lazy_static::lazy_static;
use regex::Regex;

use super::types::{
    AddBlobPermission, AliasPermission, ApplicationPermission, BlobPermission, ContextPermission,
    HttpMethod, KeyPermission, Permission, ResourceScope, UserScope,
};

/// Permission validator for checking request permissions
#[derive(Debug, Default)]
pub struct PermissionValidator;

/// Represents a path pattern and its required permissions
struct PathMapping {
    pattern: &'static str,
    handler: fn(&[&str], HttpMethod) -> Vec<Permission>,
}

lazy_static! {
    static ref PATH_MAPPINGS: Vec<PathMapping> = vec![
        // Root Key Management
        PathMapping {
            pattern: "^/auth/keys$",
            handler: |_, method| match method {
                HttpMethod::GET => vec![Permission::Keys(KeyPermission::List)],
                HttpMethod::POST => vec![Permission::Keys(KeyPermission::Create)],
                _ => vec![],
            },
        },
        PathMapping {
            pattern: "^/auth/keys/([^/]+)$",
            handler: |_, method| match method {
                HttpMethod::DELETE => vec![Permission::Keys(KeyPermission::Delete)],
                _ => vec![],
            },
        },
        // Admin API - Applications
        PathMapping {
            pattern: "^/admin-api/applications$",
            handler: |_, method| match method {
                HttpMethod::GET => vec![Permission::Application(ApplicationPermission::List(ResourceScope::Global))],
                _ => vec![],
            },
        },
        PathMapping {
            pattern: "^/admin-api/applications/([^/]+)$",
            handler: |components, method| {
                if let Some(&app_id) = components.get(1) {
                    match method {
                        HttpMethod::GET => vec![Permission::Application(ApplicationPermission::List(
                            ResourceScope::Specific(vec![app_id.to_string()])
                        ))],
                        _ => vec![],
                    }
                } else {
                    vec![]
                }
            },
        },
        PathMapping {
            pattern: "^/admin-api/install-application$",
            handler: |_, method| match method {
                HttpMethod::POST => vec![Permission::Application(ApplicationPermission::Install(ResourceScope::Global))],
                _ => vec![],
            },
        },
        PathMapping {
            pattern: "^/admin-api/uninstall-application$",
            handler: |components, method| match method {
                HttpMethod::POST => vec![Permission::Application(ApplicationPermission::Uninstall(ResourceScope::Global))],
                _ => vec![],
            },
        },
        // Admin API - Contexts
        PathMapping {
            pattern: "^/admin-api/contexts$",
            handler: |_, method| match method {
                HttpMethod::GET => vec![Permission::Context(ContextPermission::List(ResourceScope::Global))],
                HttpMethod::POST => vec![Permission::Context(ContextPermission::Create(ResourceScope::Global))],
                _ => vec![],
            },
        },
        PathMapping {
            pattern: "^/admin-api/contexts/([^/]+)$",
            handler: |components, method| {
                if let Some(&ctx_id) = components.get(1) {
                    let scope = ResourceScope::Specific(vec![ctx_id.to_string()]);
                    match method {
                        HttpMethod::GET => vec![Permission::Context(ContextPermission::List(scope))],
                        HttpMethod::DELETE => vec![Permission::Context(ContextPermission::Delete(scope))],
                        _ => vec![],
                    }
                } else {
                    vec![]
                }
            },
        },
        // JSON-RPC
        PathMapping {
            pattern: "^/jsonrpc/contexts/([^/]+)/execute$",
            handler: |components, method| {
                if let Some(&ctx_id) = components.get(1) {
                    match method {
                        HttpMethod::POST => vec![Permission::Context(ContextPermission::Execute(
                            ResourceScope::Specific(vec![ctx_id.to_string()]),
                            UserScope::Any,
                            None
                        ))],
                        _ => vec![],
                    }
                } else {
                    vec![]
                }
            },
        },
        // Context Alias endpoints
        PathMapping {
            pattern: "^/contexts/([^/]+)/alias$",
            handler: |components, method| {
                if let Some(&ctx_id) = components.get(1) {
                    match method {
                        HttpMethod::POST => vec![Permission::Context(ContextPermission::Alias(AliasPermission::Create))],
                        HttpMethod::DELETE => vec![Permission::Context(ContextPermission::Alias(AliasPermission::Delete))],
                        HttpMethod::GET => vec![Permission::Context(ContextPermission::Alias(AliasPermission::Lookup {
                            context_id: Some(ctx_id.to_string()),
                            user_id: None,
                        }))],
                        _ => vec![],
                    }
                } else {
                    vec![]
                }
            },
        },
        PathMapping {
            pattern: "^/contexts/([^/]+)/alias/([^/]+)$",
            handler: |components, method| {
                match (components.get(1), components.get(2)) {
                    (Some(&ctx_id), Some(&user_id)) => vec![Permission::Context(ContextPermission::Alias(AliasPermission::Lookup {
                        context_id: Some(ctx_id.to_string()),
                        user_id: Some(user_id.to_string()),
                    }))],
                    _ => vec![],
                }
            },
        },
        // Blob Operations
        PathMapping {
            pattern: "^/blobs/stream$",
            handler: |_, method| match method {
                HttpMethod::POST => vec![Permission::Blob(BlobPermission::Add(AddBlobPermission::Stream))],
                _ => vec![],
            },
        },
        PathMapping {
            pattern: "^/blobs/file$",
            handler: |_, method| match method {
                HttpMethod::POST => vec![Permission::Blob(BlobPermission::Add(AddBlobPermission::File))],
                _ => vec![],
            },
        },
        PathMapping {
            pattern: "^/blobs/url$",
            handler: |_, method| match method {
                HttpMethod::POST => vec![Permission::Blob(BlobPermission::Add(AddBlobPermission::Url))],
                _ => vec![],
            },
        },
        PathMapping {
            pattern: "^/blobs/([^/]+)$",
            handler: |components, method| {
                if let Some(&blob_id) = components.get(1) {
                    match method {
                        HttpMethod::DELETE => vec![Permission::Blob(BlobPermission::Remove(
                            ResourceScope::Specific(vec![blob_id.to_string()])
                        ))],
                        _ => vec![],
                    }
                } else {
                    vec![]
                }
            },
        },
    ];
}

impl PermissionValidator {
    pub fn new() -> Self {
        Self
    }

    /// Determine required permissions for a given request
    pub fn determine_required_permissions(&self, request: &Request<Body>) -> Vec<Permission> {
        let path = request.uri().path();
        let method = match request.method().as_str() {
            "GET" => HttpMethod::GET,
            "POST" => HttpMethod::POST,
            "PUT" => HttpMethod::PUT,
            "DELETE" => HttpMethod::DELETE,
            "PATCH" => HttpMethod::PATCH,
            _ => HttpMethod::Any,
        };

        let mut required_permissions = Vec::new();

        // First check path mappings
        for mapping in PATH_MAPPINGS.iter() {
            if let Ok(regex) = Regex::new(mapping.pattern) {
                if let Some(captures) = regex.captures(path) {
                    // Extract path components
                    let components: Vec<&str> = (1..captures.len())
                        .filter_map(|i| captures.get(i))
                        .map(|m| m.as_str())
                        .collect();

                    // Get permissions from the handler
                    let perms = (mapping.handler)(&components, method.clone());
                    required_permissions.extend(perms);
                }
            }
        }

        // If no matches in path mappings, try the standard resource paths
        if required_permissions.is_empty() {
            let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
            if !components.is_empty() {
                match components[0] {
                    "applications" => {
                        self.add_application_permissions(
                            &components,
                            &method,
                            &mut required_permissions,
                        );
                    }
                    "blobs" => {
                        self.add_blob_permissions(&components, &method, &mut required_permissions);
                    }
                    "contexts" => {
                        self.add_context_permissions(
                            &components,
                            &method,
                            &mut required_permissions,
                        );
                    }
                    "keys" => {
                        self.add_key_permissions(&components, &method, &mut required_permissions);
                    }
                    _ => {}
                }
            }
        }

        required_permissions
    }

    /// Validate if user permissions satisfy required permissions
    pub fn validate_permissions(
        &self,
        user_permissions: &[String],
        required: &[Permission],
    ) -> bool {
        // First check for admin permission
        if user_permissions.iter().any(|p| p == "admin") {
            return true; // Admin has access to everything
        }

        // Convert string permissions to Permission enums
        let user_perms: Vec<Permission> = user_permissions
            .iter()
            .filter_map(|p| Permission::from_str(p))
            .collect();

        // Check if any user permission satisfies each required permission
        required
            .iter()
            .all(|req| user_perms.iter().any(|user_perm| user_perm.satisfies(req)))
    }

    // Helper methods to add specific types of permissions
    fn add_application_permissions(
        &self,
        components: &[&str],
        method: &HttpMethod,
        permissions: &mut Vec<Permission>,
    ) {
        use super::types::{ApplicationPermission, ResourceScope};

        match (method, components.get(1)) {
            (HttpMethod::GET, None) => {
                permissions.push(Permission::Application(ApplicationPermission::List(
                    ResourceScope::Global,
                )));
            }
            (HttpMethod::GET, Some(&id)) => {
                permissions.push(Permission::Application(ApplicationPermission::List(
                    ResourceScope::Specific(vec![id.to_string()]),
                )));
            }
            (HttpMethod::POST, _) => {
                permissions.push(Permission::Application(ApplicationPermission::Install(
                    ResourceScope::Global,
                )));
            }
            (HttpMethod::DELETE, Some(&id)) => {
                permissions.push(Permission::Application(ApplicationPermission::Uninstall(
                    ResourceScope::Specific(vec![id.to_string()]),
                )));
            }
            _ => {}
        }
    }

    fn add_blob_permissions(
        &self,
        components: &[&str],
        method: &HttpMethod,
        permissions: &mut Vec<Permission>,
    ) {
        use super::types::{AddBlobPermission, BlobPermission, ResourceScope};

        match (method, components.get(1)) {
            (HttpMethod::POST, Some(&"stream")) => {
                permissions.push(Permission::Blob(BlobPermission::Add(
                    AddBlobPermission::Stream,
                )));
            }
            (HttpMethod::POST, Some(&"file")) => {
                permissions.push(Permission::Blob(BlobPermission::Add(
                    AddBlobPermission::File,
                )));
            }
            (HttpMethod::POST, Some(&"url")) => {
                permissions.push(Permission::Blob(BlobPermission::Add(
                    AddBlobPermission::Url,
                )));
            }
            (HttpMethod::DELETE, Some(&id)) => {
                permissions.push(Permission::Blob(BlobPermission::Remove(
                    ResourceScope::Specific(vec![id.to_string()]),
                )));
            }
            _ => {}
        }
    }

    fn add_context_permissions(
        &self,
        components: &[&str],
        method: &HttpMethod,
        permissions: &mut Vec<Permission>,
    ) {
        use super::types::{ContextPermission, ResourceScope, UserScope};

        match (method, components.get(1), components.get(2)) {
            (HttpMethod::GET, None, None) => {
                permissions.push(Permission::Context(ContextPermission::List(
                    ResourceScope::Global,
                )));
            }
            (HttpMethod::GET, Some(&id), None) => {
                permissions.push(Permission::Context(ContextPermission::List(
                    ResourceScope::Specific(vec![id.to_string()]),
                )));
            }
            (HttpMethod::POST, None, None) => {
                permissions.push(Permission::Context(ContextPermission::Create(
                    ResourceScope::Global,
                )));
            }
            (HttpMethod::DELETE, Some(&id), None) => {
                permissions.push(Permission::Context(ContextPermission::Delete(
                    ResourceScope::Specific(vec![id.to_string()]),
                )));
            }
            (HttpMethod::POST, Some(&id), Some(&"leave")) => {
                permissions.push(Permission::Context(ContextPermission::Leave(
                    ResourceScope::Specific(vec![id.to_string()]),
                    UserScope::Any,
                )));
            }
            (HttpMethod::POST, Some(&id), Some(&"invite")) => {
                permissions.push(Permission::Context(ContextPermission::Invite(
                    ResourceScope::Specific(vec![id.to_string()]),
                    UserScope::Any,
                )));
            }
            (HttpMethod::POST, Some(&id), Some(&"execute")) => {
                permissions.push(Permission::Context(ContextPermission::Execute(
                    ResourceScope::Specific(vec![id.to_string()]),
                    UserScope::Any,
                    None,
                )));
            }
            _ => {}
        }
    }

    fn add_key_permissions(
        &self,
        components: &[&str],
        method: &HttpMethod,
        permissions: &mut Vec<Permission>,
    ) {
        use super::types::KeyPermission;

        match method {
            HttpMethod::GET => {
                permissions.push(Permission::Keys(KeyPermission::List));
            }
            HttpMethod::POST => {
                permissions.push(Permission::Keys(KeyPermission::Create));
            }
            HttpMethod::DELETE => {
                permissions.push(Permission::Keys(KeyPermission::Delete));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{Method, Request};

    use super::super::types::*;
    use super::*;

    #[test]
    fn test_determine_required_permissions() {
        let validator = PermissionValidator::new();

        // Test application list permission
        let req = Request::builder()
            .method(Method::GET)
            .uri("/applications")
            .body(Body::empty())
            .unwrap();
        let perms = validator.determine_required_permissions(&req);
        assert_eq!(perms.len(), 1);
        match &perms[0] {
            Permission::Application(ApplicationPermission::List(ResourceScope::Global)) => {}
            _ => panic!("Unexpected permission type"),
        }

        // Test admin API application permission
        let req = Request::builder()
            .method(Method::GET)
            .uri("/admin-api/applications/app1")
            .body(Body::empty())
            .unwrap();
        let perms = validator.determine_required_permissions(&req);
        assert_eq!(perms.len(), 1);
        match &perms[0] {
            Permission::Application(ApplicationPermission::List(ResourceScope::Specific(ids))) => {
                assert_eq!(ids.len(), 1);
                assert_eq!(ids[0], "app1");
            }
            _ => panic!("Unexpected permission type"),
        }

        // Test JSON-RPC execute permission
        let req = Request::builder()
            .method(Method::POST)
            .uri("/jsonrpc/contexts/ctx1/execute")
            .body(Body::empty())
            .unwrap();
        let perms = validator.determine_required_permissions(&req);
        assert_eq!(perms.len(), 1);
        match &perms[0] {
            Permission::Context(ContextPermission::Execute(scope, user, method)) => {
                assert!(matches!(scope, ResourceScope::Specific(ids) if ids[0] == "ctx1"));
                assert!(matches!(user, UserScope::Any));
                assert!(method.is_none());
            }
            _ => panic!("Unexpected permission type"),
        }
    }

    #[test]
    fn test_validate_permissions() {
        let validator = PermissionValidator::new();

        // Test master permission
        let user_perms = vec!["master".to_string()];
        let required = vec![Permission::Application(ApplicationPermission::List(
            ResourceScope::Global,
        ))];
        assert!(validator.validate_permissions(&user_perms, &required));

        // Test specific permission
        let user_perms = vec!["application:list[app1,app2]".to_string()];
        let required = vec![Permission::Application(ApplicationPermission::List(
            ResourceScope::Specific(vec!["app1".to_string()]),
        ))];
        assert!(validator.validate_permissions(&user_perms, &required));

        // Test permission denied
        let user_perms = vec!["blob:add".to_string()];
        let required = vec![Permission::Application(ApplicationPermission::List(
            ResourceScope::Global,
        ))];
        assert!(!validator.validate_permissions(&user_perms, &required));
    }

    #[test]
    fn test_blob_permissions() {
        let validator = PermissionValidator::new();

        // Test stream upload permission
        let req = Request::builder()
            .method(Method::POST)
            .uri("/blobs/stream")
            .body(Body::empty())
            .unwrap();
        let perms = validator.determine_required_permissions(&req);
        assert_eq!(perms.len(), 1);
        assert!(matches!(
            &perms[0],
            Permission::Blob(BlobPermission::Add(AddBlobPermission::Stream))
        ));

        // Test file upload permission
        let req = Request::builder()
            .method(Method::POST)
            .uri("/blobs/file")
            .body(Body::empty())
            .unwrap();
        let perms = validator.determine_required_permissions(&req);
        assert_eq!(perms.len(), 1);
        assert!(matches!(
            &perms[0],
            Permission::Blob(BlobPermission::Add(AddBlobPermission::File))
        ));

        // Test URL upload permission
        let req = Request::builder()
            .method(Method::POST)
            .uri("/blobs/url")
            .body(Body::empty())
            .unwrap();
        let perms = validator.determine_required_permissions(&req);
        assert_eq!(perms.len(), 1);
        assert!(matches!(
            &perms[0],
            Permission::Blob(BlobPermission::Add(AddBlobPermission::Url))
        ));

        // Test blob deletion permission
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/blobs/123")
            .body(Body::empty())
            .unwrap();
        let perms = validator.determine_required_permissions(&req);
        assert_eq!(perms.len(), 1);
        match &perms[0] {
            Permission::Blob(BlobPermission::Remove(ResourceScope::Specific(ids))) => {
                assert_eq!(ids.len(), 1);
                assert_eq!(ids[0], "123");
            }
            _ => panic!("Unexpected permission type"),
        }

        // Test permission validation
        let validator = PermissionValidator::new();

        // Test that global add permission satisfies specific add permission
        let held = vec!["blob:add".to_string()];
        let required = vec![Permission::Blob(BlobPermission::Add(
            AddBlobPermission::Stream,
        ))];
        assert!(validator.validate_permissions(&held, &required));

        // Test that specific add permission satisfies only that type
        let held = vec!["blob:add:stream".to_string()];
        let required = vec![Permission::Blob(BlobPermission::Add(
            AddBlobPermission::Stream,
        ))];
        assert!(validator.validate_permissions(&held, &required));

        let required = vec![Permission::Blob(BlobPermission::Add(
            AddBlobPermission::File,
        ))];
        assert!(!validator.validate_permissions(&held, &required));

        // Test that global blob permission satisfies everything
        let held = vec!["blob".to_string()];
        let required = vec![
            Permission::Blob(BlobPermission::Add(AddBlobPermission::Stream)),
            Permission::Blob(BlobPermission::Remove(ResourceScope::Specific(vec![
                "123".to_string()
            ]))),
        ];
        assert!(validator.validate_permissions(&held, &required));
    }
}
