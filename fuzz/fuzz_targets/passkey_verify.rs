#![no_main]
//! The passkey ceremonies a node checks for every sign-up, sign-in, added
//! device and confirmed call: WebAuthn's client data (JSON), authenticator
//! data and attestation object (CBOR), from whoever sends them.
//!
//! The input is split at 0xff into an attestation object, client data,
//! authenticator data and a signature. A registration from those parts must
//! never verify (its client data would have to carry the challenge, the
//! origin and the type the node expects, and the fuzzer cannot sign); the
//! same holds for an assertion against a real credential.

use std::sync::OnceLock;

use calimero_identity::testing::SoftAuthenticator;
use calimero_identity::webauthn::{
    claimed_origin, verify_assertion, verify_registration, Assertion, Credential, Registration,
    RelyingParty,
};
use calimero_identity::b64url;
use libfuzzer_sys::fuzz_target;

const ORIGIN: &str = "http://localhost:3000";
const CHALLENGE: &[u8] = b"calimero-fuzz-challenge-32-bytes";

fn rp() -> RelyingParty {
    RelyingParty {
        id: "localhost".into(),
        origins: vec![ORIGIN.into()],
        require_uv: false,
    }
}

/// A credential the node holds: registered once by a real (software) authenticator.
fn credential() -> &'static Credential {
    static CREDENTIAL: OnceLock<Credential> = OnceLock::new();
    CREDENTIAL.get_or_init(|| {
        let mut pk = SoftAuthenticator::new("localhost", ORIGIN);
        let reg = pk.register(CHALLENGE);
        verify_registration(&rp(), CHALLENGE, &reg).expect("a real registration verifies")
    })
}

fuzz_target!(|data: &[u8]| {
    let mut parts = data.splitn(4, |b| *b == 0xff);
    let mut next = || parts.next().unwrap_or(&[]);
    let (attestation, client_data, auth_data, signature) = (next(), next(), next(), next());

    let _ = claimed_origin(&String::from_utf8_lossy(client_data));

    let reg = Registration {
        attestation_object: b64url(attestation),
        client_data_json: b64url(client_data),
    };
    let _ = verify_registration(&rp(), CHALLENGE, &reg);

    let cred = credential();
    let assertion = Assertion {
        credential_id: cred.id.clone(),
        authenticator_data: b64url(auth_data),
        client_data_json: b64url(client_data),
        signature: b64url(signature),
    };
    assert!(
        verify_assertion(&rp(), CHALLENGE, cred, &assertion).is_err(),
        "an assertion the fuzzer made verified"
    );
});
