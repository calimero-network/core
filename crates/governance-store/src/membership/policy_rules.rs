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
/// The POLICY names no RTMR1 — an incomplete policy, not a refused node.
pub const TEE_REJECT_RTMR1_EMPTY: &str = "rtmr1_allowlist_empty";
/// The POLICY names no RTMR2 — an incomplete policy, not a refused node.
pub const TEE_REJECT_RTMR2_EMPTY: &str = "rtmr2_allowlist_empty";
/// The POLICY names no RTMR3 — an incomplete policy, not a refused node.
pub const TEE_REJECT_RTMR3_EMPTY: &str = "rtmr3_allowlist_empty";
/// The POLICY names no MRTD — likewise the policy, not the node.
pub const TEE_REJECT_MRTD_EMPTY: &str = "mrtd_allowlist_empty";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Each variant names the specific attestation field a policy rejected; the
// shared `NotAllowed` suffix is meaningful domain vocabulary, not redundancy.
#[allow(
    clippy::enum_variant_names,
    reason = "each variant is a distinct policy-rejection reason"
)]
pub enum MembershipPolicyRejection {
    MrtdNotAllowed,
    /// The policy names no MRTD at all.
    ///
    /// Same shape as [`Self::Rtmr3AllowlistEmpty`], and for the same reason:
    /// an empty allowlist used to mean "do not check", so the weakest possible
    /// policy was the one that looked merely unfilled. `admit_tee_node` has
    /// always refused this; the op-apply path skipped it, so the two disagreed
    /// about what the same stored policy meant.
    MrtdAllowlistEmpty,
    /// The policy names no RTMR1 at all.
    ///
    /// Required for the same reason as RTMR3: see [`Self::Rtmr2AllowlistEmpty`].
    Rtmr1AllowlistEmpty,
    /// The policy names no RTMR2 at all.
    ///
    /// RTMR3 is extended by `calimero-init` with a string built from public
    /// inputs, so it only proves which image ran if everything that ran BEFORE
    /// `calimero-init` is pinned too. The firmware measures the kernel into
    /// RTMR1 and the kernel command line and initrd into RTMR2; leave either
    /// unpinned and a custom kernel or initrd can extend RTMR3 with the locked
    /// profile's string and pass as that image.
    Rtmr2AllowlistEmpty,
    /// The policy names no RTMR3 at all.
    ///
    /// Distinct from `Rtmr3NotAllowed` because the remedy is the opposite: the
    /// node is not necessarily wrong, the POLICY is incomplete. Reporting it as
    /// a mismatch would send an operator looking at the node's measurements.
    Rtmr3AllowlistEmpty,
    TcbStatusNotAllowed,
    Rtmr0NotAllowed,
    Rtmr1NotAllowed,
    Rtmr2NotAllowed,
    Rtmr3NotAllowed,
}

#[derive(Debug)]
pub struct MembershipPolicyValidationError {
    pub(crate) reason: MembershipPolicyRejection,
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
            MembershipPolicyRejection::MrtdAllowlistEmpty => {
                "MemberJoinedViaTeeAttestation rejected: the group's TEE admission policy names \
                 no MRTD. An empty allowlist is not a wildcard -- set allowed_mrtd from the \
                 release's published-mrtds.json. A mock fleet names the all-zero measurement \
                 that create_mock_quote reports, exactly as it already does for RTMR3"
            }
            MembershipPolicyRejection::Rtmr1AllowlistEmpty => {
                "MemberJoinedViaTeeAttestation rejected: the group's TEE admission policy names \
                 no RTMR1. RTMR1 measures the kernel; without it a custom kernel can replay the \
                 RTMR3 extension of a locked image. Set allowed_rtmr1 from the release's \
                 published-mrtds.json"
            }
            MembershipPolicyRejection::Rtmr2AllowlistEmpty => {
                "MemberJoinedViaTeeAttestation rejected: the group's TEE admission policy names \
                 no RTMR2. RTMR2 measures the kernel command line and initrd; without it a \
                 custom initrd can replay the RTMR3 extension of a locked image. Set \
                 allowed_rtmr2 from the release's published-mrtds.json"
            }
            MembershipPolicyRejection::Rtmr3AllowlistEmpty => {
                "MemberJoinedViaTeeAttestation rejected: the group's TEE admission policy names \
                 no RTMR3. MRTD identifies the firmware, not the image -- it is the same for \
                 every profile of a release -- so a policy without RTMR3 would admit any \
                 profile. Set allowed_rtmr3 from the release's published-mrtds.json"
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
    // Empty is a refusal, not a skip -- the same rule as RTMR3 below, which
    // this check predated. `admit_tee_node` has always refused an empty
    // `allowed_mrtd`, so leaving a skip here meant the requesting node and the
    // peers replicating its op disagreed about what the stored policy meant:
    // the admitter would not issue, but any peer would accept any firmware.
    if policy.allowed_mrtd.is_empty() {
        return Err(MembershipPolicyValidationError {
            reason: MembershipPolicyRejection::MrtdAllowlistEmpty,
        });
    }
    if !policy.allowed_mrtd.iter().any(|a| a == fields.mrtd) {
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

    // RTMR3 IS MANDATORY HERE TOO, and this is the path that decides convergence.
    //
    // `admit_tee_node` is the requesting node's own gate; THIS runs on every
    // peer replicating the op. Enforcing only in the actor would let a node
    // publish an admission its peers then accept, so the allowlist would bind
    // whoever happened to ask and nobody else.
    //
    // Empty is a refusal rather than a skip, unlike RTMR0 in the loop below.
    // MRTD identifies the firmware, not the image -- it is identical across
    // every profile of a release and constant across most releases -- so RTMR3
    // is the only field that names which image ran. A policy without it admits
    // any profile, including debug images with SSH and an unlocked root.
    if policy.allowed_rtmr3.is_empty() {
        return Err(MembershipPolicyValidationError {
            reason: MembershipPolicyRejection::Rtmr3AllowlistEmpty,
        });
    }

    // RTMR1 and RTMR2 are mandatory for RTMR3's sake. `calimero-init` extends
    // RTMR3 with `calimero-rtmr3-v2:<role>:<profile>:<root_hash>` -- public
    // inputs -- so RTMR3 only proves which image ran if the code that ran
    // before `calimero-init` is pinned as well. The firmware measures the
    // kernel into RTMR1 and the command line + initrd into RTMR2. With either
    // unpinned, a custom kernel or initrd can extend RTMR3 with a locked
    // profile's string and pass as that image. RTMR0 (the VM's hardware
    // configuration) stays optional: it varies with machine shape, not image.
    if policy.allowed_rtmr1.is_empty() {
        return Err(MembershipPolicyValidationError {
            reason: MembershipPolicyRejection::Rtmr1AllowlistEmpty,
        });
    }
    if policy.allowed_rtmr2.is_empty() {
        return Err(MembershipPolicyValidationError {
            reason: MembershipPolicyRejection::Rtmr2AllowlistEmpty,
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

#[cfg(test)]
mod rtmr3_is_mandatory {
    //! RTMR3 is the only field that identifies the image.
    //!
    //! MRTD measures the virtual firmware, so it is identical across every
    //! PROFILE of a release and constant across most RELEASES --
    //! `locked-read-only` reported the same `c1ee9c16…` for 2.3.62, 2.3.63 and
    //! 2.3.65, and every profile of each. A policy naming only an MRTD admits a
    //! `debug` image, which carries no lockdown role: openssh-server, the
    //! serial console and the rescue shell are present and root is not locked.
    //!
    //! `calimero-init` extends RTMR3 with
    //! `calimero-rtmr3-v2:<role>:<profile>:<root_hash>`, so it names exactly one
    //! (profile, release) pair -- and therefore changes every release, which is
    //! the cost of pinning it.
    //!
    //! This is the OP-APPLY path: it runs on every peer replicating the op, so
    //! it is what makes the allowlist bind anyone other than the node that
    //! asked.

    use super::*;

    const MRTD: &str = "c1";
    const RTMR3_LOCKED: &str = "74";
    const RTMR3_DEBUG: &str = "cf";

    fn policy(allowed_rtmr3: Vec<String>) -> TeeAllowlistPolicy {
        TeeAllowlistPolicy {
            allowed_mrtd: vec![MRTD.to_owned()],
            allowed_rtmr0: vec![],
            allowed_rtmr1: vec!["00".to_owned()],
            allowed_rtmr2: vec!["00".to_owned()],
            allowed_rtmr3,
            allowed_tcb_statuses: vec!["UpToDate".to_owned()],
            accept_mock: false,
        }
    }

    fn claims(rtmr3: &str) -> TeeAttestationClaims<'_> {
        TeeAttestationClaims {
            mrtd: MRTD,
            rtmr0: "00",
            rtmr1: "00",
            rtmr2: "00",
            rtmr3,
            tcb_status: "UpToDate",
        }
    }

    /// An empty `allowed_mrtd` refuses, rather than admitting every firmware.
    ///
    /// This check sat three lines above the RTMR3 rule that spells out why an
    /// empty allowlist must refuse rather than skip, and kept the old
    /// skip-on-empty shape. The consequence was a disagreement rather than a
    /// hole: `admit_tee_node` refuses an empty `allowed_mrtd` unconditionally,
    /// so the node that asked would not issue the op -- while any peer
    /// replicating one would have accepted any firmware at all.
    #[test]
    fn a_policy_naming_no_mrtd_admits_nobody() {
        let mut p = policy(vec![RTMR3_LOCKED.to_owned()]);
        p.allowed_mrtd = vec![];
        let mut c = claims(RTMR3_LOCKED);
        c.mrtd = "a-firmware-this-policy-never-named";

        let err = validate_tee_attestation_allowlists(&p, &c)
            .expect_err("an empty allowed_mrtd must refuse, not admit anything");
        assert_eq!(err.reason(), MembershipPolicyRejection::MrtdAllowlistEmpty);
    }

    /// The remedy is to fix the POLICY, not to go read the node's firmware --
    /// so this must not be reported as a mismatch.
    #[test]
    fn the_empty_mrtd_case_is_not_reported_as_a_mismatch() {
        let mut p = policy(vec![RTMR3_LOCKED.to_owned()]);
        p.allowed_mrtd = vec![];
        let err = validate_tee_attestation_allowlists(&p, &claims(RTMR3_LOCKED)).unwrap_err();
        assert_ne!(err.reason(), MembershipPolicyRejection::MrtdNotAllowed);
        assert!(
            err.to_string().contains("no MRTD"),
            "the message must say the policy is incomplete; got: {err}"
        );
    }

    /// `accept_mock` does not exempt a policy from naming its measurements.
    /// It decides whether a mock quote is entertained at all -- and a mock
    /// quote still has measurements, all zero, which a mock fleet names.
    #[test]
    fn accept_mock_does_not_waive_the_mrtd_allowlist() {
        let mut p = policy(vec![RTMR3_LOCKED.to_owned()]);
        p.allowed_mrtd = vec![];
        p.accept_mock = true;
        let err = validate_tee_attestation_allowlists(&p, &claims(RTMR3_LOCKED))
            .expect_err("accept_mock must not turn an empty allowlist into a wildcard");
        assert_eq!(err.reason(), MembershipPolicyRejection::MrtdAllowlistEmpty);
    }

    #[test]
    fn a_policy_naming_no_rtmr3_admits_nobody() {
        // Before this, an empty list meant "do not check", so the weakest policy
        // was the one that looked simply unfilled.
        let err = validate_tee_attestation_allowlists(&policy(vec![]), &claims(RTMR3_LOCKED))
            .expect_err("an empty allowed_rtmr3 must refuse, not skip");
        assert_eq!(err.reason(), MembershipPolicyRejection::Rtmr3AllowlistEmpty);
    }

    #[test]
    fn the_empty_case_is_not_reported_as_a_mismatch() {
        // The remedy is the opposite: the POLICY is incomplete, not the node.
        // Reporting a mismatch sends an operator to read the node's measurements.
        let err = validate_tee_attestation_allowlists(&policy(vec![]), &claims(RTMR3_LOCKED))
            .unwrap_err();
        assert_ne!(err.reason(), MembershipPolicyRejection::Rtmr3NotAllowed);
        assert!(
            err.to_string().contains("names \nno RTMR3") || err.to_string().contains("no RTMR3")
        );
    }

    /// RTMR3 is extended from public inputs, so it only names the image when
    /// the kernel (RTMR1) and command line + initrd (RTMR2) are pinned too:
    /// with either unpinned, a custom kernel or initrd can extend RTMR3 with a
    /// locked profile's string and reproduce exactly the value the policy names.
    #[test]
    fn a_policy_naming_no_rtmr1_or_rtmr2_admits_nobody() {
        let mut p = policy(vec![RTMR3_LOCKED.to_owned()]);
        p.allowed_rtmr1 = vec![];
        let mut c = claims(RTMR3_LOCKED);
        c.rtmr1 = "a-kernel-this-policy-never-named";
        let err = validate_tee_attestation_allowlists(&p, &c)
            .expect_err("an empty allowed_rtmr1 must refuse, not skip");
        assert_eq!(err.reason(), MembershipPolicyRejection::Rtmr1AllowlistEmpty);
        assert!(err.to_string().contains("no RTMR1"), "got: {err}");

        let mut p = policy(vec![RTMR3_LOCKED.to_owned()]);
        p.allowed_rtmr2 = vec![];
        let mut c = claims(RTMR3_LOCKED);
        c.rtmr2 = "an-initrd-this-policy-never-named";
        let err = validate_tee_attestation_allowlists(&p, &c)
            .expect_err("an empty allowed_rtmr2 must refuse, not skip");
        assert_eq!(err.reason(), MembershipPolicyRejection::Rtmr2AllowlistEmpty);
        assert!(err.to_string().contains("no RTMR2"), "got: {err}");
    }

    /// A pinned RTMR3 does not survive an unpinned-then-mismatched kernel: the
    /// replayed-RTMR3 case, with the kernel the policy actually names.
    #[test]
    fn a_locked_rtmr3_on_a_different_kernel_is_refused() {
        let mut c = claims(RTMR3_LOCKED);
        c.rtmr1 = "a-custom-kernel";
        let err = validate_tee_attestation_allowlists(&policy(vec![RTMR3_LOCKED.to_owned()]), &c)
            .expect_err("a matching RTMR3 on an unapproved kernel must not be admitted");
        assert_eq!(err.reason(), MembershipPolicyRejection::Rtmr1NotAllowed);
    }

    /// RTMR0 measures the VM's hardware configuration, which varies with
    /// machine shape rather than image, so it stays optional.
    #[test]
    fn an_empty_rtmr0_allowlist_is_still_a_skip() {
        let mut c = claims(RTMR3_LOCKED);
        c.rtmr0 = "any-machine-shape";
        validate_tee_attestation_allowlists(&policy(vec![RTMR3_LOCKED.to_owned()]), &c)
            .expect("an unpinned RTMR0 must not refuse");
    }

    #[test]
    fn a_different_profile_is_refused_despite_a_matching_mrtd() {
        // THE BUG. Same MRTD -- because every profile shares it -- and the
        // policy was written for the locked image.
        let err = validate_tee_attestation_allowlists(
            &policy(vec![RTMR3_LOCKED.to_owned()]),
            &claims(RTMR3_DEBUG),
        )
        .expect_err("a debug image must not pass a locked-image policy");
        assert_eq!(err.reason(), MembershipPolicyRejection::Rtmr3NotAllowed);
    }

    #[test]
    fn the_named_profile_is_admitted() {
        validate_tee_attestation_allowlists(
            &policy(vec![RTMR3_LOCKED.to_owned()]),
            &claims(RTMR3_LOCKED),
        )
        .expect("the profile the policy names must be admitted");
    }

    #[test]
    fn several_releases_may_be_named_at_once() {
        // RTMR3 folds in root_hash, so it changes every release. A policy is
        // expected to carry one value per (profile, release) it accepts, which
        // is how an upgrade lands without a window where nothing may join.
        let previous = "aa";
        let current = RTMR3_LOCKED;
        let p = policy(vec![previous.to_owned(), current.to_owned()]);
        validate_tee_attestation_allowlists(&p, &claims(previous)).expect("previous release");
        validate_tee_attestation_allowlists(&p, &claims(current)).expect("current release");
    }
}
