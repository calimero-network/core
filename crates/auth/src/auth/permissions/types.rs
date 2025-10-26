use std::fmt;
use std::str::FromStr;

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
            _ => write!(f, "{self:?}"),
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
            UserScope::Specific(user_id) => write!(f, "<{user_id}>"),
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
    Create(AliasType, ResourceScope),
    List(AliasType, ResourceScope),
    Lookup(AliasType, ResourceScope),
    Delete(AliasType, ResourceScope),
}

/// Types of aliases that can be managed
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AliasType {
    Context,
    Application,
    Identity,
}

/// Context capability permissions
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapabilityPermission {
    Grant(ResourceScope),
    Revoke(ResourceScope),
}

/// Context application management permissions
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextApplicationPermission {
    Update(ResourceScope),
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
    Capabilities(CapabilityPermission),
    Application(ContextApplicationPermission),
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
    Delete, // Covers both key deletion and token revocation
    // Client management
    ListClients,
    DeleteClient,
    // Permission management
    GetPermissions(ResourceScope),
    UpdatePermissions(ResourceScope),
}

/// Parse permission parameters from brackets: [context_id,context_identity,method]
/// Returns (context_ids, user_id, method)
fn parse_permission_params(s: &str) -> (ResourceScope, UserScope, Option<String>) {
    // Find bracket content
    if let Some(start) = s.find('[') {
        if let Some(end) = s.find(']') {
            if start < end {
                let params_str = &s[start + 1..end];
                if params_str.is_empty() {
                    return (ResourceScope::Global, UserScope::Any, None);
                }

                let params: Vec<&str> = params_str.split(',').collect();

                // Parse context_ids (first parameter)
                let context_scope = if let Some(&context_param) = params.first() {
                    let context_param = context_param.trim();
                    if context_param.is_empty() {
                        ResourceScope::Global
                    } else {
                        ResourceScope::Specific(vec![context_param.to_string()])
                    }
                } else {
                    ResourceScope::Global
                };

                // Parse user_id (second parameter)
                let user_scope = if let Some(&user_param) = params.get(1) {
                    let user_param = user_param.trim();
                    if user_param.is_empty() {
                        UserScope::Any
                    } else {
                        UserScope::Specific(user_param.to_string())
                    }
                } else {
                    UserScope::Any
                };

                // Parse method (third parameter)
                let method = if let Some(&method_param) = params.get(2) {
                    let method_param = method_param.trim();
                    if method_param.is_empty() {
                        None
                    } else {
                        Some(method_param.to_string())
                    }
                } else {
                    None
                };

                return (context_scope, user_scope, method);
            }
        }
    }

    (ResourceScope::Global, UserScope::Any, None)
}

/// Helper function to format permission parameters
fn format_params(scope: &ResourceScope, user: &UserScope, method: &Option<String>) -> String {
    let mut params = Vec::new();

    // Add context scope
    match scope {
        ResourceScope::Global => {} // Don't add empty context
        ResourceScope::Specific(ids) => {
            if !ids.is_empty() {
                params.push(ids[0].clone()); // Only first context ID for now
            }
        }
    }

    // Add user scope
    match user {
        UserScope::Any => {
            if !params.is_empty() {
                params.push(String::new()); // Empty user param
            }
        }
        UserScope::Specific(user_id) => {
            // Ensure we have at least context param (empty if needed)
            if params.is_empty() {
                params.push(String::new());
            }
            params.push(user_id.clone());
        }
    }

    // Add method
    if let Some(method_name) = method {
        // Ensure we have context and user params
        while params.len() < 2 {
            params.push(String::new());
        }
        params.push(method_name.clone());
    }

    if params.is_empty() || params.iter().all(|p| p.is_empty()) {
        String::new()
    } else {
        format!("[{}]", params.join(","))
    }
}

/// Helper function to format simple scope parameters (only for alias and single-scope permissions)
fn format_simple_params(scope: &ResourceScope) -> String {
    match scope {
        ResourceScope::Global => String::new(),
        ResourceScope::Specific(ids) => {
            if ids.is_empty() {
                String::new()
            } else {
                format!("[{}]", ids[0])
            }
        }
    }
}

impl FromStr for Permission {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err("Empty permission string".to_string());
        }

        // Split into category:action and parameters
        let (main_part, params_part) = if let Some(bracket_pos) = s.find('[') {
            (&s[..bracket_pos], &s[bracket_pos..])
        } else {
            (s, "")
        };

        let parts: Vec<&str> = main_part.split(':').collect();
        let category = parts.first().ok_or("Empty permission string")?;

        match *category {
            "admin" => Ok(Permission::Admin(AdminPermission)),

            "application" => {
                let action = parts.get(1).unwrap_or(&"");
                let (scope, _, _) = parse_permission_params(params_part);

                match *action {
                    "list" => Ok(Permission::Application(ApplicationPermission::List(scope))),
                    "install" => Ok(Permission::Application(ApplicationPermission::Install(
                        scope,
                    ))),
                    "uninstall" => Ok(Permission::Application(ApplicationPermission::Uninstall(
                        scope,
                    ))),
                    "" => Ok(Permission::Application(ApplicationPermission::All)),
                    _ => Err(format!("Unknown application action: {action}")),
                }
            }

            "blob" => {
                let action = parts.get(1).unwrap_or(&"");
                let subaction = parts.get(2).unwrap_or(&"");

                match *action {
                    "add" => {
                        let add_perm = match *subaction {
                            "stream" => AddBlobPermission::Stream,
                            "file" => AddBlobPermission::File,
                            "url" => AddBlobPermission::Url,
                            "" => AddBlobPermission::All,
                            _ => return Err(format!("Unknown blob add action: {subaction}")),
                        };
                        Ok(Permission::Blob(BlobPermission::Add(add_perm)))
                    }
                    "remove" => {
                        let (scope, _, _) = parse_permission_params(params_part);
                        Ok(Permission::Blob(BlobPermission::Remove(scope)))
                    }
                    "" => Ok(Permission::Blob(BlobPermission::All)),
                    _ => Err(format!("Unknown blob action: {action}")),
                }
            }

            "context" => {
                let action = parts.get(1).unwrap_or(&"");
                let subaction = parts.get(2).unwrap_or(&"");
                let subsubaction = parts.get(3).unwrap_or(&"");
                let (scope, user_scope, method) = parse_permission_params(params_part);

                match *action {
                    "create" => Ok(Permission::Context(ContextPermission::Create(scope))),
                    "list" => Ok(Permission::Context(ContextPermission::List(scope))),
                    "delete" => Ok(Permission::Context(ContextPermission::Delete(scope))),
                    "leave" => Ok(Permission::Context(ContextPermission::Leave(
                        scope, user_scope,
                    ))),
                    "invite" => Ok(Permission::Context(ContextPermission::Invite(
                        scope, user_scope,
                    ))),
                    "execute" => Ok(Permission::Context(ContextPermission::Execute(
                        scope, user_scope, method,
                    ))),
                    "capabilities" => match *subaction {
                        "grant" => Ok(Permission::Context(ContextPermission::Capabilities(
                            CapabilityPermission::Grant(scope),
                        ))),
                        "revoke" => Ok(Permission::Context(ContextPermission::Capabilities(
                            CapabilityPermission::Revoke(scope),
                        ))),
                        _ => Err(format!("Unknown context capabilities action: {subaction}")),
                    },
                    "application" => match *subaction {
                        "update" => Ok(Permission::Context(ContextPermission::Application(
                            ContextApplicationPermission::Update(scope),
                        ))),
                        _ => Err(format!("Unknown context application action: {subaction}")),
                    },
                    "alias" => match *subaction {
                        "create" => {
                            let alias_type = match *subsubaction {
                                "context" => AliasType::Context,
                                "application" => AliasType::Application,
                                "identity" => AliasType::Identity,
                                _ => return Err(format!("Unknown alias type: {subsubaction}")),
                            };
                            Ok(Permission::Context(ContextPermission::Alias(
                                AliasPermission::Create(alias_type, scope),
                            )))
                        }
                        "list" => {
                            let alias_type = match *subsubaction {
                                "context" => AliasType::Context,
                                "application" => AliasType::Application,
                                "identity" => AliasType::Identity,
                                _ => return Err(format!("Unknown alias type: {subsubaction}")),
                            };
                            Ok(Permission::Context(ContextPermission::Alias(
                                AliasPermission::List(alias_type, scope),
                            )))
                        }
                        "lookup" => {
                            let alias_type = match *subsubaction {
                                "context" => AliasType::Context,
                                "application" => AliasType::Application,
                                "identity" => AliasType::Identity,
                                _ => return Err(format!("Unknown alias type: {subsubaction}")),
                            };
                            Ok(Permission::Context(ContextPermission::Alias(
                                AliasPermission::Lookup(alias_type, scope),
                            )))
                        }
                        "delete" => {
                            let alias_type = match *subsubaction {
                                "context" => AliasType::Context,
                                "application" => AliasType::Application,
                                "identity" => AliasType::Identity,
                                _ => return Err(format!("Unknown alias type: {subsubaction}")),
                            };
                            Ok(Permission::Context(ContextPermission::Alias(
                                AliasPermission::Delete(alias_type, scope),
                            )))
                        }
                        _ => Err(format!("Unknown context alias action: {subaction}")),
                    },
                    "" => Ok(Permission::Context(ContextPermission::All(scope))),
                    _ => Err(format!("Unknown context action: {action}")),
                }
            }

            "keys" => {
                let action = parts.get(1).unwrap_or(&"");
                let subaction = parts.get(2).unwrap_or(&"");
                let (scope, _, _) = parse_permission_params(params_part);

                match *action {
                    "create" => Ok(Permission::Keys(KeyPermission::Create)),
                    "list" => Ok(Permission::Keys(KeyPermission::List)),
                    "delete" | "revoke" => Ok(Permission::Keys(KeyPermission::Delete)), // Both map to Delete
                    "clients" => match *subaction {
                        "list" => Ok(Permission::Keys(KeyPermission::ListClients)),
                        "delete" => Ok(Permission::Keys(KeyPermission::DeleteClient)),
                        "" => Ok(Permission::Keys(KeyPermission::ListClients)), // Default for /keys/clients
                        _ => Err(format!("Unknown keys clients action: {subaction}")),
                    },
                    "permissions" => match *subaction {
                        "get" => Ok(Permission::Keys(KeyPermission::GetPermissions(scope))),
                        "update" => Ok(Permission::Keys(KeyPermission::UpdatePermissions(scope))),
                        "" => Ok(Permission::Keys(KeyPermission::GetPermissions(scope))), // Default for GET
                        _ => Err(format!("Unknown keys permissions action: {subaction}")),
                    },
                    "" => Ok(Permission::Keys(KeyPermission::All)),
                    _ => Err(format!("Unknown keys action: {action}")),
                }
            }

            _ => Err(format!("Unknown permission category: {category}")),
        }
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Permission::Admin(_) => write!(f, "admin"),
            Permission::Application(app_perm) => match app_perm {
                ApplicationPermission::All => write!(f, "application"),
                ApplicationPermission::List(scope) => {
                    let params = format_params(scope, &UserScope::Any, &None);
                    write!(f, "application:list{params}")
                }
                ApplicationPermission::Install(scope) => {
                    let params = format_params(scope, &UserScope::Any, &None);
                    write!(f, "application:install{params}")
                }
                ApplicationPermission::Uninstall(scope) => {
                    let params = format_params(scope, &UserScope::Any, &None);
                    write!(f, "application:uninstall{params}")
                }
            },
            Permission::Blob(blob_perm) => match blob_perm {
                BlobPermission::All => write!(f, "blob"),
                BlobPermission::Add(add_perm) => match add_perm {
                    AddBlobPermission::All => write!(f, "blob:add"),
                    AddBlobPermission::Stream => write!(f, "blob:add:stream"),
                    AddBlobPermission::File => write!(f, "blob:add:file"),
                    AddBlobPermission::Url => write!(f, "blob:add:url"),
                },
                BlobPermission::Remove(scope) => {
                    let params = format_params(scope, &UserScope::Any, &None);
                    write!(f, "blob:remove{params}")
                }
            },
            Permission::Context(ctx_perm) => match ctx_perm {
                ContextPermission::All(scope) => {
                    let params = format_params(scope, &UserScope::Any, &None);
                    write!(f, "context{params}")
                }
                ContextPermission::Create(scope) => {
                    let params = format_params(scope, &UserScope::Any, &None);
                    write!(f, "context:create{params}")
                }
                ContextPermission::List(scope) => {
                    let params = format_params(scope, &UserScope::Any, &None);
                    write!(f, "context:list{params}")
                }
                ContextPermission::Delete(scope) => {
                    let params = format_params(scope, &UserScope::Any, &None);
                    write!(f, "context:delete{params}")
                }
                ContextPermission::Leave(scope, user) => {
                    let params = format_params(scope, user, &None);
                    write!(f, "context:leave{params}")
                }
                ContextPermission::Invite(scope, user) => {
                    let params = format_params(scope, user, &None);
                    write!(f, "context:invite{params}")
                }
                ContextPermission::Execute(scope, user, method) => {
                    let params = format_params(scope, user, method);
                    write!(f, "context:execute{params}")
                }
                ContextPermission::Alias(alias_perm) => match alias_perm {
                    AliasPermission::Create(alias_type, scope) => {
                        let type_str = match alias_type {
                            AliasType::Context => "context",
                            AliasType::Application => "application",
                            AliasType::Identity => "identity",
                        };
                        let params = format_simple_params(scope);
                        write!(f, "context:alias:create:{type_str}{params}")
                    }
                    AliasPermission::List(alias_type, scope) => {
                        let type_str = match alias_type {
                            AliasType::Context => "context",
                            AliasType::Application => "application",
                            AliasType::Identity => "identity",
                        };
                        let params = format_simple_params(scope);
                        write!(f, "context:alias:list:{type_str}{params}")
                    }
                    AliasPermission::Lookup(alias_type, scope) => {
                        let type_str = match alias_type {
                            AliasType::Context => "context",
                            AliasType::Application => "application",
                            AliasType::Identity => "identity",
                        };
                        let params = format_simple_params(scope);
                        write!(f, "context:alias:lookup:{type_str}{params}")
                    }
                    AliasPermission::Delete(alias_type, scope) => {
                        let type_str = match alias_type {
                            AliasType::Context => "context",
                            AliasType::Application => "application",
                            AliasType::Identity => "identity",
                        };
                        let params = format_simple_params(scope);
                        write!(f, "context:alias:delete:{type_str}{params}")
                    }
                },
                ContextPermission::Capabilities(cap_perm) => match cap_perm {
                    CapabilityPermission::Grant(scope) => write!(
                        f,
                        "context:capabilities:grant{}",
                        format_simple_params(scope)
                    ),
                    CapabilityPermission::Revoke(scope) => write!(
                        f,
                        "context:capabilities:revoke{}",
                        format_simple_params(scope)
                    ),
                },
                ContextPermission::Application(app_perm) => match app_perm {
                    ContextApplicationPermission::Update(scope) => write!(
                        f,
                        "context:application:update{}",
                        format_simple_params(scope)
                    ),
                },
            },
            Permission::Keys(key_perm) => match key_perm {
                KeyPermission::All => write!(f, "keys"),
                KeyPermission::Create => write!(f, "keys:create"),
                KeyPermission::List => write!(f, "keys:list"),
                KeyPermission::Delete => write!(f, "keys:delete"),
                KeyPermission::ListClients => write!(f, "keys:clients:list"),
                KeyPermission::DeleteClient => write!(f, "keys:clients:delete"),
                KeyPermission::GetPermissions(scope) => {
                    let params = format_simple_params(scope);
                    write!(f, "keys:permissions:get{params}")
                }
                KeyPermission::UpdatePermissions(scope) => {
                    let params = format_simple_params(scope);
                    write!(f, "keys:permissions:update{params}")
                }
            },
        }
    }
}

impl Permission {
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
                (
                    ContextPermission::Capabilities(held),
                    ContextPermission::Capabilities(required),
                ) => matches_capability(held, required),
                (
                    ContextPermission::Application(held),
                    ContextPermission::Application(required),
                ) => matches_application(held, required),
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
    }
}

/// Helper function to check if one alias permission satisfies another
fn matches_alias(held: &AliasPermission, required: &AliasPermission) -> bool {
    match (held, required) {
        (
            AliasPermission::Create(held_type, held_scope),
            AliasPermission::Create(req_type, req_scope),
        ) => held_type == req_type && matches_scope(held_scope, req_scope),
        (
            AliasPermission::Delete(held_type, held_scope),
            AliasPermission::Delete(req_type, req_scope),
        ) => held_type == req_type && matches_scope(held_scope, req_scope),
        (
            AliasPermission::Lookup(held_type, held_scope),
            AliasPermission::Lookup(req_type, req_scope),
        ) => held_type == req_type && matches_scope(held_scope, req_scope),
        (
            AliasPermission::List(held_type, held_scope),
            AliasPermission::List(req_type, req_scope),
        ) => held_type == req_type && matches_scope(held_scope, req_scope),
        _ => false,
    }
}

/// Helper function to check if one capability permission satisfies another
fn matches_capability(held: &CapabilityPermission, required: &CapabilityPermission) -> bool {
    match (held, required) {
        (CapabilityPermission::Grant(h_scope), CapabilityPermission::Grant(r_scope)) => {
            matches_scope(h_scope, r_scope)
        }
        (CapabilityPermission::Revoke(h_scope), CapabilityPermission::Revoke(r_scope)) => {
            matches_scope(h_scope, r_scope)
        }
        _ => false,
    }
}

/// Helper function to check if one application permission satisfies another
fn matches_application(
    held: &ContextApplicationPermission,
    required: &ContextApplicationPermission,
) -> bool {
    match (held, required) {
        (
            ContextApplicationPermission::Update(h_scope),
            ContextApplicationPermission::Update(r_scope),
        ) => matches_scope(h_scope, r_scope),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_parsing() {
        // Test master permission
        assert_eq!(
            "admin".parse::<Permission>(),
            Ok(Permission::Admin(AdminPermission))
        );

        // Test application permissions
        assert_eq!(
            "application:list[ctx1]".parse::<Permission>(),
            Ok(Permission::Application(ApplicationPermission::List(
                ResourceScope::Specific(vec!["ctx1".to_string()])
            )))
        );

        // Test context permissions
        assert_eq!(
            "context:execute[ctx1,user1,method1]".parse::<Permission>(),
            Ok(Permission::Context(ContextPermission::Execute(
                ResourceScope::Specific(vec!["ctx1".to_string()]),
                UserScope::Specific("user1".to_string()),
                Some("method1".to_string())
            )))
        );
    }

    #[test]
    fn test_permission_display() {
        // Test Display trait implementation
        let perm = Permission::Admin(AdminPermission);
        assert_eq!(format!("{}", perm), "admin");
        assert_eq!(perm.to_string(), "admin");

        let perm = Permission::Keys(KeyPermission::Create);
        assert_eq!(format!("{}", perm), "keys:create");
        assert_eq!(perm.to_string(), "keys:create");

        let perm = Permission::Context(ContextPermission::Alias(AliasPermission::Lookup(
            AliasType::Identity,
            ResourceScope::Specific(vec!["ctx-123".to_string()]),
        )));
        assert_eq!(
            format!("{}", perm),
            "context:alias:lookup:identity[ctx-123]"
        );
        assert_eq!(perm.to_string(), "context:alias:lookup:identity[ctx-123]");
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
        // Test create permission for context
        let perm = "context:alias:create:context"
            .parse::<Permission>()
            .unwrap();
        assert!(matches!(
            perm,
            Permission::Context(ContextPermission::Alias(AliasPermission::Create(
                AliasType::Context,
                ResourceScope::Global
            )))
        ));

        // Test create permission for identity with context
        let perm = "context:alias:create:identity[ctx-123]"
            .parse::<Permission>()
            .unwrap();
        match perm {
            Permission::Context(ContextPermission::Alias(AliasPermission::Create(
                AliasType::Identity,
                scope,
            ))) => {
                assert_eq!(scope, ResourceScope::Specific(vec!["ctx-123".to_string()]));
            }
            _ => panic!("Wrong permission type"),
        }

        // Test lookup permission for application
        let perm = "context:alias:lookup:application[alias-name]"
            .parse::<Permission>()
            .unwrap();
        match perm {
            Permission::Context(ContextPermission::Alias(AliasPermission::Lookup(
                AliasType::Application,
                scope,
            ))) => {
                assert_eq!(
                    scope,
                    ResourceScope::Specific(vec!["alias-name".to_string()])
                );
            }
            _ => panic!("Wrong permission type"),
        }

        // Test list permission for identity
        let perm = "context:alias:list:identity[ctx-123]"
            .parse::<Permission>()
            .unwrap();
        match perm {
            Permission::Context(ContextPermission::Alias(AliasPermission::List(
                AliasType::Identity,
                scope,
            ))) => {
                assert_eq!(scope, ResourceScope::Specific(vec!["ctx-123".to_string()]));
            }
            _ => panic!("Wrong permission type"),
        }

        // Test delete permission for context
        let perm = "context:alias:delete:context[alias-name]"
            .parse::<Permission>()
            .unwrap();
        match perm {
            Permission::Context(ContextPermission::Alias(AliasPermission::Delete(
                AliasType::Context,
                scope,
            ))) => {
                assert_eq!(
                    scope,
                    ResourceScope::Specific(vec!["alias-name".to_string()])
                );
            }
            _ => panic!("Wrong permission type"),
        }
    }

    #[test]
    fn test_context_capabilities_parsing() {
        // Test grant permission
        let perm = "context:capabilities:grant[ctx-123]"
            .parse::<Permission>()
            .unwrap();
        match perm {
            Permission::Context(ContextPermission::Capabilities(CapabilityPermission::Grant(
                scope,
            ))) => {
                assert_eq!(scope, ResourceScope::Specific(vec!["ctx-123".to_string()]));
            }
            _ => panic!("Wrong permission type"),
        }

        // Test revoke permission
        let perm = "context:capabilities:revoke[ctx-123]"
            .parse::<Permission>()
            .unwrap();
        match perm {
            Permission::Context(ContextPermission::Capabilities(CapabilityPermission::Revoke(
                scope,
            ))) => {
                assert_eq!(scope, ResourceScope::Specific(vec!["ctx-123".to_string()]));
            }
            _ => panic!("Wrong permission type"),
        }
    }

    #[test]
    fn test_context_application_parsing() {
        // Test update permission
        let perm = "context:application:update[ctx-123]"
            .parse::<Permission>()
            .unwrap();
        match perm {
            Permission::Context(ContextPermission::Application(
                ContextApplicationPermission::Update(scope),
            )) => {
                assert_eq!(scope, ResourceScope::Specific(vec!["ctx-123".to_string()]));
            }
            _ => panic!("Wrong permission type"),
        }
    }

    #[test]
    fn test_alias_permission_satisfaction() {
        // Test create permission for same alias type
        let held = "context:alias:create:context"
            .parse::<Permission>()
            .unwrap();
        let required = "context:alias:create:context"
            .parse::<Permission>()
            .unwrap();
        assert!(held.satisfies(&required));

        // Test delete permission for specific alias
        let held = "context:alias:delete:application[alias-name]"
            .parse::<Permission>()
            .unwrap();
        let required = "context:alias:delete:application[alias-name]"
            .parse::<Permission>()
            .unwrap();
        assert!(held.satisfies(&required));

        // Test lookup permission - specific context
        let held = "context:alias:lookup:identity[ctx-123]"
            .parse::<Permission>()
            .unwrap();
        let required = "context:alias:lookup:identity[ctx-123]"
            .parse::<Permission>()
            .unwrap();
        assert!(held.satisfies(&required));

        // Test different alias types don't satisfy each other
        let held = "context:alias:create:context"
            .parse::<Permission>()
            .unwrap();
        let required = "context:alias:create:application"
            .parse::<Permission>()
            .unwrap();
        assert!(!held.satisfies(&required));

        // Test different actions don't satisfy each other
        let held = "context:alias:create:context"
            .parse::<Permission>()
            .unwrap();
        let required = "context:alias:delete:context"
            .parse::<Permission>()
            .unwrap();
        assert!(!held.satisfies(&required));

        // Test master permission satisfies all
        let held = "admin".parse::<Permission>().unwrap();
        let required = "context:alias:lookup:identity[ctx-123]"
            .parse::<Permission>()
            .unwrap();
        assert!(held.satisfies(&required));
    }
}
