use std::sync::LazyLock;
use axum::body::Body;
use axum::http::Request;
use regex::Regex;

use super::types::{
    AddBlobPermission, ApplicationPermission, BlobPermission, 
    CapabilityPermission, ContextApplicationPermission, ContextPermission,
    HttpMethod, KeyPermission, Permission, ResourceScope, UserScope,
};

/// Pre-compiled regex patterns for performance
static APPLICATION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/admin-api/applications/([^/]+)$").unwrap()
});

static CONTEXT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/admin-api/contexts/([^/]+)$").unwrap()
});

static CONTEXT_CAPABILITIES_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/admin-api/contexts/([^/]+)/capabilities/(grant|revoke)$").unwrap()
});

static CONTEXT_APPLICATION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/admin-api/contexts/([^/]+)/application$").unwrap()
});

static BLOB_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/blobs/([^/]+)$").unwrap()
});

static ADMIN_KEY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/admin/keys/([^/]+)$").unwrap()
});

static KEY_PERMISSIONS_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/admin/keys/([^/]+)/permissions$").unwrap()
});

static CLIENT_MANAGEMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/admin/keys/([^/]+)/clients/([^/]+)$").unwrap()
});

/// Permission validator for checking request permissions
#[derive(Debug, Default)]
pub struct PermissionValidator;

macro_rules! route_permissions {
    ($path:expr, $method:expr) => {{
        match ($path, $method) {
            // JSON-RPC endpoints
            ("/jsonrpc", HttpMethod::POST) => vec![Permission::Context(ContextPermission::Execute(
                ResourceScope::Global, UserScope::Any, None
            ))],
            
            // Admin API - Applications
            ("/admin-api/applications", HttpMethod::GET) => vec![Permission::Application(ApplicationPermission::List(ResourceScope::Global))],
            ("/admin-api/install-application", HttpMethod::POST) => vec![Permission::Application(ApplicationPermission::Install(ResourceScope::Global))],
            ("/admin-api/uninstall-application", HttpMethod::POST) => vec![Permission::Application(ApplicationPermission::Uninstall(ResourceScope::Global))],
            
            // Admin API - Contexts  
            ("/admin-api/contexts", HttpMethod::GET) => vec![Permission::Context(ContextPermission::List(ResourceScope::Global))],
            ("/admin-api/contexts", HttpMethod::POST) => vec![Permission::Context(ContextPermission::Create(ResourceScope::Global))],
            
            // Admin auth endpoints
            ("/admin/keys", HttpMethod::GET) => vec![Permission::Keys(KeyPermission::List)],
            ("/admin/keys", HttpMethod::POST) => vec![Permission::Keys(KeyPermission::Create)],
            ("/admin/revoke", HttpMethod::POST) => vec![Permission::Keys(KeyPermission::Delete)],
            ("/admin/keys/clients", HttpMethod::GET) => vec![Permission::Keys(KeyPermission::ListClients)],
            
            // Blob endpoints
            ("/blobs/stream", HttpMethod::POST) => vec![Permission::Blob(BlobPermission::Add(AddBlobPermission::Stream))],
            ("/blobs/file", HttpMethod::POST) => vec![Permission::Blob(BlobPermission::Add(AddBlobPermission::File))],
            ("/blobs/url", HttpMethod::POST) => vec![Permission::Blob(BlobPermission::Add(AddBlobPermission::Url))],
            
            _ => vec![]
        }
    }};
}

fn get_permissions_for_path_with_params(path: &str, method: &HttpMethod) -> Vec<Permission> {
    // Handle parameterized routes with pre-compiled regex patterns
    if let Some(captures) = APPLICATION_REGEX.captures(path) {
        if let Some(app_id) = captures.get(1) {
            return match method {
                HttpMethod::GET => vec![Permission::Application(ApplicationPermission::List(
                    ResourceScope::Specific(vec![app_id.as_str().to_string()])
                ))],
                _ => vec![],
            };
        }
    }
    
    if let Some(captures) = CONTEXT_REGEX.captures(path) {
        if let Some(ctx_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![ctx_id.as_str().to_string()]);
            return match method {
                HttpMethod::GET => vec![Permission::Context(ContextPermission::List(scope))],
                HttpMethod::DELETE => vec![Permission::Context(ContextPermission::Delete(scope))],
                _ => vec![],
            };
        }
    }
    
    if let Some(captures) = CONTEXT_CAPABILITIES_REGEX.captures(path) {
        if let (Some(ctx_id), Some(action)) = (captures.get(1), captures.get(2)) {
            let scope = ResourceScope::Specific(vec![ctx_id.as_str().to_string()]);
            return match (method, action.as_str()) {
                (HttpMethod::POST, "grant") => vec![Permission::Context(ContextPermission::Capabilities(CapabilityPermission::Grant(scope)))],
                (HttpMethod::POST, "revoke") => vec![Permission::Context(ContextPermission::Capabilities(CapabilityPermission::Revoke(scope)))],
                _ => vec![],
            };
        }
    }
    
    if let Some(captures) = CONTEXT_APPLICATION_REGEX.captures(path) {
        if let Some(ctx_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![ctx_id.as_str().to_string()]);
            return match method {
                HttpMethod::POST => vec![Permission::Context(ContextPermission::Application(ContextApplicationPermission::Update(scope)))],
                _ => vec![],
            };
        }
    }
    
    if let Some(captures) = BLOB_REGEX.captures(path) {
        if let Some(blob_id) = captures.get(1) {
            return match method {
                HttpMethod::DELETE => vec![Permission::Blob(BlobPermission::Remove(
                    ResourceScope::Specific(vec![blob_id.as_str().to_string()])
                ))],
                _ => vec![],
            };
        }
    }
    
    // Admin key management endpoints
    if let Some(_captures) = ADMIN_KEY_REGEX.captures(path) {
        return match method {
            HttpMethod::DELETE => vec![Permission::Keys(KeyPermission::Delete)],
            _ => vec![],
        };
    }
    
    // Key permissions management: /admin/keys/:key_id/permissions
    if let Some(captures) = KEY_PERMISSIONS_REGEX.captures(path) {
        if let Some(key_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![key_id.as_str().to_string()]);
            return match method {
                HttpMethod::GET => vec![Permission::Keys(KeyPermission::GetPermissions(scope))],
                HttpMethod::PUT => vec![Permission::Keys(KeyPermission::UpdatePermissions(scope))],
                _ => vec![],
            };
        }
    }
    
    // Client management: /admin/keys/:key_id/clients/:client_id
    if let Some(_captures) = CLIENT_MANAGEMENT_REGEX.captures(path) {
        return match method {
            HttpMethod::DELETE => vec![Permission::Keys(KeyPermission::DeleteClient)],
            _ => vec![],
        };
    }
    
    vec![]
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

        // First check exact path matches
        required_permissions.extend(route_permissions!(path, method.clone()));
        
        // Then check parameterized paths
        required_permissions.extend(get_permissions_for_path_with_params(path, &method));

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

    /// Validate that a user has all required permissions
    pub fn validate_permissions(&self, user_permissions: &[String], required_permissions: &[Permission]) -> bool {
        // First check for admin permission
        if user_permissions.iter().any(|p| p == "admin") {
            return true; // Admin has access to everything
        }

        // Convert string permissions to Permission enums for hierarchical checking
        let user_perms: Vec<Permission> = user_permissions
            .iter()
            .filter_map(|p| p.parse::<Permission>().ok())
            .collect();

        // Check if any user permission satisfies each required permission
        required_permissions
            .iter()
            .all(|req| {
                // Check exact string match first (for simple cases)
                if user_permissions.contains(&req.to_string()) {
                    return true;
                }
                
                // Check hierarchical permissions using satisfies method
                user_perms.iter().any(|user_perm| user_perm.satisfies(req))
            })
    }

    /// Check if a user has a specific permission (string format)
    pub fn has_permission(&self, user_permissions: &[String], permission: &str) -> bool {
        user_permissions.contains(&permission.to_string())
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
        _components: &[&str],
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

        // Test admin API application list permission
        let req = Request::builder()
            .method(Method::GET)
            .uri("/admin-api/applications")
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

        // Test JSON-RPC execute permission (now path-based)
        let req = Request::builder()
            .method(Method::POST)
            .uri("/jsonrpc")
            .body(Body::empty())
            .unwrap();
        let perms = validator.determine_required_permissions(&req);
        // JSON-RPC now requires context execute permission
        assert_eq!(perms.len(), 1);
    }

    #[test]
    fn test_validate_permissions() {
        let validator = PermissionValidator::new();

        // Test admin permission
        let user_perms = vec!["admin".to_string()];
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

    #[test]
    fn test_jsonrpc_permissions() {
        let validator = PermissionValidator::new();

        // Test JSON-RPC execute endpoint
        let request = Request::builder()
            .method(Method::POST)
            .uri("/jsonrpc")
            .body(axum::body::Body::empty())
            .unwrap();

        let permissions = validator.determine_required_permissions(&request);
        assert_eq!(permissions.len(), 1);
        
        match &permissions[0] {
            Permission::Context(ContextPermission::Execute(scope, user_scope, method)) => {
                assert_eq!(*scope, ResourceScope::Global);
                assert_eq!(*user_scope, UserScope::Any);
                assert_eq!(*method, None);
            }
            _ => panic!("Expected Context Execute permission"),
        }

        // Test permission validation
        let user_permissions = vec!["context:execute".to_string()];
        assert!(validator.validate_permissions(&user_permissions, &permissions));

        let insufficient_permissions = vec!["context:list".to_string()];
        assert!(!validator.validate_permissions(&insufficient_permissions, &permissions));
    }

    #[test]
    fn test_pre_compiled_regex_performance() {
        let validator = PermissionValidator::new();
        
        // Test various parameterized routes to ensure regex patterns work correctly
        let test_routes = vec![
            ("/admin-api/applications/app-123", Method::GET),
            ("/admin-api/contexts/ctx-456", Method::GET),
            ("/admin-api/contexts/ctx-789/capabilities/grant", Method::POST),
            ("/admin-api/contexts/ctx-101/application", Method::POST),
            ("/blobs/blob-123", Method::DELETE),
            ("/admin/keys/key-456", Method::DELETE),
            ("/admin/keys/key-789/permissions", Method::GET),
            ("/admin/keys/key-101/clients/client-123", Method::DELETE),
        ];

        // This test ensures that pre-compiled regexes work correctly
        // In a real performance test, you would measure the time difference
        for (path, method) in test_routes {
            let req = Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .unwrap();
            
            let perms = validator.determine_required_permissions(&req);
            // Each parameterized route should return exactly one permission
            assert!(!perms.is_empty(), "No permissions found for {}", path);
        }
        
        // Note: The performance benefit comes from:
        // 1. LazyLock ensures regexes are compiled only once per pattern
        // 2. No repeated Regex::new() calls on each request
        // 3. Patterns are pre-optimized and cached in memory
        println!("✅ Pre-compiled regex patterns working correctly");
    }
}
