//! What a TD saw of the mero-kms replica it took a key from.
//!
//! The KMS client verifies a replica's quote against a signed mero-kms release
//! policy before it asks for a key (`calimero-kms-client`), and records the
//! quote it accepted as a [`KmsEvidence`]. A node reports that record in its
//! own attestation, bound into its quote, so whoever verifies the node also
//! learns which KMS gave it its keys.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::verify::VerificationResult;

/// The KMS quote a key fetch accepted, and the policy that accepted it.
///
/// Every field is either read from the verified quote or names the policy the
/// quote was checked against; nothing is the KMS's own description of itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KmsEvidence {
    /// The KMS URL's host (and port, when one is given), as the TD dialled it.
    pub kms_host: String,
    /// The mero-kms release whose signed policy the quote matched, without the
    /// `mero-kms-v` prefix.
    pub release: String,
    /// The image profile that policy is for, when it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// The KMS quote's registers, lowercase hex.
    pub mrtd: String,
    pub rtmr0: String,
    pub rtmr1: String,
    pub rtmr2: String,
    pub rtmr3: String,
    /// The TCB status DCAP verification reported for the KMS's platform.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcb_status: Option<String>,
    /// SHA-256 of the KMS quote's bytes, hex.
    pub quote_sha256: String,
    /// When the quote was verified, in seconds since the Unix epoch.
    pub verified_at: u64,
    /// The quote was a mock one (development builds only; never in a shipped
    /// node, which refuses mock KMS quotes).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mock: bool,
}

impl KmsEvidence {
    /// The record of the quote `quote_bytes`, whose verification is
    /// `verified`, accepted under the policy of `release`/`profile` for the KMS
    /// at `kms_host`, at `verified_at`.
    #[must_use]
    pub fn of_quote(
        kms_host: String,
        release: String,
        profile: Option<String>,
        quote_bytes: &[u8],
        verified: &VerificationResult,
        verified_at: u64,
        mock: bool,
    ) -> Self {
        let register = |value: &str| value.trim().to_ascii_lowercase();
        let body = &verified.quote.body;
        Self {
            kms_host,
            release,
            profile,
            mrtd: register(&body.mrtd),
            rtmr0: register(&body.rtmr0),
            rtmr1: register(&body.rtmr1),
            rtmr2: register(&body.rtmr2),
            rtmr3: register(&body.rtmr3),
            tcb_status: verified.tcb_status.clone(),
            quote_sha256: hex::encode(Sha256::digest(quote_bytes)),
            verified_at,
            mock,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::KmsEvidence;

    fn sample() -> KmsEvidence {
        KmsEvidence {
            kms_host: "kms.example:8080".into(),
            release: "2.4.1".into(),
            profile: Some("locked-read-only".into()),
            mrtd: "aa".repeat(48),
            rtmr0: "00".repeat(48),
            rtmr1: "11".repeat(48),
            rtmr2: "22".repeat(48),
            rtmr3: "33".repeat(48),
            tcb_status: Some("UpToDate".into()),
            quote_sha256: "44".repeat(32),
            verified_at: 1_790_000_000,
            mock: false,
        }
    }

    /// The JSON a node reports and a verifier hashes: camelCase, and an
    /// absent profile or TCB status (or a real quote's `mock`) left out.
    #[test]
    fn the_evidence_serializes_in_its_published_shape() {
        let json = serde_json::to_value(sample()).unwrap();
        assert_eq!(json["kmsHost"], "kms.example:8080");
        assert_eq!(json["quoteSha256"], "44".repeat(32));
        assert_eq!(json["verifiedAt"], 1_790_000_000_u64);
        assert!(
            json.get("mock").is_none(),
            "a real quote names no mock flag"
        );

        let bare = KmsEvidence {
            profile: None,
            tcb_status: None,
            ..sample()
        };
        let json = serde_json::to_value(bare).unwrap();
        assert!(json.get("profile").is_none());
        assert!(json.get("tcbStatus").is_none());
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let mut json = serde_json::to_value(sample()).unwrap();
        json["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<KmsEvidence>(json).is_err());
    }
}
