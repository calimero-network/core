//! That an external signature is the bytes the verifier asked for, and that the
//! allowlist cannot be talked into reaching a core credential.

use calimero_primitives::identity::PrivateKey;

use crate::domain::ALL_DOMAINS;
use crate::tests::support::key;
use crate::{sign_external, ExternalSigningDomain};

const ALL_EXTERNAL: &[ExternalSigningDomain] = &[
    ExternalSigningDomain::MdmaAccountLink,
    ExternalSigningDomain::MdmaAccountLogin,
    ExternalSigningDomain::MdmaAccountRecovery,
];

/// The signed message is `domain ‖ payload`, concatenated and signed as-is.
///
/// Pinned because it is a cross-repo contract: mdma verifies against exactly
/// these bytes, and a "tidier" encoding here — length prefixes, a hash, a
/// reordering — is a signature that fails at the far end for reasons no test in
/// this repo would otherwise show.
#[test]
fn the_signed_message_is_the_domain_then_the_payload() {
    let root = key(7);
    let payload = b"eyJ1c2UiOiJhY2NvdW50LWxvZ2luIn0.dGFn";

    let (pk, sig) = sign_external(&root, ExternalSigningDomain::MdmaAccountLogin, payload)
        .expect("sign under a known domain");

    let mut expected = b"calimero.mdma.account-login.v1\0".to_vec();
    expected.extend_from_slice(payload);

    assert!(
        pk.verify_raw_signature(&expected, &sig).is_ok(),
        "the signature must verify over `domain ‖ payload` and nothing else",
    );
    assert_eq!(pk, root.public_key(), "the reported key must be the signer");
}

/// Each domain yields a different signature over the same payload.
///
/// This is the confused-deputy property mdma states in its own constants: a
/// signature gathered while linking must not be replayable as a login. It holds
/// only because the domain is inside the signed message.
#[test]
fn one_payload_signs_differently_under_each_domain() {
    let root = key(8);
    let payload = b"same-nonce";

    let mut seen = Vec::new();
    for &domain in ALL_EXTERNAL {
        let (_, sig) = sign_external(&root, domain, payload).expect("sign");
        assert!(
            !seen.contains(&sig),
            "{domain:?} produced a signature another domain already produced",
        );
        seen.push(sig);
    }
}

/// A caller cannot name a domain that is not on the list.
#[test]
fn only_the_allowlisted_names_resolve() {
    for name in ExternalSigningDomain::names() {
        assert!(
            ExternalSigningDomain::from_name(name).is_some(),
            "{name} is advertised by names() but does not resolve",
        );
    }

    for rejected in [
        "",
        "mdma",
        "mdma.account-link ",
        "calimero.mdma.account-link.v1",
        "calimero.device.cert.v1",
        "../mdma.account-link",
        "MDMA.ACCOUNT-LINK",
    ] {
        assert!(
            ExternalSigningDomain::from_name(rejected).is_none(),
            "{rejected:?} must not resolve to a signing domain",
        );
    }
}

/// **The guard the allowlist exists to provide.**
///
/// No external domain may be a prefix of a core signing domain, or vice versa.
/// Because every external signature is `domain ‖ payload` with a caller-chosen
/// payload, a core domain that began with an external one would let a caller
/// walk the rest of the way: hand over the remaining bytes as payload and
/// receive a root signature over a core credential's preimage — a forged device
/// certificate, which is account takeover.
///
/// Prefix-disjointness rather than mere inequality, and in both directions,
/// because the payload can extend the message arbitrarily to the right.
#[test]
fn no_external_domain_shares_a_prefix_with_a_core_domain() {
    let externals: Vec<&[u8]> = ALL_EXTERNAL.iter().map(|d| d.as_bytes()).collect();

    for ext in &externals {
        for core in ALL_DOMAINS {
            assert!(
                !ext.starts_with(core) && !core.starts_with(ext),
                "external domain {:?} and core domain {:?} share a prefix — a caller \
                 could extend one into the other with a chosen payload",
                String::from_utf8_lossy(ext),
                String::from_utf8_lossy(core),
            );
        }

        // Not in `ALL_DOMAINS`: it lives in `calimero-governance-types`, and it
        // is the raw-concatenation signing site that makes a length-based guard
        // insufficient. Named explicitly so the reason stays visible here.
        const ADMITTER: &[u8] = b"calimero.admit.v1";
        assert!(
            !ext.starts_with(ADMITTER) && !ADMITTER.starts_with(ext),
            "external domain {:?} shares a prefix with the admitter-endorsement domain",
            String::from_utf8_lossy(ext),
        );
    }
}

/// Every external domain ends in NUL, which is what separates it from the payload.
///
/// mdma concatenates without a length prefix, so the terminator is the only
/// thing keeping `domain ‖ "ab"` distinct from a hypothetical `domain‖"a" ‖ "b"`
/// split. Dropping it from one constant would be invisible until two payloads
/// collided.
#[test]
fn every_external_domain_is_nul_terminated() {
    for &domain in ALL_EXTERNAL {
        let bytes = domain.as_bytes();
        assert_eq!(
            bytes.last(),
            Some(&0u8),
            "{domain:?} must end with the NUL that separates it from the payload",
        );
        assert_eq!(
            bytes.iter().filter(|b| **b == 0).count(),
            1,
            "{domain:?} must contain exactly one NUL, at the end",
        );
    }
}

/// An empty payload still signs, and is not the same as signing the domain alone
/// under a different domain.
#[test]
fn an_empty_payload_is_accepted() {
    let root: PrivateKey = key(9);
    let (_, sig) =
        sign_external(&root, ExternalSigningDomain::MdmaAccountLink, b"").expect("sign empty");
    assert!(
        root.public_key()
            .verify_raw_signature(b"calimero.mdma.account-link.v1\0", &sig)
            .is_ok(),
        "an empty payload must sign the bare domain",
    );
}
