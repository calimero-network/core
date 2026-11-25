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
//! # Relationship to other enforcement sites
//! `calimero-client`'s `attest::verify` (the one quote check clients and peers
//! share) uses `policy_valid`. Two sites implement equivalent enforcement
//! inline and keep it by design (they emit richer, mode-specific errors this
//! crate cannot):
//! - `calimero-kms-client`'s `kms.rs` — `enforce_attestation_policy`
//!   (ReleaseStrict/Config split), over `is_valid`;
//! - the mero-tee KMS `get_key.rs` — key-release side (external consumer).
//!
//! # Keep in step with mero-kms
//! This is a leaf crate (`publish = true`, consumed externally by mero-tee's
//! KMS at a pinned git rev). The TCB rules below (the constants and their
//! order) are the contract a node's admission and the KMS's key release must
//! agree on: a change here is a change there.

use crate::verify::VerificationResult;

/// Secure fail-closed default enforced when [`VerifierPolicy::allowed_tcb_statuses`]
/// is empty. An empty allowlist must NOT skip the TCB-status check (that would be
/// fail-open); it enforces against this single status instead.
///
/// Must stay the exact PascalCase value dcap-qvl emits. Do not broaden.
pub const DEFAULT_ALLOWED_TCB_STATUS: &str = "UpToDate";

/// TCB status rejected unconditionally, regardless of policy (defense in depth).
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
pub const TCB_STATUS_MOCK: &str = "Mock";

/// The oldest Intel TCB evaluation data (`tcbEvaluationDataNumber`) a verdict
/// may be computed against, enforced by [`VerificationResult::policy_valid`]
/// whatever [`VerifierPolicy::min_tcb_evaluation_data_number`] says.
///
/// `tcb_status` is only as current as the baseline that produced it, and the
/// collateral source is configurable (`CALIMERO_TEE_COLLATERAL_URL` may name
/// any PCCS). Intel signs every baseline it ever published, so a source serving
/// an older one passes signature checks and makes a platform that has since
/// fallen behind read `UpToDate`. This floor refuses such a baseline.
///
/// 20 is the `update=standard` baseline Intel PCS was serving on 2026-08-11,
/// when `update=early` had moved on to 22 (see `verify.rs`'s tests); this
/// crate asks for the standard one, so 20 refuses only baselines Intel had
/// already replaced. 17, in the 2025 fixture collateral, is below it.
///
/// # Bumping it
/// Intel announces each new number with a TCB Recovery and serves it at
/// `update=standard` once the recovery's grace period ends. Raise this to the
/// standard number once every collateral source the fleet uses
/// (`CALIMERO_TEE_COLLATERAL_URL`, Intel PCS when unset) serves it — check the
/// `tcb_evaluation_data_number` nodes log with each verdict — in a `fix:` PR.
/// `calimero-kms-client` holds the KMS's quote to this same constant; the SDK
/// verifiers (mero-js, swift-sdk, kotlin-sdk) keep their own copy, raised in
/// step.
pub const DEFAULT_MIN_TCB_EVALUATION_DATA_NUMBER: u32 = 20;

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
    /// One of `allowed_rtmr1..3` is empty. The MRTD measures the TD firmware,
    /// which on a cloud platform every image shares (on GCP every mero-tee
    /// image, profile and release has the same MRTD and RTMR0); the image —
    /// kernel, command line with the root-filesystem hash, and the runtime
    /// role and profile — is in RTMR1-3. A policy that leaves any of them
    /// unpinned admits a TD running anything, so it is a rejection.
    EmptyImageAllowlist { register: MeasurementRegister },
    /// A measurement register's value is not in its (non-empty) allowlist.
    MeasurementMismatch { register: MeasurementRegister },
    /// [`VerifierPolicy::require_app_hash`] is set but the quote's app-hash
    /// binding did not verify.
    AppHashNotBound,
    /// The TCB status was computed against Intel TCB evaluation data older
    /// than the floor (or of unknown age, `number: None`), so it may call a
    /// platform current that a newer baseline calls out of date.
    TcbEvaluationDataTooOld { number: Option<u32>, floor: u32 },
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
            Self::EmptyImageAllowlist { register } => write!(
                f,
                "attestation rejected: policy has an empty {register} allowlist (fail-closed: the MRTD measures the firmware, not the image; RTMR1-3 must be pinned)"
            ),
            Self::MeasurementMismatch { register } => write!(
                f,
                "attestation rejected: measurement {register} is not in the policy allowlist"
            ),
            Self::AppHashNotBound => f.write_str(
                "attestation rejected: policy requires an app-hash binding but it did not verify",
            ),
            Self::TcbEvaluationDataTooOld {
                number: Some(number),
                floor,
            } => write!(
                f,
                "attestation rejected: the TCB status was computed against Intel TCB evaluation data {number}, older than the minimum {floor}; point the collateral source at current collateral"
            ),
            Self::TcbEvaluationDataTooOld {
                number: None,
                floor,
            } => write!(
                f,
                "attestation rejected: the TCB evaluation data the TCB status was computed against is unknown (minimum {floor})"
            ),
        }
    }
}

impl std::error::Error for PolicyRejection {}

/// The policy [`VerificationResult::policy_valid`] enforces.
///
/// Owned by this crate on purpose: this is a leaf crate, so it depends on no
/// caller's policy type. It is lean, and richer callers convert *into* it
/// (`calimero-client`'s `attest::verifier_policy`, from a release profile).
///
/// Every field fails closed:
/// - an empty `allowed_tcb_statuses` enforces [`DEFAULT_ALLOWED_TCB_STATUS`],
///   it never skips the check;
/// - an empty `allowed_mrtd` is a rejection, never a skip;
/// - so is an empty `allowed_rtmr1`, `allowed_rtmr2` or `allowed_rtmr3`: the
///   MRTD measures the TD firmware, which images share, and the image is in
///   RTMR1-3 (see [`PolicyRejection::EmptyImageAllowlist`]);
/// - `accept_mock` defaults to `false` and `require_app_hash` to `true`;
/// - `min_tcb_evaluation_data_number` can raise the floor on Intel's TCB
///   evaluation data, never lower it below
///   [`DEFAULT_MIN_TCB_EVALUATION_DATA_NUMBER`].
///
/// `allowed_rtmr0` is the one exception: an empty one skips that register. It
/// measures the firmware's configuration (the virtual hardware), not the image.
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
    /// Allowed RTMR1 values (hex). Required non-empty — see
    /// [`PolicyRejection::EmptyImageAllowlist`].
    pub allowed_rtmr1: Vec<String>,
    /// Allowed RTMR2 values (hex). Required non-empty — see
    /// [`PolicyRejection::EmptyImageAllowlist`].
    pub allowed_rtmr2: Vec<String>,
    /// Allowed RTMR3 values (hex). Required non-empty — see
    /// [`PolicyRejection::EmptyImageAllowlist`].
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
    /// The oldest Intel TCB evaluation data a real quote's `tcb_status` may be
    /// computed against. The floor enforced is the higher of this and
    /// [`DEFAULT_MIN_TCB_EVALUATION_DATA_NUMBER`]: a policy can demand newer
    /// collateral, never accept older. Not consulted for a mock quote, which
    /// has no collateral.
    pub min_tcb_evaluation_data_number: u32,
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
            min_tcb_evaluation_data_number: DEFAULT_MIN_TCB_EVALUATION_DATA_NUMBER,
        }
    }
}

impl VerifierPolicy {
    /// A policy pinned to the given MRTD allowlist, secure defaults elsewhere.
    ///
    /// The MRTD alone admits nothing: pin `allowed_rtmr1..3` too, which name
    /// the image (see [`PolicyRejection::EmptyImageAllowlist`]).
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

    /// The evaluation data floor actually enforced: the configured one, never
    /// below the compiled-in default.
    fn tcb_evaluation_data_floor(&self) -> u32 {
        self.min_tcb_evaluation_data_number
            .max(DEFAULT_MIN_TCB_EVALUATION_DATA_NUMBER)
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
    /// 2. **TCB status** (fail-closed):
    ///    `Revoked` (case-insensitive) is rejected unconditionally; a mock quote
    ///    passes the status check only under `accept_mock`; an empty
    ///    `allowed_tcb_statuses` enforces [`DEFAULT_ALLOWED_TCB_STATUS`] rather
    ///    than skipping; otherwise the status must be in the allowlist.
    ///    A real quote's status must also have been computed against TCB
    ///    evaluation data at or above the floor
    ///    ([`VerifierPolicy::min_tcb_evaluation_data_number`], never below
    ///    [`DEFAULT_MIN_TCB_EVALUATION_DATA_NUMBER`]); an unknown number is
    ///    [`PolicyRejection::TcbEvaluationDataTooOld`] too.
    /// 3. **Measurements** — MRTD must match a non-empty `allowed_mrtd` (an
    ///    empty one is [`PolicyRejection::EmptyMrtdAllowlist`]), and RTMR1-3
    ///    each a non-empty allowlist (an empty one is
    ///    [`PolicyRejection::EmptyImageAllowlist`]); RTMR0 must match when its
    ///    allowlist is non-empty, and is skipped when it is empty.
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
        } else {
            // A status is only as current as the baseline that produced it.
            let floor = policy.tcb_evaluation_data_floor();
            let number = self.tcb_evaluation_data_number;
            if number.is_none_or(|number| number < floor) {
                return Err(PolicyRejection::TcbEvaluationDataTooOld { number, floor });
            }
        }

        if policy.allowed_mrtd.is_empty() {
            return Err(PolicyRejection::EmptyMrtdAllowlist);
        }
        for (allowlist, register) in [
            (&policy.allowed_rtmr1, MeasurementRegister::Rtmr1),
            (&policy.allowed_rtmr2, MeasurementRegister::Rtmr2),
            (&policy.allowed_rtmr3, MeasurementRegister::Rtmr3),
        ] {
            if allowlist.is_empty() {
                return Err(PolicyRejection::EmptyImageAllowlist { register });
            }
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
    use crate::quote::{
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
            tcb_evaluation_data_number: Some(DEFAULT_MIN_TCB_EVALUATION_DATA_NUMBER),
            quote,
        }
    }

    /// A policy pinning the test quote's image: MRTD and RTMR1-3.
    fn policy() -> VerifierPolicy {
        let mut policy = VerifierPolicy::new([MRTD_OK.to_owned()]);
        policy.allowed_rtmr1 = vec!["r1".to_owned()];
        policy.allowed_rtmr2 = vec!["r2".to_owned()];
        policy.allowed_rtmr3 = vec!["r3".to_owned()];
        policy
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
        let mut policy = policy();
        policy.allowed_mrtd = vec![MRTD_OTHER.to_owned()];
        assert_eq!(
            result_with("UpToDate").policy_valid(&policy),
            Err(PolicyRejection::MeasurementMismatch {
                register: MeasurementRegister::Mrtd
            })
        );
    }

    #[test]
    fn empty_rtmr0_allowlist_is_skipped() {
        let policy = policy();
        assert!(policy.allowed_rtmr0.is_empty());
        assert_eq!(result_with("UpToDate").policy_valid(&policy), Ok(()));
    }

    #[test]
    fn empty_image_allowlists_are_rejected() {
        for (register, clear) in [
            (
                MeasurementRegister::Rtmr1,
                (|p: &mut VerifierPolicy| p.allowed_rtmr1.clear()) as fn(&mut VerifierPolicy),
            ),
            (MeasurementRegister::Rtmr2, |p| p.allowed_rtmr2.clear()),
            (MeasurementRegister::Rtmr3, |p| p.allowed_rtmr3.clear()),
        ] {
            let mut policy = policy();
            clear(&mut policy);
            assert_eq!(
                result_with("UpToDate").policy_valid(&policy),
                Err(PolicyRejection::EmptyImageAllowlist { register }),
                "an MRTD-only policy must not admit a quote with {register} unpinned"
            );
        }
        // An MRTD alone, as `VerifierPolicy::new` pins it, admits nothing.
        assert_eq!(
            result_with("UpToDate").policy_valid(&VerifierPolicy::new([MRTD_OK.to_owned()])),
            Err(PolicyRejection::EmptyImageAllowlist {
                register: MeasurementRegister::Rtmr1
            })
        );
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

    /// Regression: a collateral source (`CALIMERO_TEE_COLLATERAL_URL` can name any
    /// PCCS) serving an older, still Intel-signed TCB baseline makes an
    /// out-of-date platform read `UpToDate`. The verdict is only as good as the
    /// baseline it was computed against, so one below the floor admits nothing.
    #[test]
    fn up_to_date_against_a_baseline_below_the_floor_is_refused() {
        let mut result = result_with("UpToDate");
        result.tcb_evaluation_data_number = Some(17);
        let verdict = result.policy_valid(&policy());
        assert!(
            verdict.is_err(),
            "UpToDate measured against evaluation data 17 must not admit: {verdict:?}"
        );
    }

    /// The fixture collateral (fetched from Intel PCS in June 2025) is a real,
    /// Intel-signed baseline that is now below the floor: a source serving it
    /// today is refused with the number and the floor named.
    #[test]
    fn a_verdict_from_the_2025_fixture_collateral_is_refused() {
        let collateral: dcap_qvl::QuoteCollateralV3 = serde_json::from_slice(include_bytes!(
            "../tests/fixtures/tdx_quote_collateral.json"
        ))
        .expect("fixture collateral parses");
        let number = crate::verify::tcb_evaluation_data_number_of(&collateral.tcb_info);
        assert_eq!(number, Some(17));

        let mut result = result_with("UpToDate");
        result.tcb_evaluation_data_number = number;
        assert_eq!(
            result.policy_valid(&policy()),
            Err(PolicyRejection::TcbEvaluationDataTooOld {
                number: Some(17),
                floor: DEFAULT_MIN_TCB_EVALUATION_DATA_NUMBER,
            })
        );
    }

    #[test]
    fn a_baseline_at_or_above_the_floor_is_accepted() {
        for number in [
            DEFAULT_MIN_TCB_EVALUATION_DATA_NUMBER,
            DEFAULT_MIN_TCB_EVALUATION_DATA_NUMBER + 2,
        ] {
            let mut result = result_with("UpToDate");
            result.tcb_evaluation_data_number = Some(number);
            assert_eq!(result.policy_valid(&policy()), Ok(()), "number {number}");
        }
    }

    /// The floor is a gate, not a diagnostic: a real verdict whose baseline is
    /// unknown is refused rather than waved through.
    #[test]
    fn an_unknown_baseline_is_refused() {
        let mut result = result_with("UpToDate");
        result.tcb_evaluation_data_number = None;
        assert_eq!(
            result.policy_valid(&policy()),
            Err(PolicyRejection::TcbEvaluationDataTooOld {
                number: None,
                floor: DEFAULT_MIN_TCB_EVALUATION_DATA_NUMBER,
            })
        );
    }

    /// A policy can demand newer collateral than the default, never older.
    #[test]
    fn a_policy_raises_the_floor_but_cannot_lower_it() {
        let mut raised = policy();
        raised.min_tcb_evaluation_data_number = 22;
        let mut result = result_with("UpToDate");
        result.tcb_evaluation_data_number = Some(21);
        assert_eq!(
            result.policy_valid(&raised),
            Err(PolicyRejection::TcbEvaluationDataTooOld {
                number: Some(21),
                floor: 22,
            })
        );

        let mut lowered = policy();
        lowered.min_tcb_evaluation_data_number = 0;
        result.tcb_evaluation_data_number = Some(17);
        assert_eq!(
            result.policy_valid(&lowered),
            Err(PolicyRejection::TcbEvaluationDataTooOld {
                number: Some(17),
                floor: DEFAULT_MIN_TCB_EVALUATION_DATA_NUMBER,
            })
        );
    }

    /// A mock quote fetched no collateral, so it has no baseline; it stays
    /// governed by `accept_mock` alone.
    #[test]
    fn a_mock_quote_has_no_baseline_to_judge() {
        let mut result = result_with(TCB_STATUS_MOCK);
        result.tcb_evaluation_data_number = None;
        let mut policy = policy();
        policy.accept_mock = true;
        assert_eq!(result.policy_valid(&policy), Ok(()));
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
