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

    /// The resource type
    pub resource_type: String,
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
        }
    }

    /// Get a display name for the permission
    pub fn display_name(&self) -> String {
        format!("{} ({})", self.name, self.resource_type)
    }
}
