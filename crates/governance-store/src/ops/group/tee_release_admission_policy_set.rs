//! `GroupOp::TeeReleaseAdmissionPolicySet` apply handler.

use super::context::GroupApplyCtx;
use eyre::{bail, Result as EyreResult};

/// The same admin and namespace-root rules as the list policy, plus a
/// non-empty profile list: a signed-release policy naming no profile would
/// admit nothing, and one that admitted any profile would let a debug image
/// in, so it is refused at apply rather than read as either.
pub(crate) fn apply(ctx: &mut GroupApplyCtx<'_>, allowed_profiles: &[String]) -> EyreResult<()> {
    super::tee_admission_policy_set::apply(ctx)?;
    if allowed_profiles.iter().all(|p| p.trim().is_empty()) {
        bail!("a signed-release TEE admission policy must name at least one image profile");
    }
    Ok(())
}
