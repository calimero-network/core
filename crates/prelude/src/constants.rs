/// The standard size of the digest used in bytes.
/// The digest is used everywhere: for context, public key, proposals, etc.
pub const DIGEST_SIZE: usize = 32;

/// The fixed ID used for the root state entry ID within the Root collection in `storage` crate.
pub const ROOT_STORAGE_ENTRY_ID: [u8; 32] = [118; DIGEST_SIZE];
