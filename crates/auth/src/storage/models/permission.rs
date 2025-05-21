use serde::{Deserialize, Serialize};

/// Permission storage model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permission {
    /// The permission ID
    pub permission_id: String,

    /// The name of the permission
    pub name: String,

    /// The description of the permission
    pub description: String,

    /// The resource type (e.g., "application", "blob", "context")
    pub resource_type: String,

    /// Optional specific resource IDs this permission applies to
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_ids: Option<Vec<String>>,

    /// Optional specific method this permission applies to
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,

    /// Optional specific user ID this permission applies to
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

impl Permission {
    /// Create a new permission
    ///
    /// # Arguments
    ///
    /// * `permission_id` - The permission ID
    /// * `name` - The name of the permission
    /// * `description` - The description of the permission
    /// * `resource_type` - The resource type
    ///
    /// # Returns
    ///
    /// * `Self` - The new permission
    pub fn new(
        permission_id: String,
        name: String,
        description: String,
        resource_type: String,
    ) -> Self {
        Self {
            permission_id,
            name,
            description,
            resource_type,
            resource_ids: None,
            method: None,
            user_id: None,
        }
    }

    /// Create a new scoped permission
    pub fn new_scoped(
        permission_id: String,
        name: String,
        description: String,
        resource_type: String,
        resource_ids: Option<Vec<String>>,
        method: Option<String>,
        user_id: Option<String>,
    ) -> Self {
        Self {
            permission_id,
            name,
            description,
            resource_type,
            resource_ids,
            method,
            user_id,
        }
    }

    /// Get a display name for the permission
    pub fn display_name(&self) -> String {
        let mut display = format!("{} ({})", self.name, self.resource_type);

        if let Some(ids) = &self.resource_ids {
            display.push_str(&format!(" [{}]", ids.join(", ")));
        }

        if let Some(method) = &self.method {
            display.push_str(&format!(" {{{}}}", method));
        }

        if let Some(user) = &self.user_id {
            display.push_str(&format!(" <{}>", user));
        }

        display
    }

    /// Check if this permission matches the required permission pattern
    pub fn matches(&self, required: &str) -> bool {
        // Split the required permission into parts
        let parts: Vec<&str> = required.split(&[':', '[', ']', '<', '>']).collect();

        // Base permission must match
        if parts[0] != self.permission_id {
            return false;
        }

        // Check resource IDs if specified
        if let Some(required_ids) = parts.get(1) {
            if let Some(ref allowed_ids) = self.resource_ids {
                let req_ids: Vec<&str> = required_ids.split(',').map(|s| s.trim()).collect();
                if !req_ids
                    .iter()
                    .all(|id| allowed_ids.contains(&id.to_string()))
                {
                    return false;
                }
            } else {
                return false;
            }
        }

        // Check method if specified
        if let Some(required_method) = parts.get(2) {
            if let Some(ref allowed_method) = self.method {
                if required_method != allowed_method {
                    return false;
                }
            } else {
                return false;
            }
        }

        true
    }
}
