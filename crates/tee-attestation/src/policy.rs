//! Safe-by-construction policy gate over a [`VerificationResult`].
//!
//! [`VerificationResult::is_valid`] is **crypto-only**: it answers "did *some*
//! genuine TDX platform produce a fresh quote bound to this app hash", nothing
//! more. Admitting a peer or releasing a key on that answer alone trusts any
//! well-formed TDX platform rather than a specific approved workload.
//!
//! [`VerificationResult::policy_valid`] is the gate new callers should use: it
//! folds the crypto checks together with a TCB-status allowlist, the mock
//! acceptance decision, the measurement allowlists, and the app-hash binding,
//! and fails closed on every under-specified input.
//!
//! # Relationship to existing enforcement sites
//! Three call sites already implement equivalent enforcement inline and keep it
//! by design (they emit richer, mode-specific errors this crate cannot):
//! - `crates/context/src/handlers/admit_tee_node.rs` — per-group
//!   `TeeAdmissionPolicy`;
//! - `crates/merod/src/kms/mod.rs` — `enforce_attestation_policy`
//!   (ReleaseStrict/Config split);
//! - the mero-tee KMS `get_key.rs` — key-release side (external consumer).
//!
//! `policy_valid` does not replace them; it exists so that *future* callers
//! cannot accidentally trust crypto-only validity.
//!
//! # Intentional duplication
//! The TCB rules below deliberately mirror
//! `calimero_governance_store::membership::policy_rules::tcb_status_allowed`
//! rather than importing it: this is a leaf crate (`publish = true`, consumed
//! externally by mero-tee's KMS at a pinned git rev) and must not take a
//! dependency on the governance store. The two implementations MUST stay in
//! sync — the constants and rule order here are a by-value copy of that
//! function's contract.

use crate::verify::VerificationResult;

/// Secure fail-closed default enforced when [`VerifierPolicy::allowed_tcb_statuses`]
/// is empty. An empty allowlist must NOT skip the TCB-status check (that would be
/// fail-open); it enforces against this single status instead.
///
/// Mirrors `calimero_governance_store::membership::policy_rules::DEFAULT_ALLOWED_TCB_STATUS`.
/// Must stay the exact PascalCase value dcap-qvl emits. Do not broaden.
pub const DEFAULT_ALLOWED_TCB_STATUS: &str = "UpToDate";

/// TCB status rejected unconditionally, regardless of policy (defense in depth).
///
/// Mirrors `calimero_governance_store::membership::policy_rules::TCB_STATUS_REVOKED`.
pub const TCB_STATUS_REVOKED: &str = "Revoked";

/// TCB status set by `verify_mock_attestation` (behind the default-off
/// `mock-attestation` feature; not linked here so this doc resolves in both
/// feature configurations). Real dcap-qvl never emits this value, so it
/// uniquely identifies the mock path.
///
/// Note this constant and [`VerifierPolicy::accept_mock`] are intentionally
/// *not* feature-gated: `policy_valid` only ever compares this status *string*
/// and never calls into the gated mock code, so the check must stay available
/// to production builds to reject mock quotes.
///
/// Mirrors `calimero_governance_store::membership::policy_rules::TCB_STATUS_MOCK`.
pub const TCB_STATUS_MOCK: &str = "Mock";

/// Measurement register named by [`PolicyRejection::MeasurementMismatch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasurementRegister {
    Mrtd,
    Rtmr0,
    Rtmr1,
    Rtmr2,
    Rtmr3,
}

impl MeasurementRegister {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Mrtd => "mrtd",
            Self::Rtmr0 => "rtmr0",
            Self::Rtmr1 => "rtmr1",
            Self::Rtmr2 => "rtmr2",
            Self::Rtmr3 => "rtmr3",
        }
    }
}

impl std::fmt::Display for MeasurementRegister {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why [`VerificationResult::policy_valid`] rejected an attestation.
///
/// Typed rather than a bare `bool` so callers can log/route an actionable
/// reason instead of "policy said no".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyRejection {
    /// One of the crypto/structural checks failed — see
    /// [`VerificationResult::is_valid`].
    NotCryptoValid,
    /// TCB status is `Revoked`. Rejected unconditionally, even if explicitly
    /// allowlisted and even under `accept_mock`.
    TcbRevoked,
    /// TCB status is not in the effective allowlist (which is
    /// [`DEFAULT_ALLOWED_TCB_STATUS`] when the configured allowlist is empty).
    TcbNotAllowed { status: String },
    /// A mock quote was presented but [`VerifierPolicy::accept_mock`] is false.
    MockNotAccepted,
    /// [`VerifierPolicy::allowed_mrtd`] is empty. `policy_valid` is a strict
    /// gate: an unpinned MRTD would admit any genuine TDX platform, so an empty
    /// MRTD allowlist is a rejection rather than a skip.
    EmptyMrtdAllowlist,
    /// A measurement register's value is not in its (non-empty) allowlist.
    MeasurementMismatch { register: MeasurementRegister },
    /// [`VerifierPolicy::require_app_hash`] is set but the quote's app-hash
    /// binding did not verify.
    AppHashNotBound,
}

impl std::fmt::Display for PolicyRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotCryptoValid => f.write_str(
                "attestation rejected: quote/nonce/app-hash verification did not all pass",
            ),
            Self::TcbRevoked => {
                f.write_str("attestation rejected: platform TCB status is Revoked")
            }
            Self::TcbNotAllowed { status } => write!(
                f,
                "attestation rejected: TCB status {status:?} is not in the policy allowlist"
            ),
            Self::MockNotAccepted => f.write_str(
                "attestation rejected: mock attestation presented but policy does not accept mock",
            ),
            Self::EmptyMrtdAllowlist => f.write_str(
                "attestation rejected: policy has an empty MRTD allowlist (fail-closed: an unpinned MRTD would admit any TDX platform)",
            ),
            Self::MeasurementMismatch { register } => write!(
                f,
                "attestation rejected: measurement {register} is not in the policy allowlist"
            ),
            Self::AppHashNotBound => f.write_str(
                "attestation rejected: policy requires an app-hash binding but it did not verify",
            ),
        }
    }
}

impl std::error::Error for PolicyRejection {}

/// The policy [`VerificationResult::policy_valid`] enforces.
///
/// Owned by this crate on purpose: this is a leaf crate, so it must not depend
/// on `calimero-governance-store`'s `TeeAdmissionPolicy`. It is a lean
/// duplicate-by-value that richer callers convert *into*.
///
/// Every field fails closed:
/// - an empty `allowed_tcb_statuses` enforces [`DEFAULT_ALLOWED_TCB_STATUS`],
///   it never skips the check;
/// - an empty `allowed_mrtd` is a rejection, never a skip;
/// - `accept_mock` defaults to `false` and `require_app_hash` to `true`.
///
/// The `allowed_rtmr0..3` allowlists are the one deliberate exception: an empty
/// one skips that register, because pinning RTMRs is optional in practice (they
/// vary with boot/runtime configuration) while MRTD is the workload identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifierPolicy {
    /// Allowed dcap-qvl TCB statuses. Empty => fail closed to
    /// [`DEFAULT_ALLOWED_TCB_STATUS`]. Compared case-sensitively (as upstream).
    pub allowed_tcb_statuses: Vec<String>,
    /// Allowed MRTD values (hex). Required non-empty — see
    /// [`PolicyRejection::EmptyMrtdAllowlist`].
    pub allowed_mrtd: Vec<String>,
    /// Allowed RTMR0 values (hex). Empty => register not checked.
    pub allowed_rtmr0: Vec<String>,
    /// Allowed RTMR1 values (hex). Empty => register not checked.
    pub allowed_rtmr1: Vec<String>,
    /// Allowed RTMR2 values (hex). Empty => register not checked.
    pub allowed_rtmr2: Vec<String>,
    /// Allowed RTMR3 values (hex). Empty => register not checked.
    pub allowed_rtmr3: Vec<String>,
    /// Whether mock attestations are acceptable. Mock quotes bypass all
    /// cryptographic guarantees; only ever set this in dev/test.
    pub accept_mock: bool,
    /// Whether the attestation must be bound to an app/identity hash.
    ///
    /// **Not a bypass toggle: this cannot make `policy_valid` accept anything
    /// it would otherwise reject.** The app-hash binding is unconditional —
    /// [`VerificationResult::is_valid`] already requires
    /// `application_hash_verified`, so an unbound quote is rejected either way.
    /// This flag only selects which rejection is *reported*:
    /// [`PolicyRejection::AppHashNotBound`] (the precise reason) when `true`,
    /// versus the blunter [`PolicyRejection::NotCryptoValid`] when `false`. See
    /// the `unbound_app_hash_is_rejected_even_when_not_required` test.
    pub require_app_hash: bool,
}

impl Default for VerifierPolicy {
    /// The secure baseline: no measurements pinned yet (so `policy_valid` will
    /// reject with [`PolicyRejection::EmptyMrtdAllowlist`] until the caller
    /// pins an MRTD), TCB fail-closed to [`DEFAULT_ALLOWED_TCB_STATUS`], mock
    /// rejected, app-hash binding required.
    fn default() -> Self {
        Self {
            allowed_tcb_statuses: Vec::new(),
            allowed_mrtd: Vec::new(),
            allowed_rtmr0: Vec::new(),
            allowed_rtmr1: Vec::new(),
            allowed_rtmr2: Vec::new(),
            allowed_rtmr3: Vec::new(),
            accept_mock: false,
            require_app_hash: true,
        }
    }
}

impl VerifierPolicy {
    /// A policy pinned to the given MRTD allowlist, secure defaults elsewhere.
    ///
    /// This is the intended entry point: MRTD is the one allowlist that has no
    /// safe default, so the constructor demands it up front.
    pub fn new(allowed_mrtd: impl IntoIterator<Item = String>) -> Self {
        Self {
            allowed_mrtd: allowed_mrtd.into_iter().collect(),
            ..Self::default()
        }
    }

    /// Whether `status` passes the TCB allowlist actually enforced: the
    /// configured one, or the secure default when it is empty.
    fn tcb_status_allowed(&self, status: &str) -> bool {
        if self.allowed_tcb_statuses.is_empty() {
            status == DEFAULT_ALLOWED_TCB_STATUS
        } else {
            self.allowed_tcb_statuses
                .iter()
                .any(|allowed| allowed == status)
        }
    }
}

impl VerificationResult {
    /// Safe-by-construction policy gate — use this, not [`Self::is_valid`], to
    /// decide whether to admit a peer, release a key, or grant a capability.
    ///
    /// [`Self::is_valid`] is crypto-only and always will be (it is external API
    /// consumed by the mero-tee KMS at a pinned rev). `policy_valid` layers the
    /// authorization decision on top of it, in this order:
    ///
    /// 1. **Crypto validity** — [`Self::is_valid`] must hold, else
    ///    [`PolicyRejection::NotCryptoValid`]. The app-hash component of it is
    ///    reported as [`PolicyRejection::AppHashNotBound`] when
    ///    `require_app_hash` is set, so callers get the specific reason; either
    ///    way `policy_valid` is never weaker than [`Self::is_valid`] (clearing
    ///    `require_app_hash` still cannot admit an unbound quote).
    /// 2. **TCB status** (fail-closed, mirroring
    ///    `calimero_governance_store::membership::policy_rules::tcb_status_allowed`):
    ///    `Revoked` (case-insensitive) is rejected unconditionally; a mock quote
    ///    passes the status check only under `accept_mock`; an empty
    ///    `allowed_tcb_statuses` enforces [`DEFAULT_ALLOWED_TCB_STATUS`] rather
    ///    than skipping; otherwise the status must be in the allowlist.
    /// 3. **Measurements** — MRTD must match a non-empty `allowed_mrtd` (an
    ///    empty one is [`PolicyRejection::EmptyMrtdAllowlist`]); each non-empty
    ///    `allowed_rtmr0..3` must match, empty ones are skipped.
    ///
    /// A missing `tcb_status` (`None`, which `verify_attestation` only produces
    /// when the crypto verification failed) is treated as not allowed.
    ///
    /// # Errors
    /// Returns the first [`PolicyRejection`] encountered.
    pub fn policy_valid(&self, policy: &VerifierPolicy) -> Result<(), PolicyRejection> {
        if !self.quote_verified || !self.nonce_verified {
            return Err(PolicyRejection::NotCryptoValid);
        }

        // `is_valid()` already folds in `application_hash_verified`; surfacing
        // it separately only buys the caller a specific reason. The `is_valid()`
        // guard below then keeps `policy_valid` from ever being weaker than
        // `is_valid()` — clearing `require_app_hash` must not admit an unbound
        // quote, it only changes which rejection is reported.
        if policy.require_app_hash && !self.application_hash_verified {
            return Err(PolicyRejection::AppHashNotBound);
        }
        if !self.is_valid() {
            return Err(PolicyRejection::NotCryptoValid);
        }

        let status = self.tcb_status.as_deref().unwrap_or_default();
        let is_mock = status == TCB_STATUS_MOCK;

        // Rule 1 of `tcb_status_allowed`: Revoked is rejected before anything
        // else, including the mock bypass.
        if status.eq_ignore_ascii_case(TCB_STATUS_REVOKED) {
            return Err(PolicyRejection::TcbRevoked);
        }

        // Rule 2: the mock bypass is gated on `accept_mock`, so a "Mock" status
        // is never a standing bypass token on a fleet that did not opt in.
        if is_mock {
            if !policy.accept_mock {
                return Err(PolicyRejection::MockNotAccepted);
            }
        // Rules 3 & 4: empty allowlist => the secure default; otherwise the
        // configured allowlist is honored exactly (case-sensitive).
        } else if !policy.tcb_status_allowed(status) {
            return Err(PolicyRejection::TcbNotAllowed {
                status: status.to_owned(),
            });
        }

        if policy.allowed_mrtd.is_empty() {
            return Err(PolicyRejection::EmptyMrtdAllowlist);
        }

        let body = &self.quote.body;
        for (allowlist, actual, register) in [
            (&policy.allowed_mrtd, &body.mrtd, MeasurementRegister::Mrtd),
            (
                &policy.allowed_rtmr0,
                &body.rtmr0,
                MeasurementRegister::Rtmr0,
            ),
            (
                &policy.allowed_rtmr1,
                &body.rtmr1,
                MeasurementRegister::Rtmr1,
            ),
            (
                &policy.allowed_rtmr2,
                &body.rtmr2,
                MeasurementRegister::Rtmr2,
            ),
            (
                &policy.allowed_rtmr3,
                &body.rtmr3,
                MeasurementRegister::Rtmr3,
            ),
        ] {
            if !allowlist.is_empty() && !allowlist.iter().any(|allowed| allowed == actual) {
                return Err(PolicyRejection::MeasurementMismatch { register });
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use calimero_server_primitives::admin::{
        CertificationData, QeReportCertificationDataInfo, Quote, QuoteBody, QuoteHeader,
    };

    use super::*;

    const MRTD_OK: &str = "aa11";
    const MRTD_OTHER: &str = "bb22";

    /// A minimal `Quote` fixture.
    ///
    /// Deliberately built here rather than via `generate::create_mock_quote`,
    /// which lives behind the default-off `mock-attestation` feature. These
    /// tests exercise `policy_valid`, which only ever reads `quote.body`
    /// measurement strings and never touches the mock code path — so they must
    /// (and do) run in both feature configurations. The field values are inert
    /// placeholders; every field `policy_valid` reads is overwritten below.
    fn test_quote() -> Quote {
        let zeros_48 = "0".repeat(96);
        let zeros_16 = "0".repeat(32);
        let zeros_8 = "0".repeat(16);

        Quote {
            header: QuoteHeader {
                version: 4,
                attestation_key_type: 2,
                tee_type: 0x81,
                qe_vendor_id: "939a7233f79c4ca9940a0db3957f0607".to_owned(),
                user_data: zeros_16.clone(),
            },
            body: QuoteBody {
                tdx_version: "1.0".to_owned(),
                tee_tcb_svn: zeros_16,
                mrseam: zeros_48.clone(),
                mrsignerseam: zeros_48.clone(),
                seamattributes: zeros_8.clone(),
                tdattributes: zeros_8.clone(),
                xfam: zeros_8,
                mrtd: zeros_48.clone(),
                mrconfigid: zeros_48.clone(),
                mrowner: zeros_48.clone(),
                mrownerconfig: zeros_48.clone(),
                rtmr0: zeros_48.clone(),
                rtmr1: zeros_48.clone(),
                rtmr2: zeros_48.clone(),
                rtmr3: zeros_48,
                reportdata: "0".repeat(128),
                tee_tcb_svn_2: None,
                mrservicetd: None,
            },
            signature: "0".repeat(128),
            attestation_key: "04".to_owned() + &"0".repeat(128),
            certification_data: CertificationData::QeReportCertificationData(
                QeReportCertificationDataInfo {
                    qe_report: "0".repeat(768),
                    signature: "0".repeat(128),
                    qe_authentication_data: "0".repeat(64),
                    certification_data_type: "PckCertChain".to_owned(),
                    certification_data: "0".repeat(200),
                },
            ),
        }
    }

    fn result_with(tcb_status: &str) -> VerificationResult {
        let mut quote = test_quote();
        quote.body.mrtd = MRTD_OK.to_owned();
        quote.body.rtmr0 = "r0".to_owned();
        quote.body.rtmr1 = "r1".to_owned();
        quote.body.rtmr2 = "r2".to_owned();
        quote.body.rtmr3 = "r3".to_owned();

        VerificationResult {
            quote_verified: true,
            nonce_verified: true,
            application_hash_verified: true,
            tcb_status: Some(tcb_status.to_owned()),
            advisory_ids: Vec::new(),
            quote,
        }
    }

    fn policy() -> VerifierPolicy {
        VerifierPolicy::new([MRTD_OK.to_owned()])
    }

    #[test]
    fn happy_path_accepts_uptodate_pinned_measurements() {
        let result = result_with("UpToDate");
        let mut policy = policy();
        policy.allowed_tcb_statuses = vec!["UpToDate".to_owned()];
        policy.allowed_rtmr0 = vec!["r0".to_owned()];
        policy.allowed_rtmr1 = vec!["r1".to_owned()];
        policy.allowed_rtmr2 = vec!["r2".to_owned()];
        policy.allowed_rtmr3 = vec!["r3".to_owned()];

        assert_eq!(result.policy_valid(&policy), Ok(()));
    }

    #[test]
    fn defaults_are_secure() {
        let policy = VerifierPolicy::default();
        assert!(!policy.accept_mock);
        assert!(policy.require_app_hash);
        // `new` only pins MRTD; the rest stay at the secure defaults.
        let pinned = VerifierPolicy::new([MRTD_OK.to_owned()]);
        assert!(!pinned.accept_mock);
        assert!(pinned.require_app_hash);
        assert!(pinned.allowed_tcb_statuses.is_empty());
    }

    #[test]
    fn not_crypto_valid_is_rejected_first() {
        let mut result = result_with("UpToDate");
        result.quote_verified = false;
        assert_eq!(
            result.policy_valid(&policy()),
            Err(PolicyRejection::NotCryptoValid)
        );

        let mut result = result_with("UpToDate");
        result.nonce_verified = false;
        assert_eq!(
            result.policy_valid(&policy()),
            Err(PolicyRejection::NotCryptoValid)
        );
    }

    #[test]
    fn empty_tcb_allowlist_fails_closed_to_uptodate() {
        let policy = policy();
        assert!(policy.allowed_tcb_statuses.is_empty());

        // The secure default is enforced, not skipped.
        assert_eq!(result_with("UpToDate").policy_valid(&policy), Ok(()));
        assert_eq!(
            result_with("OutOfDate").policy_valid(&policy),
            Err(PolicyRejection::TcbNotAllowed {
                status: "OutOfDate".to_owned()
            })
        );
        assert_eq!(
            result_with("SWHardeningNeeded").policy_valid(&policy),
            Err(PolicyRejection::TcbNotAllowed {
                status: "SWHardeningNeeded".to_owned()
            })
        );
    }

    #[test]
    fn missing_tcb_status_is_not_allowed() {
        let mut result = result_with("UpToDate");
        result.tcb_status = None;
        assert_eq!(
            result.policy_valid(&policy()),
            Err(PolicyRejection::TcbNotAllowed {
                status: String::new()
            })
        );
    }

    #[test]
    fn non_empty_tcb_allowlist_is_honored() {
        let mut policy = policy();
        policy.allowed_tcb_statuses = vec!["OutOfDate".to_owned()];
        assert_eq!(result_with("OutOfDate").policy_valid(&policy), Ok(()));
        assert_eq!(
            result_with("UpToDate").policy_valid(&policy),
            Err(PolicyRejection::TcbNotAllowed {
                status: "UpToDate".to_owned()
            })
        );
    }

    #[test]
    fn revoked_is_rejected_even_when_allowlisted() {
        let mut policy = policy();
        policy.allowed_tcb_statuses = vec!["Revoked".to_owned(), "UpToDate".to_owned()];
        assert_eq!(
            result_with("Revoked").policy_valid(&policy),
            Err(PolicyRejection::TcbRevoked)
        );
    }

    #[test]
    fn revoked_is_rejected_even_with_accept_mock() {
        let mut policy = policy();
        policy.accept_mock = true;
        policy.allowed_tcb_statuses = vec!["Revoked".to_owned()];
        assert_eq!(
            result_with("Revoked").policy_valid(&policy),
            Err(PolicyRejection::TcbRevoked)
        );
    }

    #[test]
    fn revoked_match_is_case_insensitive() {
        for status in ["revoked", "REVOKED", "ReVoKeD"] {
            assert_eq!(
                result_with(status).policy_valid(&policy()),
                Err(PolicyRejection::TcbRevoked),
                "status {status} must be rejected"
            );
        }
    }

    #[test]
    fn mock_is_rejected_unless_accept_mock() {
        let mut policy = policy();
        assert_eq!(
            result_with(TCB_STATUS_MOCK).policy_valid(&policy),
            Err(PolicyRejection::MockNotAccepted)
        );

        policy.accept_mock = true;
        assert_eq!(result_with(TCB_STATUS_MOCK).policy_valid(&policy), Ok(()));
    }

    #[test]
    fn mock_is_rejected_even_when_status_allowlisted_without_accept_mock() {
        let mut policy = policy();
        // An allowlisted "Mock" must NOT be a bypass token on a fleet that did
        // not opt into mock.
        policy.allowed_tcb_statuses = vec![TCB_STATUS_MOCK.to_owned()];
        assert_eq!(
            result_with(TCB_STATUS_MOCK).policy_valid(&policy),
            Err(PolicyRejection::MockNotAccepted)
        );
    }

    #[test]
    fn accept_mock_does_not_admit_a_real_disallowed_status() {
        let mut policy = policy();
        policy.accept_mock = true;
        assert_eq!(
            result_with("OutOfDate").policy_valid(&policy),
            Err(PolicyRejection::TcbNotAllowed {
                status: "OutOfDate".to_owned()
            })
        );
    }

    #[test]
    fn empty_mrtd_allowlist_is_rejected() {
        let policy = VerifierPolicy::default();
        assert_eq!(
            result_with("UpToDate").policy_valid(&policy),
            Err(PolicyRejection::EmptyMrtdAllowlist)
        );
    }

    #[test]
    fn empty_mrtd_allowlist_is_rejected_even_for_mock() {
        let policy = VerifierPolicy {
            accept_mock: true,
            ..VerifierPolicy::default()
        };
        assert_eq!(
            result_with(TCB_STATUS_MOCK).policy_valid(&policy),
            Err(PolicyRejection::EmptyMrtdAllowlist)
        );
    }

    #[test]
    fn mrtd_mismatch_is_rejected() {
        let policy = VerifierPolicy::new([MRTD_OTHER.to_owned()]);
        assert_eq!(
            result_with("UpToDate").policy_valid(&policy),
            Err(PolicyRejection::MeasurementMismatch {
                register: MeasurementRegister::Mrtd
            })
        );
    }

    #[test]
    fn empty_rtmr_allowlists_are_skipped() {
        let policy = policy();
        assert!(policy.allowed_rtmr0.is_empty());
        assert_eq!(result_with("UpToDate").policy_valid(&policy), Ok(()));
    }

    #[test]
    fn non_empty_rtmr_allowlists_must_match() {
        for (register, apply) in [
            (
                MeasurementRegister::Rtmr0,
                (|p: &mut VerifierPolicy| p.allowed_rtmr0 = vec!["nope".to_owned()])
                    as fn(&mut VerifierPolicy),
            ),
            (MeasurementRegister::Rtmr1, |p| {
                p.allowed_rtmr1 = vec!["nope".to_owned()]
            }),
            (MeasurementRegister::Rtmr2, |p| {
                p.allowed_rtmr2 = vec!["nope".to_owned()]
            }),
            (MeasurementRegister::Rtmr3, |p| {
                p.allowed_rtmr3 = vec!["nope".to_owned()]
            }),
        ] {
            let mut policy = policy();
            apply(&mut policy);
            assert_eq!(
                result_with("UpToDate").policy_valid(&policy),
                Err(PolicyRejection::MeasurementMismatch { register }),
                "{register} mismatch must be rejected"
            );
        }
    }

    #[test]
    fn unbound_app_hash_is_rejected_when_required() {
        let mut result = result_with("UpToDate");
        result.application_hash_verified = false;

        let policy = policy();
        assert!(policy.require_app_hash);
        assert_eq!(
            result.policy_valid(&policy),
            Err(PolicyRejection::AppHashNotBound)
        );
    }

    #[test]
    fn unbound_app_hash_is_rejected_even_when_not_required() {
        // `policy_valid` must never be weaker than `is_valid()`: clearing
        // `require_app_hash` changes the reported reason, not the verdict.
        let mut result = result_with("UpToDate");
        result.application_hash_verified = false;

        let mut policy = policy();
        policy.require_app_hash = false;
        assert_eq!(
            result.policy_valid(&policy),
            Err(PolicyRejection::NotCryptoValid)
        );
    }

    #[test]
    fn rejection_displays_actionable_message() {
        let rejection = PolicyRejection::MeasurementMismatch {
            register: MeasurementRegister::Mrtd,
        };
        assert!(rejection.to_string().contains("mrtd"));
        let err: &dyn std::error::Error = &rejection;
        assert!(!err.to_string().is_empty());
    }
}
