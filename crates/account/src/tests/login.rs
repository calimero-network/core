use calimero_primitives::identity::PublicKey;

use crate::device_login_payload;

/// Pinned so a client in another language can check it builds the same bytes
/// the node verifies. mero-js carries the same vector.
#[test]
fn device_login_payload_matches_the_pinned_vector() {
    let payload = device_login_payload(&[1; 32], &PublicKey::from([2; 32]));
    assert_eq!(
        hex::encode(payload),
        "a593ceb81e8587870234f5896aa89d691d8a4f794d788a143c681d35070ebe00"
    );
}

#[test]
fn a_different_challenge_or_key_changes_the_payload() {
    let base = device_login_payload(&[1; 32], &PublicKey::from([2; 32]));
    assert_ne!(
        base,
        device_login_payload(&[3; 32], &PublicKey::from([2; 32]))
    );
    assert_ne!(
        base,
        device_login_payload(&[1; 32], &PublicKey::from([4; 32]))
    );
}
