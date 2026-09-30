//! The founded-namespace-id derivation: what it commits to, and its known answer.

use calimero_primitives::identity::AccountId;

use crate::{created_subgroup_id, founded_namespace_id, is_founded_by};

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
    // An id no pair derives, such as a forged genesis would have to claim.
    let random = [0xa0; 32];
    assert!(!is_founded_by(&random, &AccountId::from(FOUNDER), &SALT));
}

/// Known answer for the subgroup id, for the SDK that derives it in another
/// language: `domain_hash("calimero.subgroup.id.v1", [creator, parent,
/// [restricted as u8], salt])`, every part length-prefixed.
#[test]
fn created_subgroup_id_has_a_known_answer() {
    assert_eq!(
        hex::encode(created_subgroup_id(
            &AccountId::from(FOUNDER),
            &[0x33; 32],
            true,
            &SALT
        )),
        "6c949a0f0e0c3c55310223f4cd03f888b6e84c7e963f9171a5639fc54ea93b1a",
    );
}

#[test]
fn the_subgroup_id_commits_to_the_whole_create() {
    let creator = AccountId::from(FOUNDER);
    let parent = [0x33; 32];
    let id = created_subgroup_id(&creator, &parent, true, &SALT);
    assert_ne!(
        id,
        created_subgroup_id(&AccountId::from([0x12; 32]), &parent, true, &SALT),
        "another creator must derive another id"
    );
    assert_ne!(
        id,
        created_subgroup_id(&creator, &[0x34; 32], true, &SALT),
        "another parent must derive another id"
    );
    assert_ne!(
        id,
        created_subgroup_id(&creator, &parent, false, &SALT),
        "another birth visibility must derive another id"
    );
    assert_ne!(
        id,
        founded_namespace_id(&creator, &SALT),
        "a subgroup id must never equal a namespace id"
    );
}
