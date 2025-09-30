/// The standard size of the digest used in bytes.
/// The digest is used everywhere: for context, public key, proposals, etc.
pub const DIGEST_SIZE: usize = 32;

// The constant for one kibibyte for a better readability and less error-prone approach on usage.
pub const ONE_KIB: u32 = 1024;
// The constant for one mibibyte for a better readability and less error-prone approach on usage.
pub const ONE_MIB: u32 = ONE_KIB * 1024;
// The constant for one gibibyte for a better readability and less error-prone approach on usage.
pub const ONE_GIB: u32 = ONE_MIB * 1024;
