#![no_main]
//! Release measurements: a node or client admits a peer's quote on the node
//! release's `published-mrtds.json` (`NodeRelease::from_published_mrtds`), and
//! a node trusts its KMS on the KMS release's policy (`kms-client`'s
//! `parse_policy_json`). Both are signed, but by whoever cut that release,
//! and the version they are checked against comes from the peer.
//!
//! The input is a version, a newline, a profile (empty for none), a newline,
//! and the JSON. What parses is for that version, and every value it admits
//! on is a 48-byte register as a verified quote renders it, so a quote can
//! only match it exactly: lowercase hex, no prefix, no padding.

use calimero_kms_client::policy::fuzz_api::{parse_policy_json, parse_policy_json_for_release};
use calimero_tee_release::NodeRelease;
use libfuzzer_sys::fuzz_target;

fn is_register(value: &str) -> bool {
    value.len() == 96
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let mut parts = text.splitn(3, '\n');
    let (version, profile, json) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    );
    let profile = (!profile.is_empty()).then_some(profile);

    if let Ok(release) = NodeRelease::from_published_mrtds(json, version) {
        assert_eq!(release.version, version);
        for p in release.profiles.values() {
            for list in [
                &p.allowed_mrtd,
                &p.allowed_rtmr1,
                &p.allowed_rtmr2,
                &p.allowed_rtmr3,
            ] {
                assert!(!list.is_empty(), "a profile admits on an unpinned register");
            }
            for list in [
                &p.allowed_mrtd,
                &p.allowed_rtmr0,
                &p.allowed_rtmr1,
                &p.allowed_rtmr2,
                &p.allowed_rtmr3,
            ] {
                assert!(list.iter().all(|v| is_register(v)), "{list:?}");
            }
        }
    }

    let any = parse_policy_json(json);
    if let Ok(policy) = parse_policy_json_for_release(json, version, profile) {
        assert_eq!(policy.release, version, "a policy for another release");
        if let Some(profile) = profile {
            assert_eq!(
                policy.profile.as_deref(),
                Some(profile),
                "a policy for another profile"
            );
        }
        assert!(any.is_ok(), "a release's policy does not parse on its own");
    }
    if let Ok(policy) = any {
        for list in [
            &policy.allowed_mrtd,
            &policy.allowed_rtmr0,
            &policy.allowed_rtmr1,
            &policy.allowed_rtmr2,
            &policy.allowed_rtmr3,
        ] {
            assert!(
                !list.is_empty(),
                "the KMS is admitted on an unpinned register"
            );
            assert!(list.iter().all(|v| is_register(v)), "{list:?}");
        }
        assert!(!policy.allowed_tcb_statuses.is_empty());
        assert!(policy
            .allowed_tcb_statuses
            .iter()
            .all(|s| !s.is_empty() && *s == s.to_lowercase()));
    }
});
