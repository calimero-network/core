//! Who may give a job node work: only a node of its cluster, shown by a signature of the
//! cluster identity key, which only a node that holds the cluster key can make.

use super::JobConfig;
use crate::api::ct_eq;
use crate::error::ApiError;
use crate::job_wire::{verify_assignment, verify_bundle, Assignment};

/// Accepts `assignment` only if the cluster identity this node was started with signed it.
pub(super) fn admit_assignment(
    config: &JobConfig,
    assignment: &Assignment,
) -> Result<(), ApiError> {
    if verify_assignment(&config.cluster_identity, assignment) {
        return Ok(());
    }
    Err(ApiError::new(
        403,
        "BAD_SIGNATURE",
        "the assignment is not signed by this node's cluster",
    ))
}

/// Accepts the component `hash` only if the cluster identity signed its name.
pub(super) fn admit_bundle(
    config: &JobConfig,
    hash: &str,
    signature: &str,
) -> Result<(), ApiError> {
    if verify_bundle(&config.cluster_identity, hash, signature) {
        return Ok(());
    }
    Err(ApiError::new(
        403,
        "BAD_SIGNATURE",
        "the component is not signed by this node's cluster",
    ))
}

/// Whether `given` (hex) is the run's capability, in constant time.
pub(super) fn token_matches(expected: &[u8; 32], given: &str) -> bool {
    hex::decode(given).is_ok_and(|g| ct_eq(expected, &g))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_matches_only_its_own_hex() {
        let token = [7u8; 32];
        assert!(token_matches(&token, &hex::encode(token)));
        assert!(!token_matches(&token, &hex::encode([8u8; 32])));
        assert!(!token_matches(&token, &hex::encode([7u8; 31])));
        assert!(!token_matches(&token, "zz"));
        assert!(!token_matches(&token, ""));
    }
}
