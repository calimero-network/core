use std::fmt;

use serde::{Deserialize, Serialize};

/// Represents a resource identifier that can be either global (*) or specific
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceScope {
    Global,
    Specific(Vec<String>),
}

impl fmt::Display for ResourceScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResourceScope::Global => write!(f, "*"),
            ResourceScope::Specific(ids) => write!(f, "[{}]", ids.join(",")),
        }
    }
}

/// Represents an HTTP method
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpMethod {
    GET,
    POST,
    PUT,
    DELETE,
    PATCH,
    Any,
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HttpMethod::Any => write!(f, "*"),
            _ => write!(f, "{:?}", self),
        }
    }
}

/// Represents a user scope that can be either any user or a specific user
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserScope {
    Any,
    Specific(String),
}

impl fmt::Display for UserScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UserScope::Any => write!(f, "*"),
            UserScope::Specific(user_id) => write!(f, "<{}>", user_id),
        }
    }
}

/// Master permission that grants all access
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminPermission;

/// Application-related permissions
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApplicationPermission {
    All,
    List(ResourceScope),
    Install(ResourceScope),
    Uninstall(ResourceScope),
}

/// Blob-related permissions
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlobPermission {
    All,
    Add(AddBlobPermission),
    Remove(ResourceScope),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AddBlobPermission {
    All,
    Stream,
    File,
    Url,
}

/// Context alias permissions
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AliasPermission {
    Create,
    Delete,
    Lookup {
        context_id: Option<String>,
        user_id: Option<String>,
    },
}

/// Context-related permissions
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextPermission {
    All(ResourceScope),
    Create(ResourceScope),
    List(ResourceScope),
    Delete(ResourceScope),
    Leave(ResourceScope, UserScope),
    Invite(ResourceScope, UserScope),
    Execute(ResourceScope, UserScope, Option<String>),
    Alias(AliasPermission),
}

/// Represents all possible permissions in the system
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Permission {
    Admin(AdminPermission),
    Application(ApplicationPermission),
    Blob(BlobPermission),
    Context(ContextPermission),
    Keys(KeyPermission),
}

/// Key-related permissions
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyPermission {
    All,
    Create,
    List,
    Delete,
}

impl Permission {
    /// Convert a permission string to a Permission enum
    pub fn from_str(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split(&[':', '[', ']', '<', '>']).collect();

        match parts.get(0)? {
            &"admin" => Some(Permission::Admin(AdminPermission)),
            &"application" => {
                let scope = if let Some(ids) = parts.get(1) {
                    ResourceScope::Specific(ids.split(',').map(String::from).collect())
                } else {
                    ResourceScope::Global
                };

                match parts.get(1).map(|&s| s) {
                    Some("list") => {
                        Some(Permission::Application(ApplicationPermission::List(scope)))
                    }
                    Some("install") => Some(Permission::Application(
                        ApplicationPermission::Install(scope),
                    )),
                    Some("uninstall") => Some(Permission::Application(
                        ApplicationPermission::Uninstall(scope),
                    )),
                    _ => Some(Permission::Application(ApplicationPermission::All)),
                }
            }
            &"blob" => match parts.get(1).map(|&s| s) {
                Some("add") => {
                    let add_perm = match parts.get(2).map(|&s| s) {
                        Some("stream") => AddBlobPermission::Stream,
                        Some("file") => AddBlobPermission::File,
                        Some("url") => AddBlobPermission::Url,
                        _ => AddBlobPermission::All,
                    };
                    Some(Permission::Blob(BlobPermission::Add(add_perm)))
                }
                Some("remove") => {
                    let scope = if let Some(ids) = parts.get(2) {
                        ResourceScope::Specific(ids.split(',').map(String::from).collect())
                    } else {
                        ResourceScope::Global
                    };
                    Some(Permission::Blob(BlobPermission::Remove(scope)))
                }
                _ => Some(Permission::Blob(BlobPermission::All)),
            },
            &"context" => {
                match (parts.get(1), parts.get(2)) {
                    // Handle context:alias:* permissions
                    (Some(&"alias"), Some(&action)) => match action {
                        "create" => Some(Permission::Context(ContextPermission::Alias(
                            AliasPermission::Create,
                        ))),
                        "delete" => Some(Permission::Context(ContextPermission::Alias(
                            AliasPermission::Delete,
                        ))),
                        "lookup" => {
                            let context_id = parts.get(3).map(|s| s.to_string());
                            let user_id = parts.get(4).map(|s| s.to_string());
                            Some(Permission::Context(ContextPermission::Alias(
                                AliasPermission::Lookup {
                                    context_id,
                                    user_id,
                                },
                            )))
                        }
                        _ => None,
                    },
                    // Handle other context permissions
                    _ => {
                        let scope = if let Some(ids) = parts.get(2) {
                            ResourceScope::Specific(ids.split(',').map(String::from).collect())
                        } else {
                            ResourceScope::Global
                        };

                        let user_scope = if let Some(user_id) = parts.get(3) {
                            UserScope::Specific(user_id.to_string())
                        } else {
                            UserScope::Any
                        };

                        match parts.get(1).map(|&s| s) {
                            Some("create") => {
                                Some(Permission::Context(ContextPermission::Create(scope)))
                            }
                            Some("list") => {
                                Some(Permission::Context(ContextPermission::List(scope)))
                            }
                            Some("delete") => {
                                Some(Permission::Context(ContextPermission::Delete(scope)))
                            }
                            Some("leave") => Some(Permission::Context(ContextPermission::Leave(
                                scope, user_scope,
                            ))),
                            Some("invite") => Some(Permission::Context(ContextPermission::Invite(
                                scope, user_scope,
                            ))),
                            Some("execute") => {
                                let method = parts.get(4).map(|s| s.to_string());
                                Some(Permission::Context(ContextPermission::Execute(
                                    scope, user_scope, method,
                                )))
                            }
                            _ => Some(Permission::Context(ContextPermission::All(scope))),
                        }
                    }
                }
            }
            &"keys" => match parts.get(1).map(|&s| s) {
                Some("create") => Some(Permission::Keys(KeyPermission::Create)),
                Some("list") => Some(Permission::Keys(KeyPermission::List)),
                Some("delete") => Some(Permission::Keys(KeyPermission::Delete)),
                _ => Some(Permission::Keys(KeyPermission::All)),
            },
            _ => None,
        }
    }

    /// Convert a Permission enum to its string representation
    pub fn to_string(&self) -> String {
        match self {
            Permission::Admin(_) => "admin".to_string(),
            Permission::Application(app_perm) => match app_perm {
                ApplicationPermission::All => "application".to_string(),
                ApplicationPermission::List(scope) => format!("application:list{}", scope),
                ApplicationPermission::Install(scope) => format!("application:install{}", scope),
                ApplicationPermission::Uninstall(scope) => {
                    format!("application:uninstall{}", scope)
                }
            },
            Permission::Blob(blob_perm) => match blob_perm {
                BlobPermission::All => "blob".to_string(),
                BlobPermission::Add(add_perm) => match add_perm {
                    AddBlobPermission::All => "blob:add".to_string(),
                    AddBlobPermission::Stream => "blob:add:stream".to_string(),
                    AddBlobPermission::File => "blob:add:file".to_string(),
                    AddBlobPermission::Url => "blob:add:url".to_string(),
                },
                BlobPermission::Remove(scope) => format!("blob:remove{}", scope),
            },
            Permission::Context(ctx_perm) => match ctx_perm {
                ContextPermission::All(scope) => format!("context{}", scope),
                ContextPermission::Create(scope) => format!("context:create{}", scope),
                ContextPermission::List(scope) => format!("context:list{}", scope),
                ContextPermission::Delete(scope) => format!("context:delete{}", scope),
                ContextPermission::Leave(scope, user) => format!("context:leave{}{}", scope, user),
                ContextPermission::Invite(scope, user) => {
                    format!("context:invite{}{}", scope, user)
                }
                ContextPermission::Execute(scope, user, method) => {
                    if let Some(m) = method {
                        format!("context:execute{}{}{}", scope, user, m)
                    } else {
                        format!("context:execute{}{}", scope, user)
                    }
                }
                ContextPermission::Alias(alias_perm) => match alias_perm {
                    AliasPermission::Create => "context:alias:create".to_string(),
                    AliasPermission::Delete => "context:alias:delete".to_string(),
                    AliasPermission::Lookup {
                        context_id,
                        user_id,
                    } => {
                        let mut s = "context:alias:lookup".to_string();
                        if let Some(ctx_id) = context_id {
                            s.push_str(&format!("[{}]", ctx_id));
                            if let Some(uid) = user_id {
                                s.push_str(&format!("[{}]", uid));
                            }
                        }
                        s
                    }
                },
            },
            Permission::Keys(key_perm) => match key_perm {
                KeyPermission::All => "keys".to_string(),
                KeyPermission::Create => "keys:create".to_string(),
                KeyPermission::List => "keys:list".to_string(),
                KeyPermission::Delete => "keys:delete".to_string(),
            },
        }
    }

    /// Check if this permission satisfies the required permission
    pub fn satisfies(&self, required: &Permission) -> bool {
        match (self, required) {
            // Master permission satisfies everything
            (Permission::Admin(_), _) => true,

            // Application permissions
            (Permission::Application(ApplicationPermission::All), Permission::Application(_)) => {
                true
            }
            (Permission::Application(held), Permission::Application(req)) => match (held, req) {
                (ApplicationPermission::List(h_scope), ApplicationPermission::List(r_scope)) => {
                    matches_scope(h_scope, r_scope)
                }
                (
                    ApplicationPermission::Install(h_scope),
                    ApplicationPermission::Install(r_scope),
                ) => matches_scope(h_scope, r_scope),
                (
                    ApplicationPermission::Uninstall(h_scope),
                    ApplicationPermission::Uninstall(r_scope),
                ) => matches_scope(h_scope, r_scope),
                _ => false,
            },

            // Blob permissions
            (Permission::Blob(BlobPermission::All), Permission::Blob(_)) => true,
            (Permission::Blob(held), Permission::Blob(req)) => match (held, req) {
                (BlobPermission::Add(h_add), BlobPermission::Add(r_add)) => {
                    matches!(h_add, AddBlobPermission::All) || h_add == r_add
                }
                (BlobPermission::Remove(h_scope), BlobPermission::Remove(r_scope)) => {
                    matches_scope(h_scope, r_scope)
                }
                _ => false,
            },

            // Context permissions
            (Permission::Context(ContextPermission::All(h_scope)), Permission::Context(req)) => {
                matches!(req, ContextPermission::All(_))
                    && matches_scope(
                        h_scope,
                        match req {
                            ContextPermission::All(scope) => scope,
                            _ => return false,
                        },
                    )
            }
            (Permission::Context(held), Permission::Context(req)) => match (held, req) {
                (ContextPermission::Create(h_scope), ContextPermission::Create(r_scope)) => {
                    matches_scope(h_scope, r_scope)
                }
                (ContextPermission::List(h_scope), ContextPermission::List(r_scope)) => {
                    matches_scope(h_scope, r_scope)
                }
                (ContextPermission::Delete(h_scope), ContextPermission::Delete(r_scope)) => {
                    matches_scope(h_scope, r_scope)
                }
                (
                    ContextPermission::Leave(h_scope, h_user),
                    ContextPermission::Leave(r_scope, r_user),
                ) => matches_scope(h_scope, r_scope) && matches_user_scope(h_user, r_user),
                (
                    ContextPermission::Invite(h_scope, h_user),
                    ContextPermission::Invite(r_scope, r_user),
                ) => matches_scope(h_scope, r_scope) && matches_user_scope(h_user, r_user),
                (
                    ContextPermission::Execute(h_scope, h_user, h_method),
                    ContextPermission::Execute(r_scope, r_user, r_method),
                ) => {
                    matches_scope(h_scope, r_scope)
                        && matches_user_scope(h_user, r_user)
                        && matches_method(h_method.as_deref(), r_method.as_deref())
                }
                (ContextPermission::Alias(held), ContextPermission::Alias(required)) => {
                    matches_alias(held, required)
                }
                _ => false,
            },

            // Key permissions
            (Permission::Keys(KeyPermission::All), Permission::Keys(_)) => true,
            (Permission::Keys(held), Permission::Keys(req)) => held == req,

            // Different permission types don't satisfy each other
            _ => false,
        }
    }
}

/// Helper function to check if one scope satisfies another
fn matches_scope(held: &ResourceScope, required: &ResourceScope) -> bool {
    match (held, required) {
        (ResourceScope::Global, _) => true,
        (ResourceScope::Specific(h_ids), ResourceScope::Specific(r_ids)) => {
            r_ids.iter().all(|r_id| h_ids.contains(r_id))
        }
        _ => false,
    }
}

/// Helper function to check if one user scope satisfies another
fn matches_user_scope(held: &UserScope, required: &UserScope) -> bool {
    match (held, required) {
        (UserScope::Any, _) => true,
        (UserScope::Specific(h_id), UserScope::Specific(r_id)) => h_id == r_id,
        _ => false,
    }
}

/// Helper function to check if one method matches another
fn matches_method(held: Option<&str>, required: Option<&str>) -> bool {
    match (held, required) {
        (None, _) => true,
        (Some(_), None) => true,
        (Some(h), Some(r)) => h == r,
        _ => false,
    }
}

/// Helper function to check if one alias permission satisfies another
fn matches_alias(held: &AliasPermission, required: &AliasPermission) -> bool {
    match (held, required) {
        (AliasPermission::Create, AliasPermission::Create) => true,
        (AliasPermission::Delete, AliasPermission::Delete) => true,
        (
            AliasPermission::Lookup {
                context_id: held_ctx,
                user_id: held_user,
            },
            AliasPermission::Lookup {
                context_id: req_ctx,
                user_id: req_user,
            },
        ) => {
            let ctx_match = match (held_ctx, req_ctx) {
                (None, _) => true,
                (Some(_), None) => false,
                (Some(h), Some(r)) => h == r,
            };

            let user_match = match (held_user, req_user) {
                (None, _) => true,
                (Some(_), None) => false,
                (Some(h), Some(r)) => h == r,
            };

            ctx_match && user_match
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_parsing() {
        // Test master permission
        assert_eq!(
            Permission::from_str("admin"),
            Some(Permission::Admin(AdminPermission))
        );

        // Test application permissions
        assert_eq!(
            Permission::from_str("application:list[app1,app2]"),
            Some(Permission::Application(ApplicationPermission::List(
                ResourceScope::Specific(vec!["app1".to_string(), "app2".to_string()])
            )))
        );

        // Test context permissions
        assert_eq!(
            Permission::from_str("context:execute[ctx1]<user1>method1"),
            Some(Permission::Context(ContextPermission::Execute(
                ResourceScope::Specific(vec!["ctx1".to_string()]),
                UserScope::Specific("user1".to_string()),
                Some("method1".to_string())
            )))
        );
    }

    #[test]
    fn test_permission_satisfaction() {
        // Test master permission
        let master = Permission::Admin(AdminPermission);
        let app_list = Permission::Application(ApplicationPermission::List(ResourceScope::Global));
        assert!(master.satisfies(&app_list));

        // Test specific application permission
        let app_list_specific =
            Permission::Application(ApplicationPermission::List(ResourceScope::Specific(vec![
                "app1".to_string(),
                "app2".to_string(),
            ])));
        let app_list_required =
            Permission::Application(ApplicationPermission::List(ResourceScope::Specific(vec![
                "app1".to_string(),
            ])));
        assert!(app_list_specific.satisfies(&app_list_required));
        assert!(!app_list_required.satisfies(&app_list_specific));
    }

    #[test]
    fn test_alias_permission_parsing() {
        // Test create permission
        let perm = Permission::from_str("context:alias:create").unwrap();
        assert!(matches!(
            perm,
            Permission::Context(ContextPermission::Alias(AliasPermission::Create))
        ));

        // Test delete permission
        let perm = Permission::from_str("context:alias:delete").unwrap();
        assert!(matches!(
            perm,
            Permission::Context(ContextPermission::Alias(AliasPermission::Delete))
        ));

        // Test lookup permission with context
        let perm = Permission::from_str("context:alias:lookup[ctx-123]").unwrap();
        match perm {
            Permission::Context(ContextPermission::Alias(AliasPermission::Lookup {
                context_id,
                user_id,
            })) => {
                assert_eq!(context_id.unwrap(), "ctx-123");
                assert!(user_id.is_none());
            }
            _ => panic!("Wrong permission type"),
        }

        // Test lookup permission with context and user
        let perm = Permission::from_str("context:alias:lookup[ctx-123,user-456]").unwrap();
        match perm {
            Permission::Context(ContextPermission::Alias(AliasPermission::Lookup {
                context_id,
                user_id,
            })) => {
                assert_eq!(context_id.unwrap(), "ctx-123");
                assert_eq!(user_id.unwrap(), "user-456");
            }
            _ => panic!("Wrong permission type"),
        }
    }

    #[test]
    fn test_alias_permission_satisfaction() {
        // Test create permission
        let held = Permission::from_str("context:alias:create").unwrap();
        let required = Permission::from_str("context:alias:create").unwrap();
        assert!(held.satisfies(&required));

        // Test delete permission
        let held = Permission::from_str("context:alias:delete").unwrap();
        let required = Permission::from_str("context:alias:delete").unwrap();
        assert!(held.satisfies(&required));

        // Test lookup permission - global access
        let held = Permission::from_str("context:alias:lookup").unwrap();
        let required = Permission::from_str("context:alias:lookup[ctx-123]").unwrap();
        assert!(held.satisfies(&required));

        // Test lookup permission - specific context
        let held = Permission::from_str("context:alias:lookup[ctx-123]").unwrap();
        let required = Permission::from_str("context:alias:lookup[ctx-123]").unwrap();
        assert!(held.satisfies(&required));

        // Test lookup permission - specific context and user
        let held = Permission::from_str("context:alias:lookup[ctx-123,user-456]").unwrap();
        let required = Permission::from_str("context:alias:lookup[ctx-123,user-456]").unwrap();
        assert!(held.satisfies(&required));

        // Test lookup permission - global context but specific user (should fail)
        let held = Permission::from_str("context:alias:lookup[ctx-123]").unwrap();
        let required = Permission::from_str("context:alias:lookup[ctx-456]").unwrap();
        assert!(!held.satisfies(&required));

        // Test master permission satisfies all
        let held = Permission::from_str("master").unwrap();
        let required = Permission::from_str("context:alias:lookup[ctx-123,user-456]").unwrap();
        assert!(held.satisfies(&required));
    }
}
