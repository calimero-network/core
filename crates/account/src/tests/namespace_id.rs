//! The founded-namespace-id derivation: what it commits to, and its known answer.

use calimero_primitives::identity::AccountId;

use crate::{founded_namespace_id, is_founded_by};

const FOUNDER: [u8; 32] = [0x11; 32];
const SALT: [u8; 32] = [0x22; 32];

/// Known answer, for a relying party written in another language.
///
/// A service that confirms "this account founded this namespace" recomputes
/// the id itself, so it has to reproduce the construction byte for byte:
/// `domain_hash("calimero.namespace.id.v1", [founder, salt])`, both parts
/// length-prefixed. This pins it.
#[test]
fn founded_namespace_id_has_a_known_answer() {
    assert_eq!(
        hex::encode(founded_namespace_id(&AccountId::from(FOUNDER), &SALT)),
        "35f5e77cc3c7cdb18eef50f1ea2808f27069dd25143e935994fcd58b3876c0bd",
    );
}

#[test]
fn the_id_commits_to_the_founder() {
    let id = founded_namespace_id(&AccountId::from(FOUNDER), &SALT);
    assert!(is_founded_by(&id, &AccountId::from(FOUNDER), &SALT));
    assert!(
        !is_founded_by(&id, &AccountId::from([0x12; 32]), &SALT),
        "another account must not be able to present itself as the founder, with any salt"
    );
}

#[test]
fn the_salt_separates_one_founders_namespaces() {
    let mut other = SALT;
    other[0] ^= 1;
    let founder = AccountId::from(FOUNDER);
    assert_ne!(
        founded_namespace_id(&founder, &SALT),
        founded_namespace_id(&founder, &other)
    );
    assert!(!is_founded_by(
        &founded_namespace_id(&founder, &SALT),
        &founder,
        &other
    ));
}

#[test]
fn a_random_id_is_founded_by_nobody() {
    // What every namespace created before derivation looks like.
    let legacy = [0xa0; 32];
    assert!(!is_founded_by(&legacy, &AccountId::from(FOUNDER), &SALT));
}
