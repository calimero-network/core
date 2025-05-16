mod client_key;
mod permission;
mod root_key;

pub use client_key::ClientKey;
pub use permission::Permission;
pub use root_key::RootKey;

/// Storage prefixes for different types of data
pub mod prefixes {
    /// Prefix for root keys
    pub const ROOT_KEY: &str = "root_key:";

    /// Prefix for client keys
    pub const CLIENT_KEY: &str = "client:";

    /// Prefix for permissions
    pub const PERMISSION: &str = "permission:";

    /// Prefix for refresh tokens
    pub const REFRESH_TOKEN: &str = "refresh:";

    /// Prefix for the secondary index of root key to client keys
    pub const ROOT_CLIENTS: &str = "root_clients:";

    /// Prefix for public key index
    pub const PUBLIC_KEY_INDEX: &str = "index:public_key:";
}
