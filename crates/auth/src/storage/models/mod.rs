mod root_key;
mod client_key;
mod permission;

pub use root_key::RootKey;
pub use client_key::ClientKey;
pub use permission::Permission;

/// Storage prefixes for different types of data
pub mod prefixes {
    /// Prefix for root keys
    pub const ROOT_KEY: &str = "root:";

    /// Prefix for client keys
    pub const CLIENT_KEY: &str = "client:";

    /// Prefix for permissions
    pub const PERMISSION: &str = "permission:";

    /// Prefix for refresh tokens
    pub const REFRESH_TOKEN: &str = "refresh:";

    /// Prefix for the secondary index of root key to client keys
    pub const ROOT_CLIENTS: &str = "root_clients:";
} 