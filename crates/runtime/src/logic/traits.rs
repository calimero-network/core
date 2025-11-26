use calimero_primitives::common::DIGEST_SIZE;
use core::fmt::Debug;

/// Interface for accessing host context information from the runtime.
///
/// This trait allows the runtime to query information about the Calimero context
/// without having a direct dependency on the node's storage implementation.
///
/// The following information could be queried:
/// * if the specific public key is a member of the context;
/// * list all the members of the context.
///
/// It bridges the gap between the sandboxed WASM environment and the node's state.
pub trait ContextHost: Send + Sync + Debug {
    /// Checks if the given public key is a member of the current context.
    ///
    /// # Arguments
    ///
    /// * `public_key` - The 32-byte public key to check.
    fn is_member(&self, public_key: &[u8; DIGEST_SIZE]) -> bool;

    /// Returns a list of all members in the current context.
    ///
    /// # Returns
    ///
    /// A vector of 32-byte public keys representing the members.
    fn members(&self) -> Vec<[u8; DIGEST_SIZE]>;
}
