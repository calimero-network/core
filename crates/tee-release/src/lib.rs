//! Fetch and Sigstore-verify signed mero-tee release assets.
//!
//! mero-tee publishes its release trust assets keyless-signed with cosign
//! under a GitHub Actions workflow identity. This crate downloads an asset
//! with its detached signature and Sigstore bundle and verifies both against
//! the expected workflow, for the two consumers core has: merod checking the
//! KMS it takes its storage key from (`Release mero-kms`), and an admitter
//! checking a joining TEE against the node release it runs
//! (`Release mero-tee`).

mod fetch;
mod node;
mod sigstore_verify;
mod version;

pub use fetch::{
    fetch_backoff, fetch_verified_asset, fetch_verified_asset_if_published, MERO_TEE_RELEASE_BASE,
};
pub use node::{
    fetch_node_release, NodeRelease, ProfileMeasurements, NODE_RELEASE_TAG_PREFIX,
    PUBLISHED_MRTDS_ASSET,
};
pub use sigstore_verify::{
    verify_signed_asset, WorkflowIdentity, GITHUB_ACTIONS_OIDC_ISSUER, KMS_RELEASE_IDENTITY,
    NODE_RELEASE_IDENTITY,
};
pub use version::{compare_release_versions, is_valid_release_version, normalize_release_version};
