mod keys;

pub use keys::{Key, KeyMetadata, KeyType};

/// Storage prefixes for different types of data
pub mod prefixes {
    /// Prefix for root keys
    pub const ROOT_KEY: &str = "root_key:";

    /// Prefix for client keys
    pub const CLIENT_KEY: &str = "client_key:";

    /// Prefix for permissions
    pub const PERMISSION: &str = "permission:";

    /// Prefix for refresh tokens
    pub const REFRESH_TOKEN: &str = "refresh:";

    /// Prefix for the secondary index of root key to client keys
    pub const ROOT_CLIENTS: &str = "root_clients:";

    /// Prefix for public key index
    pub const PUBLIC_KEY_INDEX: &str = "index:public_key:";

    /// Prefix for key permissions
    pub const KEY_PERMISSIONS: &str = "key_permissions:";
}
