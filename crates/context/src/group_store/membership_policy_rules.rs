pub const TEE_REJECT_MRTD: &str = "mrtd_not_allowed";
pub const TEE_REJECT_TCB_STATUS: &str = "tcb_status_not_allowed";
pub const TEE_REJECT_RTMR0: &str = "rtmr0_not_allowed";
pub const TEE_REJECT_RTMR1: &str = "rtmr1_not_allowed";
pub const TEE_REJECT_RTMR2: &str = "rtmr2_not_allowed";
pub const TEE_REJECT_RTMR3: &str = "rtmr3_not_allowed";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipPolicyRejection {
    MrtdNotAllowed,
    TcbStatusNotAllowed,
    Rtmr0NotAllowed,
    Rtmr1NotAllowed,
    Rtmr2NotAllowed,
    Rtmr3NotAllowed,
}

#[derive(Debug)]
pub struct MembershipPolicyValidationError {
    reason: MembershipPolicyRejection,
}

impl MembershipPolicyValidationError {
    pub fn reason(&self) -> MembershipPolicyRejection {
        self.reason
    }
}

impl std::fmt::Display for MembershipPolicyValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self.reason {
            MembershipPolicyRejection::MrtdNotAllowed => {
                "MemberJoinedViaTeeAttestation rejected: MRTD not in policy allowlist"
            }
            MembershipPolicyRejection::TcbStatusNotAllowed => {
                "MemberJoinedViaTeeAttestation rejected: TCB status not in policy allowlist"
            }
            MembershipPolicyRejection::Rtmr0NotAllowed => {
                "MemberJoinedViaTeeAttestation rejected: RTMR0 not in policy allowlist"
            }
            MembershipPolicyRejection::Rtmr1NotAllowed => {
                "MemberJoinedViaTeeAttestation rejected: RTMR1 not in policy allowlist"
            }
            MembershipPolicyRejection::Rtmr2NotAllowed => {
                "MemberJoinedViaTeeAttestation rejected: RTMR2 not in policy allowlist"
            }
            MembershipPolicyRejection::Rtmr3NotAllowed => {
                "MemberJoinedViaTeeAttestation rejected: RTMR3 not in policy allowlist"
            }
        };
        write!(f, "{message}")
    }
}

impl std::error::Error for MembershipPolicyValidationError {}

pub struct TeeAllowlistPolicy {
    pub allowed_mrtd: Vec<String>,
    pub allowed_rtmr0: Vec<String>,
    pub allowed_rtmr1: Vec<String>,
    pub allowed_rtmr2: Vec<String>,
    pub allowed_rtmr3: Vec<String>,
    pub allowed_tcb_statuses: Vec<String>,
}

pub struct TeeAttestationClaims<'a> {
    pub mrtd: &'a str,
    pub rtmr0: &'a str,
    pub rtmr1: &'a str,
    pub rtmr2: &'a str,
    pub rtmr3: &'a str,
    pub tcb_status: &'a str,
}

pub fn validate_tee_attestation_allowlists(
    policy: &TeeAllowlistPolicy,
    fields: &TeeAttestationClaims<'_>,
) -> Result<(), MembershipPolicyValidationError> {
    if !policy.allowed_mrtd.is_empty() && !policy.allowed_mrtd.iter().any(|a| a == fields.mrtd) {
        return Err(MembershipPolicyValidationError {
            reason: MembershipPolicyRejection::MrtdNotAllowed,
        });
    }

    if !policy.allowed_tcb_statuses.is_empty()
        && !policy
            .allowed_tcb_statuses
            .iter()
            .any(|a| a == fields.tcb_status)
    {
        return Err(MembershipPolicyValidationError {
            reason: MembershipPolicyRejection::TcbStatusNotAllowed,
        });
    }

    for (allowlist, actual, reason) in [
        (
            &policy.allowed_rtmr0,
            fields.rtmr0,
            MembershipPolicyRejection::Rtmr0NotAllowed,
        ),
        (
            &policy.allowed_rtmr1,
            fields.rtmr1,
            MembershipPolicyRejection::Rtmr1NotAllowed,
        ),
        (
            &policy.allowed_rtmr2,
            fields.rtmr2,
            MembershipPolicyRejection::Rtmr2NotAllowed,
        ),
        (
            &policy.allowed_rtmr3,
            fields.rtmr3,
            MembershipPolicyRejection::Rtmr3NotAllowed,
        ),
    ] {
        if !allowlist.is_empty() && !allowlist.iter().any(|a| a == actual) {
            return Err(MembershipPolicyValidationError { reason });
        }
    }

    Ok(())
}
