/// Secure fail-closed default enforced when a policy's `allowed_tcb_statuses`
/// is empty. An empty allowlist must NOT skip the TCB-status check (that was a
/// fail-open hole — audit findings #356 / #17): instead it enforces against
/// this single status. Must be the exact PascalCase value dcap-qvl emits
/// (`crates/tee-attestation`) and matches the mero-tee KMS key-delivery gate,
/// keeping "admitted ⟹ can get key" consistent. Do not broaden.
pub const DEFAULT_ALLOWED_TCB_STATUS: &str = "UpToDate";

/// dcap-qvl TCB status that is rejected unconditionally, regardless of policy
/// (defense-in-depth). Real verification already bails on `Revoked`, but the
/// subgroup-reuse admission path reuses a STORED tcb_status string without
/// re-running verify — this guards that path.
pub const TCB_STATUS_REVOKED: &str = "Revoked";

/// Mock-attestation TCB status (set by `calimero_tee_attestation` mock verify).
/// Real dcap-qvl never emits this value, so it uniquely identifies the mock
/// path. Mock admission is gated upstream by `accept_mock`, so the TCB
/// allowlist must not apply to it.
pub const TCB_STATUS_MOCK: &str = "Mock";

/// Shared TCB-status gate used by every `allowed_tcb_statuses` enforcement site.
///
/// Rules (in order):
/// 1. `Revoked` (case-insensitive) → always rejected, even for the stored-status
///    subgroup-reuse path that does not re-run dcap-qvl verify.
/// 2. Mock path → allowed **only when the group policy sets `accept_mock`**. The
///    mock signal is either the explicit runtime `is_mock` flag (admit_tee_node)
///    or the reserved `"Mock"` status that carries it on the op-apply /
///    subgroup-reuse path (which has `is_mock = false`). Gating both on
///    `accept_mock` means a stored `"Mock"` status replayed onto a real fleet
///    (`accept_mock = false`) is rejected instead of being a permanent bypass
///    token — it no longer relies solely on the upstream admission gate
///    (audit follow-up to #356 / #17).
/// 3. Empty allowlist → fail-closed: enforce against the secure default
///    [`DEFAULT_ALLOWED_TCB_STATUS`] instead of skipping the check.
/// 4. Non-empty allowlist → honored exactly (case-sensitive, as today).
pub fn tcb_status_allowed(
    allowed_tcb_statuses: &[String],
    tcb_status: &str,
    is_mock: bool,
    accept_mock: bool,
) -> bool {
    if tcb_status.eq_ignore_ascii_case(TCB_STATUS_REVOKED) {
        return false;
    }
    if (is_mock || tcb_status == TCB_STATUS_MOCK) && accept_mock {
        return true;
    }
    if allowed_tcb_statuses.is_empty() {
        return tcb_status == DEFAULT_ALLOWED_TCB_STATUS;
    }
    allowed_tcb_statuses.iter().any(|a| a == tcb_status)
}

pub const TEE_REJECT_MRTD: &str = "mrtd_not_allowed";
pub const TEE_REJECT_TCB_STATUS: &str = "tcb_status_not_allowed";
pub const TEE_REJECT_RTMR0: &str = "rtmr0_not_allowed";
pub const TEE_REJECT_RTMR1: &str = "rtmr1_not_allowed";
pub const TEE_REJECT_RTMR2: &str = "rtmr2_not_allowed";
pub const TEE_REJECT_RTMR3: &str = "rtmr3_not_allowed";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Each variant names the specific attestation field a policy rejected; the
// shared `NotAllowed` suffix is meaningful domain vocabulary, not redundancy.
#[allow(
    clippy::enum_variant_names,
    reason = "each variant is a distinct policy-rejection reason"
)]
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
    /// Whether this group accepts mock attestations. Gates the mock TCB bypass
    /// on the op-apply path so a stored `"Mock"` status only passes on a fleet
    /// that actually opted into mock. See [`tcb_status_allowed`].
    pub accept_mock: bool,
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

    // Fail-closed TCB-status gate (shared with `admit_tee_node`). This is the
    // op-apply path: it runs on every node replicating the op and has no
    // explicit `is_mock` flag, so mock is detected via the reserved "Mock"
    // status inside `tcb_status_allowed`. That mock bypass is in turn gated on
    // the group's stored `accept_mock`, so a replayed `"Mock"` status cannot
    // bypass the gate on a fleet that did not opt into mock.
    if !tcb_status_allowed(
        &policy.allowed_tcb_statuses,
        fields.tcb_status,
        false,
        policy.accept_mock,
    ) {
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
